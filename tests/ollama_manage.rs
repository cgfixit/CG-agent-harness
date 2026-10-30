//! Native Ollama inventory/profile/pull/warmup: loopback only, no num_ctx, abortable.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Json;
use axum::routing::{get, post};
use axum::Router;
use common::*;
use reqwest::Method;
use serde_json::{json, Value};

/// `models.local_llm.model` in `assets/config.default.yaml`; the fixture Ollama
/// serves it, so a changed default needs no edit here (#685).
fn shipped_model() -> &'static str {
    static MODEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MODEL.get_or_init(|| shipped_str("models.local_llm.model"))
}

struct NativeOllama {
    base: String,
    pulls: Arc<Mutex<Vec<Value>>>,
    generates: Arc<Mutex<Vec<Value>>>,
    chats: Arc<Mutex<Vec<Value>>>,
    pull_delay_ms: Arc<Mutex<u64>>,
    /// Delay for the configured model's raw speed sample (the startup tune).
    sample_delay_ms: Arc<Mutex<u64>>,
    load_delay_ms: Arc<Mutex<u64>>,
    /// The window `/api/ps` reports for `longctx:q8`; 0 means not loaded.
    longctx_window: Arc<Mutex<u64>>,
    /// The window a generate loads `longctx:q8` with while it is not loaded.
    longctx_load_window: Arc<Mutex<u64>>,
}

impl NativeOllama {
    fn openai_url(&self) -> String {
        format!("{}/v1", self.base)
    }
}

