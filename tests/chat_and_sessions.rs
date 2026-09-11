//! Chat turns against the mock model, cancel, busy gate, loop gating, sessions
//! routes, and the 422 validation envelope.

mod common;

#[tokio::test]
async fn new_session_isolates_history_goal_and_skills_but_keeps_explicit_shared_notes() {
    let model = common::start_mock_model().await;
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let (_, first) = s.post_json("/api/sessions", serde_json::json!({})).await;
    let id = first["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{id}/goal"),
        serde_json::json!({"goal":"OLD_SESSION_GOAL"}),
    )
    .await;
    s.post_json(
        "/api/chat",
        serde_json::json!({"session_id":id,"message":"OLD_SESSION_MESSAGE"}),
    )
    .await;
    s.post_json(
        "/api/memory/add",
        serde_json::json!({"text":"SHARED_NOTE_2_PLUS_2_EQUALS_4"}),
    )
    .await;
    let (_, second) = s.post_json("/api/sessions", serde_json::json!({})).await;
    let fresh = second["session_id"].as_str().unwrap();
    assert_ne!(fresh, id);
    assert_eq!(second["message_count"], 0);
    assert_eq!(second["tokens"]["total"], 0);
    assert!(s.state.store.get(fresh).unwrap().selected_skills.is_empty());
    for enabled in [true, false] {
        s.post_json("/api/memory", serde_json::json!({"enabled":enabled})).await;
        let (_, preview) = s
            .post_json("/api/prompt/preview", serde_json::json!({"session_id":fresh}))
            .await;
        let (status, reply) = s
            .post_json(
                "/api/chat",
                serde_json::json!({"session_id":fresh,"message":"List memories"}),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        let request = model.last_request().unwrap();
        let prompt = request["messages"][0]["content"].as_str().unwrap();
        assert_eq!(preview["prompt"], prompt);
        assert!(!request.to_string().contains("OLD_SESSION_MESSAGE"));
        assert!(!request.to_string().contains("OLD_SESSION_GOAL"));
        assert_eq!(prompt.contains("SHARED_NOTE_2_PLUS_2_EQUALS_4"), enabled);
        assert!(prompt.contains(&format!("memory={enabled}")));
        assert!(prompt.contains("You have no callable tools in this chat, including gh"));
        for command in [
            "/memory add",
            "/memory forget",
            "/session new",
            "/prompt",
            "/goal stage",
            "/skill use",
            "/web",
        ] {
            assert!(prompt.contains(command), "missing operator guidance for {command}");
        }
        assert!(prompt.contains("not a placeholder"));
    }
    assert_eq!(
        s.get_json("/api/memory").await.1["count"],
        1,
        "disabling inclusion preserves notes"
    );
    assert_eq!(
        s.state.store.get(id).unwrap().messages.len(),
        2,
        "old sessions are preserved"
    );
}

use common::*;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn chat_turn_records_the_exchange_and_tally() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "ping"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "pong");
    assert_eq!(body["usage"]["prompt_tokens"], 10);
    assert_eq!(body["tally"]["total"], 12);
    let sid = body["session_id"].as_str().unwrap().to_string();
    assert_eq!(sid.len(), 12);
    // Fresh chat has no implicit repository or coding-skill assignment.
    let req = model.last_request().unwrap();
    let system = req["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("general conversation"));
    assert!(!system.contains("Discipline contract:"));
    assert!(!system.contains("Begin the review now"));
    assert!(!system.contains("CyClaw"));
    assert!(req.get("tools").is_none(), "chat has no model tool dispatcher");
    let (_, status) = s.open_get("/api/status").await;
    assert!(status["repo_root"].is_null());
    assert_eq!(status["chat_mode"], "conversation");
    assert_eq!(status["chat_tools_available"], false);
    assert_eq!(req["messages"][1]["content"], "ping");
    assert_eq!(req["stream"], false);
    assert_eq!(
        req["reasoning_effort"], "none",
        "shipped config sends reasoning_effort for ollama"
    );
    // Second turn on the same session carries history.
    let (status, body2) = s
        .post_json("/api/chat", json!({"message": "again", "session_id": sid}))
        .await;
    assert_eq!(status, 200, "{body2}");
    let req2 = model.last_request().unwrap();
    assert_eq!(req2["messages"].as_array().unwrap().len(), 4);
    assert_eq!(body2["tally"]["exchanges"], 2);
    // Session read is guarded and returns content; status counts tokens.
    let (status, full) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(status, 200);
    assert_eq!(full["messages"].as_array().unwrap().len(), 4);
    let (_, st) = s.open_get("/api/status").await;
    assert_eq!(st["total_tokens"], 24);
    assert_eq!(st["sessions"], 1);
}

