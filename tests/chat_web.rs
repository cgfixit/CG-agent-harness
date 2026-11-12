//! Real HTTP tool-protocol checks; model output cannot grant URL permission.
mod common;
use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[tokio::test]
async fn chat_batches_multiple_reads_with_one_protocol_budget_and_no_malformed_prefix_execution() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r = reads.clone();
    let fixture=Router::new().route("/docs/{page}",get(move || {let r=r.clone();async move {r.fetch_add(1,Ordering::SeqCst); ([("content-type","text/plain")],"BATCH_BACKUP_EVIDENCE")}}))
        .route("/v1/chat/completions",post(|Json(body):Json<Value>|async move {
            let messages=body["messages"].as_array().unwrap();
            let mode=messages.iter().find(|m|m["role"]=="user").unwrap()["content"].as_str().unwrap();
            let finished=messages.last().unwrap()["role"]=="tool";
            let tool_messages:Vec<_>=messages.iter().filter(|m|m["role"]=="tool").collect();
            if finished && mode=="denied" {
                // One permitted read and one refusal, handed back as that call's result.
                assert_eq!(tool_messages.len(),2);
                assert!(tool_messages[0]["content"].as_str().unwrap().contains("BATCH_BACKUP_EVIDENCE"));
                let refused:Value=serde_json::from_str(tool_messages[1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(refused["error"],"WEB_HOST_DENIED","{refused}");
                assert!(refused.get("text").is_none(),"a refused read carries no page text: {refused}");
                return Json(common::ok_reply("Compared the permitted source only.",20,5));
            }
            if finished && mode=="later-denied" {
                if tool_messages.len()==1 {
                    // After one good read, ask for a refused URL: the batch fails entirely.
                    return Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[
                        json!({"id":"batch_late","type":"function","function":{"name":"web_fetch","arguments":json!({"url":"http://docs.example/private"}).to_string()}})]}}],"usage":{"prompt_tokens":10,"completion_tokens":3}})); // DevSkim: ignore DS137138 because this synthetic URL tests permission refusal against the loopback fixture.
                }
                let refused:Value=serde_json::from_str(tool_messages[1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(refused["error"],"WEB_HOST_DENIED","{refused}");
                assert!(body.get("tools").is_none(),"an all-failed batch after evidence ends the tool rounds");
                return Json(common::ok_reply("Answered from the first read after a refusal.",20,5));
            }
            if finished && !mode.starts_with("cumulative") {
                assert_eq!(tool_messages.len(),2);
                assert!(tool_messages.iter().all(|m|m["content"].as_str().unwrap().contains("BATCH_BACKUP_EVIDENCE")));
                return Json(common::ok_reply("Compared both fetched sources.",20,5));
            }
            let mut calls:Vec<_>=(0..if mode=="overlimit" {4} else if mode=="later-denied" {1} else {2}).map(|i|json!({"id":format!("batch_{i}"),"type":"function","function":{"name":"web_fetch","arguments":json!({"url":format!("http://docs.example/docs/{i}")}).to_string()}})).collect(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            if mode=="duplicate" { calls[1]["id"]=calls[0]["id"].clone(); }
            if mode=="malformed" { calls[1]["function"]["arguments"]=json!("{}"); }
            if mode=="denied" { calls[1]["function"]["arguments"]=json!(json!({"url":"http://docs.example/private"}).to_string()); } // DevSkim: ignore DS137138 because this synthetic URL tests permission refusal against the loopback fixture.
            let usage = if mode=="cumulative-missing" && !finished { json!({"prompt_tokens":10}) } else { json!({"prompt_tokens":10,"completion_tokens":3}) };
            Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":calls}}],"usage":usage}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default().with("web.chat_tool_calls", "3")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, reply) = s.post_json("/api/chat", json!({"message":"compare"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Compared both fetched sources.");
    assert_eq!(reply["web_tools"].as_array().unwrap().len(), 2);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    for (mode, code) in [
        ("overlimit", "WEB_TOOL_LIMIT"),
        ("duplicate", "WEB_TOOL_RESPONSE"),
        ("malformed", "WEB_TOOL_ARGUMENTS"),
    ] {
        let (status, reply) = s.post_json("/api/chat", json!({"message":mode})).await;
        assert_eq!(status, 502, "{reply}");
        assert_eq!(common::code(&reply), code, "{reply}");
        assert_eq!(reply["detail"]["details"]["completion_tokens"], 3);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "no valid prefix reads before malformed/over-budget suffix refusal"
        );
        assert!(!s.state.generation_gate.is_held());
    }
    let (_, reply) = s.post_json("/api/chat", json!({"message":"cumulative"})).await;
    assert_eq!(common::code(&reply), "WEB_TOOL_LIMIT");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "two calls plus two more exceeds total budget three"
    );
    assert_eq!(reply["detail"]["details"]["prompt_tokens"], 20);
    assert_eq!(reply["detail"]["details"]["completion_tokens"], 6);
    assert_eq!(reply["detail"]["details"]["known_completion_tokens"], 6);
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let rows: Vec<Value> = ledger.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(), 5, "success and each rejected logical turn record once");
    for (row, input, output) in [
        (&rows[1], 10, 3),
        (&rows[2], 10, 3),
        (&rows[3], 10, 3),
        (&rows[4], 20, 6),
    ] {
        assert_eq!(row["outcome"], "failed_after_billing");
        assert_eq!(row["input_tokens"], input);
        assert_eq!(row["output_tokens"], output);
        assert_eq!(row["usage_missing"], false);
    }
    for private in ["docs.example", "BATCH_BACKUP_EVIDENCE", "arguments", &s.api_key] {
        assert!(!ledger.contains(private));
    }
    // A refused sibling is that call's result, not the end of the turn: the
    // permitted read is kept and the model answers from it. Before, one denied
    // URL in a batch discarded every sibling result and the turn had no answer.
    let (status, reply) = s.post_json("/api/chat", json!({"message":"denied"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Compared the permitted source only.", "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], true, "{reply}");
    assert_eq!(reply["web_tools"][1]["ok"], false, "{reply}");
    assert_eq!(reply["web_tools"][1]["code"], "WEB_HOST_DENIED");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        5,
        "a permitted call cannot authorize its denied sibling"
    );
    // A batch that fails entirely after evidence ends the tool rounds, not the turn.
    let (status, reply) = s.post_json("/api/chat", json!({"message":"later-denied"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["reply"], "Answered from the first read after a refusal.",
        "{reply}"
    );
    assert_eq!(reply["web_tools"][1]["code"], "WEB_HOST_DENIED", "{reply}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        6,
        "one more permitted read, none for the refusal"
    );
    let (_, reply) = s.post_json("/api/chat", json!({"message":"cumulative-missing"})).await;
    assert_eq!(common::code(&reply), "WEB_TOOL_LIMIT");
    assert_eq!(reply["detail"]["details"]["prompt_tokens"], 20);
    assert!(reply["detail"]["details"]["completion_tokens"].is_null());
    assert_eq!(reply["detail"]["details"]["known_completion_tokens"], 3);
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let rows: Vec<Value> = ledger.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(), 8);
    let row = rows.last().unwrap();
    assert_eq!(row["outcome"], "failed_after_billing");
    assert_eq!(row["input_tokens"], 20);
    assert!(row["output_tokens"].is_null());
    assert_eq!(row["usage_missing"], true);
    task.abort();
}

#[tokio::test]
async fn loop_advertises_only_permitted_web_tool_types_when_enabled() {
    let model = common::start_mock_model().await;
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    s.post_json("/api/web", json!({"enabled":true})).await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let id = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{id}/goal"),
        // Explicit `all`: this test covers the web on/off toggle, not the shipped
        // goal default (`all`), which the goal_web_default tests lock.
        json!({"goal":format!("Explain arithmetic. {} FINAL_CONSTRAINT: use the operator data only.", "Check each input and assumption. ".repeat(45)), "web_mode":"all"}),
    )
    .await;
    let (status, body) = s
        .post_json("/api/chat", json!({"message":"continue", "session_id":id, "loop":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let request = model.last_request().unwrap();
    let names: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web_search", "web_fetch"]);
    let prompt = request["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("you MAY call:"));
    assert!(prompt.contains("FINAL_CONSTRAINT: use the operator data only."));
    s.post_json("/api/web", json!({"enabled":false})).await;
    let (status, body) = s
        .post_json("/api/chat", json!({"message":"continue", "session_id":id, "loop":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(model.last_request().unwrap().get("tools").is_none());
}

#[tokio::test]
async fn goal_web_modes_persist_filter_definitions_and_reject_mixed_batches_before_reads() {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (r, seen) = (reads.clone(), observed.clone());
    let fixture = Router::new()
        .route("/page", get(move || { r.fetch_add(1, Ordering::SeqCst); async { "evidence" } }))
        .route("/v1/chat/completions", post(move |Json(body):Json<Value>| {
            seen.lock().unwrap().push(body.clone());
            async move {
                if body["messages"].as_array().unwrap().last().unwrap()["content"] != "malicious batch" {
                    return Json(common::ok_reply("Complete. GOAL_DONE", 10, 3));
                }
                Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[
                    {"id":"allowed_first","type":"function","function":{"name":"web_fetch","arguments":json!({"url":"http://docs.example/page"}).to_string()}},
                    {"id":"forbidden_second","type":"function","function":{"name":"web_search","arguments":json!({"query":"private goal must not leave"}).to_string()}}
                ]}}],"usage":{"prompt_tokens":10,"completion_tokens":3}})) // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = common::spawn_server(
        &format!("http://{addr}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), addr)),
            ..common::ServerOptions::default().with("web.chat_tool_calls", "3")
        },
    )
    .await;
    s.post_json("/api/web", json!({"enabled":true})).await;
    s.post_json("/api/web/allow", json!({"urls":["http://docs.example/page"]}))
        .await; // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    for (mode, names) in [
        ("off", vec![]),
        ("fetch", vec!["web_fetch"]),
        ("all", vec!["web_search", "web_fetch"]),
    ] {
        let (status, set) = s
            .post_json(
                &format!("/api/sessions/{sid}/goal"),
                json!({"goal":"Read only the designated inputs", "web_mode":mode}),
            )
            .await;
        assert_eq!(status, 200, "{set}");
        assert_eq!(set["goal_web_mode"], mode);
        let (_, loaded) = s.get_json(&format!("/api/sessions/{sid}")).await;
        assert_eq!(loaded["goal_web_mode"], mode);
        let (status, reply) = s
            .post_json("/api/chat", json!({"session_id":sid,"message":"continue","loop":true}))
            .await;
        assert_eq!(status, 200, "{reply}");
        let last = observed.lock().unwrap().last().unwrap().clone();
        let actual: Vec<_> = last["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(actual, names);
        let prompt = last["messages"][0]["content"].as_str().unwrap();
        if mode == "fetch" {
            assert!(!prompt.contains("- web_search:"));
        }
        if mode == "off" {
            assert!(prompt.contains("no chat-callable web tools"));
        }
    }
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read a page, never search", "web_mode":"fetch"}),
    )
    .await;
    let (status, denied) = s
        .post_json(
            "/api/chat",
            json!({"session_id":sid,"message":"malicious batch","loop":true}),
        )
        .await;
    assert_eq!(status, 502, "{denied}");
    assert_eq!(common::code(&denied), "WEB_TOOL_DENIED");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "a forbidden suffix prevents every read in the batch"
    );
    assert!(!s.state.generation_gate.is_held());
    let (status, _) = s
        .post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"test", "web_mode":"unknown"}),
        )
        .await;
    assert_eq!(status, 422);
    let (_, cleared) = s
        .post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"", "web_mode":"off"}),
        )
        .await;
    assert_eq!(cleared["goal_web_mode"], "all");
    // No mode named: the shipped default is all, so an unattended loop may search;
    // search listings grant no URL permission, so fetches stay policy-gated.
    let (status, defaulted) = s
        .post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"Read the designated page"}),
        )
        .await;
    assert_eq!(status, 200, "{defaulted}");
    assert_eq!(defaulted["goal_web_mode"], "all");
    let (status, reply) = s
        .post_json("/api/chat", json!({"session_id":sid,"message":"continue","loop":true}))
        .await;
    assert_eq!(status, 200, "{reply}");
    let last = observed.lock().unwrap().last().unwrap().clone();
    let offered: Vec<_> = last["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(offered, vec!["web_search", "web_fetch"]);
    task.abort();
}

