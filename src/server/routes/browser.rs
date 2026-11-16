//! `POST /api/browser/run`: one model-driven browser task for an administrator.
//!
//! Gated by literal `browser.enabled` (ships true, restart-only). The request
//! names the task and the origins it may reach; the page and the model cannot
//! add one. The work runs in a fixed-argv `cgagentharness browser run` child
//! with the browser environment allowlist, so the server spawns nothing itself
//! and a timeout kills the child's whole tree. Non-public IP-literal origins
//! and origins on the shared block list are refused; the block list is read
//! again before each model turn (in the child) and before delivery.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::browser::Session;
use crate::common::browser_host::{blocked_origin, child_environment, MAX_TASK_CHARS};
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;
use crate::shim::ShimError;

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
    let session = Session::general(&req.origins)
        .map_err(|_| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "BROWSER_BAD_ORIGIN", "origins"))?;
    // A name is checked at connect time by the sidecar; an IP literal skips DNS,
    // so loopback and private literals are refused here (fixtures are test-only).
    if session.grant_strings().iter().any(|origin| non_public_literal(origin)) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "BROWSER_PRIVATE_ADDRESS",
            "Browser tasks reach public addresses only; a loopback or private IP origin is refused.",
        ));
    }
    // shortcut: only origin-level block rules are enforced; a path-level rule needs the proxy to see it.
    if blocked_origin(&current_policy(&state)?, &session).is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "BROWSER_ORIGIN_BLOCKED",
            "An origin is on the shared web block list, or could not be checked; /web status lists the rules.",
        ));
    }
    // One model operation at a time: browser tasks share the local backend.
    let _gate = state.generation_gate.claim("browser").ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_BUSY",
            "a model operation is already running",
        )
    })?;
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
    // The task goes in a private temporary file (0600, removed when this
    // handler returns), not in argv, which other local accounts can list.
    let task_file = write_task_file(&req.task)?;
    // Canonical origins only: a requested URL's path or query (a token, say)
    // never reaches argv, the audit log or the report.
    let grants = session.grant_strings();
    let mut argv = vec![
        exe.to_string_lossy().into_owned(),
        "browser".into(),
        "run".into(),
        format!("--task-file={}", task_file.path().display()),
        format!("--sidecar={}", cfg.str_or("browser.sidecar", "browser/sidecar.mjs")),
        format!("--max-turns={}", cfg.limit("browser.max_turns")),
    ];
    argv.extend(grants.iter().map(|origin| format!("--origin={origin}")));
    let timeout = Duration::from_secs(cfg.limit("browser.timeout_sec"));
    let result = crate::shim::run_argv_with_env(&argv, &cwd, timeout, child_environment()).await;
    drop(task_file);
    let exit = result.as_ref().map(|(code, _, _)| *code).ok();
    state.audit.log(json!({
        "event": "browser.run",
        "owner": owner,
        "origins": grants,
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
    // Permission checked at launch is stale after the run; deliver nothing if
    // the block list now covers a granted origin.
    if current_policy(&state).map_or(true, |policy| blocked_origin(&policy, &session).is_some()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "WEB_POLICY_CHANGED",
            "The URL policy changed during this browser task; nothing was delivered.",
        ));
    }
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

fn write_task_file(task: &str) -> ApiResult<tempfile::NamedTempFile> {
    use std::io::Write;
    let failed = |_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "the task file could not be written",
        )
    };
    let mut file = tempfile::Builder::new()
        .prefix("cgah-browse-task-")
        .tempfile()
        .map_err(failed)?;
    file.write_all(task.as_bytes()).map_err(failed)?;
    file.flush().map_err(failed)?;
    Ok(file)
}

fn current_policy(state: &AppState) -> ApiResult<crate::server::web_policy::Policy> {
    state.web_snapshot().policy().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "WEB_POLICY_UNAVAILABLE",
            "The shared URL policy file is missing or unreadable, so browser tasks are refused.",
        )
    })
}

/// True for a loopback, private or other non-public IP-literal origin.
fn non_public_literal(origin: &str) -> bool {
    match url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host().map(|host| host.to_owned()))
    {
        Some(url::Host::Ipv4(ip)) => !crate::server::web_policy::is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => !crate::server::web_policy::is_public_ip(ip.into()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_origins_must_be_public() {
        for origin in [
            "https://127.0.0.1:8080",
            "https://10.0.0.5:443",
            "https://[::1]:443",
            "https://192.168.1.1:443",
        ] {
            assert!(non_public_literal(origin), "{origin}");
        }
        for origin in [
            "https://example.com:443",
            "https://8.8.8.8:443",
            "https://localhost:443",
        ] {
            assert!(!non_public_literal(origin), "{origin}");
        }
    }
}
