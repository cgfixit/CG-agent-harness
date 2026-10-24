//! Server-side `/loop` runner: config exposure, start/status/stop round-trip,
//! unlimited-turn behavior, validation refusals, owner binding and per-turn
//! account revalidation.

mod common;

use common::{code, ok_reply, spawn_server, start_mock_model, ServerOptions};
use serde_json::{json, Value};

/// A mock OpenAI-compatible endpoint whose completion text increments per
/// request ("step 1", "step 2", ...), with reported usage.
async fn start_counting_mock() -> (String, std::sync::Arc<std::sync::atomic::AtomicU64>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let count = Arc::new(AtomicU64::new(0));
    let c2 = count.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |_body: axum::Json<Value>| {
            let c2 = c2.clone();
            async move {
                let n = c2.fetch_add(1, Ordering::SeqCst) + 1;
                (
                    axum::http::StatusCode::OK,
                    axum::Json(ok_reply(&format!("step {n}"), 10, 2)),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}/v1", addr.port()), count) // DevSkim: ignore DS137138,DS162092 because this test-only model has no credentials and binds only to loopback.
}

async fn session_with_goal(s: &common::TestServer) -> String {
    let (_, created) = s.post_json("/api/sessions", json!({"title": "loop"})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    let (status, goal) = s
        .post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "ship it"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(goal["goal"], "ship it");
    sid
}

async fn wait_for<F>(s: &common::TestServer, run_id: &str, what: &str, mut ready: F) -> Value
where
    F: FnMut(&Value) -> bool,
{
    for _ in 0..200 {
        let run = loop_status(s, run_id).await;
        if ready(&run) {
            return run;
        }
        if run["state"]
            .as_str()
            .is_some_and(|st| ["stopped", "failed", "done"].contains(&st))
            && !ready(&run)
        {
            panic!("run reached terminal state while waiting for {what}: {run}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn loop_status(s: &common::TestServer, run_id: &str) -> Value {
    let (status, body) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
    assert_eq!(status, 200, "{body}");
    body["run"].clone()
}

#[tokio::test]
async fn failed_loop_turn_counts_compaction_before_the_rejected_reply() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    for (finish, text, reported, expected) in [
        ("length", "unfinished", true, 240),
        ("stop", "", true, 240),
        ("length", "unfinished", false, 20),
        ("stop", "", false, 20),
    ] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                calls.fetch_add(1, Ordering::SeqCst);
                let summary = body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("Summarize this chat history");
                let mut reply = ok_reply(
                    if summary { "Earlier steps were reviewed." } else { text },
                    100,
                    if summary { 20 } else { 220 },
                );
                if !summary {
                    reply["choices"][0]["finish_reason"] = json!(finish);
                    if !reported {
                        reply["usage"].as_object_mut().unwrap().remove("completion_tokens");
                    }
                }
                async move { axum::Json(reply) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
        let addr = listener.local_addr().unwrap();
        let mock = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let s = spawn_server(
            &format!("http://{addr}/v1"), // DevSkim: ignore DS137138 because this synthetic model uses no credentials and binds only to loopback.
            ServerOptions::default()
                .with("chat.compact_prompt_tokens", "4096")
                .with("chat.compact_keep_messages", "2")
                .with("api.harness_loop_rate_limit.max_tokens", "256"),
        )
        .await;
        assert_eq!(s.post_json("/api/web", json!({"enabled":false})).await.0, 200);
        let sid = session_with_goal(&s).await;
        for i in 0..6 {
            s.state
                .store
                .for_owner("local")
                .record_exchange(
                    &sid,
                    &format!("u{i} {}", "u".repeat(2000)),
                    &format!("a{i} {}", "a".repeat(2000)),
                    "fixture",
                    &cgagentharness::server::sessions::TokenTally::default(),
                    &[],
                )
                .unwrap();
        }
        let before = s.state.store.for_owner("local").get(&sid).unwrap().messages;
        let (status, body) = s.post_json("/api/loop/start", json!({
            "session_id":sid,"max_turns":3,"max_total_completion_tokens":expected - 10,"max_failures":3,"cooldown_ms":0
        })).await;
        assert_eq!(status, 200, "{body}");
        let run_id = body["run"]["run_id"].as_str().unwrap();
        let run = wait_for(&s, run_id, "the failed turn's budget stop", |run| {
            run["state"] == "done" || run["state"] == "failed"
        })
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 2, "{finish}/{reported}: {run}");
        assert_eq!(run["stats"]["total_completion_tokens"], expected, "{run}");
        assert_eq!(run["stats"]["turns_completed"], 0);
        assert_eq!(run["stats"]["consecutive_failures"], 1);
        assert_eq!(run["state"], "done");
        assert_eq!(s.state.store.for_owner("local").get(&sid).unwrap().messages, before);
        mock.abort();
    }
}

#[tokio::test]
async fn loop_budget_counts_failed_inference_without_successful_exchanges() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let mut unknown = ok_reply("", 0, 0);
    unknown.as_object_mut().unwrap().remove("usage");
    let mut truncated = ok_reply("unfinished", 100, 220);
    truncated["choices"][0]["finish_reason"] = json!("length");
    for (name, replies, total, turns, failures, state) in [
        ("empty", vec![ok_reply("", 100, 220)], 220, 0, 1, "done"),
        ("truncated", vec![truncated], 220, 0, 1, "done"),
        ("unknown", vec![unknown.clone(), unknown], 0, 0, 2, "failed"),
        (
            "success then failure",
            vec![ok_reply("progress", 100, 20), ok_reply("", 100, 220)],
            240,
            1,
            1,
            "done",
        ),
        (
            "failure then success",
            vec![ok_reply("", 100, 20), ok_reply("progress", 100, 80)],
            100,
            1,
            0,
            "done",
        ),
    ] {
        let expected_requests = replies.len();
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                // An unexpected retry remains observable without crashing the mock.
                let reply = replies[n.min(replies.len() - 1)].clone();
                async move { axum::Json(reply) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
        let addr = listener.local_addr().unwrap();
        let mock = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let s = spawn_server(&format!("http://{addr}/v1"), ServerOptions::default()).await; // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        let sid = session_with_goal(&s).await;
        let (status, body) = s
            .post_json(
                "/api/loop/start",
                json!({
                    "session_id": sid,
                    "max_turns": 3,
                    "max_total_completion_tokens": 100,
                    "max_failures": 2,
                    "cooldown_ms": 0,
                }),
            )
            .await;
        assert_eq!(status, 200, "{name}: {body}");
        let run_id = body["run"]["run_id"].as_str().unwrap();
        let run = wait_for(&s, run_id, "budget or failure stop", |run| run["state"] == state).await;
        assert_eq!(count.load(Ordering::SeqCst), expected_requests, "{name}: {run}");
        assert_eq!(run["stats"]["total_completion_tokens"], total, "{name}: {run}");
        assert_eq!(run["stats"]["turns_completed"], turns, "{name}: {run}");
        assert_eq!(run["stats"]["consecutive_failures"], failures, "{name}: {run}");
        let reason = run["reason"].as_str().unwrap();
        assert!(
            reason.contains(if state == "done" {
                "token budget exhausted"
            } else {
                "consecutive chat failures"
            }),
            "{name}: {run}"
        );
        let (_, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
        assert_eq!(session["tokens"]["exchanges"], turns, "{name}: {session}");
        assert_eq!(
            session["messages"].as_array().unwrap().len(),
            turns as usize * 2,
            "{name}: {session}"
        );
        let persisted: Value = serde_json::from_str(
            &std::fs::read_to_string(s.home.join("loops").join(format!("{run_id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted["stats"], run["stats"], "{name}");
        mock.abort();
    }
}

#[tokio::test]
async fn status_exposes_loop_config_and_server_loop_flag() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s.get_json("/api/status").await;
    assert_eq!(status, 200);
    assert_eq!(body["server_loop"], true);
    assert_eq!(body["loop"]["max_turns"], 0);
    assert_eq!(body["loop"]["max_total_completion_tokens"], 0);
    assert_eq!(body["loop"]["max_failures"], 2);
    assert_eq!(body["loop"]["cooldown_ms"], 2000);
}

#[tokio::test]
async fn loop_start_status_stop_round_trip() {
    let (url, _count) = start_counting_mock().await;
    let s = spawn_server(
        &url,
        ServerOptions::default()
            .with("loop.cooldown_ms", "0")
            .with("api.harness_loop_rate_limit.max_requests", "1000"),
    )
    .await;
    let sid = session_with_goal(&s).await;
    let (status, body) = s
        .post_json("/api/loop/start", json!({"session_id": sid, "max_turns": 50}))
        .await;
    assert_eq!(status, 200, "{body}");
    let run = body["run"].clone();
    assert_eq!(run["state"], "running");
    assert_eq!(run["session_id"], sid);
    let run_id = run["run_id"].as_str().unwrap().to_string();

    let run = wait_for(&s, &run_id, "the first loop turn", |run| {
        run["stats"]["turns_completed"].as_u64().unwrap_or(0) >= 1
    })
    .await;
    assert!(run["stats"]["turns_completed"].as_u64().unwrap_or(0) >= 1);

    // The SSE stream replays the run snapshot and streams per-turn events.
    let mut stream = s
        .req(reqwest::Method::GET, &format!("/api/loop/events/{run_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), reqwest::StatusCode::OK);
    let mut frames = String::new();
    while !frames.contains("\"type\":\"turn\"") {
        let chunk = stream.chunk().await.unwrap();
        let Some(bytes) = chunk else {
            panic!("SSE stream ended early: {frames}")
        };
        frames.push_str(std::str::from_utf8(&bytes).unwrap());
    }
    assert!(frames.contains("\"state\":\"running\""), "{frames}");

    let (status, stopped) = s.post_json("/api/loop/stop", json!({"run_id": run_id})).await;
    assert_eq!(status, 200, "{stopped}");
    let run = wait_for(&s, &run_id, "a stopped run", |run| {
        run["state"].as_str() == Some("stopped")
    })
    .await;
    assert_eq!(run["state"], "stopped");

    // The run state was persisted next to the home's loops/ directory.
    let persisted = std::fs::read_dir(s.home.join("loops")).unwrap();
    assert!(
        persisted
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy() == format!("{run_id}.json")),
        "run state persisted under loops/"
    );
    // A new run for the session replaces the finished one in memory; the old
    // run's record stays on disk.
    let (status, body) = s
        .post_json("/api/loop/start", json!({"session_id": sid, "max_turns": 50}))
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = s.get_json(&format!("/api/loop/status?run_id={run_id}")).await;
    assert_eq!(status, 404, "a finished run is evicted when its session starts another");
    assert!(s.home.join("loops").join(format!("{run_id}.json")).is_file());
    let next = body["run"]["run_id"].as_str().unwrap().to_string();
    s.post_json("/api/loop/stop", json!({"run_id": next})).await;
}

#[tokio::test]
async fn unlimited_turns_do_not_stop_at_five() {
    let (url, count) = start_counting_mock().await;
    let s = spawn_server(
        &url,
        ServerOptions::default()
            .with("loop.max_turns", "0")
            .with("loop.max_total_completion_tokens", "0")
            .with("loop.cooldown_ms", "0")
            .with("api.harness_loop_rate_limit.max_requests", "1000"),
    )
    .await;
    let sid = session_with_goal(&s).await;
    let (status, body) = s.post_json("/api/loop/start", json!({"session_id": sid})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run"]["config"]["max_turns"], 0);
    let run_id = body["run"]["run_id"].as_str().unwrap().to_string();

    // Well past the shipped 5-turn cap, the run is still turning.
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if count.load(std::sync::atomic::Ordering::SeqCst) >= 8 {
            break;
        }
    }
    assert!(
        count.load(std::sync::atomic::Ordering::SeqCst) >= 8,
        "the mock served at least 8 loop turns"
    );
    let run = loop_status(&s, &run_id).await;
    assert_eq!(run["state"], "running", "{run}");
    assert!(run["stats"]["turns_completed"].as_u64().unwrap_or(0) >= 7, "{run}");

    s.post_json("/api/loop/stop", json!({"run_id": run_id})).await;
    let run = wait_for(&s, &run_id, "a stopped run", |run| {
        run["state"].as_str() == Some("stopped")
    })
    .await;
    assert_eq!(run["state"], "stopped");
}

#[tokio::test]
async fn loop_start_validates_goal_config_and_run_ids() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, created) = s.post_json("/api/sessions", json!({"title": "t"})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();

    // No stored goal and none on the request.
    let (status, body) = s.post_json("/api/loop/start", json!({"session_id": sid})).await;
    assert_eq!(status, 400);
    assert_eq!(code(&body), "LOOP_REQUIRES_GOAL");

    // A goal on the request persists on the session, then the run starts.
    let (status, body) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id": sid, "goal": "ship it", "cooldown_ms": 0}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let run_id = body["run"]["run_id"].as_str().unwrap().to_string();
    let (status, session) = s.get_json(&format!("/api/sessions/{sid}")).await;
    assert_eq!(status, 200);
    assert_eq!(session["goal"], "ship it");

    // A live run for the session is refused.
    let (status, body) = s
        .post_json("/api/loop/start", json!({"session_id": sid, "max_turns": 1}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(code(&body), "LOOP_IN_FLIGHT");

    // Out-of-range overrides and unknown web modes refuse before anything runs.
    let (status, body) = s
        .post_json("/api/loop/start", json!({"session_id": sid, "max_turns": 1000001}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(code(&body), "LOOP_CONFIG_INVALID");
    let (status, body) = s
        .post_json("/api/loop/start", json!({"session_id": sid, "web_mode": "sideways"}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(code(&body), "LOOP_WEB_MODE_INVALID");

    let (_, _) = s.post_json("/api/loop/stop", json!({"run_id": run_id})).await;

    // Unknown run ids 404 on status and stop.
    let (status, body) = s.get_json("/api/loop/status?run_id=nope").await;
    assert_eq!(status, 404);
    assert_eq!(code(&body), "LOOP_RUN_NOT_FOUND");
    let (status, body) = s.post_json("/api/loop/stop", json!({"run_id": "nope"})).await;
    assert_eq!(status, 404);
    assert_eq!(code(&body), "LOOP_RUN_NOT_FOUND");
}

async fn account(s: &common::TestServer, name: &str, role: &str) -> (String, String) {
    let auth = s.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user(name, &password, role).unwrap();
    let owner = auth.get_user(name).unwrap().user_id;
    let response = s
        .client
        .post(s.url("/api/auth/login"))
        .json(&json!({"username": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (owner, cookie)
}

async fn as_account(
    s: &common::TestServer,
    cookie: &str,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let response = s
        .req(method, path)
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn runs_are_owner_bound_and_end_when_the_account_is_disabled() {
    use reqwest::Method;
    let (model_url, _count) = start_counting_mock().await;
    let s = spawn_server(
        &model_url,
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("loop.cooldown_ms", "200")
            .with("api.harness_loop_rate_limit.max_requests", "1000"),
    )
    .await;
    let (alice_id, alice) = account(&s, "alice", "operator").await;
    let (_, bob) = account(&s, "bob", "operator").await;
    let (status, created) = as_account(&s, &alice, Method::POST, "/api/sessions", json!({"title": "loop"})).await;
    assert_eq!(status, 201);
    let sid = created["session_id"].as_str().unwrap().to_string();
    let (status, started) = as_account(
        &s,
        &alice,
        Method::POST,
        "/api/loop/start",
        json!({"session_id": sid, "goal": "ship it", "max_turns": 1000}),
    )
    .await;
    assert_eq!(status, 200);
    let run_id = started["run"]["run_id"].as_str().unwrap().to_string();

    // Another account cannot read, stream, discover or stop the run.
    let (status, body) = as_account(
        &s,
        &bob,
        Method::GET,
        &format!("/api/loop/status?run_id={run_id}"),
        json!({}),
    )
    .await;
    assert_eq!((status, code(&body).as_str()), (404, "LOOP_RUN_NOT_FOUND"));
    let (status, body) = as_account(&s, &bob, Method::GET, &format!("/api/loop/events/{run_id}"), json!({})).await;
    assert_eq!((status, code(&body).as_str()), (404, "LOOP_RUN_NOT_FOUND"));
    let (status, body) = as_account(&s, &bob, Method::POST, "/api/loop/stop", json!({"run_id": run_id})).await;
    assert_eq!((status, code(&body).as_str()), (404, "LOOP_RUN_NOT_FOUND"));
    let (status, body) = as_account(
        &s,
        &bob,
        Method::GET,
        &format!("/api/loop/status?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(body["run"].is_null(), "a foreign session's run is not discoverable");

    // The owner discovers the live run by session, as a reopened console does.
    let (status, body) = as_account(
        &s,
        &alice,
        Method::GET,
        &format!("/api/loop/status?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["run"]["run_id"], run_id.as_str());
    assert_eq!(s.state.loops.get(&alice_id, &run_id).unwrap().state.as_str(), "running");

    // Disabling the starting account ends the run before its next turn.
    s.state.auth.as_ref().unwrap().disable_user("alice").unwrap();
    for _ in 0..200 {
        let run = s.state.loops.get(&alice_id, &run_id).unwrap();
        if run.state.terminal() {
            assert_eq!(run.state.as_str(), "failed", "{}", run.reason);
            assert!(run.reason.contains("disabled"), "{}", run.reason);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("a disabled account's loop kept running");
}