#[tokio::test]
async fn model_errors_are_502_without_echoing_the_body() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    model.set_reply(json!({"__status": 401}));
    let (status, body) = s.post_json("/api/chat", json!({"message": "x"})).await;
    assert_eq!(status, 502);
    assert_eq!(code(&body), "HARNESS_LLM_ERROR");
    assert!(message(&body).contains("HTTP 401"));
    assert!(!body.to_string().contains("\"error\":\"x\""));
    model.set_reply(json!({"choices": [{"message": {"content": 5}}]}));
    let (status, body) = s.post_json("/api/chat", json!({"message": "x"})).await;
    assert_eq!(status, 502);
    assert!(message(&body).contains("malformed"));
    // Usage is cosmetic: a malformed usage block degrades to 0, not an error.
    model.set_reply(json!({"choices": [{"finish_reason": "stop", "message": {"content": "ok"}}], "usage": "nope"}));
    let (status, body) = s.post_json("/api/chat", json!({"message": "x"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["usage"]["prompt_tokens"], 0);
    // Gate was released after every failure.
    assert!(!s.state.generation_gate.is_held());
}

#[tokio::test]
async fn cancel_aborts_the_in_flight_turn_and_releases_the_gate() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    model.set_delay_ms(5_000);
    let s2 = spawn_server(&model.base_url(), ServerOptions::default()).await; // separate app for the poll
    drop(s2);
    let chat = {
        let s = &s;
        async move { s.post_json("/api/chat", json!({"message": "slow"})).await }
    };
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        // While the turn is in flight, a second chat is CHAT_BUSY with a cancel hint.
        let (status, body) = s.post_json("/api/chat", json!({"message": "second"})).await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(code(&body), "CHAT_BUSY");
        assert_eq!(body["detail"]["details"]["cancel"], "/api/chat/cancel");
        let (status, body) = s.post_json("/api/chat/cancel", json!({})).await;
        assert_eq!(status, 200);
        assert_eq!(body["cancelled"], true);
    };
    let ((status, body), ()) = tokio::join!(chat, cancel);
    assert_eq!(status, 502, "{body}");
    assert_eq!(code(&body), "HARNESS_LLM_ERROR");
    assert!(message(&body).contains("cancelled"));
    assert!(!s.state.generation_gate.is_held());
    // Idempotent when idle.
    let (status, _) = s.post_json("/api/chat/cancel", json!({})).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn loop_turns_need_a_session_a_goal_and_the_tool_allowlist() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "x", "loop": true})).await;
    assert_eq!(status, 400);
    assert_eq!(code(&body), "LOOP_REQUIRES_SESSION");
    assert_eq!(
        s.open_get("/api/sessions").await.1["sessions"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "no orphan session"
    );
    let (_, created) = s.post_json("/api/sessions", json!({"title": "t"})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "x", "loop": true, "session_id": sid}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(code(&body), "LOOP_REQUIRES_GOAL");
    let (status, g) = s
        .post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "ship it"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(g["goal"], "ship it");
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "x", "loop": true, "session_id": sid}))
        .await;
    assert_eq!(status, 200, "{body}");
    let req = model.last_request().unwrap();
    assert_eq!(req["max_tokens"], 2048, "loop turns use the loop token budget");
    assert!(req["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Operator goal (session, read-only)"));
    // Empty allowlist denies loop turns with 403 and releases the in-flight claim.
    let deny = ServerOptions {
        deny_all_tools: true,
        ..Default::default()
    };
    let d = spawn_server(&model.base_url(), deny).await;
    let (_, created) = d.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    d.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "g"}))
        .await;
    let (status, body) = d
        .post_json("/api/chat", json!({"message": "x", "loop": true, "session_id": sid}))
        .await;
    assert_eq!(status, 403);
    assert_eq!(code(&body), "TOOL_DENIED");
    assert!(d.state.loop_inflight.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sessions_routes_round_trip_and_validate_ids() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, created) = s.post_json("/api/sessions", json!({"title": "  My session  "})).await;
    assert_eq!(status, 201);
    assert_eq!(created["title"], "My session");
    let sid = created["session_id"].as_str().unwrap().to_string();
    let (status, renamed) = s
        .post_json(&format!("/api/sessions/{sid}/rename"), json!({"title": "Renamed"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(renamed["title"], "Renamed");
    for bad in [
        "nothex",
        "..%2F..%2Fetc",
        "abcdefabcdefab",
        "ABCDEFABCDEF",
        "abcdefabcdef%0A",
    ] {
        let (status, body) = s.get_json(&format!("/api/sessions/{bad}")).await;
        assert_eq!(status, 404, "{bad}");
        assert_eq!(code(&body), "HARNESS_SESSION_ERROR");
    }
    let (status, _) = s.get_json("/api/sessions/0123456789ab").await;
    assert_eq!(status, 404, "unknown but well-formed id");
    // Corrupt file is skipped by the listing, not a 500.
    std::fs::write(s.home.join("sessions").join("ffffffffffff.json"), "{not json").unwrap();
    let (status, list) = s.open_get("/api/sessions").await;
    assert_eq!(status, 200);
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    // Goal clears with an empty string.
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "x"}))
        .await;
    let (_, cleared) = s
        .post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": ""}))
        .await;
    assert_eq!(cleared["goal"], "");
    // Model + soul toggles persist.
    let (status, m) = s.post_json("/api/model", json!({"model": " qwen-x "})).await;
    assert_eq!(status, 200);
    assert_eq!(m["model"], "qwen-x");
    let (_, st) = s.open_get("/api/status").await;
    assert_eq!(st["model"], "qwen-x");
    let (_, soul) = s.post_json("/api/soul", json!({"enabled": false})).await;
    assert_eq!(soul["enabled"], false);
    assert_eq!(s.open_get("/api/soul").await.1["enabled"], false);
}

