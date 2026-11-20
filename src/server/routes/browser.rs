//! `POST /api/browser/run`: one model-driven browser task for an administrator.
//!
//! Gated by literal `browser.enabled` (ships true, restart-only) and literal
//! `auth.enabled`: without accounts the admin check does not run. The request
//! names the task and the origins it may reach; the page and the model cannot
//! add one. The work runs in a fixed-argv `cgagentharness browser run` child
//! with the browser environment allowlist, so the server spawns nothing itself
//! and a timeout kills the child's whole tree. Non-public IP-literal origins
//! and origins on the shared block list are refused; the block list is read
//! again before each model turn (in the child) and before delivery. The child
//! is told chat's local model selection, so `/model use` applies to it, and a
//! deadline inside `browser.timeout_sec`, so it reports before it is stopped.
//! A tuned model is loaded and its window read before the child starts.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::browser::Session;
use crate::common::browser_host::{blocked_origin, child_environment, MAX_TASK_CHARS, TASK_PROMPT_TOKENS};
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
    // Without accounts the admin check in guards.rs does not run, so any
    // client holding the console's CSRF token could drive Chrome (#945).
    if !cfg.flag_is_true("auth.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_AUTH_REQUIRED",
            "Browser tasks need accounts: set auth.enabled: true in config.yaml and restart, then sign in as an administrator.",
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
    // Refused at once, never queued; the refusal says what holds the model.
    let _gate = state.generation_gate.claim_or_busy_owner("browser").map_err(|holder| {
        ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_BUSY",
            format!(
                "The local model is busy: {}. Nothing was started; send /browse again when it finishes.",
                busy_reason(&holder)
            ),
        )
    })?;
    // Under the gate: a tuned model that is not loaded (keep_alive ran out) is
    // loaded here, outside the task's time, and a window too small for a task's
    // prompt is refused; Ollama would cut the prompt without saying so.
    let model = task_model(&state.backend.source, state.local_memory_model());
    if let Some(model) = &model {
        if state
            .ensure_window_allows(model, TASK_PROMPT_TOKENS, 0, false)
            .await
            .is_err()
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "BROWSER_WINDOW",
                "Ollama does not report the selected model loaded with a window that fits a browser task; \
                 nothing was started. /model profile shows the window.",
            ));
        }
    }
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
    let timeout_sec = cfg.limit("browser.timeout_sec");
    argv.push(format!("--deadline-sec={}", child_deadline_sec(timeout_sec)));
    argv.extend(model.iter().map(|model| format!("--model={model}")));
    argv.extend(grants.iter().map(|origin| format!("--origin={origin}")));
    let timeout = Duration::from_secs(timeout_sec);
    let result = crate::shim::run_argv_with_env(&argv, &cwd, timeout, child_environment()).await;
    drop(task_file);
    let exit = result.as_ref().map(|(code, _, _)| *code).ok();
    state.audit.log(json!({
        "event": "browser.run",
        "owner": owner,
        "origins": grants,
        "model": model,
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
    // #901: composed navigations refused and typed text held, by origin and
    // count only. The child reports origins, never a refused URL's path or query.
    if let Some(report) = &report {
        let typed_held = report["for_operator"].as_array().map_or(0, |rows| {
            rows.iter()
                .filter(|row| row["reason"] == crate::common::browser::TYPED_TEXT_HOLD)
                .count()
        });
        let refused = report["refused_composed"].as_array().filter(|rows| !rows.is_empty());
        if refused.is_some() || typed_held > 0 {
            state.audit.log(json!({
                "event": "browser.provenance",
                "owner": owner,
                "refused_composed": refused,
                "typed_held": typed_held,
            }));
        }
    }
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

/// The model tag the child is told to use: chat's `/model use` selection when
/// it is local, else the configured local model. `None` leaves the child on
/// `models.local_llm.model`. The child talks only to the configured primary
/// endpoint, so a server on its fallback backend holds no tag valid there.
fn task_model(backend_source: &str, model: String) -> Option<String> {
    (backend_source == "primary" && !model.trim().is_empty()).then_some(model)
}

/// Seconds the child gets of `browser.timeout_sec`: the rest is for closing
/// Chrome and printing the report before the runner stops the process tree.
fn child_deadline_sec(timeout_sec: u64) -> u64 {
    timeout_sec.saturating_sub(TEARDOWN_SEC).clamp(1, 1800)
}

/// The sidecar gets 5 s to exit after its pipe closes.
const TEARDOWN_SEC: u64 = 20;

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
            "The shared URL policy file is missing or unreadable, so browser tasks are refused. \
             An administrator's /web allow creates a missing one; an unreadable file is never reset.",
        )
    })
}

/// What holds the model gate, in the operator's words. The gate's labels are
/// internal, so one this list does not know is described, not printed.
fn busy_reason(holder: &str) -> &'static str {
    use crate::server::{model_limits::TUNE_OWNER, structured_memory_suggest::GATE_OWNER};
    match holder {
        TUNE_OWNER => "its speed is being measured after /model use, which takes a few seconds",
        "chat" => "a chat reply is being written",
        "browser" => "another browser task is running",
        "agent" => "a coding run is using it",
        "consolidation" | GATE_OWNER => "memory is being updated",
        _ => "another model operation is running",
    }
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

    /// #894: the busy refusal names what holds the model, in fixed words.
    #[test]
    fn a_busy_refusal_names_the_holder_without_printing_its_label() {
        let tune = busy_reason(crate::server::model_limits::TUNE_OWNER);
        assert!(tune.contains("/model use"), "{tune}");
        assert_eq!(busy_reason("browser"), "another browser task is running");
        for holder in ["attachment_unpin", "", "anything-new"] {
            assert_eq!(busy_reason(holder), "another model operation is running");
        }
    }

    #[test]
    fn only_a_primary_backend_names_the_task_model() {
        assert_eq!(
            task_model("primary", "gemma4:26b".into()).as_deref(),
            Some("gemma4:26b")
        );
        assert_eq!(task_model("fallback", "lmstudio-tag".into()), None);
        assert_eq!(task_model("primary", " ".into()), None);
    }

    #[test]
    fn the_child_deadline_stays_inside_the_task_timeout() {
        assert_eq!(child_deadline_sec(300), 280);
        assert_eq!(child_deadline_sec(30), 10);
        assert_eq!(child_deadline_sec(1800), 1780);
        assert_eq!(child_deadline_sec(5), 1);
    }

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
