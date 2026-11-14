//! `POST /api/browser/run`: one model-driven browser task for an administrator.
//!
//! Gated by literal `browser.enabled` (ships false, restart-only). The request
//! names the task and the origins it may reach; the page and the model cannot
//! add one. The work runs in a fixed-argv `cgagentharness browser run` child
//! with the browser environment allowlist, so the server spawns nothing itself
//! and a timeout kills the child's whole tree.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::browser::Session;
use crate::common::browser_host::{child_environment, MAX_TASK_CHARS};
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;
use crate::shim::ShimError;

/// One Chrome and one local model run at a time on this machine.
static BUSY: AtomicBool = AtomicBool::new(false);

struct Release;
impl Drop for Release {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRunRequest {
    pub task: String,
    pub origins: Vec<String>,
}

impl Validate for BrowserRunRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !(1..=MAX_TASK_CHARS).contains(&self.task.chars().count()) {
            bad.push("task".into());
        }
        if self.origins.is_empty() || Session::general(&self.origins).is_err() {
            bad.push("origins".into());
        }
        bad
    }
}

pub async fn run(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<BrowserRunRequest>,
) -> ApiResult<Json<Value>> {
    let cfg = state.cfg.clone();
    if !cfg.flag_is_true("browser.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_DISABLED",
            "Browser sessions are off. Set browser.enabled: true in config.yaml and restart.",
        ));
    }
    // The shared block list wins here as it does for web reads.
    // shortcut: only origin-level rules are enforced; a path-level block rule needs the proxy to see it.
    let policy = state.web_snapshot().policy().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "WEB_POLICY_UNAVAILABLE",
            "The shared URL policy file is missing or unreadable, so browser tasks are refused.",
        )
    })?;
    if req
        .origins
        .iter()
        .any(|origin| crate::server::web_policy::canonical_url(origin).is_ok_and(|url| policy.blocks(&url)))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "BROWSER_ORIGIN_BLOCKED",
            "An origin is on the shared web block list; /web status lists the rules.",
        ));
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_BUSY",
            "A browser task is already running.",
        ));
    }
    let _release = Release;
    let owner = super::auth::context_owner(user);
    let exe = std::env::current_exe()
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "BROWSER_FAILED", "harness binary not found"))?;
    let cwd = std::env::current_dir().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "working directory unreadable",
        )
    })?;
    let mut argv = vec![
        exe.to_string_lossy().into_owned(),
        "browser".into(),
        "run".into(),
        format!("--task={}", req.task),
        format!("--sidecar={}", cfg.str_or("browser.sidecar", "browser/sidecar.mjs")),
        format!("--max-turns={}", cfg.limit("browser.max_turns")),
    ];
    argv.extend(req.origins.iter().map(|origin| format!("--origin={origin}")));
    let timeout = Duration::from_secs(cfg.limit("browser.timeout_sec"));
    let result = crate::shim::run_argv_with_env(&argv, &cwd, timeout, child_environment()).await;
    let exit = result.as_ref().map(|(code, _, _)| *code).ok();
    state.audit.log(json!({
        "event": "browser.run",
        "owner": owner,
        "origins": req.origins,
        "task_sha256": crate::common::sha256_hex(&req.task),
        "exit": exit,
    }));
    let (code, stdout, stderr) = result.map_err(|err| match err {
        ShimError::Timeout { .. } => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "BROWSER_TIMEOUT",
            "The browser task did not finish within browser.timeout_sec; its processes were stopped.",
        ),
        _ => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "The browser child could not run.",
        ),
    })?;
    let report = serde_json::from_str::<Value>(stdout.trim()).ok();
    let mut body = json!({"finished": code == 0, "exit_code": code, "report": report});
    if body["report"].is_null() {
        // Only the child's own `cgagentharness:` lines; Node and page output stay out.
        let reason = stderr
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("cgagentharness: "))
            .unwrap_or("the browser child printed no report");
        body["reason"] = json!(reason.chars().take(300).collect::<String>());
    }
    Ok(Json(body))
}