#[tokio::test]
async fn validation_errors_use_the_envelope_and_never_echo_values() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    // Unknown key: replaced wholesale, never echoed.
    let secret_key = "sk-ant-api03-SUPERSECRETKEYVALUE0000000000";
    let (status, body) = s.post_json("/api/chat", json!({"message": "x", secret_key: 1})).await;
    assert_eq!(status, 422);
    assert_eq!(code(&body), "VALIDATION_ERROR");
    assert_eq!(body["detail"]["details"]["fields"][0], "(unexpected field)");
    assert!(!body.to_string().contains("SUPERSECRET"));
    // Oversized unknown key cannot flood the console.
    let huge = "k".repeat(10_000);
    let (status, body) = s.post_json("/api/chat", json!({"message": "x", huge: 1})).await;
    assert_eq!(status, 422);
    assert!(body.to_string().len() < 1000);
    // Missing declared field is named; the value of a bad field is never shown.
    let (status, body) = s.post_json("/api/chat", json!({})).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"]["details"]["fields"][0], "message");
    let (status, body) = s.post_json("/api/chat", json!({"message": ""})).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"]["details"]["fields"][0], "message");
    let (status, body) = s
        .post_json("/api/model", json!({"model": "VALUE-THAT-IS-TOO-LONG".repeat(20)}))
        .await;
    assert_eq!(status, 422);
    assert!(!body.to_string().contains("VALUE-THAT-IS-TOO-LONG"));
    // Non-JSON body.
    let resp = s
        .req(Method::POST, "/api/model")
        .body("not json")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 422);
    let (status, body) = s
        .post_json(
            "/api/agent/runs/00000000000000000000000000000000/decision",
            json!({"decision": "maybe"}),
        )
        .await;
    assert_eq!(status, 422, "{body}");
}

