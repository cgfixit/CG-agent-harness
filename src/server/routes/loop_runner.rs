//! Server-side `/loop` runner routes: start, stop, status and an SSE event stream.
//!
//! All routes sit behind the account gate and the process CSRF token (the
//! `guarded` router), like every other console mutation.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::auth_store::UserSummary;
use crate::server::config_reload::LoopLimits;
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::loop_runner::{LoopRun, LoopRunState, MAX_GOAL_CHARS};
use crate::server::schemas::ValidJson;
use crate::server::sessions::GoalWebMode;
use crate::server::state::AppState;

#[derive(Debug, Deserialize)]
pub struct StartLoopRequest {
    pub session_id: String,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub web_mode: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u64>,
    #[serde(default)]
    pub max_total_completion_tokens: Option<u64>,
    #[serde(default)]
    pub max_failures: Option<u64>,
    #[serde(default)]
    pub cooldown_ms: Option<u64>,
}

impl crate::server::schemas::Validate for StartLoopRequest {
    fn validate(&self) -> Vec<String> {
        if self.session_id.is_empty() {
            vec!["session_id".into()]
        } else {
            vec![]
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StopLoopRequest {
    pub run_id: String,
}

impl crate::server::schemas::Validate for StopLoopRequest {
    fn validate(&self) -> Vec<String> {
        if self.run_id.is_empty() {
            vec!["run_id".into()]
        } else {
            vec![]
        }
    }
}

fn override_limit(name: &str, value: Option<u64>, min: u64, max: u64) -> ApiResult<Option<u64>> {
    match value {
        None => Ok(None),
        Some(v) if (min..=max).contains(&v) => Ok(Some(v)),
        Some(_) => Err(ApiError::bad_request(
            "LOOP_CONFIG_INVALID",
            format!("{name} must be an integer from {min} to {max}"),
        )),
    }
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StartLoopRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    let session = state
        .store
        .for_owner(&owner)
        .get(&req.session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let portal_auditor = user
        .as_ref()
        .is_some_and(|u| u.role == crate::common::authn::PORTAL_AUDITOR_ROLE);
    if portal_auditor {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AUTH_PERMISSION_DENIED",
            "portal_auditor cannot start loop runs",
        ));
    }
    let web_mode = match req.web_mode.as_deref() {
        None => None,
        Some("off") => Some(GoalWebMode::Off),
        Some("fetch") => Some(GoalWebMode::Fetch),
        Some("all") => Some(GoalWebMode::All),
        Some(_) => {
            return Err(ApiError::bad_request(
                "LOOP_WEB_MODE_INVALID",
                "web_mode must be off, fetch or all",
            ))
        }
    };
    // A goal or web mode on the request persists on the session first, exactly
    // like the console's `/loop <goal>` does before it starts turning.
    let session = if req.goal.is_some() || web_mode.is_some() {
        let goal = req.goal.clone().unwrap_or_else(|| session.goal.clone());
        if goal.trim().is_empty() {
            return Err(ApiError::bad_request(
                "LOOP_REQUIRES_GOAL",
                "set a /goal before /loop (the goal text cannot be blank)",
            ));
        }
        if goal.chars().count() > MAX_GOAL_CHARS {
            return Err(ApiError::bad_request(
                "LOOP_GOAL_TOO_LONG",
                format!("goal is too long; keep it under {MAX_GOAL_CHARS} characters"),
            ));
        }
        let mode = web_mode.unwrap_or(state.runtime_limits().web.goal_web_default);
        state
            .store
            .for_owner(&owner)
            .set_goal(&session.session_id, &goal, mode)
            .map_err(|e| ApiError::from_err(session_status(&e), &e))?
    } else {
        session
    };
    if session.goal.trim().is_empty() {
        return Err(ApiError::bad_request("LOOP_REQUIRES_GOAL", "set a /goal before /loop"));
    }
    let base = state.runtime_limits().loop_limits;
    let config = LoopLimits {
        max_turns: override_limit("max_turns", req.max_turns, 0, 1_000_000)?.unwrap_or(base.max_turns),
        max_total_completion_tokens: override_limit(
            "max_total_completion_tokens",
            req.max_total_completion_tokens,
            0,
            100_000_000,
        )?
        .unwrap_or(base.max_total_completion_tokens),
        max_failures: override_limit("max_failures", req.max_failures, 1, 100)?.unwrap_or(base.max_failures),
        cooldown_ms: override_limit("cooldown_ms", req.cooldown_ms, 0, 300_000)?.unwrap_or(base.cooldown_ms),
    };
    let run = LoopRun {
        run_id: uuid::Uuid::new_v4().to_string(),
        session_id: session.session_id.clone(),
        goal: session.goal.clone(),
        web_mode: session.goal_web_mode.as_str().to_string(),
        config,
        state: LoopRunState::Running,
        reason: String::new(),
        stats: Default::default(),
    };
    let run = state
        .loops
        .start(state.clone(), run, user)
        .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))?;
    state.audit.log(json!({
        "event": "loop_run_started",
        "run_id": run.run_id,
        "session_id": run.session_id,
        "owner": owner,
        "max_turns": run.config.max_turns,
        "max_total_completion_tokens": run.config.max_total_completion_tokens,
    }));
    Ok(Json(json!({"ok": true, "run": run})))
}

pub async fn stop(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<StopLoopRequest>,
) -> ApiResult<Json<Value>> {
    let run = state.loops.stop(&req.run_id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "LOOP_RUN_NOT_FOUND",
            "no loop run with that run_id",
        )
    })?;
    Ok(Json(json!({"ok": true, "run": run})))
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> ApiResult<Json<Value>> {
    // Parsed by hand so a query-less probe (the catalog smoke test sends `{}`
    // with no query string) gets a typed 404 JSON instead of axum's plain-text
    // query rejection.
    let run_id = query
        .as_deref()
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "run_id")
        .map(|(_, value)| value.to_string());
    let run_id = match run_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "LOOP_RUN_NOT_FOUND",
                "no loop run with that run_id",
            ))
        }
    };
    let run = state.loops.get(&run_id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "LOOP_RUN_NOT_FOUND",
            "no loop run with that run_id",
        )
    })?;
    Ok(Json(json!({"ok": true, "run": run})))
}

/// Per-turn progress. The stream sends the run's current snapshot first, then
/// every broadcast event, and ends after the run reaches a terminal state (so
/// the connection does not outlive the run it watches).
pub async fn events(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> ApiResult<Response> {
    use axum::response::sse::{Event, KeepAlive, Sse};

    let receiver = state.loops.subscribe(&run_id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "LOOP_RUN_NOT_FOUND",
            "no loop run with that run_id",
        )
    })?;
    let snapshot = state.loops.get(&run_id).map(|run| json!({"type": "state", "run": run}));
    let stream = futures_util::stream::unfold(
        (receiver, snapshot, false),
        |(mut receiver, snapshot, mut done)| async move {
            if done {
                return None;
            }
            let event = match snapshot {
                Some(value) => value,
                None => loop {
                    match receiver.recv().await {
                        Ok(value) => break value,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                },
            };
            if event["run"]["state"].as_str().is_some_and(LoopRunState::terminal_str) {
                done = true;
            }
            Some((
                Ok::<_, std::convert::Infallible>(Event::default().data(event.to_string())),
                (receiver, None, done),
            ))
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
}

impl LoopRunState {
    fn terminal_str(s: &str) -> bool {
        matches!(s, "stopped" | "failed" | "done")
    }
}
