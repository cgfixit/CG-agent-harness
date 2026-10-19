//! Real guarded HTTP routes, synthetic credentials and loopback-only cloud fixtures.
mod common;

use axum::{body::Body, response::IntoResponse, routing::post, Json, Router};
use cgagentharness::{
    common::config::AppConfig,
    server::{build_app, AppOptions},
};
use common::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "cgah-test-synthetic-cloud-key";

fn no_secret(value: &Value) {
    let text = value.to_string();
    for forbidden in [KEY, "••••", "PRIVATE_PROMPT_CANARY"] {
        assert!(
            !text.contains(forbidden),
            "response disclosed a credential, mask or prompt"
        );
    }
}

async fn switch(s: &TestServer, model: &str, confirm: bool, expected: u16) -> Value {
    s.state.audit.flush_async().await;
    let path = s.home.join("logs/audit.jsonl");
    let count = || {
        std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("model_switched"))
            .count()
    };
    let before = count();
    let settings = std::fs::read(s.state.home.settings_path()).unwrap();
    let (status, body) = s
        .post_json("/api/model", json!({"model":model,"confirm":confirm}))
        .await;
    assert_eq!(status, expected, "{body}");
    no_secret(&body);
    if expected != 200 {
        assert_eq!(std::fs::read(s.state.home.settings_path()).unwrap(), settings);
    }
    s.state.audit.flush_async().await;
    assert_eq!(count(), before + 1, "one switch attempt must produce one audit row");
    body
}

async fn profiled_server(model: &str, budget: bool) -> TestServer {
    let mut p = prepare_server(model, ServerOptions::default()).await;
    let cfg = p.app_opts.config.as_mut().unwrap();
    for provider in ["grok", "claude"] {
        cfg.raw["models"]["cloud_chat"][provider]["profile"] = serde_yaml_ng::from_str(
            "{context_window: 128, max_output_tokens: 16, timeout_sec: 1, count_timeout_sec: 1, temperature: 1, tool_support: true, rpm: 2}"
        ).unwrap();
    }
    if budget {
        cfg.raw["models"]["cloud_chat"]["max_usd_per_call"] = serde_yaml_ng::from_str("0.0000001").unwrap();
        // Exercise the budget gate with a priced fixture, independently of the seed's model.
        cfg.raw["models"]["cloud_chat"]["claude"]["model"] = serde_yaml_ng::Value::String("claude-sonnet-5".into());
    }
    AppConfig::write_private(&cfg.path, &serde_yaml_ng::to_string(&cfg.raw).unwrap()).unwrap();
    let (app, state) = build_app(p.app_opts).await.unwrap();
    start_test_server(p.tmp, p.home_dir, p.api_key, app, state).await
}