#[tokio::test]
async fn loop_budget_refuses_extra_model_calls_without_blocking_ordinary_chat() {
    let model = start_mock_model().await;
    let opts = ServerOptions::default()
        .with("api.harness_loop_rate_limit.max_requests", "1")
        .with("api.harness_loop_rate_limit.max_tokens", "37");
    let s = spawn_server(&model.base_url(), opts).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal":"Explain a patch"}))
        .await;
    let turn = json!({"message":"Next step", "session_id":sid, "loop":true});
    assert_eq!(s.post_json("/api/chat", turn.clone()).await.0, 200);
    assert_eq!(model.last_request().unwrap()["max_tokens"], 37);
    let response = s.req(Method::POST, "/api/chat").json(&turn).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 429);
    assert!(
        response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert_eq!(code(&response.json().await.unwrap()), "LOOP_RATE_LIMIT");
    assert_eq!(
        model.requests.lock().unwrap().len(),
        1,
        "refused turn never reaches model"
    );
    assert!(s.state.loop_inflight.lock().unwrap().is_empty());
    assert_eq!(
        s.post_json("/api/chat", json!({"message":"Normal chat", "session_id":sid}))
            .await
            .0,
        200
    );
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    let (_, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(session["tokens"]["exchanges"], 2, "refused loop adds no history/tokens");
}

#[tokio::test]
async fn failed_and_cancelled_loop_turns_release_claims_for_a_later_turn() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal":"Review changes"}))
        .await;
    let turn = json!({"message":"Continue", "session_id":sid, "loop":true});
    model.set_reply(json!({"__status":500}));
    assert_eq!(s.post_json("/api/chat", turn.clone()).await.0, 502);
    assert!(s.state.loop_inflight.lock().unwrap().is_empty());
    assert!(!s.state.generation_gate.is_held());

    model.set_reply(ok_reply("next", 10, 2));
    model.set_delay_ms(30_000);
    let chat = s.post_json("/api/chat", turn.clone());
    let cancel = async {
        // Synchronize with the actual model request, not a guessed sleep.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while model.requests.lock().unwrap().len() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loop request reached mock model");
        let (status, body) = s.post_json("/api/chat", turn.clone()).await;
        assert_eq!(status, 409);
        assert_eq!(code(&body), "LOOP_IN_FLIGHT");
        assert_eq!(s.post_json("/api/chat/cancel", json!({})).await.0, 200);
    };
    let ((status, body), ()) = tokio::join!(chat, cancel);
    assert_eq!(status, 502, "{body}");
    assert!(s.state.loop_inflight.lock().unwrap().is_empty());
    assert!(!s.state.generation_gate.is_held());
    model.set_delay_ms(0);
    assert_eq!(s.post_json("/api/chat", turn).await.0, 200);
    let (_, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(session["tokens"]["exchanges"], 1, "only the successful turn persists");
}

#[tokio::test]
async fn skill_identity_and_soul_status_match_the_actual_prompt() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, fresh) = s.open_get("/api/soul").await;
    assert_eq!(fresh["enabled"], true);
    assert_eq!(fresh["loaded"], false);
    assert_eq!(fresh["unavailable_reason"], "missing");
    let skills = s.home.join("skills");
    std::fs::create_dir_all(skills.join("custom")).unwrap();
    std::fs::write(
        skills.join("custom/SKILL.md"),
        "---\nname: ponytail\n---\nNOT_LOADED_MARKER",
    )
    .unwrap();
    std::fs::write(
        skills.join("ponytail/SKILL.md"),
        "---\nname: renamed\n---\nACTUAL_DISCIPLINE_MARKER",
    )
    .unwrap();
    std::fs::write(s.home.join("soul.md"), "PERSONA_MARKER").unwrap();
    let (_, inventory) = s.open_get("/api/skills").await;
    let rows = inventory["skills"].as_array().unwrap();
    assert_eq!(rows.iter().find(|r| r["id"] == "custom").unwrap()["role"], "repo");
    assert_eq!(rows.iter().find(|r| r["id"] == "ponytail").unwrap()["role"], "repo");
    assert!(rows.iter().all(|r| r["invoked"] == false));
    s.post_json("/api/chat", json!({"message":"test"})).await;
    let req = model.last_request().unwrap();
    let prompt = req["messages"][0]["content"].as_str().unwrap();
    assert!(
        !prompt.contains("ACTUAL_DISCIPLINE_MARKER"),
        "even an existing coding skill is opt-in"
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    assert_eq!(
        s.post_json(&format!("/api/sessions/{sid}/skills"), json!({"ids":["ponytail"]}))
            .await
            .0,
        200
    );
    let (_, preview) = s.post_json("/api/prompt/preview", json!({"session_id":sid})).await;
    s.post_json(
        "/api/chat",
        json!({"message":"Discuss the supplied context", "session_id":sid}),
    )
    .await;
    let selected = model.last_request().unwrap();
    assert_eq!(selected["messages"][0]["content"], preview["prompt"]);
    assert_eq!(
        preview["selected_skill_sections"][0]["chars"],
        "ACTUAL_DISCIPLINE_MARKER".len()
    );
    assert_eq!(
        preview["selected_skill_sections"][0]["sha256"],
        cgagentharness::common::sha256_hex("ACTUAL_DISCIPLINE_MARKER")
    );
    assert!(preview["prompt"].as_str().unwrap().contains("ACTUAL_DISCIPLINE_MARKER"));
    assert!(!preview["prompt"].as_str().unwrap().contains("NOT_LOADED_MARKER"));
    s.post_json(&format!("/api/sessions/{sid}/skills"), json!({"ids":[]}))
        .await;
    s.post_json(
        "/api/chat",
        json!({"message":"Continue general conversation", "session_id":sid}),
    )
    .await;
    assert!(!model.last_request().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("ACTUAL_DISCIPLINE_MARKER"));
    assert!(prompt.contains("PERSONA_MARKER"));
    assert!(!prompt.contains("NOT_LOADED_MARKER"));
    assert!(!prompt.contains("name: renamed"));
    std::fs::write(s.home.join("soul.md"), "   ").unwrap();
    assert_eq!(s.open_get("/api/soul").await.1["unavailable_reason"], "empty");
    std::fs::remove_file(s.home.join("soul.md")).unwrap();
    std::fs::create_dir(s.home.join("soul.md")).unwrap();
    assert_eq!(s.open_get("/api/soul").await.1["unavailable_reason"], "unreadable");
}

