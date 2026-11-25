//! Real guarded HTTP routes, synthetic credentials and loopback-only cloud fixtures.
mod common;

use axum::{body::Body, response::IntoResponse, routing::post, Json, Router};
use cgagentharness::{common::config::AppConfig, server::build_app};
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
    // This target has one test, so it owns its process environment. No operator
    // credential is used, and the guard restores every original value on exit or panic.
    let _cleared = EnvGuard::set(&[
        ("GROK_API_KEY", None),
        ("ANTHROPIC_API_KEY", None),
        ("XAI_API_KEY", None),
        ("CLAUDE_API_KEY", None),
        ("CGAH_TEST_CLOUD_BASE_URL", None),
    ]);
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
    let configured = [
        shipped_str("models.cloud_chat.claude.model"),
        shipped_str("models.cloud_chat.grok.model"),
    ];
    for alias in ["ClAuDe", "ANTHROPIC", "grok", "XAI"]
        .into_iter()
        .chain(configured.iter().map(String::as_str))
    {
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
    let fixture_url = format!("http://{}", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 - synthetic loopback fixture.

    // Locals drop in reverse order: this guard restores the cleared state, then
    // `_cleared` restores the operator's originals.
    let _fixture = EnvGuard::set(&[
        ("CGAH_TEST_CLOUD_BASE_URL", Some(fixture_url.as_str())),
        ("GROK_API_KEY", Some(KEY)),
        ("ANTHROPIC_API_KEY", Some(KEY)),
    ]);
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
    let grok_model = shipped_str("models.cloud_chat.grok.model");
    assert_eq!(chosen["model"], grok_model);
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
    // A server-run loop would send those same turns: its start refuses before
    // it saves the goal or registers a run that could only fail.
    let (code_status, body) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":created["session_id"],"goal":"LOOP_GOAL_CANARY","max_turns":1}),
        )
        .await;
    assert_eq!(code_status, 400, "{body}");
    assert_eq!(code(&body), "CLOUD_CHAT_LOOP");
    let session_path = format!("/api/sessions/{}", created["session_id"].as_str().unwrap());
    assert_eq!(
        s.get_json(&session_path).await.1["goal"],
        "",
        "a refused start persisted its goal"
    );
    assert!(
        !s.audit_log().contains("loop_run_started"),
        "a refused start registered a run"
    );
    // The message leaves the machine redacted (policy.privacy); no extra call,
    // the fixture profile allows two per minute.
    let (code_status, body) = s
        .post_json(
            "/api/chat",
            json!({"message":"PRIVATE_PROMPT_CANARY mail pii.canary@example.com card 4111 1111 1111 1111"}),
        )
        .await;
    assert_eq!(code_status, 200, "{body}");
    no_secret(&body);
    let sent = requests.lock().unwrap().last().unwrap().clone();
    let outbound = sent.to_string();
    assert!(
        !outbound.contains("pii.canary@example.com") && !outbound.contains("4111 1111 1111 1111"),
        "an email and a card number reached the cloud provider"
    );
    assert!(outbound.contains("[REDACTED_EMAIL]") && outbound.contains("[REDACTED_PII]"));
    s.state.audit.flush_async().await;
    assert!(s.audit_log().contains("\"cloud_chat_redacted\""));
    assert_eq!(sent["model"], grok_model);
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
    let mut options = common::app_options(s.state.home.clone());
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

    // A completed cloud turn may stage an episode, but its memory inference
    // must return to the configured local model. The cloud fixture sees only
    // the newly typed message; it never receives history or memory prompts.
    let memory_options = ServerOptions::default()
        .with("structured_memory.enabled", "true")
        .with("structured_memory.episode_capture", "true")
        .with("structured_memory.consolidation", "true")
        .with("structured_memory.auto_suggest_chat", "true")
        .with("structured_memory.suggestion_idle_ms", "500")
        .with("models.local_llm.model", "mock-model")
        .with("models.local_llm.max_tokens", "1024");
    let mut prepared = prepare_server(&local.base_url(), memory_options).await;
    let memory_config = prepared.app_opts.config.as_mut().unwrap();
    for provider in ["grok", "claude"] {
        memory_config.raw["models"]["cloud_chat"][provider]["profile"] = serde_yaml_ng::from_str(
            "{context_window: 128, max_output_tokens: 16, timeout_sec: 1, count_timeout_sec: 1, temperature: 1, tool_support: true, rpm: 60}",
        )
        .unwrap();
    }
    AppConfig::write_private(
        &memory_config.path,
        &serde_yaml_ng::to_string(&memory_config.raw).unwrap(),
    )
    .unwrap();
    let (memory_app, memory_state) = build_app(prepared.app_opts).await.unwrap();
    let memory_server = start_test_server(
        prepared.tmp,
        prepared.home_dir,
        prepared.api_key,
        memory_app,
        memory_state,
    )
    .await;
    switch(&memory_server, "xai", true, 200).await;
    let cloud_before = requests.lock().unwrap().len();
    let (status, cloud_body) = memory_server
        .post_json("/api/chat", json!({"message":"Remember that I prefer metric units."}))
        .await;
    assert_eq!(status, 200, "{cloud_body}");
    let episode_id = cloud_body["episode"]["id"].as_str().unwrap();
    let run_id = cloud_body["memory_suggestion"]["run_id"].as_str().unwrap();
    assert_eq!(cloud_body["memory_suggestion"]["queued"], true);
    local.set_reply(ok_reply(
        &json!({"candidates":[{
            "action":"add",
            "category":"insight",
            "content":"The synthetic cloud user prefers metric units.",
            "confidence":0.95,
            "uncertainty":"",
            "sensitivity":"normal",
            "source_refs":[episode_id]
        }]})
        .to_string(),
        20,
        12,
    ));
    let cloud_sent = requests.lock().unwrap()[cloud_before].clone();
    assert_eq!(cloud_sent["input"].as_array().unwrap().len(), 1);
    assert_eq!(
        cloud_sent["input"][0]["content"],
        "Remember that I prefer metric units."
    );
    for field in ["messages", "tools", "system", "response_format"] {
        assert!(cloud_sent.get(field).is_none(), "cloud request contained {field}");
    }
    let run = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, run) = memory_server
                .get_json(&format!("/api/structured-memory/consolidation/{run_id}"))
                .await;
            if run["state"] != "idle" && run["state"] != "running" {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run["state"], "done", "{run}");
    assert_eq!(run["proposal_count"], 1);
    let local_sent = local.last_request().unwrap();
    assert_eq!(local_sent["model"], "mock-model");
    assert_eq!(local_sent["max_tokens"], 1024);
    assert_eq!(local_sent["response_format"], json!({"type":"json_object"}));
    let local_input: Value = serde_json::from_str(
        local_sent["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(local_input["episode_id"], episode_id);
    assert_eq!(
        requests.lock().unwrap().len(),
        cloud_before + 1,
        "memory inference reached cloud fixture"
    );
    let (_, proposals) = memory_server.get_json("/api/structured-memory/proposals").await;
    assert_eq!(proposals["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(proposals["proposals"][0]["source_episode_ids"], json!([episode_id]));
    assert_eq!(proposals["proposals"][0]["status"], "pending");
    let (_, facts) = memory_server.get_json("/api/structured-memory/facts").await;
    assert!(facts["facts"].as_array().unwrap().is_empty());

    // A cloud turn whose client hangs up while its spend row is written still
    // stores the exchange it was billed for. Park the ledger append, so the
    // turn is known to be waiting on it when the client goes away.
    let (_, created) = memory_server.post_json("/api/sessions", json!({})).await;
    let session_path = format!("/api/sessions/{}", created["session_id"].as_str().unwrap());
    let spend_rows = || {
        std::fs::read_to_string(&memory_server.state.spend_file)
            .unwrap_or_default()
            .lines()
            .count()
    };
    let (rows_before, sent_before) = (spend_rows(), requests.lock().unwrap().len());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _appends = cgagentharness::llm::spend::hold_appends();
        held_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    held_rx.recv().unwrap();
    let turn = tokio::spawn(
        memory_server
            .req(reqwest::Method::POST, "/api/chat")
            .json(&json!({"session_id":created["session_id"],"message":"billed, then the client hangs up"}))
            .send(),
    );
    let stored = || async { memory_server.get_json(&session_path).await.1["tokens"]["exchanges"] == 1 };
    tokio::time::timeout(Duration::from_secs(10), async {
        while requests.lock().unwrap().len() == sent_before {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cloud fixture answered the turn");
    // The exchange write does not wait for the parked row.
    for _ in 0..100 {
        if stored().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(spend_rows(), rows_before, "the spend row is still parked");
    turn.abort();
    assert!(turn.await.unwrap_err().is_cancelled());
    // Give the server the moment it needs to see the closed connection.
    tokio::time::sleep(Duration::from_millis(200)).await;
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while spend_rows() == rows_before {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the billed row reached the ledger");
    assert_eq!(spend_rows(), rows_before + 1, "one row for the one billed turn");
    assert!(
        stored().await,
        "a billed cloud turn lost its exchange when the client hung up"
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

/// An Ollama cloud tag (`:cloud`, `-cloud`) would make the local Ollama
/// forward the whole local prompt to ollama.com, unredacted and without cloud
/// consent. It is refused as a local selection, as a chat turn's model and as
/// a pull; the model is never called, and a plain local name still works.
#[tokio::test]
async fn an_ollama_cloud_tag_is_refused_as_a_local_model() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for tag in ["gpt-oss:120b-cloud", "glm-4.6:cloud"] {
        let (status, body) = s.post_json("/api/model", json!({"model": tag})).await;
        assert_eq!(status, 422, "selecting {tag}");
        assert_eq!(body["detail"]["code"], "MODEL_SELECTION_REJECTED", "selecting {tag}");
        let (status, body) = s
            .post_json("/api/chat", json!({"message": "private notes canary", "model": tag}))
            .await;
        assert_eq!(status, 422, "a chat turn naming {tag}");
        assert_eq!(
            body["detail"]["code"], "MODEL_SELECTION_REJECTED",
            "a chat turn naming {tag}"
        );
        let (status, _) = s.post_json("/api/ollama/pull", json!({"model": tag})).await;
        assert_ne!(status, 200, "pulling {tag}");
    }
    assert!(
        model.requests.lock().unwrap().is_empty(),
        "no refused turn reached the model"
    );
    let (status, _) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "a local turn still runs");
}

/// A cloud tag cannot be selected, but a configured one is still the model a
/// server starts on. A `/loop` start then refuses with the turn's own code
/// before it saves the goal or registers a run that could only fail.
#[tokio::test]
async fn a_loop_start_on_a_configured_ollama_cloud_tag_saves_no_goal() {
    let model = start_mock_model().await;
    let mut options = ServerOptions::default();
    options
        .overrides
        .push(("models.local_llm.model".into(), "glm-4.6:cloud".into()));
    let s = spawn_server(&model.base_url(), options).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let (status, body) = s
        .post_json(
            "/api/loop/start",
            json!({"session_id":created["session_id"],"goal":"LOOP_GOAL_CANARY","max_turns":1}),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["detail"]["code"], "MODEL_SELECTION_REJECTED", "{body}");
    let session_path = format!("/api/sessions/{}", created["session_id"].as_str().unwrap());
    assert_eq!(
        s.get_json(&session_path).await.1["goal"],
        "",
        "a refused start persisted its goal"
    );
    assert!(
        !s.audit_log().contains("loop_run_started"),
        "a refused start registered a run"
    );
    assert!(model.requests.lock().unwrap().is_empty(), "no turn reached the model");
}

/// A configured cloud tag is still the model a server starts on. Neither the
/// startup warmup nor auto_tune may send it to Ollama: both load the model
/// with `/api/generate`, and a signed-in Ollama would forward that to
/// ollama.com. Each skips with an audited `cloud_tag` reason instead.
#[tokio::test]
async fn startup_warmup_and_auto_tune_never_load_a_configured_ollama_cloud_tag() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let record = seen.clone();
    let app = Router::new().fallback(move |req: axum::extract::Request| {
        let record = record.clone();
        async move {
            record
                .lock()
                .unwrap()
                .push(format!("{} {}", req.method(), req.uri().path()));
            Json(json!({})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut options = ServerOptions::default();
    for (key, value) in [
        ("models.local_llm.model", "glm-4.6:cloud"),
        ("models.local_llm.warmup.enabled", "true"),
        ("models.local_llm.auto_tune", "true"),
    ] {
        options.overrides.push((key.into(), value.into()));
    }
    let s = spawn_server(&format!("http://127.0.0.1:{port}/v1"), options).await;
    let skipped = |log: &str| {
        log.contains(r#""event":"ollama_warmup_degraded""#)
            && log.contains(r#""reason":"cloud_tag""#)
            && log.contains(r#""event":"model_tune_skipped""#)
    };
    for _ in 0..50 {
        if skipped(&s.audit_log()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let log = s.audit_log();
    assert!(skipped(&log), "warmup and auto_tune did not skip the cloud tag: {log}");
    let tune_line = log
        .lines()
        .find(|line| line.contains(r#""event":"model_tune_skipped""#))
        .unwrap_or_default();
    assert!(tune_line.contains(r#""reason":"cloud_tag""#), "{tune_line}");
    let sent = seen.lock().unwrap().clone();
    assert!(
        !sent
            .iter()
            .any(|row| row.contains("/api/generate") || row.contains("/api/show")),
        "the cloud tag reached Ollama: {sent:?}"
    );
}
