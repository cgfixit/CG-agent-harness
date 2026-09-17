//! Native Ollama inventory/pull/warmup: loopback only, no num_ctx, abortable.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Json;
use axum::routing::{get, post};
use axum::Router;
use common::*;
use reqwest::Method;
use serde_json::{json, Value};

struct NativeOllama {
    base: String,
    pulls: Arc<Mutex<Vec<Value>>>,
    generates: Arc<Mutex<Vec<Value>>>,
    pull_delay_ms: Arc<Mutex<u64>>,
}

impl NativeOllama {
    fn openai_url(&self) -> String {
        format!("{}/v1", self.base)
    }
}

async fn start_native_ollama() -> NativeOllama {
    let pulls = Arc::new(Mutex::new(Vec::new()));
    let generates = Arc::new(Mutex::new(Vec::new()));
    let delay = Arc::new(Mutex::new(0u64));
    let p2 = pulls.clone();
    let g2 = generates.clone();
    let d2 = delay.clone();
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "qwen3.8:27b-mlx"}]})) }),
        )
        .route(
            "/api/tags",
            get(|| async { Json(json!({"models": [{"name": "qwen3.8:27b-mlx", "size": 1}, {"name": "tinyllama:latest", "size": 2}]})) }),
        )
        .route(
            "/api/pull",
            post(move |Json(body): Json<Value>| {
                let p = p2.clone();
                let d = d2.clone();
                async move {
                    p.lock().unwrap().push(body);
                    let ms = *d.lock().unwrap();
                    if ms > 0 {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
                        "{\"status\":\"pulling manifest\"}\n{\"status\":\"success\"}\n".to_string(),
                    )
                }
            }),
        )
        .route(
            "/api/generate",
            post(move |Json(body): Json<Value>| {
                let g = g2.clone();
                async move {
                    g.lock().unwrap().push(body);
                    Json(json!({"done": true}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    NativeOllama {
        base: format!("http://127.0.0.1:{}", addr.port()), // DevSkim: ignore DS162092 because this fixture binds only to loopback.
        pulls,
        generates,
        pull_delay_ms: delay,
    }
}

#[tokio::test]
async fn inventory_is_csrf_guarded_and_lists_tags() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(&ollama.openai_url(), ServerOptions::default()).await;
    let open = s.open_get("/api/ollama/inventory").await;
    assert_eq!(open.0, 403);
    let (status, body) = s.get_json("/api/ollama/inventory").await;
    assert_eq!(status, 200, "{body}");
    let names: Vec<&str> = body["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert!(names.contains(&"tinyllama:latest"), "{body}");
    assert_eq!(body["configured_state"], "installed");
}

#[tokio::test]
async fn pull_posts_name_without_num_ctx_and_refreshes_inventory() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(&ollama.openai_url(), ServerOptions::default()).await;
    let (status, body) = s
        .post_json("/api/ollama/pull", json!({"model": "tinyllama:latest"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "installed");
    let sent = ollama.pulls.lock().unwrap()[0].clone();
    assert_eq!(sent["name"], "tinyllama:latest");
    assert_eq!(sent["model"], "tinyllama:latest");
    assert_eq!(sent["stream"], true);
    assert!(sent.get("num_ctx").is_none());
    assert!(!sent.to_string().contains("num_ctx"));
}

#[tokio::test]
async fn second_pull_is_busy_while_the_first_is_in_flight() {
    let ollama = start_native_ollama().await;
    *ollama.pull_delay_ms.lock().unwrap() = 400;
    let s = spawn_server(&ollama.openai_url(), ServerOptions::default()).await;
    let first = s
        .req(Method::POST, "/api/ollama/pull")
        .json(&json!({"model": "tinyllama:latest"}));
    let handle = tokio::spawn(async move { first.send().await.unwrap().status().as_u16() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, body) = s
        .post_json("/api/ollama/pull", json!({"model": "tinyllama:latest"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "OLLAMA_PULL_BUSY");
    assert_eq!(handle.await.unwrap(), 200);
}

#[tokio::test]
async fn cancel_aborts_an_in_flight_pull() {
    let ollama = start_native_ollama().await;
    *ollama.pull_delay_ms.lock().unwrap() = 800;
    let s = spawn_server(&ollama.openai_url(), ServerOptions::default()).await;
    let first = s
        .req(Method::POST, "/api/ollama/pull")
        .json(&json!({"model": "tinyllama:latest"}));
    let handle = tokio::spawn(async move { first.send().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(40)).await;
    let (status, body) = s.post_json("/api/ollama/pull/cancel", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["cancelled"], true);
    let resp = handle.await.unwrap();
    assert_eq!(resp.status().as_u16(), 409);
}

#[tokio::test]
async fn non_ollama_provider_refuses_pull() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default().with("models.local_llm.provider", "\"lmstudio\""),
    )
    .await;
    let (status, body) = s
        .post_json("/api/ollama/pull", json!({"model": "tinyllama:latest"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "OLLAMA_PROVIDER_REQUIRED");
}

#[tokio::test]
async fn warmup_flag_is_true_posts_generate_without_num_ctx() {
    let ollama = start_native_ollama().await;
    let _s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default().with("models.local_llm.warmup.enabled", "true"),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let gens = ollama.generates.lock().unwrap().clone();
    assert_eq!(gens.len(), 1, "{gens:?}");
    assert_eq!(gens[0]["prompt"], "");
    assert!(gens[0].get("num_ctx").is_none());
    assert!(!gens[0].to_string().contains("num_ctx"));
}

#[tokio::test]
async fn quoted_warmup_true_does_not_generate() {
    let ollama = start_native_ollama().await;
    let _s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default().with("models.local_llm.warmup.enabled", "\"true\""),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(ollama.generates.lock().unwrap().is_empty());
}