#[cfg(unix)]
#[tokio::test]
async fn soul_prompt_read_cannot_escape_the_home_through_a_symlink() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("outside.md");
    std::fs::write(&secret, "OUTSIDE_HOME_MARKER").unwrap();
    std::os::unix::fs::symlink(&secret, s.home.join("soul.md")).unwrap();
    let (_, status) = s.open_get("/api/soul").await;
    assert_eq!(status["loaded"], false);
    assert_eq!(status["unavailable_reason"], "unreadable");
    s.post_json("/api/chat", json!({"message":"Check containment"})).await;
    assert!(!model.last_request().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("OUTSIDE_HOME_MARKER"));
}

#[tokio::test]
async fn upstream_error_body_never_reaches_debug_logs() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let writer = log.reopen().unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.try_clone().unwrap())
        .finish();
    // Capture spawned HTTP tasks as well as the calling test thread.
    tracing::subscriber::set_global_default(subscriber).unwrap();
    tracing::debug!("DEBUG_CAPTURE_ACTIVE");
    let model = start_mock_model().await;
    model.set_reply(json!({"__status": 401}));
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "x"})).await;
    assert_eq!(status, 502);
    assert!(message(&body).contains("HTTP 401"));
    let logged = std::fs::read_to_string(log.path()).unwrap();
    assert!(logged.contains("DEBUG_CAPTURE_ACTIVE"), "debug capture must be active");
    assert!(logged.contains("harness chat upstream HTTP 401"));
    assert!(
        !logged.contains(r#"{"error":"x"}"#),
        "upstream error body leaked into debug logs"
    );
}

