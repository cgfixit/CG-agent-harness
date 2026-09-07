//! The loopback HTTP console (`cgagentharness serve`).
//!
//! `build_app` is the test boundary (mirrors CyClaw's `create_app`): tests
//! pass a temp home, an optional config override and a mock model URL, and
//! get back the router plus the shared state. `serve_blocking` adds the
//! bind guard (loopback only), the port bounds and the port-in-use probe.

pub mod agent_jobs;
pub mod agent_policy;
pub mod console;
pub mod env_keys;
pub mod errors;
pub mod generation_gate;
pub mod guards;
pub mod headers;
pub mod memory_notes;
pub mod prompts;
pub mod request_log;
pub mod routes;
pub mod schemas;
pub mod sessions;
pub mod state;
pub mod views;
pub mod web_search;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;

use crate::common::audit::Audit;
use crate::common::auth_store::AuthManager;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::home::{is_loopback_host, validate_port, HarnessSettings, Home, DEFAULT_HOST};
use crate::common::ratelimit::RateLimiter;
use crate::llm::backend::resolve_local_backend;
use crate::llm::openai_chat::{ChatClient, DEFAULT_CHAT_TIMEOUT_SEC};
use crate::shim::ShimContext;

use generation_gate::GenerationGate;
use memory_notes::MemoryNotes;
use sessions::SessionStore;
use state::AppState;
use web_search::WebTool;

pub const HOST_ENV: &str = "CGAGENTHARNESS_HARNESS_HOST";
pub const PORT_ENV: &str = "CGAGENTHARNESS_HARNESS_PORT";
const DEFAULT_LOOP_MAX_REQUESTS: u64 = 8;
const DEFAULT_LOOP_WINDOW_SEC: f64 = 300.0;
const DEFAULT_LOOP_MAX_TOKENS: u64 = 2048;

/// Options for `build_app`; everything optional except the home.
pub struct AppOptions {
    pub home: Home,
    /// Overrides the home's config.yaml when set (tests).
    pub config: Option<AppConfig>,
    /// Snapshot of the API key; `None` reads `CGAGENTHARNESS_API_KEY`.
    pub api_key: Option<String>,
    /// Test hook: replace the closed tool allowlists (empty set = deny all).
    pub tool_allowlist_override: Option<BTreeSet<String>>,
    /// Test hook: pin a hostname to an address for `/web` fetches.
    pub web_test_resolve: Option<(String, SocketAddr)>,
    /// Test hook: the executable the shim spawns (tests run inside the test
    /// harness binary, whose `current_exe()` is not `cgagentharness`).
    pub shim_exe: Option<PathBuf>,
}

impl AppOptions {
    pub fn new(home: Home) -> Self {
        Self {
            home,
            config: None,
            api_key: None,
            tool_allowlist_override: None,
            web_test_resolve: None,
            shim_exe: None,
        }
    }
}