async fn start_native_ollama() -> NativeOllama {
    let pulls = Arc::new(Mutex::new(Vec::new()));
    let generates = Arc::new(Mutex::new(Vec::new()));
    let chats = Arc::new(Mutex::new(Vec::new()));
    let delay = Arc::new(Mutex::new(0u64));
    let sample_delay = Arc::new(Mutex::new(0u64));
    let s2 = sample_delay.clone();
    let load_delay = Arc::new(Mutex::new(0u64));
    let ld2 = load_delay.clone();
    let longctx_window = Arc::new(Mutex::new(65_536u64));
    let w2 = longctx_window.clone();
    let w3 = longctx_window.clone();
    let longctx_load_window = Arc::new(Mutex::new(65_536u64));
    let l2 = longctx_load_window.clone();
    let p2 = pulls.clone();
    let g2 = generates.clone();
    let c2 = chats.clone();
    let d2 = delay.clone();
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": shipped_model()}]})) }),
        )
        .route(
            "/api/tags",
            get(|| async { Json(json!({"models": [{"name": shipped_model(), "size": 1}, {"name": "tinyllama:latest", "size": 2}]})) }),
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
            "/api/show",
            post(|Json(body): Json<Value>| async move {
                if body.get("num_ctx").is_none() && body["model"] == "humanizer:q8" {
                    // A rewriting fine-tune whose template has no tool block.
                    return (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "details": {"family": "gemma4", "parameter_size": "12.0B", "quantization_level": "Q8_0"},
                            "model_info": {"general.architecture": "gemma4", "gemma4.context_length": 131072},
                            "capabilities": ["completion"],
                        })),
                    );
                }
                if body.get("num_ctx").is_none() && body["model"] == "longctx:q8" {
                    // Loaded at 65536 tokens (OLLAMA_CONTEXT_LENGTH=65536).
                    return (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "details": {"family": "qwen35", "parameter_size": "9.0B", "quantization_level": "Q8_0"},
                            "model_info": {"general.architecture": "qwen35", "qwen35.context_length": 262144},
                            "capabilities": ["completion", "tools"],
                        })),
                    );
                }
                if body.get("num_ctx").is_some() || body["model"] != shipped_model() {
                    return (axum::http::StatusCode::NOT_FOUND, Json(json!({"error": "fixture-private-missing"})));
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({
                        "license": "fixture-private-license",
                        "parameters": "num_ctx 32768",
                        "details": {"family": "qwen38", "parameter_size": "27.8B", "quantization_level": "Q4_K_M"},
                        "model_info": {"general.architecture": "qwen38", "qwen38.context_length": 262144},
                        "capabilities": ["completion", "tools"],
                    })),
                )
            }),
        )
        .route(
            "/api/ps",
            get(move || {
                let window = *w2.lock().unwrap();
                async move {
                    let mut models = vec![
                        json!({"name": shipped_model(), "context_length": 32768, "size": 20, "size_vram": 20}),
                        json!({"name": "humanizer:q8", "context_length": 16384, "size": 13, "size_vram": 13}),
                    ];
                    if window > 0 {
                        models.push(json!({"name": "longctx:q8", "context_length": window, "size": 11, "size_vram": 11}));
                    }
                    Json(json!({"models": models}))
                }
            }),
        )
        .route(
            "/api/generate",
            post(move |Json(body): Json<Value>| {
                let g = g2.clone();
                let s = s2.clone();
                let ld = ld2.clone();
                let (w, l) = (w3.clone(), l2.clone());
                async move {
                    g.lock().unwrap().push(body.clone());
                    if body["raw"] != true {
                        let ms = *ld.lock().unwrap();
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    // Any generate loads longctx with Ollama's current default window.
                    if body["model"] == "longctx:q8" && *w.lock().unwrap() == 0 {
                        *w.lock().unwrap() = *l.lock().unwrap();
                    }
                    let timed = body["raw"] == true;
                    let ms = *s.lock().unwrap();
                    if timed && ms > 0 && body["model"] == shipped_model() {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    // Warmup sends an empty prompt; the auto_tune sample is raw and
                    // gets Ollama's nanosecond durations: 1500 tok/s in, 50 tok/s out.
                    Json(if timed {
                        json!({"done": true, "load_duration": 0, "prompt_eval_count": 300,
                            "prompt_eval_duration": 200_000_000u64, "eval_count": 64, "eval_duration": 1_280_000_000u64})
                    } else {
                        json!({"done": true})
                    })
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let c = c2.clone();
                async move {
                    c.lock().unwrap().push(body);
                    Json(json!({"model": "humanizer:q8", "choices": [{"finish_reason": "stop",
                        "message": {"role": "assistant", "content": "Rewritten."}}],
                        "usage": {"prompt_tokens": 12, "completion_tokens": 2}}))
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
        chats,
        pull_delay_ms: delay,
        sample_delay_ms: sample_delay,
        load_delay_ms: load_delay,
        longctx_window,
        longctx_load_window,
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
    assert_eq!(body["tags_state"], "listed", "{body}");
}

#[tokio::test]
async fn profile_reports_declared_and_loaded_facts_without_side_effects() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(&ollama.openai_url(), ServerOptions::default()).await;
    assert_eq!(s.open_get("/api/ollama/profile").await.0, 403);
    let (status, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "profiled", "{body}");
    assert_eq!(body["declared"]["tools"], true);
    assert_eq!(body["declared"]["native_context"], 262144);
    assert_eq!(body["declared"]["modelfile_num_ctx"], 32768);
    assert_eq!(body["loaded"]["context_length"], 32768);
    assert_eq!(body["loaded"]["gpu_fraction"], 1.0);
    // The fixture loads the tuned 32768 window, so the proposal is the shipped shape.
    assert_eq!(body["proposed"]["state"], "shipped", "{body}");
    assert!(!body["proposed"]["values"].as_array().unwrap().is_empty(), "{body}");
    assert!(!body.to_string().contains("fixture-private"), "{body}");
    // Profiling never loads, pulls or warms a model (test warmup is off).
    assert!(ollama.generates.lock().unwrap().is_empty());
    assert!(ollama.pulls.lock().unwrap().is_empty());
    // The console sends every slash line through this parser first. As a known
    // subcommand, a typo is suggested back rather than dispatched as `/model`.
    let (_, parsed) = s.post_json("/api/slash/parse", json!({"line": "/model profile"})).await;
    assert_eq!(parsed["dispatch"], true, "{parsed}");
    assert_eq!(parsed["canonical"], "/model profile", "{parsed}");
    let (_, parsed) = s.post_json("/api/slash/parse", json!({"line": "/model profil"})).await;
    assert_eq!(parsed["dispatch"], false, "{parsed}");
    assert_eq!(parsed["suggestions"][0]["line"], "/model profile", "{parsed}");
    let (_, parsed) = s.post_json("/api/slash/parse", json!({"line": "/model list"})).await;
    assert_eq!(parsed["dispatch"], true, "{parsed}");
    assert_eq!(parsed["canonical"], "/model list", "{parsed}");
    // A fixed-argument subcommand refuses extra text instead of ignoring it.
    for line in ["/model list extra", "/model list --dry-run", "/model profile extra"] {
        let (_, parsed) = s.post_json("/api/slash/parse", json!({"line": line})).await;
        assert_eq!(parsed["dispatch"], false, "{line}: {parsed}");
    }
    // `/model list` renders these rows: the exact tags `/model use` takes.
    let (_, inventory) = s.get_json("/api/ollama/inventory").await;
    assert!(
        inventory["models"].as_array().is_some_and(|m| !m.is_empty()),
        "{inventory}"
    );

    let (status, _) = s.post_json("/api/model", json!({"model": "missing:latest"})).await;
    assert_eq!(status, 200);
    let (_, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(body["state"], "tag_missing", "{body}");
    assert!(!body.to_string().contains("fixture-private"), "{body}");

    // Read-only profiling still refuses to probe a cloud selection, including a legacy saved one.
    s.state.settings.lock().unwrap().selected_model = "grok".into();
    let (_, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(body["state"], "not_probed", "{body}");
}

/// The profiled trigger counts one reply, like chat's, on a backend whose
/// reasoning doubles the reservation only the hard limit keeps.
#[tokio::test]
async fn the_profiled_trigger_counts_one_reply_when_reasoning_is_on() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.reasoning_effort", "high")
            .with("chat.compact_prompt_tokens", "4096"),
    )
    .await;
    s.post_json("/api/web", json!({"enabled": false})).await;
    let (status, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(status, 200, "{body}");
    // max_tokens (4096) + 4096 headroom, not the doubled 8192 + 4096.
    assert_eq!(body["in_force"]["prompt_limit"], 4096 + 4096, "{body}");
    assert_eq!(
        body["in_force"]["limit_source"], "models.local_llm.max_tokens",
        "{body}"
    );
}

#[tokio::test]
async fn auto_tune_measures_the_selection_and_tightens_its_chat_limits() {
    let ollama = start_native_ollama().await;
    // The startup tune for the configured model holds the generation gate while
    // its slow sample runs; the selection's tune must wait for it, then land.
    *ollama.sample_delay_ms.lock().unwrap() = 1500;
    let s = spawn_server(
        &ollama.openai_url(),
        // Polling for the background result must not trip the 60/min API limit.
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("api.rate_limit.max_requests", "1000"),
    )
    .await;
    let (status, body) = s.post_json("/api/model", json!({"model": "humanizer:q8"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["tuning"], "pending");
    // Tuning runs in the background; the read-only profile shows when it lands.
    let mut profile = Value::Null;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let (status, body) = s.get_json("/api/ollama/profile").await;
        assert_eq!(status, 200, "{body}");
        if !body["tuning"].is_null() {
            profile = body;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tuning = &profile["tuning"];
    assert_eq!(tuning["window"], 16384, "{profile}");
    assert_eq!(tuning["max_tokens"], 2048, "{profile}");
    assert_eq!(tuning["tools_off"], true, "{profile}");
    // 12000/1500 + (2048 + 15 x 128)/50 = 87.4 s; doubled and rounded up to 180.
    assert_eq!(tuning["timeout_sec"], 180, "{profile}");
    // Research's answer deadline follows the same sample: (1500 + 1000)/1500 +
    // 512/50 = 11.9 s, doubled and rounded up, floored at 60.
    assert_eq!(tuning["synthesis_seconds"], 60, "{profile}");
    assert_eq!(tuning["speed"]["decode_tps"], 50.0, "{profile}");
    // The sample was one raw generate for the selection, never with num_ctx.
    let generates = ollama.generates.lock().unwrap().clone();
    assert!(
        generates
            .iter()
            .any(|g| g["raw"] == true && g["model"] == "humanizer:q8"),
        "{generates:?}"
    );
    assert!(generates.iter().all(|g| !g.to_string().contains("num_ctx")));
    // With web on, chat still offers this tool-less model no tools, and asks for
    // the tuned reply budget instead of the configured 4096.
    s.post_json("/api/web", json!({"enabled": true})).await;
    let (status, reply) = s.post_json("/api/chat", json!({"message": "Rewrite: hello"})).await;
    assert_eq!(status, 200, "{reply}");
    let chats = ollama.chats.lock().unwrap().clone();
    let request = chats.last().expect("chat reached the model");
    assert_eq!(request["model"], "humanizer:q8");
    assert_eq!(request["max_tokens"], 2048, "{request}");
    assert!(request.get("tools").is_none(), "{request}");
    let (_, status) = s.get_json("/api/status").await;
    assert_eq!(status["chat_tools_available"], false, "{status}");
}

async fn wait_for_tuning(s: &TestServer, window: u64) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = s.get_json("/api/ollama/profile").await;
        assert_eq!(status, 200, "{body}");
        if body["tuning"]["window"] == window {
            return body;
        }
        assert!(std::time::Instant::now() < deadline, "no tuning at {window}: {body}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn research_cold_load_obeys_its_deadline_and_releases_the_gate() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.model", "longctx:q8")
            .with("models.local_llm.auto_tune", "true")
            .with("web.research_seconds", "10"),
    )
    .await;
    wait_for_tuning(&s, 65536).await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"https://example.org/*"}))
            .await
            .0,
        200
    );
    *ollama.longctx_window.lock().unwrap() = 0;
    *ollama.load_delay_ms.lock().unwrap() = 30_000;
    ollama.generates.lock().unwrap().clear();
    let (_, result) = tokio::time::timeout(
        Duration::from_secs(12),
        s.post_json("/api/web/research", json!({"query":"cold load deadline"})),
    )
    .await
    .expect("the research deadline must interrupt the cold load");
    assert_eq!(result["detail"]["code"], "WEB_RESEARCH_TIMEOUT", "{result}");
    assert_eq!(ollama.generates.lock().unwrap().len(), 1);
    assert!(ollama.chats.lock().unwrap().is_empty());
    assert!(!s.state.generation_gate.is_held());
    *ollama.load_delay_ms.lock().unwrap() = 0;
    let (status, result) = s.post_json("/api/chat", json!({"message":"Recovery"})).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(ollama.chats.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn research_preload_keeps_the_model_for_both_phase_budgets() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.model", "longctx:q8")
            .with("models.local_llm.auto_tune", "true")
            .with("models.local_llm.warmup.keep_alive_sec", "1")
            .with("web.research_seconds", "10")
            .with("web.synthesis_seconds", "60"),
    )
    .await;
    wait_for_tuning(&s, 65536).await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"https://example.org/*"}))
            .await
            .0,
        200
    );
    *ollama.longctx_window.lock().unwrap() = 0;
    ollama.generates.lock().unwrap().clear();
    s.post_json("/api/web/research", json!({"query":"preload residency"}))
        .await;
    let generates = ollama.generates.lock().unwrap();
    assert_eq!(generates.len(), 1, "{generates:?}");
    assert_eq!(generates[0]["keep_alive"], 70, "{generates:?}");
    assert_eq!(generates[0]["prompt"], "");
    assert!(generates[0].get("num_ctx").is_none());
}

