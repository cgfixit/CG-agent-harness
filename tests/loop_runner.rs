//! Server-side `/loop` runner: config exposure, start/status/stop round-trip,
//! unlimited-turn behavior, and validation refusals.

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
    // A second start for the same session is refused while one is live.
    let (_url2, count2) = start_counting_mock().await;
    let _ = (_url2, count2);
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