pub async fn build_app(opts: AppOptions) -> Result<(Router, Arc<AppState>)> {
    let home = opts.home;
    home.ensure_layout()?;
    let cfg = Arc::new(match opts.config {
        Some(c) => c,
        None => home.load_config()?,
    });
    let settings = HarnessSettings::load(&home)?;
    let store = SessionStore::new(&home.sessions_dir())?;
    let audit = Audit::from_home(&home.root, &cfg);
    let auth = if cfg.flag_is_true("auth.enabled") {
        let mgr = AuthManager::open(&home.auth_path(), &cfg)?;
        mgr.bootstrap_if_empty()?;
        Some(mgr)
    } else {
        None
    };
    let backend = resolve_local_backend(&cfg).await?;
    let chat = ChatClient::new(
        &backend.base_url,
        &backend.model,
        cfg.f64_or("models.local_llm.timeout_sec", DEFAULT_CHAT_TIMEOUT_SEC),
        &backend.api_key,
        backend.reasoning_effort.clone(),
    )?;
    let rate_limiter = RateLimiter::new(
        cfg.u64_or("api.rate_limit.max_requests", 60) as usize,
        cfg.f64_or("api.rate_limit.window_seconds", 60.0).max(0.001),
    );
    let loop_rate_limiter = RateLimiter::new(
        cfg.u64_or("api.harness_loop_rate_limit.max_requests", DEFAULT_LOOP_MAX_REQUESTS)
            .max(1) as usize,
        cfg.f64_or("api.harness_loop_rate_limit.window_seconds", DEFAULT_LOOP_WINDOW_SEC)
            .max(0.001),
    );
    let loop_max_tokens = match cfg.u64_or("api.harness_loop_rate_limit.max_tokens", DEFAULT_LOOP_MAX_TOKENS) {
        0 => DEFAULT_LOOP_MAX_TOKENS,
        n => n,
    };
    let csrf_token = crate::common::random_urlsafe(32);
    let console_html = console::HARNESS_HTML.replace(console::CSRF_PLACEHOLDER, &csrf_token);
    let api_key = opts
        .api_key
        .or_else(|| std::env::var(crate::common::apikey::API_KEY_ENV).ok());
    let mut web = WebTool::new(&home.tools_dir());
    web.test_resolve = opts.web_test_resolve;
    let planner = cfg.u64_or(
        "agentic.deepagent_github.planner_timeout_sec",
        crate::shim::REAL_REPO_RUN_FALLBACK_PLANNER_SEC,
    );
    let mut shim = ShimContext::new(&cfg.path, &home.root, &home.tmp_dir(), planner)
        .map_err(|e| HarnessError::harness_config(format!("cannot locate own executable: {e}")))?;
    if let Some(exe) = opts.shim_exe {
        shim.exe = exe;
    }
    let state = Arc::new(AppState {
        notes: MemoryNotes::new(&home.memory_dir()),
        web,
        home,
        cfg: cfg.clone(),
        settings: Mutex::new(settings),
        store,
        backend,
        chat,
        audit,
        rate_limiter,
        loop_rate_limiter,
        loop_max_tokens,
        loop_inflight: Mutex::new(Default::default()),
        generation_gate: GenerationGate::new(),
        agent_run_gate: GenerationGate::new(),
        csrf_token,
        console_html,
        api_key_optional: cfg.flag_is_true("security.api_key_optional"),
        api_key,
        auth,
        tool_allowlist_override: opts.tool_allowlist_override,
        shim,
        jobs: agent_jobs::JobStore::new(),
        request_log: cfg.flag_is_true("logging.request_log"),
    });
    Ok((routes::build_router(state.clone()), state))
}

fn port_in_use(host: &str, port: u16) -> bool {
    let addr = format!("{host}:{port}");
    match addr.parse::<SocketAddr>() {
        Ok(sa) => std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_millis(500)).is_ok(),
        Err(_) => std::net::TcpStream::connect((host, port)).is_ok(),
    }
}

/// `cgagentharness serve`: loopback-only bind, port bounds, port-in-use probe.
pub fn serve_blocking(host: Option<String>, port: Option<u16>) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let home = Home::resolve(None);
    home.ensure_layout()?;
    let settings = HarnessSettings::load(&home)?;
    let host = host
        .or_else(|| std::env::var(HOST_ENV).ok())
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    if !is_loopback_host(&host) {
        anyhow::bail!("harness binds loopback only (threat model: single-operator)");
    }
    let port = match port {
        Some(p) => validate_port(&p.to_string())?,
        None => match std::env::var(PORT_ENV) {
            Ok(v) if !v.trim().is_empty() => validate_port(v.trim())?,
            _ => settings.port,
        },
    };
    if port_in_use(&host, port) {
        println!("\nCGagentHarness may already be running on {host}:{port}.\nClose the other instance, or wait for the port to release, then try again.");
        return Ok(());
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let (app, state) = build_app(AppOptions::new(home)).await?;
        let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
        tracing::info!(
            "CGagentHarness console on http://{host}:{port}/ (home {})",
            state.home.root.display()
        );
        if state.api_key.as_deref().unwrap_or("").is_empty() && !state.api_key_optional {
            tracing::warn!(
                "{} is not set: guarded routes will refuse every request (fail-closed)",
                crate::common::apikey::API_KEY_ENV
            );
        }
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
        Ok::<(), anyhow::Error>(())
    })
}

/// Location of the executable for the shim (exposed for diagnostics).
pub fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cgagentharness"))
}