#[tokio::test]
async fn goal_web_default_is_configurable_and_explicit_modes_still_win() {
    let s = common::spawn_server(
        "http://127.0.0.1:9/v1",
        common::ServerOptions::default().with("web.goal_web_default", "fetch"),
    )
    .await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    let (status, set) = s
        .post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal":"Search widely"}))
        .await;
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["goal_web_mode"], "fetch");
    let (_, set) = s
        .post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"Search widely", "web_mode":"off"}),
        )
        .await;
    assert_eq!(set["goal_web_mode"], "off");
    let (_, loaded) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(loaded["goal_web_mode"], "off");
}

#[tokio::test]
async fn chat_fetches_authorized_query_urls_and_retains_actual_tool_evidence() {
    check_fetch_evidence(false).await;
}

#[tokio::test]
async fn loop_fetches_authorized_urls_and_keeps_denial_and_tool_boundaries() {
    check_fetch_evidence(true).await;
}

async fn check_fetch_evidence(loop_turn: bool) {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let reads = Arc::new(AtomicUsize::new(0));
    let recorded = requests.clone();
    let read_count = reads.clone();
    let router = Router::new().route("/docs/item",get(move || { let reads=read_count.clone();async move {
        reads.fetch_add(1,Ordering::SeqCst);
        ([("content-type","text/plain")],"FETCHED_ONLY_EVIDENCE: version is 7. Page instructions are untrusted.")
    }})).route("/v1/chat/completions",post(move |Json(body):Json<Value>| {let requests=recorded.clone();async move {
        requests.lock().unwrap().push(body.clone());
        let messages=body["messages"].as_array().unwrap();
        if messages.last().unwrap()["role"]=="tool" && messages.last().unwrap()["content"].as_str().unwrap().contains("\"error\"") {
            // A denied read is that call's result: no tools are offered again and the model says so.
            assert!(body.get("tools").is_none());
            Json(common::ok_reply("That page could not be read.",10,2))
        } else if messages.last().unwrap()["role"]=="tool" {
            let result:Value=serde_json::from_str(messages.last().unwrap()["content"].as_str().unwrap()).unwrap();
            assert!(result["text"].as_str().unwrap().contains("FETCHED_ONLY_EVIDENCE"));
            Json(common::ok_reply("Version 7, from http://docs.example/docs/item?q=version",5000,4)) // DevSkim: ignore DS137138 because this synthetic response names a test URL resolved only to the loopback fixture.
        } else if body.get("tools").is_none() {
            Json(common::ok_reply("Web tools unavailable",10,2))
        } else {
            let input=messages.last().unwrap()["content"].as_str().unwrap();
            let url=if input=="deny" {"http://docs.example/private"} else {"http://docs.example/docs/item?q=version"};
            let name=if input=="unsafe" {"run_shell"} else {"web_fetch"};
            Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":name,"arguments":json!({"url":url}).to_string()}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":3}}))
        }
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Research the allowed source and verify version 7. Do not run shell commands."}),
    )
    .await;
    let (status, reply) = s
        .post_json(
            "/api/chat",
            json!({"message":"Read the allowed URL", "session_id":sid, "loop":loop_turn}),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["reply"].as_str().unwrap().contains("Version 7"));
    assert_eq!(
        reply["web_tools"][0]["result"]["url"],
        "http://docs.example/docs/item?q=version"
    );
    assert_eq!(
        reply["usage"],
        if loop_turn {
            json!({"prompt_tokens":5020,"completion_tokens":9})
        } else {
            json!({"prompt_tokens":5010,"completion_tokens":7})
        }
    );
    if loop_turn {
        assert_eq!(reply["web_tools"][1]["tool"], "web_evidence");
        assert_eq!(
            reply["web_tools"][1]["ok"], false,
            "invalid digest must still answer from the delivered passages"
        );
    }
    let session = s
        .state
        .store
        .for_owner("local")
        .get(reply["session_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        session.token_calibration.unwrap().ratio,
        1.0,
        "calibrate from the initial 10-token prompt, not the 5010-token aggregate"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(requests.lock().unwrap().len(), if loop_turn { 3 } else { 2 });
    let (_, denied) = s
        .post_json(
            "/api/chat",
            json!({"message":"deny", "session_id":sid, "loop":loop_turn}),
        )
        .await;
    assert_eq!(denied["web_tools"][0]["ok"], false);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "denied destination must not be fetched"
    );
    // The session keeps a ledger of both reads (targets and counts, no page
    // text); the audit gets each turn's counts without the targets.
    let (_, stored) = s.get_json(&format!("/api/sessions/{sid}")).await;
    let ledger = stored["web_sources"].as_array().unwrap();
    assert_eq!(ledger.len(), 2, "{stored}");
    assert_eq!(ledger[0]["target"], "http://docs.example/docs/item?q=version");
    assert_eq!(
        (&ledger[0]["reads"], &ledger[0]["cited"], &ledger[0]["turns"]),
        (&json!(1), &json!(1), &json!([1]))
    );
    assert_eq!(ledger[0]["delivered_chars"], ledger[0]["source_chars"]);
    assert_eq!((&ledger[1]["failures"], &ledger[1]["turns"]), (&json!(1), &json!([2])));
    assert!(!stored["web_sources"].to_string().contains("FETCHED_ONLY_EVIDENCE"));
    let turns: Vec<Value> = s
        .audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "chat_web_turn")
        .collect();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(
        (
            &turns[0]["turn"],
            &turns[0]["ok"],
            &turns[0]["cited"],
            &turns[0]["loop_turn"]
        ),
        (&json!(1), &json!(1), &json!(1), &json!(loop_turn))
    );
    assert_eq!((&turns[1]["failed"], &turns[1]["calls"]), (&json!(1), &json!(1)));
    assert!(!format!("{turns:?}").contains("docs.example"), "{turns:?}");
    let (status, unsafe_tool) = s
        .post_json(
            "/api/chat",
            json!({"message":"unsafe", "session_id":sid, "loop":loop_turn}),
        )
        .await;
    assert_eq!(status, 502);
    assert_eq!(common::code(&unsafe_tool), "WEB_TOOL_DENIED");
    s.post_json("/api/web", json!({"enabled":false})).await;
    let (_, off) = s
        .post_json(
            "/api/chat",
            json!({"message":"Read the allowed URL", "session_id":sid, "loop":loop_turn}),
        )
        .await;
    assert_eq!(off["reply"], "Web tools unavailable");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    server.abort();
}