#[tokio::test]
async fn upstream_error_headers_release_chat_without_waiting_for_the_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 1000000\r\n\r\n")
            .await
            .unwrap();
        // Deliberately never send the advertised body or close the connection.
        std::future::pending::<()>().await;
    });
    let s = spawn_server(&base, ServerOptions::default()).await;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        s.post_json("/api/chat", json!({"message": "x"})),
    )
    .await;
    upstream.abort();
    s.state.chat.abort_in_flight();
    let (status, body) = response.expect("known HTTP error must not wait for its body");
    assert_eq!(status, 502);
    assert_eq!(code(&body), "HARNESS_LLM_ERROR");
    assert!(message(&body).contains("HTTP 503"));
    assert!(!s.state.generation_gate.is_held());
}

#[tokio::test]
async fn clear_session_history_requires_intent_and_cannot_be_resurrected() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, chat) = s.post_json("/api/chat", json!({"message":"private fixture"})).await;
    let id = chat["session_id"].as_str().unwrap();
    let sessions = s.home.join("sessions");
    std::fs::write(sessions.join("ffffffffffff.json"), "corrupt private fixture").unwrap();
    std::fs::write(sessions.join(".staged.fixture.tmp"), "interrupted private write").unwrap();
    std::fs::write(sessions.join("keep.txt"), "unrelated").unwrap();
    s.post_json("/api/memory/add", json!({"text":"keep note"})).await;
    let intent = json!({"confirm":true,"reason":"Clear disposable test history"});
    let unguarded = s
        .client
        .post(s.url("/api/sessions/clear"))
        .json(&intent)
        .send()
        .await
        .unwrap();
    assert!(matches!(unguarded.status().as_u16(), 401 | 403));
    for body in [
        json!({}),
        json!({"reason":"test"}),
        json!({"confirm":false,"reason":"test"}),
        json!({"confirm":true,"reason":" "}),
        json!({"confirm":"true","reason":"test"}),
    ] {
        assert_eq!(s.post_json("/api/sessions/clear", body).await.0, 422);
        assert!(s.state.store.get(id).is_ok());
    }
    assert_eq!(s.post_json("/api/sessions/clear", intent.clone()).await.0, 200);
    assert!(s.state.store.list().is_empty());
    assert!(!sessions.join(format!("{id}.json")).exists());
    assert!(!sessions.join("ffffffffffff.json").exists());
    assert!(!sessions.join(".staged.fixture.tmp").exists());
    assert_eq!(std::fs::read_to_string(sessions.join("keep.txt")).unwrap(), "unrelated");
    assert_eq!(s.get_json("/api/memory").await.1["count"], 1);
    assert_eq!(s.open_get("/api/status").await.1["total_tokens"], 0);
    assert!(s
        .state
        .store
        .record_exchange(id, "late", "late", "fixture", &Default::default(), &[])
        .is_err());
    assert!(s.state.store.rename(id, Some("late"), None).is_err());
    assert!(s.state.store.list().is_empty());
    assert_eq!(s.post_json("/api/sessions/clear", intent).await.0, 200);
    assert_eq!(s.post_json("/api/sessions", json!({})).await.0, 201);
}