#[tokio::test]
async fn cloud_switches_fail_closed_apply_profiles_cancel_and_reset_without_secret_disclosure() {
    // This integration target owns its process environment; no operator credential is used.
    for key in [
        "GROK_API_KEY",
        "ANTHROPIC_API_KEY",
        "XAI_API_KEY",
        "CLAUDE_API_KEY",
        "CGAH_TEST_CLOUD_BASE_URL",
    ] {
        std::env::remove_var(key);
    }
    let local = start_mock_model().await;
    let missing = spawn_server(&local.base_url(), ServerOptions::default()).await;
    let settings = std::fs::read(missing.state.home.settings_path()).unwrap();
    let (status, body) = missing.post_json("/api/model", json!({"model":"   "})).await;
    assert_eq!(status, 422);
    no_secret(&body);
    assert_eq!(std::fs::read(missing.state.home.settings_path()).unwrap(), settings);
    for local_tag in ["xai-custom-local", "sk-ant-small-local"] {
        assert_eq!(switch(&missing, local_tag, false, 200).await["model"], local_tag);
    }
    for alias in ["ClAuDe", "ANTHROPIC", "grok", "XAI", "claude-opus-5-5", "grok-4.6"] {
        let body = switch(&missing, alias, true, 409).await;
        assert_eq!(code(&body), "MODEL_PROVIDER_NOT_READY");
        assert!(body.to_string().contains("API Keys"));
    }
    for secret in [
        "sk-ant-syntheticfixturecredential000000", // gitleaks:allow -- synthetic refusal fixture, never a provider credential.
        "xai-syntheticfixturecredential000000",
        "••••private",
    ] {
        let (status, body) = missing.post_json("/api/model", json!({"model":secret})).await;
        assert_eq!(status, 422);
        assert!(!body.to_string().contains(secret));
    }
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    let streaming = Arc::new(tokio::sync::Notify::new());
    let started = streaming.clone();
    let handler = move |Json(body): Json<Value>| {
        let seen = seen.clone();
        let started = started.clone();
        async move {
            seen.lock().unwrap().push(body.clone());
            if body.to_string().contains("HOLD_FOR_SWITCH") {
                return axum::http::Response::new(Body::from_stream(futures_util::stream::unfold(0, move |step| {
                    let started = started.clone();
                    async move {
                        if step > 0 {
                            started.notify_one();
                            std::future::pending::<()>().await;
                        }
                        Some((Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{")), 1))
                    }
                })))
                .into_response();
            }
            Json(if body.get("input").is_some() {
                json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"cloud reply"}]}],"usage":{"input_tokens":7,"output_tokens":2}})
            } else {
                json!({"stop_reason":"end_turn","content":[{"type":"text","text":"cloud reply"}],"usage":{"input_tokens":7,"output_tokens":2}})
            }).into_response()
        }
    };
    let counted = requests.clone();
    let count_started = streaming.clone();
    let fixture = Router::new()
        .route("/responses", post(handler.clone()))
        .route("/messages", post(handler))
        .route(
            "/messages/count_tokens",
            post(move |Json(body): Json<Value>| {
                let holding = body.to_string().contains("HOLD_COUNT");
                counted.lock().unwrap().push(body);
                let count_started = count_started.clone();
                async move {
                    if holding {
                        count_started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Json(json!({"input_tokens":7}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 - test-only loopback fixture.
    std::env::set_var(
        "CGAH_TEST_CLOUD_BASE_URL",
        format!("http://{}", listener.local_addr().unwrap()), // DevSkim: ignore DS137138 - synthetic loopback fixture.
    );
    std::env::set_var("GROK_API_KEY", KEY);
    std::env::set_var("ANTHROPIC_API_KEY", KEY);
    let fixture_task = tokio::spawn(async {
        axum::serve(listener, fixture).await.unwrap();
    });
    for gate in ["models.cloud_chat.enabled", "models.cloud_chat.grok.enabled"] {
        let disabled = spawn_server(&local.base_url(), ServerOptions::default().with(gate, "false")).await;
        assert_eq!(
            code(&switch(&disabled, "xai", true, 409).await),
            "MODEL_PROVIDER_NOT_READY"
        );
    }
    let s = profiled_server(&local.base_url(), false).await;
    for line in [
        format!("/model use {KEY}"),
        format!("/model {KEY}"),
        "/model use ••••1234".into(),
    ] {
        let (status, body) = s.post_json("/api/slash/parse", json!({"line":line})).await;
        assert_eq!(status, 200);
        assert_eq!(body["dispatch"], false);
        no_secret(&body);
    }
    let settings = std::fs::read(s.state.home.settings_path()).unwrap();
    let (status, body) = s.post_json("/api/model", json!({"model":KEY})).await;
    assert_eq!(status, 422);
    no_secret(&body);
    assert_eq!(std::fs::read(s.state.home.settings_path()).unwrap(), settings);
    assert_eq!(
        code(&switch(&s, "XAI", false, 409).await),
        "CLOUD_CHAT_CONFIRM_REQUIRED"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "selection and status must not probe providers"
    );
    let chosen = switch(&s, "XAI", true, 200).await;
    assert_eq!(chosen["model"], "grok-4.6");
    assert_eq!(chosen["tuning"], "profile");
    let (_, status) = s.get_json("/api/status").await;
    no_secret(&status);
    assert_eq!(status["chat_tools_available"], false);
    assert_eq!(status["model_status"]["key_present"], true);
    assert_eq!(
        status["model_status"]["tuning_profile"]["sources"]["max_output_tokens"],
        "profile"
    );
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let path = format!(
        "/api/sessions/{}/prompt-history",
        created["session_id"].as_str().unwrap()
    );
    let (status, body) = s.post_json(&path, json!({"text":format!("/model use {KEY}")})).await;
    assert_eq!(status, 422);
    no_secret(&body);
    let (code_status, body) = s
        .post_json(
            "/api/chat",
            json!({"session_id":created["session_id"],"message":"PRIVATE_PROMPT_CANARY","loop":true}),
        )
        .await;
    assert_eq!(code_status, 400, "{body}");
    assert_eq!(code(&body), "CLOUD_CHAT_LOOP");
    no_secret(&body);
    let (code_status, body) = s
        .post_json("/api/chat", json!({"message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(code_status, 200, "{body}");
    no_secret(&body);
    let sent = requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(sent["model"], "grok-4.6");
    assert_eq!(sent["max_output_tokens"], 16);
    assert_eq!(sent["temperature"].as_f64(), Some(1.0));
    assert_eq!(sent["input"].as_array().unwrap().len(), 1);
    for field in ["messages", "tools", "system"] {
        assert!(sent.get(field).is_none());
    }
    let client = s.client.clone();
    let url = format!("{}/api/chat", s.base);
    let csrf = s.csrf.clone();
    let pending = tokio::spawn(async move {
        client
            .post(url)
            .header(CSRF_HEADER, csrf)
            .json(&json!({"message":"HOLD_FOR_SWITCH"}))
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), streaming.notified())
        .await
        .unwrap();
    // As in the provider cancellation unit: let the client receive the 2xx before switching.
    tokio::time::sleep(Duration::from_millis(200)).await;
    switch(&s, "LOCAL", false, 200).await;
    assert!(!tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .status()
        .is_success());
    assert_eq!(s.state.settings.lock().unwrap().selected_model, "");
    let (status, body) = s
        .post_json("/api/chat", json!({"model":"grok","message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(code(&body), "CLOUD_CHAT_CONFIRM_REQUIRED");
    no_secret(&body);
    switch(&s, "grok", true, 200).await;
    let (status, body) = s.post_json("/api/chat", json!({"message":"retry"})).await;
    assert_eq!(status, 429);
    assert_eq!(code(&body), "CLOUD_RATE_LIMITED");
    no_secret(&body);
    switch(&s, "aNtHrOpIc", true, 200).await;
    let (status, body) = s
        .post_json("/api/chat", json!({"message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(status, 200, "{body}");
    no_secret(&body);
    let sent = requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(sent["max_tokens"], 16);
    assert!(sent.get("tools").is_none());
    assert!(sent.get("system").is_none());
    let (status, prediction) = s
        .post_json("/api/spend/predict", json!({"message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(status, 200, "{prediction}");
    no_secret(&prediction);
    assert_eq!(prediction["reserved_output_tokens"], 16);
    let (status, body) = s
        .post_json("/api/spend/predict", json!({"message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(status, 429);
    assert_eq!(code(&body), "CLOUD_RATE_LIMITED");
    no_secret(&body);
    let mut options = AppOptions::new(s.state.home.clone());
    let mut changed_config = s.state.cfg.as_ref().clone();
    changed_config.raw["models"]["cloud_chat"]["claude"]["model"] =
        serde_yaml_ng::Value::String("renamed-cloud-id".into());
    options.config = Some(changed_config);
    let (_, restarted) = build_app(options).await.unwrap();
    assert_eq!(restarted.current_model(), restarted.backend.model);
    assert!(restarted.cloud_consents.lock().unwrap().is_empty());
    let ledger = std::fs::read_to_string(&s.state.spend_file).unwrap();
    let rows: Vec<Value> = ledger.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().filter(|r| r["outcome"] == "failed_after_billing").count(),
        1
    );
    let budget = profiled_server(&local.base_url(), true).await;
    switch(&budget, "claude", true, 200).await;
    let before = requests.lock().unwrap().len();
    let (status, body) = budget
        .post_json("/api/chat", json!({"message":"PRIVATE_PROMPT_CANARY"}))
        .await;
    assert_eq!(status, 422);
    assert_eq!(code(&body), "CLOUD_CHAT_BUDGET");
    no_secret(&body);
    assert_eq!(requests.lock().unwrap().len(), before + 1, "only the token counter ran");
    assert!(!budget.state.spend_file.exists());
    let context = profiled_server(&local.base_url(), false).await;
    switch(&context, "xai", true, 200).await;
    let before = requests.lock().unwrap().len();
    let (status, body) = context.post_json("/api/chat", json!({"message":"a".repeat(512)})).await;
    assert_eq!(status, 422);
    assert_eq!(code(&body), "CLOUD_CHAT_CONTEXT");
    assert_eq!(
        requests.lock().unwrap().len(),
        before,
        "context refusal generated nothing"
    );
    assert!(!context.state.spend_file.exists());
    switch(&context, "claude", true, 200).await;
    let client = context.client.clone();
    let csrf = context.csrf.clone();
    let url = format!("{}/api/spend/predict", context.base);
    let prediction = tokio::spawn(async move {
        client
            .post(url)
            .header(CSRF_HEADER, csrf)
            .json(&json!({"message":"HOLD_COUNT"}))
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), streaming.notified())
        .await
        .unwrap();
    switch(&context, "local", false, 200).await;
    assert!(!tokio::time::timeout(Duration::from_secs(2), prediction)
        .await
        .unwrap()
        .unwrap()
        .status()
        .is_success());
    assert!(
        !context.state.spend_file.exists(),
        "cancelled prediction cannot create spend"
    );
    budget.state.audit.flush_async().await;
    s.state.audit.flush_async().await;
    for server in [&s, &budget] {
        let text = std::fs::read_to_string(server.home.join("logs/audit.jsonl")).unwrap();
        no_secret(&json!(text));
    }
    no_secret(&json!(ledger));
    fixture_task.abort();
}