/// With `web.carry_forward_sources` above 0, the next web turn's system prompt
/// lists the URL an earlier turn read (no page text) in a fenced untrusted
/// block; at the shipped 0 it lists nothing until a reload raises it.
#[tokio::test]
async fn a_later_web_turn_lists_earlier_reads_only_when_carry_forward_is_on() {
    for rows in ["0", "5"] {
        let systems = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = systems.clone();
        let router = Router::new()
            .route(
                "/docs/item",
                get(|| async { ([("content-type", "text/plain")], "PAGE_TEXT_NOT_CARRIED") }),
            )
            .route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let systems = recorded.clone();
                    async move {
                        let messages = body["messages"].as_array().unwrap();
                        // The ledger rides with the new message on a local chat
                        // turn: record the system prompt and every user turn.
                        let carried: Vec<&str> = messages
                            .iter()
                            .filter(|m| m["role"] == "system" || m["role"] == "user")
                            .filter_map(|m| m["content"].as_str())
                            .collect();
                        systems.lock().unwrap().push(carried.join("\n"));
                        if messages.last().unwrap()["role"] == "tool" {
                            Json(common::ok_reply("Read it.", 10, 2))
                        } else {
                            Json(tool_call_reply(
                                "web_fetch",
                                r#"{"url":"http://docs.example/docs/item"}"#,
                                "carry_1",
                            ))
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let s = common::spawn_server(
            &format!("http://{address}/v1"),
            common::ServerOptions {
                web_resolve: Some(("docs.example".into(), address)),
                ..common::ServerOptions::default().with("web.carry_forward_sources", rows)
            },
        )
        .await;
        assert_eq!(
            s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
                .await
                .0,
            200
        );
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let sid = session["session_id"].as_str().unwrap();
        for message in ["first", "second"] {
            let (status, reply) = s
                .post_json("/api/chat", json!({"message": message, "session_id": sid}))
                .await;
            assert_eq!(status, 200, "{reply}");
        }
        let systems_handle = systems.clone();
        let systems = systems.lock().unwrap().clone();
        // Calls: turn 1 (tool call, follow-up), turn 2 (tool call, follow-up).
        assert_eq!(systems.len(), 4, "{rows}");
        assert!(
            !systems[0].contains("WEB_READS_DATA"),
            "nothing to carry on the first turn"
        );
        let second = &systems[2];
        if rows == "0" {
            assert!(!second.contains("WEB_READS_DATA"), "off at the shipped 0");
            // Reloadable: raising the key in config.yaml and reloading turns it
            // on for the next turn, with no restart.
            let path = &s.state.cfg.path;
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.contains("carry_forward_sources: 0"), "{text}");
            std::fs::write(
                path,
                text.replace("carry_forward_sources: 0", "carry_forward_sources: 5"),
            )
            .unwrap();
            let reloaded = cgagentharness::server::config_reload::reload(s.state.clone(), "test")
                .await
                .unwrap();
            assert_eq!(reloaded["changed"], true, "{reloaded}");
            let (status, reply) = s
                .post_json("/api/chat", json!({"message": "third", "session_id": sid}))
                .await;
            assert_eq!(status, 200, "{reply}");
            let third = systems_handle.lock().unwrap()[4].clone();
            assert!(
                third.contains("- fetched http://docs.example/docs/item (turns 1, 2)"),
                "{third}"
            );
        } else {
            assert!(second.contains("<<<WEB_READS_DATA>>>"), "{second}");
            assert!(
                second.contains("- fetched http://docs.example/docs/item (turns 1)"),
                "{second}"
            );
            assert!(!second.contains("PAGE_TEXT_NOT_CARRIED"), "page text is never carried");
        }
        server.abort();
    }
}

/// A tool round sends the whole prompt again with the result. Web chat must keep
/// that follow-up inside web.total_tokens: clip a fetched page to the room left.
/// Each prompt is charged once (the history is re-read, not spent twice), so a
/// session past the budget is compacted and still gets its tools; before, it
/// was admitted without them from about a third of the budget onward.
#[tokio::test]
async fn web_chat_tool_rounds_stay_inside_the_web_token_budget() {
    let reads = Arc::new(AtomicUsize::new(0));
    let last_prompt = Arc::new(AtomicU64::new(0));
    let offered = Arc::new(AtomicBool::new(false));
    let (read_count, prompt_seen, tools_seen) = (reads.clone(), last_prompt.clone(), offered.clone());
    let router = Router::new()
        .route(
            "/docs/item",
            get(move || {
                let reads = read_count.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(2000))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let (prompt_seen, tools_seen) = (prompt_seen.clone(), tools_seen.clone());
                async move {
                    // Report the real prompt size so the web budget charges what was sent.
                    let prompt = (body["messages"].to_string().len()
                        + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                        as u64
                        / 4;
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let tools = body.get("tools").is_some_and(|t| t.as_array().is_some_and(|t| !t.is_empty()));
                    if last["role"] == "tool" {
                        Json(common::ok_reply("Answer from the fetched page", prompt, 20))
                    } else if last["content"].as_str().is_some_and(|text| text.starts_with("fetch")) {
                        tools_seen.store(tools, Ordering::SeqCst);
                        if !tools {
                            return Json(common::ok_reply("Answered without the page", prompt, 20));
                        }
                        let arguments = json!({"url":"http://docs.example/docs/item"}).to_string(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":"web_fetch","arguments":arguments}}]}}],
                            "usage":{"prompt_tokens":prompt,"completion_tokens":20}}))
                    } else {
                        // Track the history's size from seed turns only.
                        prompt_seen.store(prompt, Ordering::SeqCst);
                        Json(common::ok_reply("Noted.", prompt, 20))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // LM Studio doubles the reply reservation: 4096 tokens for max_tokens 2048. A
    // round needs this prompt, the ~272 definition tokens, a 256-token minimal
    // result and one reply inside the shipped 28000-token budget, so tools fit up
    // to about 23,376 input tokens; chat.compact_prompt_tokens (24000, projected
    // with the reservation and definitions) binds first at about 19,632.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.provider", "lmstudio")
        .with("models.local_llm.max_tokens", "2048"),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let mut session = Value::Null;
    // Phase 1: a large history still permits a read. Repeated page content is
    // deduplicated into a selected passage before the follow-up is budgeted.
    for turn in 0..50 {
        if last_prompt.load(Ordering::SeqCst) >= 18_300 {
            break;
        }
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        session = body["session_id"].clone();
    }
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "the follow-up after the fetch must fit: {body}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(body["reply"], "Answer from the fetched page");
    let text = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !text.is_empty() && text.len() < 24_000 && "PAGE_EVIDENCE ".repeat(2000).starts_with(text),
        "the page is clipped to the room left: {} chars",
        text.len()
    );
    // Phase 2: past the compaction trigger the next seed is compacted (the
    // mock's "Noted." is its summary) and the history shrinks; a fetch then runs
    // with tools and the selected passage retains a complete receipt.
    let mut peak = last_prompt.load(Ordering::SeqCst);
    for turn in 50..100 {
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let now = last_prompt.load(Ordering::SeqCst);
        if now + 4_000 < peak {
            break;
        }
        peak = peak.max(now);
    }
    assert!(
        peak >= 19_600,
        "the history grew to the trigger before compacting: {peak}"
    );
    assert!(
        last_prompt.load(Ordering::SeqCst) + 4_000 < peak,
        "compaction shrank the prompt"
    );
    offered.store(false, Ordering::SeqCst);
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(offered.load(Ordering::SeqCst), "tools stay offered after compaction");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(body["reply"], "Answer from the fetched page");
    let result = &body["web_tools"][0]["result"];
    let receipt = &result["receipts"][0];
    assert_eq!(
        result["text"].as_str().unwrap().chars().count() as u64,
        receipt["end"].as_u64().unwrap() - receipt["start"].as_u64().unwrap(),
        "the selected passage fits whole after compaction"
    );
    let (_, stored) = s
        .get_json(&format!("/api/sessions/{}", session.as_str().unwrap()))
        .await;
    assert!(stored.to_string().contains("[session-compacted]"), "{stored}");
    server.abort();
}

/// With the prompt charged once, every admitted web prompt fits a tool round:
/// the compaction trigger keeps a minimal result's room on top of the
/// projection, so a session that outgrows the budget is compacted and keeps
/// its tools instead of being admitted without them.
#[tokio::test]
async fn web_chat_compacts_before_a_tool_round_stops_fitting() {
    let reads = Arc::new(AtomicUsize::new(0));
    let last_prompt = Arc::new(AtomicU64::new(0));
    let offered = Arc::new(AtomicBool::new(false));
    let (read_count, prompt_seen, tools_seen) = (reads.clone(), last_prompt.clone(), offered.clone());
    let router = Router::new()
        .route(
            "/docs/item",
            get(move || {
                let reads = read_count.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(200))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let (prompt_seen, tools_seen) = (prompt_seen.clone(), tools_seen.clone());
                async move {
                    let prompt = (body["messages"].to_string().len()
                        + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                        as u64
                        / 4;
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let tools = body.get("tools").is_some_and(|t| t.as_array().is_some_and(|t| !t.is_empty()));
                    if last["role"] == "tool" {
                        Json(common::ok_reply("Answer from the page", prompt, 20))
                    } else if last["content"] == "fetch the page" {
                        tools_seen.fetch_or(tools, Ordering::SeqCst);
                        if !tools {
                            return Json(common::ok_reply("Answered without the page", prompt, 20));
                        }
                        // The tool-call reply spends its whole cap, as a reasoning model can.
                        let arguments = json!({"url":"http://docs.example/docs/item"}).to_string(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":"web_fetch","arguments":arguments}}]}}],
                            "usage":{"prompt_tokens":prompt,"completion_tokens":2048}}))
                    } else {
                        prompt_seen.store(prompt, Ordering::SeqCst);
                        Json(common::ok_reply("Noted.", prompt, 20))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Shipped Ollama with reasoning "none" reserves one 2048-token reply per call.
    // A round needs this prompt, the ~272 definition tokens, a 256-token minimal
    // result and that reply inside 16000: tools fit up to 13,424 input tokens,
    // and web chat admits exactly that much before compacting.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.max_tokens", "2048")
        .with("web.total_tokens", "16000"),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let mut session = Value::Null;
    // Seed past 11,500 input tokens (the fixture reports JSON bytes, so the
    // session learns a ratio near 1.06): still admitted, and a round still fits.
    for turn in 0..40 {
        if last_prompt.load(Ordering::SeqCst) >= 11_500 {
            break;
        }
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        session = body["session_id"].clone();
    }
    let seeded = last_prompt.load(Ordering::SeqCst);
    assert!((11_500..12_200).contains(&seeded), "seeded to {seeded} input tokens");
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page", "{body}");
    assert!(offered.load(Ordering::SeqCst), "a round that fits is offered its tools");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    // The next seeds cross the trigger: the session is compacted (the mock's
    // "Noted." is the summary), and the fetch after it still runs with tools.
    let mut peak = seeded;
    for turn in 40..80 {
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let now = last_prompt.load(Ordering::SeqCst);
        if now + 3_000 < peak {
            break;
        }
        peak = peak.max(now);
    }
    assert!(
        last_prompt.load(Ordering::SeqCst) + 3_000 < peak,
        "compaction shrank the prompt: {peak}"
    );
    offered.store(false, Ordering::SeqCst);
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page", "{body}");
    assert!(offered.load(Ordering::SeqCst), "tools stay offered after compaction");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(
        body["web_tools"].as_array().unwrap().iter().all(|e| e["ok"] == true),
        "{body}"
    );
    server.abort();
}

#[tokio::test]
async fn web_chat_batches_keep_calibrated_room_for_each_later_result() {
    // A 504-character title makes the second page's smallest result about 180
    // tokens, 360 at the learned ratio of 2: more than an uncalibrated 256-token
    // reserve, less than the calibrated 512.
    let title = "Release notes ".repeat(36);
    let page = format!("<html><head><title>{title}</title></head><body><p>TITLED_EVIDENCE</p></body></html>");
    let router = Router::new()
        .route(
            "/docs/big",
            get(|| async { ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(2000)) }),
        )
        .route(
            "/docs/titled",
            get(move || {
                let page = page.clone();
                async move { ([("content-type", "text/html")], page) }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                // Report twice the sent size: the session learns a token ratio of about 2.
                let prompt = (body["messages"].to_string().len()
                    + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                    as u64
                    / 2;
                let messages = body["messages"].as_array().unwrap();
                let last = messages.last().unwrap();
                if last["role"] == "tool" {
                    Json(common::ok_reply("Answer from both pages", prompt, 20))
                } else if last["content"] == "fetch both pages" {
                    let calls: Vec<_> = ["big", "titled"]
                        .iter()
                        .map(|page| {
                            json!({"id":format!("call_{page}"),"type":"function","function":{"name":"web_fetch",
                                "arguments":json!({"url":format!("http://docs.example/docs/{page}")}).to_string()}}) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        })
                        .collect();
                    Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                        "tool_calls":calls}}],"usage":{"prompt_tokens":prompt,"completion_tokens":20}}))
                } else {
                    Json(common::ok_reply("Noted.", prompt, 20))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // 19000 tokens leave the follow-up about 8.6k after the ~8.7k-token prompt, the
    // 1024-token reply reservation and the second call's calibrated 512-token
    // reserve, so the first page (about 12k at ratio 2) is cut.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.provider", "lmstudio")
        .with("models.local_llm.max_tokens", "512")
        .with("web.total_tokens", "19000"),
    )
    .await;
    s.seed_default_soul();
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, body) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message": "fetch both pages", "session_id": body["session_id"]}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from both pages", "{body}");
    let first = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !first.is_empty() && first.len() < 24_000,
        "the first page is cut to leave room: {} chars",
        first.len()
    );
    let second = &body["web_tools"][1]["result"];
    assert_eq!(second["title"], title.trim(), "{body}");
    assert!(
        second["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty() && "TITLED_EVIDENCE".starts_with(text)),
        "{body}"
    );
    server.abort();
}

/// A mock model for the budget tests below. It reports bytes / 4 of what it is
/// sent and records every request. Offered tools for a message starting with
/// "fetch", it asks for `calls` parallel reads of `url` (default: the counted
/// page) from a reply that carries `content` and spends its whole 2048-token
/// cap. Without tools, or asked to "search", it says it cannot search the web.
fn budget_model(
    calls: usize,
    content: Option<String>,
    url: Option<String>,
    requests: Arc<Mutex<Vec<Value>>>,
) -> Router {
    let url = url.unwrap_or_else(|| "http://docs.example/docs/item".into()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
    Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let (requests, content, url) = (requests.clone(), content.clone(), url.clone());
            async move {
                requests.lock().unwrap().push(body.clone());
                let prompt = (body["messages"].to_string().len()
                    + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                    as u64
                    / 4;
                let last = body["messages"].as_array().unwrap().last().unwrap().clone();
                let fetch = last["content"].as_str().is_some_and(|text| text.starts_with("fetch"));
                let search = last["content"].as_str().is_some_and(|text| text.starts_with("search"));
                if search {
                    Json(common::ok_reply("I cannot search the web.", prompt, 20))
                } else if last["role"] == "tool" {
                    Json(common::ok_reply("Answer from the page", prompt, 20))
                } else if !fetch {
                    Json(common::ok_reply("Noted.", prompt, 20))
                } else if body.get("tools").is_none() {
                    Json(common::ok_reply("I cannot search the web.", prompt, 20))
                } else {
                    let calls: Vec<_> = (0..calls)
                        .map(|n| {
                            json!({"id":format!("call_{n}"),"type":"function","function":{"name":"web_fetch",
                                "arguments":json!({"url":url}).to_string()}})
                        })
                        .collect();
                    Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":content,
                        "tool_calls":calls}}],"usage":{"prompt_tokens":prompt,"completion_tokens":2048}}))
                }
            }
        }),
    )
}

/// Starts `router` plus a counted 6000-token page at /docs/item, and a server
/// with shipped Ollama (reasoning "none": one 2048-token reply reserved per
/// call), `web.total_tokens` 9000 and `calls` web calls a turn. Returns the
/// server, the read counter, and the input tokens of a fresh session's first
/// short message, which the tests pad to the size they need (one message stays
/// under the chat message cap). Web chat admits a prompt of up to about 6,424
/// input tokens here (9000 less a 2048-token reply, the ~272 definition tokens
/// and a 256-token minimal result), and offers tools to every admitted prompt.
async fn budget_server(router: Router, calls: usize) -> (common::TestServer, Arc<AtomicUsize>, u64) {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let router = router.route(
        "/docs/item",
        get(move || {
            let reads = read_count.clone();
            async move {
                reads.fetch_add(1, Ordering::SeqCst);
                (
                    [("content-type", "text/plain")],
                    (0..30)
                        .map(|n| format!("Section {n}: {}\n\n", format!("PAGE_EVIDENCE_{n} ").repeat(90)))
                        .collect::<String>(),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.max_tokens", "2048")
        .with("api.harness_loop_rate_limit.max_tokens", "2048")
        .with("web.total_tokens", "9000")
        .with("web.chat_tool_calls", &calls.to_string()),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, body) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    let base = body["usage"]["prompt_tokens"].as_u64().unwrap();
    assert!(
        base < 4_600,
        "the budget tests pad a short first message of {base} tokens"
    );
    (s, reads, base)
}

/// The model may batch every call it has left (`parallel_tool_calls`). A batch
/// whose calls cannot each fit a minimal result in the follow-up runs none of
/// its reads, and the model answers without tools. Before, the first of ten
/// reads ran, then the turn ended with WEB_TOKEN_BUDGET and no answer.
#[tokio::test]
async fn web_chat_runs_no_read_of_a_batch_without_room_for_each_result() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(10, None, None, requests.clone()), 10).await;
    // About 5,250 input tokens: a round fits (up to about 6,424 tokens), but the
    // follow-up with the batch, ten minimal results (2,560) and a reply does not.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_250 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    // Verbatim: the answering request offered no tools, so grounding must not
    // claim this turn includes them.
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs for a batch that cannot fit"
    );
    assert_eq!(body["web_tools"][0]["code"], "WEB_TOKEN_BUDGET", "{body}");
    let text = body["web_tools"][0]["message"].as_str().unwrap();
    assert!(text.contains("10-call batch; none ran"), "{text}");
    // Asked once with tools (the batch), then once without.
    let requests = requests.lock().unwrap();
    let asked: Vec<bool> = requests
        .iter()
        .filter(|r| r["messages"].as_array().unwrap().last().unwrap()["content"] == message)
        .map(|r| r.get("tools").is_some())
        .collect();
    assert_eq!(asked, [true, false]);
}

/// The follow-up resends the model's own tool-call message, so a call whose reply
/// carries long content can leave no room for its result. The batch check counts
/// that echo before the read; without it, the read ran and the turn ended with
/// WEB_TOKEN_BUDGET.
#[tokio::test]
async fn web_chat_counts_the_echoed_tool_call_message_before_a_read() {
    let content = format!("Let me look that up. {}", "y".repeat(6_000));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, Some(content), None, requests), 1).await;
    // About 5,575 input tokens: a one-call round fits (up to about 6,424), but the
    // follow-up with the ~1,500-token echo, a minimal result and a reply does not.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_575 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs when the echo leaves no room"
    );
    let text = body["web_tools"][0]["message"].as_str().unwrap();
    assert!(text.contains("1-call batch; none ran"), "{text}");
}

