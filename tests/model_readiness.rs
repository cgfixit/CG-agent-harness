use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{routing::get, Router};
use cgagentharness::common::config::AppConfig;
use cgagentharness::llm::backend::resolve_local_backend;
use cgagentharness::llm::inventory::{model_readiness, InventoryLimits};
use serde_json::json;

struct Inventory {
    url: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Inventory {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn inventory(model: &str) -> Inventory {
    raw_inventory(200, json!({"data": [{"id": model}]}).to_string(), None, 0).await
}

async fn raw_inventory(status: u16, body: String, redirect: Option<String>, delay_ms: u64) -> Inventory {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = Router::new().route(
        "/v1/models",
        get(move || {
            count.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            let redirect = redirect.clone();
            async move {
                if delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
                let mut response = axum::http::Response::builder().status(status);
                if let Some(location) = redirect {
                    response = response.header("location", location);
                }
                response.body(axum::body::Body::from(body)).unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Inventory { url, hits, task }
}

fn configuration(primary: &Inventory, secondary: &Inventory, enabled: bool) -> AppConfig {
    let value = json!({"models":{"local_llm":{
        "base_url":primary.url, "model":"selected-primary", "provider":"ollama",
        "fallback":{"enabled":enabled, "base_url":secondary.url,
            "model":"selected-secondary", "provider":"lmstudio", "probe_timeout_sec":1.5}
    }}});
    AppConfig::from_str(
        &serde_yaml_ng::to_string(&value).unwrap(),
        std::path::Path::new("fixture.yaml"),
    )
    .unwrap()
}

#[tokio::test]
async fn fallback_requires_the_exact_model_in_a_successful_inventory() {
    let primary = inventory("selected-primary-mlx").await;
    let secondary = inventory("selected-secondary").await;
    let result = resolve_local_backend(&configuration(&primary, &secondary, true))
        .await
        .unwrap();
    assert_eq!(
        result.source, "fallback",
        "HTTP success with a different tag is not readiness"
    );
    assert_eq!(result.model, "selected-secondary");
    assert!(!result.degraded);
    assert_eq!(primary.hits.load(Ordering::SeqCst), 1);
    assert_eq!(secondary.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_fallback_never_probes_and_missing_models_degrade() {
    let primary = inventory("other").await;
    let secondary = inventory("other").await;
    let result = resolve_local_backend(&configuration(&primary, &secondary, false))
        .await
        .unwrap();
    assert_eq!(result.source, "primary");
    assert_eq!(primary.hits.load(Ordering::SeqCst), 0);
    assert_eq!(secondary.hits.load(Ordering::SeqCst), 0);
    let result = resolve_local_backend(&configuration(&primary, &secondary, true))
        .await
        .unwrap();
    assert!(result.degraded, "neither selected tag exists");
    assert_eq!(result.source, "primary");
}

#[tokio::test]
async fn ready_primary_does_not_probe_fallback() {
    let primary = inventory("selected-primary").await;
    let secondary = inventory("selected-secondary").await;
    let result = resolve_local_backend(&configuration(&primary, &secondary, true))
        .await
        .unwrap();
    assert_eq!(result.source, "primary");
    assert!(!result.degraded);
    assert_eq!(primary.hits.load(Ordering::SeqCst), 1);
    assert_eq!(secondary.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn inventory_refuses_redirects_unsafe_destinations_and_provider_error_text() {
    let destination = inventory("selected").await;
    let redirect = raw_inventory(302, String::new(), Some(format!("{}/models", destination.url)), 0).await;
    let limits = InventoryLimits {
        timeout: std::time::Duration::from_secs(1),
        max_bytes: 1024,
    };
    let result = model_readiness(&redirect.url, "selected", "fixture-token", limits).await;
    assert_eq!(result["state"], "unavailable");
    for endpoint in [
        destination.url.replace("http://", "http://user:fixture-secret@"),
        format!("{}?token=fixture-secret", destination.url),
        format!("{}#fragment", destination.url),
        destination.url.replace("http://", "ftp://"),
        "https://unapproved.invalid/v1".to_string(),
    ] {
        let result = model_readiness(&endpoint, "selected", "fixture-token", limits).await;
        assert_eq!(result["state"], "not_probed");
        assert!(!result.to_string().contains("fixture-secret"));
    }
    assert_eq!(
        destination.hits.load(Ordering::SeqCst),
        0,
        "no redirected or unapproved request"
    );
    let denied = raw_inventory(401, "fixture-private-provider-error".into(), None, 0).await;
    let result = model_readiness(&denied.url, "selected", "fixture-token", limits).await;
    assert_eq!(result["state"], "unavailable");
    assert!(!result.to_string().contains("fixture-private-provider-error"));
    assert!(!result.to_string().contains("fixture-token"));
}

#[tokio::test]
async fn inventory_bounds_bytes_time_and_requires_a_model_array() {
    let limits = InventoryLimits {
        timeout: std::time::Duration::from_millis(100),
        max_bytes: 1024,
    };
    for body in [
        "{malformed".to_string(),
        json!({"data":"wrong-type"}).to_string(),
        json!({"data":[{"id":"selected"}],"padding":"x".repeat(2048)}).to_string(),
    ] {
        let server = raw_inventory(200, body, None, 0).await;
        assert_eq!(
            model_readiness(&server.url, "selected", "", limits).await["state"],
            "unavailable"
        );
    }
    let slow = raw_inventory(200, json!({"data":[{"id":"selected"}]}).to_string(), None, 2000).await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        model_readiness(&slow.url, "selected", "", limits),
    )
    .await
    .expect("inventory must honor its request deadline");
    assert_eq!(result["state"], "unavailable");
}

#[test]
fn inventory_configuration_rejects_invalid_types_nonfinite_values_and_unbounded_limits() {
    for field in [
        "timeout_sec: .nan",
        "timeout_sec: .inf",
        "timeout_sec: -1",
        "timeout_sec: 31",
        "timeout_sec: '2'",
        "max_response_bytes: '262144'",
        "max_response_bytes: 0",
        "max_response_bytes: 1048577",
    ] {
        let yaml = format!("models:\n  local_llm:\n    inventory:\n      {field}\n");
        let cfg = AppConfig::from_str(&yaml, std::path::Path::new("fixture.yaml")).unwrap();
        assert!(InventoryLimits::from_config(&cfg).is_err(), "{field}");
    }
    let cfg = AppConfig::from_str("{}", std::path::Path::new("fixture.yaml")).unwrap();
    assert_eq!(InventoryLimits::from_config(&cfg).unwrap().max_bytes, 262144);
    for timeout in [".nan", "0", "-1", "'1.5'", "60"] {
        let yaml = format!("models:\n  local_llm:\n    fallback:\n      probe_timeout_sec: {timeout}\n");
        let cfg = AppConfig::from_str(&yaml, std::path::Path::new("fixture.yaml")).unwrap();
        assert!(InventoryLimits::for_fallback(&cfg).is_err(), "{timeout}");
    }
}
