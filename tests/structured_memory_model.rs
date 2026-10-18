//! Exercise the selected model at the real local HTTP boundary, with synthetic evidence.
mod common;

use cgagentharness::server::structured_memory::EpisodeDraft;
use cgagentharness::server::structured_memory_suggest::{self as suggest, Source};
use common::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn fixture() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body.clone());
                // Reproduce a stale configured model while the selected model works.
                if body["model"] == "missing-startup" {
                    return (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({"error":"model not found"})));
                }
                let system = body["messages"][0]["content"].as_str().unwrap_or("");
                let content = if system == suggest::SYSTEM_PROMPT || system.contains("local structured-memory consolidator") {
                    let payload: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                    let id = if system == suggest::SYSTEM_PROMPT { &payload["episode_id"] } else { &payload["episodes"][0]["id"] };
                    json!({"candidates":[{"action":"add","content":"The operator prefers metric units.","category":"insight","confidence":0.9,"sensitivity":"normal","source_refs":[id]}]}).to_string()
                } else {
                    "Acknowledged the operator's metric preference.".to_string()
                };
                let mut response = ok_reply(&content, 10, 5);
                response["model"] = body["model"].clone();
                (axum::http::StatusCode::OK, axum::Json(response))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let base = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, requests)
}

fn options() -> ServerOptions {
    ServerOptions::default()
        .with("models.local_llm.model", "\"missing-startup\"")
        .with("structured_memory.enabled", "true")
        .with("structured_memory.episode_capture", "true")
        .with("structured_memory.auto_suggest_chat", "true")
        .with("structured_memory.auto_suggest_coding", "true")
        .with("structured_memory.consolidation", "true")
        .with("structured_memory.suggestion_idle_ms", "100")
}

async fn select(s: &TestServer, model: &str) {
    if s.state.cloud_chat.is_cloud_selection(model) {
        // Memory routing never activates paid chat credentials; selection guards have their own HTTP coverage.
        s.state.settings.lock().unwrap().selected_model = model.into();
        return;
    }
    let (status, response) = s.post_json("/api/model", json!({"model":model})).await;
    assert_eq!(status, 200, "{response}");
}

async fn settled(s: &TestServer) {
    for _ in 0..300 {
        let status = suggest::status(&s.state, "local");
        if status["queued"] == 0 && status["running"] == false {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("suggestion worker did not settle");
}

fn stage(s: &TestServer) -> String {
    s.state
        .structured_memory
        .as_ref()
        .unwrap()
        .stage_episode(
            "local",
            EpisodeDraft {
                model_id: "fixture",
                outcome: "completed",
                sensitivity: "normal",
                user_chars: 1,
                assistant_chars: 1,
            },
        )
        .unwrap()
        .id
}

#[tokio::test]
async fn chat_suggestion_uses_the_turn_model_after_selection_changes() {
    let (base, requests) = fixture().await;
    let s = spawn_server(&base, options()).await;
    select(&s, "selected-local").await;
    for override_model in [None, Some("per-request-local")] {
        let mut request = json!({"message":"I prefer metric units."});
        if let Some(model) = override_model {
            request["model"] = json!(model);
        }
        let (status, chat) = s.post_json("/api/chat", request).await;
        assert_eq!(status, 200, "{chat}");
        assert_eq!(chat["memory_suggestion"]["queued"], true, "{chat}");
        select(&s, "later-selection").await;
        settled(&s).await;
        let expected = override_model.unwrap_or("selected-local");
        let run = s
            .state
            .structured_memory
            .as_ref()
            .unwrap()
            .list_consolidation_runs("local")
            .unwrap()
            .into_iter()
            .find(|run| {
                run.episode_ids
                    .contains(&chat["episode"]["id"].as_str().unwrap().to_string())
            })
            .unwrap();
        assert_eq!(run.state, "done", "{run:?}");
        assert_eq!(run.model_id, expected);
        {
            let seen = requests.lock().unwrap();
            assert_eq!(seen.last().unwrap()["model"], expected);
            assert!(seen.last().unwrap().get("tools").is_none());
        }
        select(&s, "selected-local").await;
    }
    assert!(s
        .state
        .structured_memory
        .as_ref()
        .unwrap()
        .list_facts("local")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn coding_suggestion_captures_selection_and_cloud_uses_local_fallback() {
    let (base, requests) = fixture().await;
    let s = spawn_server(&base, options().with("models.local_llm.model", "\"configured-local\"")).await;
    for (selection, expected) in [
        ("selected-local", "selected-local"),
        ("grok", "configured-local"),
        ("claude", "configured-local"),
    ] {
        select(&s, selection).await;
        let queued = suggest::coding_completed(
            &s.state,
            "local",
            "Use metric units.",
            &json!({"ok":true,"parsed":{"status":"pending_decision"}}),
        );
        assert_eq!(queued["queued"], true, "{queued}");
        select(&s, "later-selection").await;
        settled(&s).await;
        let seen = requests.lock().unwrap();
        assert_eq!(seen.last().unwrap()["model"], expected);
        assert!(seen.last().unwrap().get("tools").is_none());
    }
}

#[tokio::test]
async fn manual_and_idle_consolidation_resolve_selected_local_model() {
    let (base, requests) = fixture().await;
    let s = spawn_server(&base, options()).await;
    select(&s, "manual-local").await;
    let id = stage(&s);
    let (status, run) = s
        .post_json("/api/structured-memory/consolidation", json!({"episode_ids":[id]}))
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["state"], "done");
    assert_eq!(run["model_id"], "manual-local");
    select(&s, "idle-local").await;
    let id = stage(&s);
    let store = s.state.structured_memory.as_ref().unwrap();
    store
        .set_episode_summary("local", &id, "The operator prefers metric units.", "fixture")
        .unwrap();
    let (status, enabled) = s
        .post_json(
            "/api/structured-memory/gates",
            json!({"gate":"auto_consolidation","enabled":true}),
        )
        .await;
    assert_eq!(status, 200, "{enabled}");
    for _ in 0..300 {
        if store
            .list_consolidation_runs("local")
            .unwrap()
            .iter()
            .any(|run| run.episode_ids.contains(&id) && run.state == "done")
        {
            assert_eq!(requests.lock().unwrap().last().unwrap()["model"], "idle-local");
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("idle consolidation did not use selected local model");
}

#[tokio::test]
async fn failed_suggestion_and_consolidation_record_the_attempted_local_model() {
    let (base, requests) = fixture().await;
    let s = spawn_server(&base, options()).await;
    select(&s, "grok").await;
    let id = stage(&s);
    assert_eq!(
        suggest::enqueue(
            &s.state,
            "local",
            Source::Chat,
            Some(&id),
            "Metric units",
            "Acknowledged"
        )["queued"],
        true
    );
    settled(&s).await;
    let id = stage(&s);
    assert_ne!(
        s.post_json("/api/structured-memory/consolidation", json!({"episode_ids":[id]}))
            .await
            .0,
        200
    );
    let runs = s
        .state
        .structured_memory
        .as_ref()
        .unwrap()
        .list_consolidation_runs("local")
        .unwrap();
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run.state, "failed");
        assert_eq!(run.model_id, "missing-startup");
    }
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "both failures reached only the local fixture"
    );
}