/// URLs are never cut, so a long one makes even a minimal result large. The batch
/// check reserves each call's own minimal result: a flat 256-token floor let this
/// read of a ~2 KB URL run, and the turn then failed with WEB_TOKEN_BUDGET.
#[tokio::test]
async fn web_chat_reserves_a_long_urls_minimal_result_before_the_read() {
    let url = format!("http://docs.example/docs/item?ref={}", "a".repeat(2_000)); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
    let (s, reads, base) = budget_server(budget_model(1, None, Some(url), Arc::default()), 1).await;
    // A nominal 6,350 tokens (the fixture counts the definitions and JSON overhead,
    // so the prompt itself is about 6,050): a one-call round fits (up to about
    // 6,424). With the URL's ~550-token minimal result (and its ~540-token echo)
    // reserved the follow-up does not fit; with a flat 256 it would.
    let message = format!("fetch the page {}", "x".repeat(4 * (6_350 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs when the URL's result cannot fit"
    );
}

/// Grounding rewrites "I cannot search the web" only when the request that
/// produced it offered the web tools. Web chat withholds them when no tool round
/// fits, and then the statement is true.
#[tokio::test]
async fn web_chat_grounds_a_web_denial_only_when_the_answer_was_offered_tools() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, _reads, _base) = budget_server(budget_model(1, None, None, requests.clone()), 1).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "search the web"})).await;
    assert_eq!(status, 200, "{body}");
    let reply = body["reply"].as_str().unwrap();
    assert!(
        reply.starts_with("This turn includes web_fetch, web_search."),
        "{reply}"
    );
    // Match the web budget's estimate of the captured fresh-session prompt.
    // Reserve midway between fitting the bare prompt and fitting a tool round.
    let (prompt, round) = {
        let requests = requests.lock().unwrap();
        let request = requests.last().unwrap();
        let messages = request["messages"].as_array().unwrap();
        let sent = messages[0]["content"].as_str().unwrap().len() + serde_json::to_vec(&messages[1..]).unwrap().len();
        let definitions = request["tools"].as_array().unwrap();
        let with_tools = sent + serde_json::to_vec(definitions).unwrap().len();
        (
            (sent as u64).div_ceil(4),
            (with_tools as u64).div_ceil(4) + cgagentharness::server::web_search::MIN_EVIDENCE_TOKENS,
        )
    };
    let total = 16_000;
    let reservation = total - prompt - (round - prompt) / 2;
    assert!(
        prompt + reservation < total && round + reservation > total,
        "reservation {reservation} must fit the bare prompt ({prompt}) but not a tool round ({round}) in {total}"
    );
    let s = {
        let router = budget_model(1, None, None, Arc::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        common::spawn_server(
            &format!("http://{address}/v1"),
            common::ServerOptions {
                web_resolve: Some(("docs.example".into(), address)),
                ..Default::default()
            }
            .with("models.local_llm.max_tokens", &reservation.to_string())
            .with("web.total_tokens", &total.to_string()),
        )
        .await
    };
    let (status, body) = s.post_json("/api/chat", json!({"message": "search the web"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(body["web_tools"][0]["code"], "WEB_TOKEN_BUDGET", "{body}");
    assert_eq!(body["web_tools"][0]["ok"], false, "{body}");
}

/// The last call of a turn carries no tools, so its result is cut to the room a
/// tool-free follow-up leaves. Counting the definitions anyway cut the page to
/// leave their room unused, and the follow-up's own admission check refused it.
#[tokio::test]
async fn web_chat_sizes_a_tool_free_follow_up_without_the_tool_definitions() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, None, None, requests.clone()), 1).await;
    // About 6,350 input tokens: a one-call round fits (up to about 6,424), and the
    // follow-up (this prompt, the batch, the page and a 2048-token reply) must fit
    // 9000, so the 6000-token page must be cut.
    let message = format!("fetch the page {}", "x".repeat(4 * (6_350 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page", "{body}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let text = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !text.is_empty() && text.len() < 24_000,
        "the page is cut: {} chars",
        text.len()
    );
    let requests = requests.lock().unwrap();
    let last = |request: &Value| request["messages"].as_array().unwrap().last().unwrap().clone();
    let call = requests
        .iter()
        .find(|r| r.get("tools").is_some() && last(r)["content"] != "hello")
        .unwrap();
    let follow_up = requests.iter().find(|r| last(r)["role"] == "tool").unwrap();
    assert!(follow_up.get("tools").is_none(), "the last call carries no tools");
    assert_eq!(
        follow_up["tool_choice"], "none",
        "historical calls must not enable more tool use"
    );
    // The web budget's estimate of the follow-up: that prompt plus one reply.
    let definitions = call["tools"].to_string().len() as u64;
    let messages = follow_up["messages"].as_array().unwrap();
    let sent = messages[0]["content"].as_str().unwrap().len() + serde_json::to_vec(&messages[1..]).unwrap().len();
    let spent = (sent as u64).div_ceil(4) + 2048;
    assert!(
        spent <= 9_000 && 9_000 - spent < definitions / 8,
        "the result fills the room left: {spent} of 9000, definitions {} tokens",
        definitions / 4
    );
}

#[tokio::test]
async fn cancel_during_fetch_prevents_followup_model_call_and_releases_turn() {
    check_fetch_cancel(false, false).await;
}

#[tokio::test]
async fn loop_cancel_during_fetch_releases_turn_and_allows_recovery() {
    check_fetch_cancel(true, false).await;
}

#[tokio::test]
async fn cancel_during_fetch_preserves_missing_usage_dimension() {
    check_fetch_cancel(false, true).await;
}

async fn check_fetch_cancel(loop_turn: bool, partial_usage: bool) {
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let router = Router::new().route(
        "/slow",
        get(move || {
            let signal = signal.clone();
            async move {
                signal.notify_one();
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                "late page"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let model = common::start_mock_model().await;
    let mut response = json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{"id":"slow","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":\"http://docs.example/slow\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}});
    if partial_usage {
        response["usage"].as_object_mut().unwrap().remove("completion_tokens");
    }
    model.set_reply(response);
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/slow"}))
        .await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal":"Read slow URL"}))
        .await;
    let read = s.post_json(
        "/api/chat",
        json!({"message":"Read slow URL", "loop":loop_turn, "session_id":sid}),
    );
    let cancel = async {
        started.notified().await;
        s.post_json("/api/chat/cancel", json!({})).await
    };
    let ((status, reply), (cancel_status, _)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async { tokio::join!(read, cancel) })
            .await
            .expect("cancellation must not wait for the slow operation");
    assert_eq!(status, 502);
    assert_eq!(common::code(&reply), "WEB_CANCELLED");
    assert_eq!(cancel_status, 200);
    assert_eq!(
        reply["detail"]["details"]["known_completion_tokens"],
        if partial_usage { 0 } else { 2 }
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1, "no follow-up model call");
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let rows: Vec<Value> = ledger.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(), 1, "one failed logical turn: {ledger}");
    assert_eq!(rows[0]["provider"], "local");
    assert_eq!(rows[0]["source"], if loop_turn { "loop" } else { "chat" });
    assert_eq!(rows[0]["outcome"], "failed_after_billing");
    assert_eq!(rows[0]["input_tokens"], 10);
    if partial_usage {
        assert!(rows[0]["output_tokens"].is_null());
    } else {
        assert_eq!(rows[0]["output_tokens"], 2);
    }
    assert_eq!(rows[0]["usage_missing"], partial_usage);
    for private in ["Read slow URL", "docs.example", "arguments", &s.api_key] {
        assert!(!ledger.contains(private));
    }
    assert!(!s.state.generation_gate.is_held());
    let (_, cancelled_session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(cancelled_session["tokens"]["exchanges"], 0);
    model.set_reply(common::ok_reply("ready again", 10, 2));
    assert_eq!(
        s.post_json("/api/chat", json!({"message":"hello"})).await.1["reply"],
        "ready again"
    );
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    assert_eq!(ledger.lines().count(), 2, "recovery records only its own success");
    page_server.abort();
}

#[tokio::test]
async fn cancel_before_model_response_does_not_invent_spend() {
    let model = common::start_mock_model().await;
    model.set_delay_ms(30_000);
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let read = s.post_json("/api/chat", json!({"message":"hello"}));
    let cancel = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while model.requests.lock().unwrap().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("model request started");
        s.post_json("/api/chat/cancel", json!({})).await
    };
    let ((status, reply), (cancel_status, _)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async { tokio::join!(read, cancel) })
            .await
            .expect("cancellation must not wait for the slow operation");
    assert_eq!(status, 502, "{reply}");
    assert_eq!(common::code(&reply), "WEB_CANCELLED");
    assert_eq!(cancel_status, 200);
    s.state.audit.flush_async().await;
    assert!(!s.home.join("logs/spend.jsonl").exists());
    assert!(!s.state.generation_gate.is_held());
    model.set_delay_ms(0);
    assert_eq!(
        s.post_json("/api/chat", json!({"message":"hello"})).await.1["reply"],
        "pong"
    );
}

#[tokio::test]
async fn revoked_source_discards_inflight_answer() {
    check_revoked_source("answer").await;
}

#[tokio::test]
async fn revoked_source_discards_inflight_goal_digest_and_synthesis() {
    check_revoked_source("digest").await;
    check_revoked_source("synthesis").await;
}

async fn check_revoked_source(phase: &'static str) {
    let answering = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let signal = answering.clone();
    let release = resume.clone();
    let router = Router::new()
        .route("/page", get(|| async { "Evidence to be revoked" }))
        .route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
            let signal = signal.clone();
            let release = release.clone();
            async move {
                let last = body["messages"].as_array().unwrap().last().unwrap();
                let digest = body["response_format"]["type"] == "json_object";
                let synthesis = last["content"].as_str().unwrap_or("").contains("Untrusted evidence digest");
                if (phase == "answer" && last["role"] == "tool")
                    || (phase == "digest" && digest)
                    || (phase == "synthesis" && synthesis)
                {
                    signal.notify_one();
                    release.notified().await;
                }
                if digest {
                    Json(common::ok_reply(&json!({"supported":[{"text":"Evidence exists.","citations":[{"id":"S1","quote":"Evidence to be revoked"}]}],"conflicts":[],"inferences":[],"missing":[]}).to_string(), 10, 2))
                } else if last["role"] == "tool" || synthesis {
                    Json(common::ok_reply("Answer with revoked evidence", 10, 2))
                } else {
                    Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{"id":"fetch","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":\"http://docs.example/page\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/page"}))
        .await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the page and review the evidence."}),
    )
    .await;
    let read = s.post_json(
        "/api/chat",
        json!({"message":"Read the page","session_id":sid,"loop":phase != "answer"}),
    );
    let revoke = async {
        answering.notified().await;
        let result = s
            .post_json("/api/web/deny", json!({"url":"http://docs.example/page"}))
            .await;
        resume.notify_one();
        result
    };
    let ((status, reply), (revoke_status, _)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(read, revoke) })
            .await
            .unwrap();
    assert_eq!(revoke_status, 200);
    assert_eq!(status, 502);
    assert_eq!(common::code(&reply), "WEB_ALLOWLIST_EMPTY", "{reply}");
    assert!(!reply.to_string().contains("Answer with revoked evidence"));
    server.abort();
}

/// Fresh homes enable web with an empty URL policy. Chat tools must fail closed
/// the same way dedicated `/api/web/*` routes do (`WEB_ALLOWLIST_EMPTY`).
#[tokio::test]
async fn chat_empty_allowlist_refuses_fetch_and_search() {
    let model = common::start_mock_model().await;
    model.set_reply(call_then_answer(
        "web_fetch",
        r#"{"url":"http://docs.example/docs/item"}"#,
        "empty_fetch",
    ));
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200);
    assert_eq!(web["enabled"], true);
    assert!(
        web["allowlist"].as_array().map(|rows| rows.is_empty()).unwrap_or(false),
        "{web}"
    );
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Read http://docs.example/docs/item"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY", "{reply}");
    assert_failed_batch_is_answered(&model, &reply, "WEB_ALLOWLIST_EMPTY");
    assert_no_serpapi_secret(&reply, &s.home);

    model.set_reply(call_then_answer(
        "web_search",
        r#"{"query":"docs.example"}"#,
        "empty_search",
    ));
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Search Google for docs.example"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(
        reply["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY",
        "empty policy fails closed before Google-pattern matching: {reply}"
    );
    assert_failed_batch_is_answered(&model, &reply, "WEB_ALLOWLIST_EMPTY");
    assert_no_serpapi_secret(&reply, &s.home);
}

/// The model is asked to answer after its only web call was refused and writes
/// nothing. The turn still succeeds: the refusal is the reply, and the failed
/// read stays in the response and in the session's web ledger.
#[tokio::test]
async fn an_empty_answer_after_a_failed_batch_keeps_the_refusal() {
    let model = common::start_mock_model().await;
    model.set_reply(json!([
        tool_call_reply("web_fetch", r#"{"url":"http://docs.example/docs/item"}"#, "empty_fetch"), // DevSkim: ignore DS137138 because docs.example is never contacted; the empty policy refuses it.
        common::ok_reply("", 20, 0)
    ]));
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Read http://docs.example/docs/item"})) // DevSkim: ignore DS137138 because docs.example is never contacted; the empty policy refuses it.
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY", "{reply}");
    let text = reply["reply"].as_str().unwrap();
    assert!(text.starts_with("Web operation failed: WEB_ALLOWLIST_EMPTY"), "{reply}");
    let sid = reply["session_id"].as_str().unwrap();
    let (_, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(
        session["web_sources"][0]["last_code"], "WEB_ALLOWLIST_EMPTY",
        "{session}"
    );
    assert_eq!(session["web_sources"][0]["failures"], 1, "{session}");
}

/// After an administrator grants a fixture-origin rule, `web_fetch` succeeds
/// within `web.chat_tool_calls` (default 30). The N+1 model tool request is
/// refused with `WEB_TOOL_LIMIT` and the generation gate is released.
#[tokio::test]
async fn chat_origin_grant_fetches_within_bound_and_refuses_n_plus_one() {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let router = Router::new().route(
        "/page",
        get(move || {
            let reads = read_count.clone();
            async move {
                reads.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "PHASE2_FETCH_OK")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let model = common::start_mock_model().await;
    model.set_reply(tool_call_reply(
        "web_fetch",
        r#"{"url":"http://docs.example/page"}"#,
        "bound_fetch",
    ));
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
                .with("web.pace_ms", "100")
                .with("models.local_llm.max_tokens", "256")
        },
    )
    .await;
    assert_eq!(s.state.web.limits.chat_tool_calls, 30, "shipped default N");
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/*"}))
            .await
            .0,
        200
    );
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200, "{web}");
    assert_eq!(web["allowlist"], json!(["http://docs.example/*"]), "{web}");
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Read the granted origin repeatedly"}))
        .await;
    assert_eq!(status, 502, "{reply}");
    assert_eq!(common::code(&reply), "WEB_TOOL_LIMIT", "{reply}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        s.state.web.limits.chat_tool_calls,
        "exactly N fetches; the extra tool call must not hit the network"
    );
    assert_no_serpapi_secret(&reply, &s.home);
    model.set_reply(common::ok_reply("ready after tool-limit", 10, 2));
    let (status, next) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{next}");
    assert_eq!(next["reply"], "ready after tool-limit");
    page_server.abort();
}

/// A non-empty allowlist that is not a Google search grant still refuses
/// `web_search`. Tip returns `WEB_GOOGLE_PERMISSION` after `require_enabled`
/// succeeds (empty policy is the `WEB_ALLOWLIST_EMPTY` case above).
#[tokio::test]
async fn chat_search_without_google_pattern_returns_google_permission() {
    let model = common::start_mock_model().await;
    model.set_reply(call_then_answer(
        "web_search",
        r#"{"query":"veeam software cve"}"#,
        "google_denied",
    ));
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/*"}))
            .await
            .0,
        200
    );
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Search Google for veeam software cve"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_GOOGLE_PERMISSION", "{reply}");
    assert_failed_batch_is_answered(&model, &reply, "WEB_GOOGLE_PERMISSION");
    assert_no_serpapi_secret(&reply, &s.home);

    let (status, body) = s
        .post_json(
            "/api/web/search",
            json!({"query": "veeam software cve", "engine": "google"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(common::code(&body), "WEB_GOOGLE_PERMISSION", "{body}");
    assert!(common::message(&body).contains("https://www.google.com/*"), "{body}");
    assert_no_serpapi_secret(&body, &s.home);
}

/// A public Google CAPTCHA pauses that provider: the next loop turn's search is
/// refused `WEB_SEARCH_COOLDOWN` before any request, the model is told when it
/// ends, and the turn still completes.
#[tokio::test]
async fn loop_search_after_a_google_challenge_cools_down_without_a_second_request() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let google = Router::new().route(
        "/search",
        get(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                (
                    [("content-type", "text/html")],
                    "<html>Our systems have detected unusual traffic. <div class='g-recaptcha'></div></html>",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, google).await.unwrap() });
    let model = common::start_mock_model().await;
    let search = || call_then_answer("web_search", r#"{"query":"veeam cve"}"#, "cooldown_search");
    model.set_reply(search());
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("google.fixture".into(), address)),
            public_search: Some(url::Url::parse("http://google.fixture/search").unwrap()), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            // Inline appends, so the audit file is complete when the reply returns.
            ..common::ServerOptions::default().with("logging.audit_queue_lines", "0")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://google.fixture/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Find current CVEs"}),
    )
    .await;
    let (status, first) = s
        .post_json("/api/chat", json!({"message":"continue","session_id":sid,"loop":true}))
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["web_tools"][0]["code"], "WEB_GOOGLE_CHALLENGE", "{first}");
    assert_failed_batch_is_answered(&model, &first, "WEB_GOOGLE_CHALLENGE");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    model.set_reply(search());
    let (status, second) = s
        .post_json("/api/chat", json!({"message":"continue","session_id":sid,"loop":true}))
        .await;
    assert_eq!(status, 200, "a cooled-down search still completes the turn: {second}");
    assert_eq!(second["web_tools"][0]["code"], "WEB_SEARCH_COOLDOWN", "{second}");
    assert_failed_batch_is_answered(&model, &second, "WEB_SEARCH_COOLDOWN");
    let message = second["web_tools"][0]["message"].as_str().unwrap();
    assert!(message.contains("search is cooling down until "), "{message}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "no second request reached the provider");
    assert_eq!(
        cgagentharness::server::loop_runner::next_tool_failures(0, second["web_tools"].as_array().unwrap()),
        0,
        "a cooldown turn does not count toward loop.max_failures"
    );
    let audit = std::fs::read_to_string(s.home.join("logs").join("audit.jsonl")).unwrap();
    let starts: Vec<&str> = audit
        .lines()
        .filter(|l| l.contains("\"web_search_cooldown\""))
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "one line when the pause starts, none while it holds: {audit}"
    );
    assert!(starts[0].contains("\"provider\":\"google-public\""), "{}", starts[0]);
    assert!(starts[0].contains("WEB_GOOGLE_CHALLENGE"), "{}", starts[0]);
    let (status, panel) = s
        .post_json("/api/web/search", json!({"query":"veeam cve","engine":"google"}))
        .await;
    assert_eq!(status, 409, "{panel}");
    assert_eq!(common::code(&panel), "WEB_SEARCH_COOLDOWN", "{panel}");
    assert!(
        common::message(&panel).contains("search is cooling down until "),
        "{panel}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    fixture.abort();
}

/// Two public Google searches in one chat turn leave at least the 3 s floor
/// apart, whatever `web.pace_ms` says; both still answer.
#[tokio::test]
async fn chat_public_google_searches_in_one_turn_are_paced_by_the_floor() {
    let sent = Arc::new(Mutex::new(Vec::<std::time::Instant>::new()));
    let log = sent.clone();
    let fixture = Router::new()
        .route(
            "/search",
            get(move || {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(std::time::Instant::now());
                    (
                        [("content-type", "text/html")],
                        "<a href='/url?q=https%3A%2F%2Fveeam.com%2Fkb1'><h3>Paced result</h3></a>",
                    )
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                let messages = body["messages"].as_array().unwrap();
                if messages.last().unwrap()["role"] == "tool" {
                    return Json(common::ok_reply("Both listings arrived.", 10, 3));
                }
                Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                    "tool_calls":[
                        {"id":"pace_a","type":"function","function":{"name":"web_search","arguments":json!({"query":"veeam cve"}).to_string()}},
                        {"id":"pace_b","type":"function","function":{"name":"web_search","arguments":json!({"query":"veeam backup advisory"}).to_string()}}]}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":2}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        common::ServerOptions {
            web_resolve: Some(("google.fixture".into(), address)),
            public_search: Some(url::Url::parse("http://google.fixture/search").unwrap()), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            ..common::ServerOptions::default().with("web.pace_ms", "100")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://google.fixture/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (status, reply) = s.post_json("/api/chat", json!({"message":"Search twice"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Both listings arrived.", "{reply}");
    let tools = reply["web_tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "{reply}");
    assert!(tools.iter().all(|t| t["ok"] == true), "{reply}");
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    // The fixture stamps arrivals, which trail each send by connect and write
    // jitter, so an exactly paced pair can arrive a hair under 3 s apart (2.9986 s
    // was seen on ubuntu CI). The exact floor is asserted on the send stamp below.
    // Unpaced, or paced by `web.pace_ms` (100 ms) instead of the floor, the
    // arrivals land well under 1 s apart, so a 2 s bound still catches either.
    let gap = sent[1] - sent[0];
    assert!(
        gap >= std::time::Duration::from_millis(2000),
        "public Google searches {gap:?} apart; the floor is 3 s"
    );
    let sends: Vec<Value> = s
        .audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "web_search_sent")
        .collect();
    assert_eq!(sends.len(), 2, "{sends:?}");
    assert!(
        sends[0]["gap_ms"].is_null(),
        "the first send has no earlier send: {sends:?}"
    );
    assert!(
        sends[1]["gap_ms"].as_u64().unwrap() >= 3000,
        "the send stamps must be at least the 3 s floor apart: {sends:?}"
    );
    task.abort();
}

/// #647 Phase 2: with Tavily down and the public Google hop on, a loop turn's
/// search is answered by the next hop, the tool result says so, and the turn's
/// `chat_web_turn` audit line records the provider and the fallback.
#[tokio::test]
async fn loop_search_falls_back_from_a_down_tavily_and_audits_the_provider() {
    let google_hits = Arc::new(AtomicUsize::new(0));
    let g = google_hits.clone();
    let fixture = Router::new()
        .route(
            "/search",
            get(move || {
                let g = g.clone();
                async move {
                    g.fetch_add(1, Ordering::SeqCst);
                    "<a href='/url?q=https%3A%2F%2Fveeam.com%2Fkb1'><h3>Fallback result</h3></a>"
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                let messages = body["messages"].as_array().unwrap();
                if messages.last().unwrap()["role"] == "tool" {
                    return Json(common::ok_reply("Answered from the fallback listing.", 10, 3));
                }
                Json(tool_call_reply(
                    "web_search",
                    r#"{"query":"veeam cve"}"#,
                    "fallback_search",
                ))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
    // A loopback port with nothing listening: Tavily is down.
    let down = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let down_addr = down.local_addr().unwrap();
    drop(down);
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        common::ServerOptions {
            web_resolve: Some(("google.fixture".into(), address)),
            public_search: Some(url::Url::parse("http://google.fixture/search").unwrap()), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            resolve_extra: vec![("tavily.fixture".into(), down_addr)],
            tavily_search: Some(url::Url::parse("http://tavily.fixture/search").unwrap()), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the refused loopback port.
            ..common::ServerOptions::default()
                .with("logging.audit_queue_lines", "0")
                .with("security.allow_plaintext_key_file", "true")
                .with("web.search.provider", "tavily")
                .with("web.search.fallback_to_public_google", "true")
        },
    )
    .await;
    // The key is read on each search, so saving it after boot is enough.
    cgagentharness::server::env_keys::write_keys(
        &s.home.join(".env"),
        &std::collections::BTreeMap::from([("TAVILY_API_KEY".into(), "fixture-tavily-key".into())]),
    )
    .unwrap();
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://google.fixture/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Find current CVEs"}),
    )
    .await;
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"continue","session_id":sid,"loop":true}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Answered from the fallback listing.", "{reply}");
    let result = &reply["web_tools"][0]["result"];
    assert_eq!(result["provider"], "google-public", "{reply}");
    assert_eq!(result["fallback_from"], json!(["tavily"]), "{reply}");
    assert_eq!(google_hits.load(Ordering::SeqCst), 1);
    let audit = std::fs::read_to_string(s.home.join("logs").join("audit.jsonl")).unwrap();
    let lines: Vec<Value> = audit
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let fallback: Vec<&Value> = lines.iter().filter(|l| l["event"] == "web_search_fallback").collect();
    assert_eq!(fallback.len(), 1, "{audit}");
    assert_eq!(fallback[0]["from"], "tavily");
    assert_eq!(fallback[0]["to"], "google-public");
    let turn = lines
        .iter()
        .rfind(|l| l["event"] == "chat_web_turn")
        .expect("chat_web_turn audit line");
    assert_eq!(turn["providers"], json!(["google-public"]), "{turn}");
    assert_eq!(turn["fallbacks"], 1, "{turn}");
    task.abort();
}

fn tool_call_reply(name: &str, arguments: &str, id: &str) -> Value {
    json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments}}]}}],
        "usage":{"prompt_tokens":10,"completion_tokens":2}})
}

/// After a batch that failed entirely, the model is asked once more without
/// tools, holding each failure (the harness's own code) as that call's result.
/// Before, the turn ended there with a canned failure line and no answer.
fn assert_failed_batch_is_answered(model: &common::MockModel, reply: &Value, code: &str) {
    assert_eq!(reply["reply"], "No web result; answering without it.", "{reply}");
    let request = model.last_request().unwrap();
    assert!(request.get("tools").is_none(), "{request}");
    assert_eq!(request["tool_choice"], "none");
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "tool");
    assert!(last["content"].as_str().unwrap().contains(code), "{last}");
}

/// A tool call, then the answer the model gives once tools are withdrawn.
fn call_then_answer(name: &str, arguments: &str, id: &str) -> Value {
    json!([
        tool_call_reply(name, arguments, id),
        common::ok_reply("No web result; answering without it.", 20, 6)
    ])
}

fn assert_no_serpapi_secret(body: &Value, home: &std::path::Path) {
    let dumped = body.to_string();
    let audit = std::fs::read_to_string(home.join("logs").join("audit.jsonl")).unwrap_or_default();
    for (label, text) in [("HTTP JSON", dumped.as_str()), ("audit", audit.as_str())] {
        assert!(
            !text.contains("fixture-secret-key"),
            "{label} echoed the synthetic SerpAPI fixture key: {text}"
        );
        assert!(
            !text.contains("SERPAPI_API_KEY"),
            "{label} named SERPAPI_API_KEY: {text}"
        );
    }
}

/// The shapes a 7B–27B local model or an older OpenAI-compatible server sends:
/// a complete tool call ending in `stop`, an argument object with a key the
/// schema does not name, a final answer led by an inline `<think>` block and
/// carrying `"tool_calls": null`. Each is a normal round; nothing extra is read.
#[tokio::test]
async fn small_model_tool_call_shapes_complete_one_bounded_round() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r = reads.clone();
    let fixture = Router::new()
        .route("/docs/a", get(move || {
            let r = r.clone();
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "SMALL_MODEL_EVIDENCE")
            }
        }))
        .route("/v1/chat/completions", post(|Json(body): Json<Value>| async move {
            let messages = body["messages"].as_array().unwrap();
            if messages.last().unwrap()["role"] == "tool" {
                assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("SMALL_MODEL_EVIDENCE"));
                return Json(json!({"model":"mock","choices":[{"finish_reason":"stop","message":{"role":"assistant",
                    "content":"<think>\nThe page says SMALL_MODEL_EVIDENCE.\n</think>\n\nThe page was read.","tool_calls":null}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":3}}));
            }
            let mut reply = tool_call_reply(
                "web_fetch",
                &json!({"url":"http://docs.example/docs/a","reason":"the user asked"}).to_string(), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                "call_small",
            );
            reply["choices"][0]["finish_reason"] = json!("stop");
            Json(reply)
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (status, reply) = s.post_json("/api/chat", json!({"message":"read the doc"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["reply"], "The page was read.",
        "the reasoning block is not the answer: {reply}"
    );
    assert_eq!(reply["web_tools"][0]["ok"], true, "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let (_, session) = s
        .get_json(&format!("/api/sessions/{}", reply["session_id"].as_str().unwrap()))
        .await;
    assert!(
        !session.to_string().contains("<think>"),
        "stored history must not re-send reasoning: {session}"
    );
    task.abort();
}

/// The loaded-window warning counts what a web turn adds: a prompt whose
/// history fits Ollama's window can outgrow it once a fetched page is sent back.
#[tokio::test]
async fn window_warning_counts_the_tool_results_a_web_turn_sends() {
    let window = Arc::new(AtomicU64::new(1));
    let reported = window.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let fixture = Router::new()
        .route(
            "/docs/big",
            get(|| async {
                (
                    [("content-type", "text/plain")],
                    "Backoff doubles each retry. ".repeat(280),
                )
            }),
        )
        .route(
            "/api/ps",
            get(move || {
                let n = reported.load(Ordering::SeqCst);
                async move { Json(json!({"models":[{"name":"mock-model","context_length":n}]})) }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if call == 0 || body["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
                        return Json(common::ok_reply("done", 10, 2));
                    }
                    Json(tool_call_reply(
                        "web_fetch",
                        &json!({"url":"http://docs.example/docs/big","reason":"the user asked"}).to_string(), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        "call_big",
                    ))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
                .with("models.local_llm.model", "mock-model")
                .with("models.local_llm.inventory.refresh_sec", "1")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    // A tool-free turn against a 1-token window reports the base projection.
    let (status, plain) = s.post_json("/api/chat", json!({"message":"read the doc"})).await;
    assert_eq!(status, 200, "{plain}");
    let base = plain["context_window"]["projected_tokens"].as_u64().unwrap();
    // A window with room for that prompt warns only because of the fetched page.
    window.store(base + 64, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (status, fetched) = s.post_json("/api/chat", json!({"message":"read the doc"})).await;
    assert_eq!(status, 200, "{fetched}");
    assert_eq!(fetched["web_tools"][0]["ok"], true, "{fetched}");
    assert_eq!(fetched["context_window"]["window_tokens"], base + 64, "{fetched}");
    assert!(
        fetched["context_window"]["projected_tokens"].as_u64().unwrap() > base + 1000,
        "{fetched}"
    );
    task.abort();
}

/// A middle web round can be the turn's largest request: a big result plus the
/// tool definitions, which the final round (calls used up) no longer carries.
/// Its projection must still reach the window check.
#[tokio::test]
async fn window_warning_counts_the_largest_middle_round() {
    let fixture = Router::new()
        .route(
            "/docs/big",
            get(|| async {
                (
                    [("content-type", "text/plain")],
                    "Backoff doubles each retry. ".repeat(280),
                )
            }),
        )
        .route("/docs/tiny", get(|| async { ([("content-type", "text/plain")], "ok") }))
        .route(
            "/api/ps",
            get(|| async { Json(json!({"models":[{"name":"mock-model","context_length":1}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                let messages = body["messages"].as_array().unwrap();
                let flow = messages.iter().find(|m| m["role"] == "user").unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let reads = messages.iter().filter(|m| m["role"] == "tool").count();
                let fetch = |page: &str, id: &str| {
                    Json(tool_call_reply(
                        "web_fetch",
                        &json!({"url":format!("http://docs.example/docs/{page}")}).to_string(), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        id,
                    ))
                };
                match (flow.as_str(), reads) {
                    ("flow b" | "flow c", 0) => fetch("big", "call_big"),
                    ("flow b", 1) => fetch("tiny", "call_tiny"),
                    _ => Json(common::ok_reply("done", 10, 2)),
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
                .with("models.local_llm.model", "mock-model")
                .with("web.chat_tool_calls", "2")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let mut projected = std::collections::BTreeMap::new();
    for flow in ["flow b", "flow c"] {
        let (status, reply) = s.post_json("/api/chat", json!({"message":flow})).await;
        assert_eq!(status, 200, "{reply}");
        projected.insert(flow, reply["context_window"]["projected_tokens"].as_u64().unwrap());
    }
    // Flow c's last request is the big result with the definitions (one call
    // left). Flow b sends that same request in its middle round, then a final
    // one with a small second result and no definitions.
    assert!(projected["flow b"] + 2 >= projected["flow c"], "{projected:?}");
    task.abort();
}

#[tokio::test]
async fn loop_reserves_final_synthesis_without_reporting_successful_reads_as_failures() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, None, None, requests.clone()), 3).await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Summarize the fetched page", "web_mode":"fetch"}),
    )
    .await;
    // About 6,150 input tokens: the page is cut to the follow-up's room, which then
    // holds about 6,950 tokens, so a second round (definitions, a minimal result
    // and a reply) no longer fits and the final answer is reserved.
    let message = format!("fetch the page {}", "x".repeat(4 * (6_150 - base) as usize));
    let (status, body) = s
        .post_json("/api/chat", json!({"session_id":sid,"loop":true,"message":message}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let events = body["web_tools"].as_array().unwrap();
    assert!(
        events
            .iter()
            .filter(|e| e["tool"] != "web_evidence")
            .all(|e| e["ok"] == true),
        "{events:?}"
    );
    assert!(events.iter().any(|e| e["code"] == "WEB_TOOLS_WITHHELD"));
    assert_eq!(
        events.iter().find(|event| event["tool"] == "web_evidence").unwrap()["code"],
        "WEB_ANSWER_INVALID",
        "the deliberately non-JSON digest does not turn the successful read into a failure"
    );
    let requests = requests.lock().unwrap();
    let final_call = requests
        .iter()
        .rev()
        .find(|r| r.get("response_format").is_none())
        .unwrap();
    assert!(final_call.get("tools").is_none());
    assert_eq!(final_call["tool_choice"], "none");
    let digest = requests
        .iter()
        .find(|request| request["response_format"]["type"] == "json_object")
        .unwrap();
    assert_eq!(digest["response_format"]["type"], "json_object");
    assert!(digest.get("tools").is_none(), "the digest cannot call tools");
}

#[tokio::test]
async fn active_goal_reuses_a_validated_digest_with_source_urls_without_another_read() {
    check_goal_evidence_reuse(false, None, true, 6000).await;
}

#[tokio::test]
async fn repeated_goal_reads_reuse_cache_unless_explicitly_refreshed() {
    check_goal_evidence_reuse(true, None, true, 6000).await;
    check_goal_evidence_reuse(true, Some(true), true, 6000).await;
}

/// A model that reports the prompt size the harness itself estimates (bytes/4
/// of every message, plus the tool definitions) must leave the session's token
/// ratio at 1. Before, a turn that restored earlier evidence compared that
/// usage with an estimate that left the restored message out, so the ratio
/// rose by its share and every later budget of the run shrank with it.
#[tokio::test]
async fn restored_evidence_does_not_inflate_token_calibration() {
    let seen = Arc::new(AtomicBool::new(false));
    let restored = seen.clone();
    let router = Router::new()
        .route(
            "/docs/item",
            get(|| async {
                (
                    [("content-type", "text/plain")],
                    format!(
                        "RECOVERY_ANCHOR requires two replicas.{}",
                        " Each replica is checked before failback.".repeat(20)
                    ),
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let restored = restored.clone();
                async move {
                    let messages = body["messages"].as_array().unwrap();
                    let size = |text: &str| text.len().div_ceil(4) as u64;
                    let mut prompt: u64 = messages.iter().map(|m| size(m["content"].as_str().unwrap_or(""))).sum();
                    if let Some(tools) = body.get("tools") {
                        prompt += size(&tools.to_string());
                    }
                    let usage = json!({"prompt_tokens": prompt, "completion_tokens": 10});
                    let answer = |text: &str| {
                        let mut reply = common::ok_reply(text, 0, 0);
                        reply["usage"] = usage.clone();
                        Json(reply)
                    };
                    if body["response_format"]["type"] == "json_object" {
                        return answer(&json!({"supported":[{"text":"Recovery requires two replicas.","citations":[{"id":"S1","quote":"requires two replicas"}]}],"conflicts":[],"inferences":[],"missing":[]}).to_string());
                    }
                    let last = messages.last().unwrap();
                    let content = last["content"].as_str().unwrap_or("");
                    if content.contains("Untrusted active-goal findings") {
                        restored.store(true, Ordering::SeqCst);
                        return answer("The retained finding still supports two replicas.");
                    }
                    if last["role"] == "tool" || content.contains("Untrusted evidence digest") {
                        return answer("Recovery needs two replicas, per http://docs.example/docs/item."); // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
                    }
                    let mut call = tool_call_reply(
                        "web_fetch",
                        &json!({"url":"http://docs.example/docs/item","query":"RECOVERY_ANCHOR"}).to_string(), // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
                        "read",
                    );
                    call["usage"] = usage.clone();
                    Json(call)
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    let goal =
        json!({"goal":"Read the recovery manual, then review its implication in a second step.","web_mode":"fetch"});
    assert_eq!(s.post_json(&format!("/api/sessions/{sid}/goal"), goal).await.0, 200);
    let (status, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid,"max_turns":2,"cooldown_ms":0}),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let run_id = started["run"]["run_id"].as_str().unwrap();
    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
            if matches!(value["run"]["state"].as_str(), Some("done" | "failed" | "stopped")) {
                break value["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["stats"]["turns_completed"], 2, "{run}");
    assert!(seen.load(Ordering::SeqCst), "the second turn must restore evidence");
    let ratio = s
        .state
        .store
        .for_owner("local")
        .get(sid)
        .unwrap()
        .token_calibration
        .expect("usage was reported")
        .ratio;
    assert!(ratio < 1.01, "restored evidence must not inflate the token ratio");
    fixture.abort();
}

#[tokio::test]
async fn invalid_digest_retains_passages_for_the_next_goal_turn() {
    check_goal_evidence_reuse(true, None, false, 6000).await;
}

#[tokio::test]
async fn restored_passages_yield_to_the_bounded_digest_when_context_is_tight() {
    check_goal_evidence_reuse(false, None, true, 512).await;
}

async fn check_goal_evidence_reuse(reread: bool, refresh: Option<bool>, valid_digest: bool, evidence_budget: u64) {
    let reads = Arc::new(AtomicUsize::new(0));
    let digests = Arc::new(AtomicUsize::new(0));
    let restored = Arc::new(AtomicBool::new(false));
    let (r, d, seen) = (reads.clone(), digests.clone(), restored.clone());
    let router = Router::new()
        .route("/docs/item", get(move || {
            r.fetch_add(1, Ordering::SeqCst);
            async move { ([("content-type", "text/plain")], format!("RECOVERY_ANCHOR requires two replicas.{}", if evidence_budget == 512 { " A source qualification.".repeat(40) } else { String::new() })) }
        }))
        .route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
            let (d, seen) = (d.clone(), seen.clone());
            async move {
                let messages = body["messages"].as_array().unwrap();
                if body["response_format"]["type"] == "json_object" {
                    d.fetch_add(1, Ordering::SeqCst);
                    assert!(body["max_tokens"].as_u64().unwrap() <= 1024);
                    if !valid_digest { return Json(common::ok_reply("invalid digest", 100, 40)); }
                    return Json(common::ok_reply(&json!({"supported":[{"text":"Recovery requires two replicas.","citations":[{"id":"S1","quote":"requires two replicas"}]}],"conflicts":[],"inferences":[],"missing":[]}).to_string(), 100, 40));
                }
                let last = messages.last().unwrap();
                let content = last["content"].as_str().unwrap_or("");
                let fetch = || {
                    let mut args = json!({"url":"http://docs.example/docs/item","query":"RECOVERY_ANCHOR replicas"});
                    if let Some(refresh) = refresh { args["refresh"] = json!(refresh); }
                    Json(tool_call_reply("web_fetch", &args.to_string(), "reread"))
                };
                if content.contains("Untrusted active-goal findings") {
                    let context: Value = serde_json::from_str(content).unwrap();
                    assert!(context["current_turn"].as_str().unwrap().starts_with("Loop turn 2 of 2."));
                    assert_eq!(context["sources"][0]["url"], "http://docs.example/docs/item");
                    assert!(content.len().div_ceil(4) <= evidence_budget as usize);
                    if evidence_budget == 512 {
                        assert!(context["passages"].as_array().unwrap().is_empty());
                    } else {
                        assert!(context["passages"][0]["text"].as_str().unwrap().contains("requires two replicas"));
                    }
                    if valid_digest {
                        assert_eq!(context["findings"]["supported"][0]["citations"][0]["id"], context["sources"][0]["id"]);
                    } else {
                        assert!(context["findings"].is_null());
                        assert!(context["passages"][0]["text"].as_str().unwrap().contains("requires two replicas"));
                    }
                    seen.store(true, Ordering::SeqCst);
                    if reread { return fetch(); }
                    return Json(common::ok_reply("The retained finding still supports two replicas.", 100, 10));
                }
                if last["role"] == "tool" {
                    assert!(d.load(Ordering::SeqCst) > 0, "digest must precede the next tool decision");
                    let result: Value = serde_json::from_str(content).unwrap();
                    let tool_count = messages.iter().filter(|m| m["role"] == "tool").count();
                    let later_turn = messages.iter().any(|m| m["content"].as_str().unwrap_or("").contains("Untrusted active-goal findings"));
                    assert_eq!(result["cache_reused"], reread && (tool_count > 1 || later_turn) && refresh != Some(true));
                    if reread && tool_count == 1 && !later_turn { return fetch(); }
                }
                if content.contains("Untrusted evidence digest") {
                    let context: Value = serde_json::from_str(content).unwrap();
                    assert!(context["current_turn"].as_str().unwrap().starts_with("Loop turn 1 of 2."));
                    if reread { return fetch(); }
                }
                if last["role"] == "tool" || content.contains("Untrusted evidence digest") {
                    return Json(common::ok_reply("Recovery needs two replicas, per http://docs.example/docs/item.", 100, 10));
                }
                Json(tool_call_reply("web_fetch", &json!({"url":"http://docs.example/docs/item","query":"RECOVERY_ANCHOR"}).to_string(), "read"))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default().with("web.evidence_tokens", &evidence_budget.to_string())
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    assert_eq!(
        s.post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"Read the recovery manual, then review its implication in a second step.","web_mode":"fetch"})
        )
        .await
        .0,
        200
    );
    let (status, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid,"max_turns":2,"cooldown_ms":0}),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let run_id = started["run"]["run_id"].as_str().unwrap();
    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
            if matches!(value["run"]["state"].as_str(), Some("done" | "failed" | "stopped")) {
                break value["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["stats"]["turns_completed"], 2, "{run}");
    assert!(
        restored.load(Ordering::SeqCst),
        "the second turn must receive validated references"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        if reread && refresh == Some(true) { 3 } else { 1 }
    );
    assert_eq!(
        digests.load(Ordering::SeqCst),
        1,
        "unchanged evidence needs no new digest inference"
    );
    fixture.abort();
}

#[tokio::test]
async fn truncated_goal_digest_counts_reported_usage() {
    let missing_usage = Arc::new(AtomicBool::new(false));
    let missing = missing_usage.clone();
    let router = Router::new()
        .route(
            "/docs/item",
            get(|| async { ([("content-type", "text/plain")], "Relevant source passage.") }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let missing = missing.clone();
                async move {
                    if body["response_format"]["type"] == "json_object" {
                        let mut reply = common::ok_reply("partial", 100, 40);
                        reply["choices"][0]["finish_reason"] = json!("length");
                        if missing.load(Ordering::SeqCst) {
                            reply["usage"].as_object_mut().unwrap().remove("completion_tokens");
                        }
                        return Json(reply);
                    }
                    Json(tool_call_reply(
                        "web_fetch",
                        &json!({"url":"http://docs.example/docs/item","query":"source"}).to_string(),
                        "read",
                    )) // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default().with("loop.max_failures", "1")
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
        .await; // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the source", "web_mode":"fetch"}),
    )
    .await;
    let (status, reply) = s
        .post_json(
            "/api/chat",
            json!({"session_id":sid,"loop":true,"message":"Read the source"}),
        )
        .await;
    assert_ne!(status, 200, "{reply}");
    assert_eq!(reply["detail"]["details"]["known_completion_tokens"], 42, "{reply}");
    assert_eq!(reply["detail"]["details"]["completion_tokens"], 42, "{reply}");
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let row: Value = serde_json::from_str(ledger.lines().last().unwrap()).unwrap();
    assert_eq!(row["output_tokens"], 42, "{row}");
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the source", "web_mode":"fetch"}),
    )
    .await;
    let (_, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid,"max_turns":1,"cooldown_ms":0}),
        )
        .await;
    let run_id = started["run"]["run_id"].as_str().unwrap();
    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
            if value["run"]["state"] == "failed" {
                break value["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["stats"]["total_completion_tokens"], 42, "{run}");
    missing_usage.store(true, Ordering::SeqCst);
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the source", "web_mode":"fetch"}),
    )
    .await;
    let (_, reply) = s
        .post_json(
            "/api/chat",
            json!({"session_id":sid,"loop":true,"message":"Read the source"}),
        )
        .await;
    assert!(reply["detail"]["details"]["completion_tokens"].is_null(), "{reply}");
    assert_eq!(reply["detail"]["details"]["known_completion_tokens"], 2, "{reply}");
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let row: Value = serde_json::from_str(ledger.lines().last().unwrap()).unwrap();
    assert!(row["output_tokens"].is_null(), "{row}");
    assert_eq!(row["usage_missing"], true, "{row}");
    fixture.abort();
}

#[tokio::test]
async fn refreshed_goal_digest_excludes_superseded_passages() {
    let reads = Arc::new(AtomicUsize::new(0));
    let digests = Arc::new(AtomicUsize::new(0));
    let (r, d) = (reads.clone(), digests.clone());
    let router = Router::new()
        .route(
            "/docs/item",
            get(move || {
                let n = r.fetch_add(1, Ordering::SeqCst);
                async move {
                    (
                        [("content-type", "text/plain")],
                        if n == 0 {
                            "OLD_VERSION evidence"
                        } else {
                            "NEW_VERSION evidence"
                        },
                    )
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let d = d.clone();
                async move {
                    if body["response_format"]["type"] == "json_object" {
                        let n = d.fetch_add(1, Ordering::SeqCst);
                        let sent = body.to_string();
                        if n == 0 {
                            assert!(sent.contains("OLD_VERSION"), "{sent}");
                        } else {
                            assert!(sent.contains("NEW_VERSION") && !sent.contains("OLD_VERSION"), "{sent}");
                        }
                        return Json(common::ok_reply(
                            &json!({"supported":[],"conflicts":[],"inferences":[],"missing":[]}).to_string(),
                            10,
                            2,
                        ));
                    }
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let content = last["content"].as_str().unwrap_or("");
                    if last["role"] == "tool" {
                        let second = messages
                            .iter()
                            .any(|m| m["content"].as_str().unwrap_or("").starts_with("Loop turn 2"));
                        return Json(common::ok_reply(
                            if second {
                                "Reviewed the refreshed page."
                            } else {
                                "Read the original page."
                            },
                            10,
                            2,
                        ));
                    }
                    if content.contains("Untrusted active-goal findings") || content.starts_with("Loop turn 1") {
                        return Json(tool_call_reply(
                            "web_fetch",
                            &json!({"url":"http://docs.example/docs/item","query":"evidence","refresh":true})
                                .to_string(),
                            "read",
                        )); // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
                    }
                    Json(common::ok_reply("No read requested.", 10, 2))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
        .await; // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the evidence", "web_mode":"fetch"}),
    )
    .await;
    let (_, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid,"max_turns":2,"cooldown_ms":0}),
        )
        .await;
    let run_id = started["run"]["run_id"].as_str().unwrap();
    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
            if matches!(value["run"]["state"].as_str(), Some("done" | "failed" | "stopped")) {
                break value["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["stats"]["turns_completed"], 2, "{run}");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(digests.load(Ordering::SeqCst), 2);
    fixture.abort();
}

#[tokio::test]
async fn restored_goal_evidence_fits_the_remaining_turn_budget() {
    let seen = Arc::new(AtomicBool::new(false));
    let checked = seen.clone();
    let router = Router::new()
        .route(
            "/docs/item",
            get(|| async {
                (
                    [("content-type", "text/plain")],
                    format!("SOURCE_MARKER {}", "qualification ".repeat(700)),
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let checked = checked.clone();
                async move {
                    if body["response_format"]["type"] == "json_object" {
                        return Json(common::ok_reply(
                            &json!({"supported":[],"conflicts":[],"inferences":[],"missing":[]}).to_string(),
                            100,
                            2,
                        ));
                    }
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let content = last["content"].as_str().unwrap_or("");
                    if content.contains("Untrusted active-goal findings") || content.starts_with("Loop turn 2") {
                        checked.store(true, Ordering::SeqCst);
                        return Json(common::ok_reply("Second turn completed.", 100, 2));
                    }
                    if last["role"] == "tool" || content.contains("Untrusted evidence digest") {
                        return Json(common::ok_reply(&format!("First turn. {}", "x".repeat(17500)), 100, 2));
                    }
                    Json(tool_call_reply(
                        "web_fetch",
                        &json!({"url":"http://docs.example/docs/item","query":"SOURCE_MARKER"}).to_string(),
                        "read",
                    )) // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
        }
        .with("models.local_llm.max_tokens", "2048")
        .with("web.total_tokens", "16000"),
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
        .await; // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let sid = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal":"Read the source and review it", "web_mode":"fetch"}),
    )
    .await;
    let (_, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid,"max_turns":2,"cooldown_ms":0}),
        )
        .await;
    let run_id = started["run"]["run_id"].as_str().unwrap();
    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
            if matches!(value["run"]["state"].as_str(), Some("done" | "failed" | "stopped")) {
                break value["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["stats"]["turns_completed"], 2, "{run}");
    assert!(seen.load(Ordering::SeqCst), "the second turn must reach inference");
    fixture.abort();
}

/// A page site and a stand-in for public Google whose one result is that
/// site's `/page`. Returns the address both hosts resolve to and how often
/// `/page` and `/other` were read.
async fn listing_fixture() -> (std::net::SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let count = reads.clone();
    let page = move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            ([("content-type", "text/plain")], "LISTED_PAGE_EVIDENCE")
        }
    };
    let router = Router::new()
        .route(
            "/search",
            get(|| async { "<a href='/url?q=http%3A%2F%2Fdocs.example%2Fpage'><h3>Listed page</h3></a>" }),
        )
        .route("/page", get(page.clone()))
        .route("/other", get(page));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, reads, task)
}

/// A server whose public search is `listing_fixture` and whose URL policy
/// permits only that search host: the listed page itself is not permitted.
async fn listing_server(
    model: &common::MockModel,
    address: std::net::SocketAddr,
    result_reads: bool,
) -> common::TestServer {
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("google.fixture".into(), address)),
            resolve_extra: vec![("docs.example".into(), address)],
            public_search: Some(url::Url::parse("http://google.fixture/search").unwrap()), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            ..common::ServerOptions::default()
                .with("logging.audit_queue_lines", "0")
                .with("web.pace_ms", "100")
                .with(
                    "web.search.result_reads.enabled",
                    if result_reads { "true" } else { "false" },
                )
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://google.fixture/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    s
}

const LISTED_PAGE: &str = r#"{"url":"http://docs.example/page"}"#; // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.

/// The measured failure (issue #772): a search succeeds, the read of its
/// result is refused, tools are withdrawn, and the model returns no text on
/// the call that offers none. Before, that was 502 EMPTY_MODEL_RESPONSE and
/// the search result was lost. Now the harness asks once more in the same
/// turn, and if that is empty too it writes the reply itself.
#[tokio::test]
async fn an_empty_final_reply_after_a_search_and_a_refused_read_is_retried_once_then_written_by_the_harness() {
    let (address, reads, task) = listing_fixture().await;
    let model = common::start_mock_model().await;
    let s = listing_server(&model, address, false).await;

    // The retry answers.
    model.set_reply(json!([
        tool_call_reply("web_search", r#"{"query":"listed page"}"#, "final_search"),
        tool_call_reply("web_fetch", LISTED_PAGE, "final_fetch"),
        common::ok_reply("", 20, 63),
        common::ok_reply("Answered from the listing.", 30, 9)
    ]));
    let (status, reply) = s.post_json("/api/chat", json!({"message":"Search and tell me"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Answered from the listing.", "{reply}");
    let tools = reply["web_tools"].as_array().unwrap();
    assert_eq!(
        tools.len(),
        2,
        "the mixed events are kept and nothing is added: {reply}"
    );
    assert_eq!(
        (&tools[0]["tool"], &tools[0]["ok"]),
        (&json!("web_search"), &json!(true))
    );
    assert_eq!(
        (&tools[1]["ok"], &tools[1]["code"]),
        (&json!(false), &json!("WEB_HOST_DENIED"))
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0, "a listing grants no read as shipped");
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 4, "exactly one extra model call");
        let empty = &requests[2];
        assert!(empty.get("tools").is_none(), "{empty}");
        assert_eq!(empty["messages"].as_array().unwrap().last().unwrap()["role"], "tool");
        let retry = &requests[3];
        assert!(retry.get("tools").is_none(), "the retry offers no tools: {retry}");
        let note = retry["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(note["role"], "user");
        assert!(
            note["content"].as_str().unwrap().contains("no web tools are available"),
            "{note}"
        );
    }
    // Both calls of the final step are billed, and the note is not stored.
    assert_eq!(reply["usage"]["prompt_tokens"], 10 + 10 + 20 + 30, "{reply}");
    assert_eq!(reply["usage"]["completion_tokens"], 2 + 2 + 63 + 9, "{reply}");
    let sid = reply["session_id"].as_str().unwrap();
    let (_, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert!(!session["messages"].to_string().contains("Harness note"), "{session}");
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    assert!(audit.contains(r#""event":"web_final_empty""#) && audit.contains(r#""action":"retry""#));

    // The retry is empty too: the harness writes the reply and marks the turn.
    model.requests.lock().unwrap().clear();
    model.set_reply(json!([
        tool_call_reply("web_search", r#"{"query":"listed page"}"#, "final_search"),
        tool_call_reply("web_fetch", LISTED_PAGE, "final_fetch"),
        common::ok_reply("", 20, 63)
    ]));
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"Search and tell me again"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    let text = reply["reply"].as_str().unwrap();
    assert!(
        text.starts_with("The model returned no answer after its web calls"),
        "{text}"
    );
    let listed = "Listed page — http://docs.example/page"; // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
    let refused = "Not read: http://docs.example/page (WEB_HOST_DENIED)"; // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
    assert!(text.contains(listed) && text.contains(refused), "{text}");
    let tools = reply["web_tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3, "{reply}");
    assert_eq!(tools[0]["ok"], true);
    assert_eq!(tools[1]["code"], "WEB_HOST_DENIED");
    assert_eq!(
        (&tools[2]["ok"], &tools[2]["code"]),
        (&json!(false), &json!("WEB_FINAL_EMPTY"))
    );
    assert_eq!(model.requests.lock().unwrap().len(), 4, "one retry, never a second");
    assert_eq!(reply["usage"]["completion_tokens"], 2 + 2 + 63 + 63, "{reply}");
    assert!(!s.state.generation_gate.is_held());
    task.abort();
}

/// `web.search.result_reads` (off as shipped): a turn may read the exact URL
/// its own search listed. Not another URL on that host, not on a later turn,
/// not past a blocked rule, and nothing is written to the saved policy.
#[tokio::test]
async fn a_turn_may_read_the_exact_urls_its_own_search_listed_when_enabled() {
    let (address, reads, task) = listing_fixture().await;
    let model = common::start_mock_model().await;
    let s = listing_server(&model, address, true).await;
    let saved = std::fs::read(s.home.join("data/tools/web_allowlist.json")).ok();
    let search_then = |url: &str, answer: &str| {
        json!([
            tool_call_reply("web_search", r#"{"query":"listed page"}"#, "grant_search"),
            tool_call_reply("web_fetch", &json!({"url":url}).to_string(), "grant_fetch"),
            common::ok_reply(answer, 20, 6)
        ])
    };

    model.set_reply(search_then("http://docs.example/page", "Read the listed page.")); // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
    let (status, reply) = s.post_json("/api/chat", json!({"message":"Search and read"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Read the listed page.", "{reply}");
    let tools = reply["web_tools"].as_array().unwrap();
    assert_eq!(tools[0]["ok"], true, "{reply}");
    assert!(
        tools[0]["result"]["notice"]
            .as_str()
            .unwrap()
            .contains("may read listed URLs"),
        "{reply}"
    );
    assert_eq!(tools[1]["ok"], true, "{reply}");
    assert_eq!(tools[1]["result"]["granted_by"], "search_result", "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let handed = model.last_request().unwrap()["messages"].to_string();
    assert!(handed.contains("LISTED_PAGE_EVIDENCE"), "{handed}");
    let sid = reply["session_id"].as_str().unwrap().to_string();

    // The next turn starts with no grant: the same URL is refused again.
    model.set_reply(json!([
        tool_call_reply("web_fetch", LISTED_PAGE, "later_fetch"),
        common::ok_reply("No web result; answering without it.", 20, 6)
    ]));
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"Read it again", "session_id":sid}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_HOST_DENIED", "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1, "a grant ends with its turn");

    // Exact URL only: another path on the listed host is not granted.
    model.set_reply(search_then("http://docs.example/other", "Only the listing.")); // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"Search and read another"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][1]["code"], "WEB_HOST_DENIED", "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);

    // The saved policy holds no grant, and every granted read is audited.
    assert_eq!(
        std::fs::read(s.home.join("data/tools/web_allowlist.json")).ok(),
        saved,
        "the saved URL policy is unchanged"
    );
    let (_, web) = s.get_json("/api/web").await;
    assert!(!web.to_string().contains("docs.example"), "{web}");
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    assert_eq!(audit.matches(r#""event":"web_result_read""#).count(), 1, "{audit}");
    assert!(audit.contains(r#""event":"web_result_grants""#), "{audit}");

    // A blocked rule wins over a grant.
    let (status, blocked) = s
        .post_json("/api/web/block", json!({"urls":["http://docs.example/*"]})) // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
        .await;
    assert_eq!(status, 200, "{blocked}");
    model.set_reply(search_then("http://docs.example/page", "Blocked.")); // DevSkim: ignore DS137138 because docs.example resolves only to the loopback fixture.
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"Search and read blocked"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][1]["code"], "WEB_BLOCKED", "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    task.abort();
}

/// A server-run `/loop` carries its search-result grants from one turn to the
/// next: turn 1 searches, turn 2 reads the page that search listed.
#[tokio::test]
async fn a_loop_run_keeps_its_search_result_grants_between_turns() {
    let (address, reads, task) = listing_fixture().await;
    let handed = Arc::new(AtomicBool::new(false));
    let saw = handed.clone();
    let model_router = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"mock-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let saw = saw.clone();
                async move {
                    // The evidence digest after a read asks for JSON; give it none.
                    if body.get("response_format").is_some() {
                        return Json(common::ok_reply("{}", 5, 1));
                    }
                    let messages = body["messages"].as_array().unwrap();
                    let results: Vec<_> = messages.iter().filter(|m| m["role"] == "tool").collect();
                    if results.iter().any(|m| {
                        m["content"]
                            .as_str()
                            .is_some_and(|c| c.contains("LISTED_PAGE_EVIDENCE"))
                    }) {
                        saw.store(true, Ordering::SeqCst);
                    }
                    let second = messages.iter().any(|m| {
                        m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.starts_with("Loop turn 2"))
                    });
                    Json(match (second, results.is_empty()) {
                        (false, true) => tool_call_reply("web_search", r#"{"query":"listed page"}"#, "loop_search"),
                        (true, true) => tool_call_reply("web_fetch", LISTED_PAGE, "loop_fetch"),
                        (false, false) => common::ok_reply("Listing noted.", 10, 3),
                        (true, false) => common::ok_reply("Read the listed page.", 10, 3),
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
    let model_address = listener.local_addr().unwrap();
    let model_task = tokio::spawn(async move { axum::serve(listener, model_router).await.unwrap() });
    let model = common::MockModel {
        base: format!("http://{model_address}"), // DevSkim: ignore DS137138 because this test-only model binds only to loopback.
        reply: Arc::default(),
        requests: Arc::default(),
        delay_ms: Arc::default(),
    };
    let s = listing_server(&model, address, true).await;
    let (_, session) = s.post_json("/api/sessions", json!({"title":"loop"})).await;
    let sid = session["session_id"].as_str().unwrap().to_string();
    let (status, goal) = s
        .post_json(
            &format!("/api/sessions/{sid}/goal"),
            json!({"goal":"read the listed page"}),
        )
        .await;
    assert_eq!(status, 200, "{goal}");
    let (status, started) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":sid, "max_turns":2, "cooldown_ms":0}),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let run_id = started["run"]["run_id"].as_str().unwrap().to_string();
    let mut run = Value::Null;
    for _ in 0..400 {
        run = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await.1["run"].clone();
        if run["stats"]["turns_completed"] == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(run["stats"]["turns_completed"], 2, "{run}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "turn 2 read the page turn 1's search listed"
    );
    assert!(handed.load(Ordering::SeqCst), "the model was handed the page text");
    task.abort();
    model_task.abort();
}