#[tokio::test]
async fn cleared_small_window_tuning_still_checks_the_live_window() {
    let ollama = start_native_ollama().await;
    *ollama.longctx_window.lock().unwrap() = 16_384;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.model", "longctx:q8")
            .with("models.local_llm.auto_tune", "true"),
    )
    .await;
    wait_for_tuning(&s, 16_384).await;
    let prompt_cap = s.state.prompt_cap("longctx:q8");
    assert_eq!(prompt_cap, 15_000);
    s.state.set_tuning(None);
    *ollama.longctx_window.lock().unwrap() = 8_192;
    let refused = s
        .state
        .ensure_window_allows("longctx:q8", prompt_cap, 0, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED");
    assert!(ollama.chats.lock().unwrap().is_empty());
    let refused = s
        .state
        .ensure_window_allows("longctx:q8", 0, 16_000, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED");
    *ollama.longctx_window.lock().unwrap() = 16_384;
    s.state
        .ensure_window_allows("longctx:q8", prompt_cap, 16_000, true)
        .await
        .unwrap();
    *ollama.longctx_window.lock().unwrap() = 0;
    assert!(s
        .state
        .ensure_window_allows("longctx:q8", 1_000, 0, true)
        .await
        .is_err());
    s.state
        .ensure_window_allows("longctx:q8", 30_000, 32_000, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_measured_larger_window_lifts_the_caps_but_only_configured_values_grow_budgets() {
    let ollama = start_native_ollama().await;
    // Raised past the old 32000 / 30000 code caps; untuned, the caps still hold them.
    let options = || {
        ServerOptions::default()
            .with("web.total_tokens", "60000")
            .with("chat.compact_prompt_tokens", "50000")
            .with("api.rate_limit.max_requests", "1000")
    };
    let plain = spawn_server(&ollama.openai_url(), options()).await;
    // With web on, the web budget is checked exactly as chat computes it: 32000
    // held at the cap, less a 256-token minimal tool result (31744), which no
    // longer binds below the 30000 prompt cap now that each prompt is charged once.
    let (_, body) = plain.get_json("/api/ollama/profile").await;
    assert_eq!(body["in_force"]["limit_source"], "chat.compact_prompt_tokens", "{body}");
    assert_eq!(body["in_force"]["prompt_limit"], 30000, "{body}");
    assert_eq!(body["in_force"]["web_total_tokens"], 32000, "{body}");
    plain.post_json("/api/web", json!({"enabled": false})).await;
    let (_, body) = plain.get_json("/api/ollama/profile").await;
    assert_eq!(
        body["in_force"],
        json!({"prompt_cap": 30000, "prompt_limit": 30000, "limit_source": "chat.compact_prompt_tokens",
            "web_total_tokens": 32000, "web_total_ceiling": 32000}),
        "{body}"
    );
    assert_eq!(body["proposed"]["state"], "shipped", "{body}");

    let s = spawn_server(
        &ollama.openai_url(),
        options().with("models.local_llm.auto_tune", "true"),
    )
    .await;
    s.post_json("/api/web", json!({"enabled": false})).await;
    // The startup tune measures the configured 32768-token model: shipped caps.
    let body = wait_for_tuning(&s, 32768).await;
    assert_eq!(body["in_force"]["prompt_cap"], 30000, "{body}");
    assert_eq!(body["in_force"]["web_total_tokens"], 28000, "{body}");
    let (status, body) = s.post_json("/api/model", json!({"model": "longctx:q8"})).await;
    assert_eq!(status, 200, "{body}");
    let body = wait_for_tuning(&s, 65536).await;
    assert_eq!(body["proposed"]["state"], "scaled_up", "{body}");
    // Caps double with the window; the budgets grow only to what config allows.
    assert_eq!(
        body["in_force"],
        json!({"prompt_cap": 60000, "prompt_limit": 48000, "limit_source": "chat.compact_prompt_tokens",
            "web_total_tokens": 56000, "web_total_ceiling": 56000}),
        "{body}"
    );
    assert_eq!(body["tuning"]["max_tokens"], 8192, "{body}");
    // Ollama restarted with a 32768 window: the next use re-reads /api/ps (no
    // cache), and the caps fall back though the recorded tuning still says 65536.
    *ollama.longctx_window.lock().unwrap() = 32768;
    let (_, stale) = s.get_json("/api/ollama/profile").await;
    assert_eq!(stale["tuning"]["window"], 65536, "{stale}");
    assert_eq!(stale["in_force"]["prompt_cap"], 30000, "{stale}");
    assert_eq!(stale["in_force"]["web_total_ceiling"], 32000, "{stale}");
    // Each model call sized above the defaults re-reads the window first: a
    // shrink between the turn's start and its reply refuses the call.
    *ollama.longctx_window.lock().unwrap() = 65536;
    let state = &s.state;
    state
        .ensure_window_allows("longctx:q8", 48_000, 56_000, true)
        .await
        .expect("65536 still loaded");
    *ollama.longctx_window.lock().unwrap() = 32768;
    let refused = state
        .ensure_window_allows("longctx:q8", 48_000, 0, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    let refused = state
        .ensure_window_allows("longctx:q8", 0, 56_000, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    // Reloaded below the defaults: the caps shrink to the reported 16384, and a
    // call sized for the 30000 default is refused rather than truncated.
    *ollama.longctx_window.lock().unwrap() = 16384;
    let (_, small) = s.get_json("/api/ollama/profile").await;
    assert_eq!(small["in_force"]["prompt_cap"], 15000, "{small}");
    assert_eq!(small["in_force"]["web_total_ceiling"], 16000, "{small}");
    let refused = state
        .ensure_window_allows("longctx:q8", 30_000, 0, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    // Ollama restarted with a 16384 default and has not reloaded the model.
    // The profile only reads: no load, and the defaults while it is not resident.
    *ollama.longctx_window.lock().unwrap() = 0;
    *ollama.longctx_load_window.lock().unwrap() = 16384;
    let before = ollama.generates.lock().unwrap().len();
    let (_, absent) = s.get_json("/api/ollama/profile").await;
    assert_eq!(absent["in_force"]["prompt_cap"], 30000, "{absent}");
    assert_eq!(
        ollama.generates.lock().unwrap().len(),
        before,
        "a profile must not load the model"
    );
    // The check before a model call (under the caller's generation gate) loads it
    // with the warmup request and reads the window it now serves.
    state
        .ensure_window_allows("longctx:q8", 15_000, 0, true)
        .await
        .expect("15000 fits the reloaded 16384 window");
    let loads = ollama.generates.lock().unwrap()[before..].to_vec();
    assert!(
        loads
            .iter()
            .any(|g| g["model"] == "longctx:q8" && g["prompt"] == "" && g.get("num_ctx").is_none()),
        "{loads:?}"
    );
    let (_, reloaded) = s.get_json("/api/ollama/profile").await;
    assert_eq!(reloaded["in_force"]["prompt_cap"], 15000, "{reloaded}");
    // A load that leaves the window unreported is refused, not given the
    // 32768 defaults: the model was measured, so an unknown window is a failure.
    *ollama.longctx_window.lock().unwrap() = 0;
    *ollama.longctx_load_window.lock().unwrap() = 0;
    let refused = state
        .ensure_window_allows("longctx:q8", 1_000, 0, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    assert!(refused.message.contains("even after loading it"), "{refused:?}");
    // The load under the gate refuses on its own failure, after one attempt, so
    // the check before the model call does not repeat it.
    let before = ollama.generates.lock().unwrap().len();
    let refused = state.load_if_absent("longctx:q8", 0).await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    assert_eq!(ollama.generates.lock().unwrap().len() - before, 1, "one load, not two");
    // A turn sized for the tuning, which a model switch then clears: its raised
    // limits are still checked against the live window, not waved through.
    *ollama.longctx_window.lock().unwrap() = 32768;
    let tuning = state.tuning_for("longctx:q8").expect("tuned");
    state.set_tuning(None);
    let refused = state
        .ensure_window_allows("longctx:q8", 48_000, 0, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    state
        .ensure_window_allows("longctx:q8", 30_000, 0, false)
        .await
        .expect("defaults need no tuning");
    state.set_tuning(Some((*tuning).clone()));
    // A window at the 32768 defaults allows calls sized for them.
    state
        .ensure_window_allows("longctx:q8", 30_000, 32_000, true)
        .await
        .expect("32768 allows the defaults");
    *ollama.longctx_window.lock().unwrap() = 65536;
    let (status, reply) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{reply}");
    let request = ollama
        .chats
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("chat reached the model");
    assert_eq!(request["model"], "longctx:q8");
    // Reply budgets still only tighten: the configured 4096 wins over the proposed 8192.
    assert_eq!(request["max_tokens"], 4096, "{request}");
    // About 40000 tokens of history, inside the 48000 tuned threshold at 65536.
    let mut session = Value::Null;
    for turn in 0..5 {
        let long = format!("turn {turn} {}", "x".repeat(32_000));
        let (status, reply) = s
            .post_json("/api/chat", json!({"session_id": session, "message": long}))
            .await;
        assert_eq!(status, 200, "{reply}");
        session = reply["session_id"].clone();
    }
    // keep_alive expired: the next turn wakes the model under its gate and is
    // sized for 65536, so the history is sent whole, not refused or compacted
    // against the 30000 default of an unknown window.
    *ollama.longctx_window.lock().unwrap() = 0;
    *ollama.longctx_load_window.lock().unwrap() = 65536;
    let before = ollama.generates.lock().unwrap().len();
    let (status, reply) = s
        .post_json("/api/chat", json!({"session_id": session, "message": "and now?"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert!(
        ollama.generates.lock().unwrap()[before..]
            .iter()
            .any(|g| g["model"] == "longctx:q8" && g["prompt"] == ""),
        "the turn loads the absent model"
    );
    let request = ollama.chats.lock().unwrap().last().cloned().unwrap();
    let sent = request["messages"].to_string();
    assert!(
        sent.contains("turn 0 ") && sent.contains("and now?"),
        "history was compacted or cut"
    );
}

/// The window-scaled trigger bounds the input measured prefill reads in
/// chat.compact_max_prefill_sec, plus the turn's own reply: a /loop turn
/// (2048) compacts at the same input as a chat turn (4096).
#[tokio::test]
async fn a_loop_turn_compacts_at_the_input_measured_prefill_reads() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("api.rate_limit.max_requests", "1000")
            .with("api.harness_loop_rate_limit.max_tokens", "2048"),
    )
    .await;
    s.post_json("/api/web", json!({"enabled": false})).await;
    wait_for_tuning(&s, 32768).await;
    let (status, body) = s.post_json("/api/model", json!({"model": "longctx:q8"})).await;
    assert_eq!(status, 200, "{body}");
    wait_for_tuning(&s, 65536).await;
    // A slower prefill: 300 tok/s reads 36000 tokens in 120 s, under the 48000 proposal.
    let mut tuning = (*s.state.tuning_for("longctx:q8").expect("tuned")).clone();
    tuning.speed = Some(json!({"prefill_tps": 300.0}));
    s.state.set_tuning(Some(tuning));
    let (_, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(body["in_force"]["prompt_limit"], 36_000 + 4096, "{body}");
    let (_, created) = s.post_json("/api/sessions", json!({"title": "t"})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "g"}))
        .await;
    // About 31000 tokens of history, under what prefill reads.
    for turn in 0..4 {
        let long = format!("turn {turn} {}", "x".repeat(31_000));
        let (status, reply) = s
            .post_json("/api/chat", json!({"session_id": sid, "message": long}))
            .await;
        assert_eq!(status, 200, "{reply}");
        assert!(reply["compaction"].is_null(), "{reply}");
    }
    // A /loop turn of about 37000 input tokens compacts, though with its
    // 2048-token reply it is still under the 40096 a chat turn counts with 4096.
    let (status, reply) = s
        .post_json(
            "/api/chat",
            json!({"message": "y".repeat(16_000), "loop": true, "session_id": sid}),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["compaction"]["threshold"], 36_000 + 2048, "{reply}");
}

#[tokio::test]
async fn a_reload_at_a_smaller_window_scales_the_lifted_trigger_down_with_it() {
    let ollama = start_native_ollama().await;
    *ollama.longctx_window.lock().unwrap() = 131_072;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("chat.compact_prompt_tokens", "24000")
            .with("api.rate_limit.max_requests", "1000"),
    )
    .await;
    s.post_json("/api/web", json!({"enabled": false})).await;
    let (status, body) = s.post_json("/api/model", json!({"model": "longctx:q8"})).await;
    assert_eq!(status, 200, "{body}");
    // The configured 24000 trigger lifts to the 96000 proposed for 131072.
    let body = wait_for_tuning(&s, 131_072).await;
    assert_eq!(body["in_force"]["prompt_limit"], 96000, "{body}");
    // Ollama restarted smaller, or unloaded it, while the tuning still says
    // 131072: the trigger follows the verified window in proportion, below its
    // prompt cap, and an unknown window keeps the configured 24000.
    for (window, cap, trigger) in [(65_536, 60000, 48000), (32_768, 30000, 24000), (0, 30000, 24000)] {
        *ollama.longctx_window.lock().unwrap() = window;
        let (_, body) = s.get_json("/api/ollama/profile").await;
        assert_eq!(body["tuning"]["window"], 131_072, "{body}");
        assert_eq!(body["in_force"]["prompt_cap"], cap, "{body}");
        assert_eq!(body["in_force"]["prompt_limit"], trigger, "{body}");
    }
}

#[tokio::test]
async fn a_configured_trigger_sized_for_the_measured_window_follows_a_smaller_reload_down() {
    let ollama = start_native_ollama().await;
    *ollama.longctx_window.lock().unwrap() = 131_072;
    // The operator raised the trigger to the 96000 the profile proposes for 131072.
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("chat.compact_prompt_tokens", "96000")
            .with("api.rate_limit.max_requests", "1000"),
    )
    .await;
    s.post_json("/api/web", json!({"enabled": false})).await;
    let (status, body) = s.post_json("/api/model", json!({"model": "longctx:q8"})).await;
    assert_eq!(status, 200, "{body}");
    let body = wait_for_tuning(&s, 131_072).await;
    assert_eq!(body["in_force"]["prompt_limit"], 96000, "{body}");
    // Reloaded at 65536 the configured 96000 would sit above the 60000 hard
    // limit with no headroom; it scales with the window to the 48000 proposal,
    // and an unknown window keeps the 24000 the 32768 defaults give.
    for (window, cap, trigger) in [(65_536, 60000, 48000), (32_768, 30000, 24000), (0, 30000, 24000)] {
        *ollama.longctx_window.lock().unwrap() = window;
        let (_, body) = s.get_json("/api/ollama/profile").await;
        assert_eq!(body["tuning"]["window"], 131_072, "{body}");
        assert_eq!(body["in_force"]["prompt_cap"], cap, "{body}");
        assert_eq!(body["in_force"]["prompt_limit"], trigger, "{body}");
    }
}

#[tokio::test]
async fn a_planner_on_the_tuned_model_budgets_runs_with_its_measured_timeout() {
    let ollama = start_native_ollama().await;
    let planner_url = format!("\"{}\"", ollama.openai_url());
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("agentic.enabled", "true")
            .with("agentic.deepagent_github.enabled", "true")
            .with("agentic.deepagent_github.model", &format!("\"{}\"", shipped_model()))
            .with("api.rate_limit.max_requests", "1000")
            .with("agentic.deepagent_github.base_url", &planner_url),
    )
    .await;
    // The startup tune measures the shipped chat model, which is also the planner model here.
    let body = wait_for_tuning(&s, 32768).await;
    assert_eq!(body["tuning"]["planner_timeout_sec"], 180, "{body}");
    let run = json!({"instruction": "fix the parser", "branch": "claude/parser-fix",
        "commit_message": "fix: parser", "reason": "triage", "max_iterations": 10,
        "checks": ["cargo-test", "cargo-clippy", "cargo-fmt", "pytest", "ruff"]});
    let (status, resp) = s.post_json("/api/agent/run", run.clone()).await;
    assert_eq!(status, 422, "{resp}");
    let details = &resp["detail"]["details"];
    // Budgeted with 180 s per planner call instead of the configured 720.
    let budget = |planner, n| cgagentharness::shim::real_repo_run_budget_sec(planner, Some(n), 5);
    let fit = |planner| (1..=10).filter(|n| budget(planner, *n) <= 3600).max().unwrap_or(0);
    assert_eq!(details["estimated_sec"], budget(180, 10), "{resp}");
    assert_eq!(details["max_iterations_that_fit"], fit(180), "{resp}");
    assert!(fit(180) > fit(720), "{resp}");
    // The measured override cannot reach a different planner loaded by the child.
    let mut changed = s.state.cfg.raw.clone();
    changed["agentic"]["deepagent_github"]["model"] = "other-model".into();
    changed["agentic"]["deepagent_github"]["planner_timeout_sec"] = 1200.into();
    cgagentharness::common::config::AppConfig::write_private(
        &s.home.join("config.yaml"),
        &serde_yaml_ng::to_string(&changed).unwrap(),
    )
    .unwrap();
    let mut run = run;
    run["max_iterations"] = json!(1);
    run["checks"] = json!(["cargo-fmt"]);
    let (status, result) = s.post_json("/api/agent/run", run).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["exit_code"], 3, "{result}");
    assert!(
        result["stderr"].as_str().unwrap().contains("restart required"),
        "{result}"
    );
}

#[tokio::test]
async fn planner_endpoint_defaults_and_aliases_keep_sync_and_job_budgets_consistent() {
    let chat = start_native_ollama().await;
    let other = start_native_ollama().await;
    let chat_url = chat.openai_url();
    let cases = [
        ("omitted", None, 720),
        ("same listener", Some(chat_url.clone()), 180),
        (
            "normalized URL",
            Some(format!("{}/", chat_url.replacen("http:", "HTTP:", 1))),
            180,
        ),
        ("distinct listener", Some(other.openai_url()), 720),
        ("hostname alias", Some(chat_url.replace("127.0.0.1", "localhost")), 720),
        (
            "IPv4/IPv6 distinction",
            Some(chat_url.replace("127.0.0.1", "[::1]")),
            720,
        ),
        ("path case", Some(chat_url.replace("/v1", "/V1")), 720),
        ("different path", Some(chat_url.replace("/v1", "/other/v1")), 720),
    ];
    for (label, planner_url, expected_timeout) in cases {
        let mut prepared = prepare_server(
            &chat_url,
            ServerOptions::default()
                .with("models.local_llm.auto_tune", "true")
                .with("api.rate_limit.max_requests", "1000")
                .with("agentic.repo", "fixture/repository")
                .with("agentic.deepagent_github.model", &format!("\"{}\"", shipped_model()))
                .with("agentic.deepagent_github.planner_timeout_sec", "720"),
        )
        .await;
        let cfg = prepared.app_opts.config.as_mut().unwrap();
        let planner = cfg.raw["agentic"]["deepagent_github"].as_mapping_mut().unwrap();
        let key = serde_yaml_ng::Value::String("base_url".into());
        if let Some(url) = &planner_url {
            planner.insert(key, serde_yaml_ng::Value::String(url.clone()));
        } else {
            planner.remove(&key);
        }
        // Persist before startup: this is sparse-config compatibility, not a reload race.
        std::fs::write(&cfg.path, serde_yaml_ng::to_string(&cfg.raw).unwrap()).unwrap();
        if planner_url.is_none() {
            let child = cgagentharness::agentic::config::load_agentic_config(cfg, &prepared.home_dir).unwrap();
            assert_eq!(child.deepagent.base_url, "http://localhost:11434/v1");
            assert_eq!(child.deepagent.planner_timeout_sec, 720);
        }
        let (router, state) = cgagentharness::server::build_app(prepared.app_opts).await.unwrap();
        let s = start_test_server(prepared.tmp, prepared.home_dir, prepared.api_key, router, state).await;
        let profile = wait_for_tuning(&s, 32768).await;
        assert_eq!(profile["tuning"]["planner_timeout_sec"], 180, "{label}: {profile}");
        let run = json!({"instruction": "fix the parser", "branch": "codex/parser-fix",
            "commit_message": "fix: parser", "reason": "endpoint regression", "max_iterations": 10,
            "checks": ["cargo-test", "cargo-clippy", "cargo-fmt", "pytest", "ruff"]});
        let budget = cgagentharness::shim::real_repo_run_budget_sec(expected_timeout, Some(10), 5);
        for route in ["/api/agent/run", "/api/agent/jobs"] {
            let (status, response) = s.post_json(route, run.clone()).await;
            assert_eq!(status, 422, "{label} {route}: {response}");
            assert_eq!(
                response["detail"]["code"], "AGENTIC_BUDGET_EXCEEDED",
                "{label} {route}: {response}"
            );
            assert_eq!(
                response["detail"]["details"]["estimated_sec"], budget,
                "{label} {route}: {response}"
            );
        }
    }
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

#[tokio::test]
async fn startup_reconciles_saved_selection_before_warmup_and_tuning() {
    // Changed config wins; unchanged config retains an explicit selection.
    // A legacy mismatch is reset, while a legacy matching tag is migrated.
    for (saved, previous, expected) in [
        ("longctx:q8", Some("old:q8"), shipped_model()),
        ("longctx:q8", Some(shipped_model()), "longctx:q8"),
        ("longctx:q8", None, shipped_model()),
        (shipped_model(), None, shipped_model()),
        ("", None, shipped_model()),
        ("grok", Some(shipped_model()), shipped_model()),
    ] {
        let ollama = start_native_ollama().await;
        let prepared = prepare_server(
            &ollama.openai_url(),
            ServerOptions::default()
                .with("models.local_llm.warmup.enabled", "true")
                .with("models.local_llm.auto_tune", "true"),
        )
        .await;
        std::fs::write(
            prepared.home_dir.join("harness.json"),
            json!({
                "selected_model": saved, "selected_model_config": previous,
            })
            .to_string(),
        )
        .unwrap();
        let (router, state) = cgagentharness::server::build_app(prepared.app_opts).await.unwrap();
        let s = start_test_server(prepared.tmp, prepared.home_dir, prepared.api_key, router, state).await;
        assert_eq!(s.state.current_model(), expected);
        for _ in 0..200 {
            if s.state.tuning_for(expected).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let gens = ollama.generates.lock().unwrap().clone();
        assert_eq!(gens.len(), 2, "{saved} / {previous:?}: {gens:?}");
        assert_eq!(gens[0]["prompt"], "");
        assert_eq!(gens[1]["raw"], true);
        assert!(gens
            .iter()
            .all(|g| g["model"] == expected && !g.to_string().contains("num_ctx")));
        let persisted: Value =
            serde_json::from_str(&std::fs::read_to_string(s.home.join("harness.json")).unwrap()).unwrap();
        assert_eq!(persisted["selected_model_config"], shipped_model());
        let status: Value = s
            .req(Method::GET, "/api/status")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(status["configured_model"], shipped_model());
        assert_eq!(status["selection_overrides_config"], expected != shipped_model());
        let (code, body) = s.post_json("/api/model", json!({"model": "longctx:q8"})).await;
        assert_eq!(code, 200, "{body}");
        let saved = cgagentharness::common::home::HarnessSettings::load(&s.state.home).unwrap();
        assert_eq!(saved.selected_model, "longctx:q8");
        assert_eq!(saved.selected_model_config.as_deref(), Some(shipped_model()));
    }
}

#[tokio::test]
async fn status_distinguishes_automatic_fallback_from_a_saved_model_override() {
    let ollama = start_native_ollama().await;
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.model", "missing-primary:fixture")
            .with("models.local_llm.fallback.enabled", "true")
            .with("models.local_llm.fallback.provider", "ollama")
            .with(
                "models.local_llm.fallback.base_url",
                &format!("\"{}\"", ollama.openai_url()),
            )
            .with("models.local_llm.fallback.model", shipped_model()),
    )
    .await;
    assert_eq!(s.state.backend.source, "fallback");
    let (code, status) = s.get_json("/api/status").await;
    assert_eq!(code, 200);
    assert_eq!(status["model"], shipped_model());
    assert_eq!(status["configured_model"], "missing-primary:fixture");
    assert_eq!(status["selection_overrides_config"], false);
    assert!(s.state.settings.lock().unwrap().selected_model.is_empty());
    let (code, body) = s.post_json("/api/model", json!({"model":"longctx:q8"})).await;
    assert_eq!(code, 200, "{body}");
    let (_, status) = s.get_json("/api/status").await;
    assert_eq!(status["model"], "longctx:q8");
    assert_eq!(status["selection_overrides_config"], true);
}

#[tokio::test]
async fn pull_refuses_a_progress_line_over_max_response_bytes() {
    // A native server whose pull would succeed, except that one progress line
    // is 2048 bytes: only the configured byte bound can refuse it.
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": shipped_model()}]})) }),
        )
        .route(
            "/api/tags",
            get(|| async { Json(json!({"models": [{"name": "tinyllama:latest", "size": 1}]})) }),
        )
        .route(
            "/api/pull",
            post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
                    format!("{{\"status\":\"{}\"}}\n{{\"status\":\"success\"}}\n", "x".repeat(2048)),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let s = spawn_server(
        &format!("{base}/v1"),
        ServerOptions::default().with("models.local_llm.pull.max_response_bytes", "1024"),
    )
    .await;
    let (status, body) = s
        .post_json("/api/ollama/pull", json!({"model": "tinyllama:latest"}))
        .await;
    assert_eq!(status, 502, "{body}");
    let finished: Vec<Value> = s
        .audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == "ollama_pull_finished")
        .collect();
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(finished[0]["ok"], false);
}
