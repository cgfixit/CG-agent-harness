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

struct NativeOllama {
    base: String,
    pulls: Arc<Mutex<Vec<Value>>>,
    generates: Arc<Mutex<Vec<Value>>>,
    chats: Arc<Mutex<Vec<Value>>>,
    pull_delay_ms: Arc<Mutex<u64>>,
    /// Delay for the configured model's raw speed sample (the startup tune).
    sample_delay_ms: Arc<Mutex<u64>>,
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
                if body.get("num_ctx").is_some() || body["model"] != "qwen3.8:27b-mlx" {
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
                        json!({"name": "qwen3.8:27b-mlx", "context_length": 32768, "size": 20, "size_vram": 20}),
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
                let (w, l) = (w3.clone(), l2.clone());
                async move {
                    // Any generate loads longctx with Ollama's current default window.
                    if body["model"] == "longctx:q8" && *w.lock().unwrap() == 0 {
                        *w.lock().unwrap() = *l.lock().unwrap();
                    }
                    let timed = body["raw"] == true;
                    let ms = *s.lock().unwrap();
                    if timed && ms > 0 && body["model"] == "qwen3.8:27b-mlx" {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    g.lock().unwrap().push(body);
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

    let (status, _) = s.post_json("/api/model", json!({"model": "missing:latest"})).await;
    assert_eq!(status, 200);
    let (_, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(body["state"], "tag_missing", "{body}");
    assert!(!body.to_string().contains("fixture-private"), "{body}");

    let (status, _) = s.post_json("/api/model", json!({"model": "grok"})).await;
    assert_eq!(status, 200);
    let (_, body) = s.get_json("/api/ollama/profile").await;
    assert_eq!(body["state"], "not_probed", "{body}");
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
    // 12000/1500 + (2048 + 5 x 128)/50 = 61.8 s; doubled and rounded up to 150.
    assert_eq!(tuning["timeout_sec"], 150, "{profile}");
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
    // With web on, the web budget binds the prompt, exactly as chat computes it:
    // 32000 held at the cap, less one 4096-token reply reservation.
    let (_, body) = plain.get_json("/api/ollama/profile").await;
    assert_eq!(body["in_force"]["limit_source"], "web.total_tokens", "{body}");
    assert_eq!(body["in_force"]["prompt_limit"], 27904, "{body}");
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
        .ensure_window_allows("longctx:q8", 48_000, 56_000)
        .await
        .expect("65536 still loaded");
    *ollama.longctx_window.lock().unwrap() = 32768;
    let refused = state.ensure_window_allows("longctx:q8", 48_000, 0).await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    let refused = state.ensure_window_allows("longctx:q8", 0, 56_000).await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    // Reloaded below the defaults: the caps shrink to the reported 16384, and a
    // call sized for the 30000 default is refused rather than truncated.
    *ollama.longctx_window.lock().unwrap() = 16384;
    let (_, small) = s.get_json("/api/ollama/profile").await;
    assert_eq!(small["in_force"]["prompt_cap"], 15000, "{small}");
    assert_eq!(small["in_force"]["web_total_ceiling"], 16000, "{small}");
    let refused = state.ensure_window_allows("longctx:q8", 30_000, 0).await.unwrap_err();
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
        .ensure_window_allows("longctx:q8", 15_000, 0)
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
    let refused = state.ensure_window_allows("longctx:q8", 1_000, 0).await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    assert!(refused.message.contains("even after loading it"), "{refused:?}");
    // The load under the gate refuses on its own failure, after one attempt, so
    // the check before the model call does not repeat it.
    let before = ollama.generates.lock().unwrap().len();
    let refused = state.load_if_absent("longctx:q8").await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    assert_eq!(ollama.generates.lock().unwrap().len() - before, 1, "one load, not two");
    // A turn sized for the tuning, which a model switch then clears: its raised
    // limits are still checked against the live window, not waved through.
    *ollama.longctx_window.lock().unwrap() = 32768;
    let tuning = state.tuning_for("longctx:q8").expect("tuned");
    state.set_tuning(None);
    let refused = state.ensure_window_allows("longctx:q8", 48_000, 0).await.unwrap_err();
    assert_eq!(refused.code, "OLLAMA_WINDOW_CHANGED", "{refused:?}");
    state
        .ensure_window_allows("longctx:q8", 30_000, 0)
        .await
        .expect("defaults need no tuning");
    state.set_tuning(Some((*tuning).clone()));
    // A window at the 32768 defaults allows calls sized for them.
    state
        .ensure_window_allows("longctx:q8", 30_000, 32_000)
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

#[tokio::test]
async fn a_planner_on_the_tuned_model_budgets_runs_with_its_measured_timeout() {
    let ollama = start_native_ollama().await;
    let planner_url = format!("\"{}\"", ollama.openai_url());
    let s = spawn_server(
        &ollama.openai_url(),
        ServerOptions::default()
            .with("models.local_llm.auto_tune", "true")
            .with("api.rate_limit.max_requests", "1000")
            .with("agentic.deepagent_github.base_url", &planner_url),
    )
    .await;
    // The startup tune measures qwen3.8:27b-mlx, which is also the planner model.
    let body = wait_for_tuning(&s, 32768).await;
    assert_eq!(body["tuning"]["planner_timeout_sec"], 180, "{body}");
    let run = json!({"instruction": "fix the parser", "branch": "claude/parser-fix",
        "commit_message": "fix: parser", "reason": "triage", "max_iterations": 10,
        "checks": ["cargo-test", "cargo-clippy", "cargo-fmt", "pytest", "ruff"]});
    let (status, resp) = s.post_json("/api/agent/run", run).await;
    assert_eq!(status, 422, "{resp}");
    let details = &resp["detail"]["details"];
    // Budgeted with 180 s per planner call instead of the configured 720.
    let budget = |planner, n| cgagentharness::shim::real_repo_run_budget_sec(planner, Some(n), 5);
    let fit = |planner| (1..=10).filter(|n| budget(planner, *n) <= 3600).max().unwrap_or(0);
    assert_eq!(details["estimated_sec"], budget(180, 10), "{resp}");
    assert_eq!(details["max_iterations_that_fit"], fit(180), "{resp}");
    assert!(fit(180) > fit(720), "{resp}");
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
