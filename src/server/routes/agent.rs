//! `/api/github/status` and the seven `/api/agent/*` routes. Port of
//! `harness/agent_routes.py`. Every call crosses the shim; a non-zero CLI exit
//! is a successful shim call (HTTP 200 with `ok=false`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::common::tool_broker::assert_allowed;
use crate::server::agent_policy::{available_profiles, resolve_check_profiles, run_id_re};
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::*;
use crate::server::state::{AppState, AGENT_RUN_TOOL};
use crate::shim::{self, OpsRequest, ShimError, REAL_REPO_RUN_MAX_TIMEOUT_SEC};

const MAX_ECHOED_RUN_ID_LEN: usize = 64;

fn validated_run_id(run_id: &str) -> ApiResult<String> {
    if !run_id_re().is_match(run_id) {
        return Err(
            ApiError::bad_request("INVALID_RUN_ID", "run_id must be 32 lowercase hex characters")
                .details(json!({"run_id": crate::common::clip_chars(run_id, MAX_ECHOED_RUN_ID_LEN)})),
        );
    }
    Ok(run_id.to_string())
}

async fn agentic_call(state: &AppState, req: OpsRequest) -> ApiResult<Json<Value>> {
    let action = req.action.clone();
    let result = shim::run_agentic_op(&state.shim, &req).await.map_err(|e| match e {
        ShimError::Ops(msg) => ApiError::bad_request("AGENTIC_ERROR", state.audit.redact(&msg)),
        ShimError::Timeout { action, timeout_sec } => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "AGENTIC_TIMEOUT",
            format!("agentic {action} exceeded its {timeout_sec}s budget"),
        )
        .details(json!({"action": action, "timeout_sec": timeout_sec})),
        ShimError::Io(msg) => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "SHIM_IO_ERROR",
            format!("{action} shim failed: {}", state.audit.redact(&msg)),
        ),
    })?;
    if result.is_disabled_layer_success() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "AGENTIC_DISABLED",
            "agentic layer is disabled; nothing was executed",
        )
        .details(json!({"action": action})));
    }
    Ok(Json(result.to_json(state.audit.redactors())))
}

pub async fn github_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let result = shim::run_agentic_op(&state.shim, &OpsRequest::new("status"))
        .await
        .map_err(|e| match e {
            ShimError::Timeout { action, timeout_sec } => ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "AGENTIC_TIMEOUT",
                format!("agentic {action} exceeded its {timeout_sec}s budget"),
            ),
            other => ApiError::bad_request("AGENTIC_ERROR", state.audit.redact(&other.to_string())),
        })?;
    Ok(Json(result.to_json(state.audit.redactors())))
}

/// Open: a static allow-list listing, spawns nothing.
pub async fn agent_checks() -> Json<Value> {
    let profiles: Vec<Value> = available_profiles()
        .into_iter()
        .map(|(n, d)| json!({"name": n, "description": d}))
        .collect();
    Json(json!({"profiles": profiles}))
}

/// True when the run's planner and /api/chat target the same backend.
fn agent_run_shares_chat_backend(state: &AppState) -> bool {
    let deep = state
        .cfg
        .str_or("agentic.deepagent_github.base_url", "")
        .trim()
        .to_string();
    if deep.is_empty() {
        return true;
    }
    match (
        canonical_backend_key(&state.backend.base_url),
        canonical_backend_key(&deep),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

fn canonical_backend_key(raw: &str) -> Option<(String, String, Option<u16>, String)> {
    let u = url::Url::parse(raw).ok()?;
    let scheme = u.scheme().to_lowercase();
    let mut host = u.host_str().unwrap_or("").to_lowercase();
    if crate::llm::backend::LOOPBACK_HOSTS.contains(&host.as_str()) {
        host = "127.0.0.1".into();
    }
    let port = u.port().or(match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    });
    Some((scheme, host, port, u.path().trim_end_matches('/').to_string()))
}

pub async fn agent_run(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<AgentRunRequest>,
) -> ApiResult<Json<Value>> {
    let requested: Vec<String> = req
        .checks
        .clone()
        .unwrap_or_else(|| vec![crate::server::agent_policy::DEFAULT_CHECK_PROFILE.to_string()]);
    let checks = resolve_check_profiles(&requested).map_err(|e| {
        ApiError::bad_request("UNKNOWN_CHECK_PROFILE", e.message).details(json!({"requested": requested}))
    })?;
    let planner = state.shim.planner_timeout_sec;
    let estimated = shim::real_repo_run_budget_sec(planner, req.max_iterations, checks.len());
    if estimated > REAL_REPO_RUN_MAX_TIMEOUT_SEC {
        let max_fitting = (1..=MAX_ITERATIONS_CEILING as i64)
            .filter(|n| {
                shim::real_repo_run_budget_sec(planner, Some(*n), checks.len()) <= REAL_REPO_RUN_MAX_TIMEOUT_SEC
            })
            .max()
            .unwrap_or(0);
        let remedy = if max_fitting > 0 {
            format!(
                "at {} check profile(s) the most that fits is max_iterations={max_fitting}",
                checks.len()
            )
        } else {
            format!(
                "no iteration count fits with {} check profile(s) -- select fewer checks",
                checks.len()
            )
        };
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "AGENTIC_BUDGET_EXCEEDED",
            format!(
                "requested shape budgets ~{estimated}s but the synchronous run cap is {REAL_REPO_RUN_MAX_TIMEOUT_SEC}s, so the run would be killed mid-flight; {remedy}"
            ),
        )
        .details(json!({
            "estimated_sec": estimated,
            "cap_sec": REAL_REPO_RUN_MAX_TIMEOUT_SEC,
            "max_iterations": req.max_iterations,
            "check_count": checks.len(),
            "max_iterations_that_fit": max_fitting,
        })));
    }
    assert_allowed(
        AGENT_RUN_TOOL,
        &["real-repo-run".into()],
        &state.agent_run_tool_allowlist(),
        &state.audit,
    )
    .map_err(|e| ApiError::from_err(StatusCode::FORBIDDEN, &e))?;
    let Some(_run_gate) = state.agent_run_gate.claim("agent") else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "AGENT_RUN_BUSY",
            "another agent run is already in progress",
        ));
    };
    let _chat_gate = if agent_run_shares_chat_backend(&state) {
        match state.generation_gate.claim("agent") {
            Some(g) => Some(g),
            None => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "AGENT_RUN_BUSY",
                    "a local model chat turn is already running",
                ))
            }
        }
    } else {
        None
    };
    let ops = OpsRequest {
        action: "real-repo-run".into(),
        instruction: Some(req.instruction.clone()),
        checks: Some(checks),
        branch: Some(req.branch.clone()),
        commit_message: Some(req.commit_message.clone()),
        reason: Some(req.reason.clone()),
        confirm: req.confirm,
        max_iterations: req.max_iterations,
        plan: req.plan.clone(),
        read_files: req.canonical_read_files(),
        pr: req.pr,
        issue: req.issue,
        ..Default::default()
    };
    agentic_call(&state, ops).await
}

pub async fn agent_run_status(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-status");
    ops.run_id = Some(checked);
    agentic_call(&state, ops).await
}

pub async fn agent_run_decision(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ValidJson(req): ValidJson<AgentDecisionRequest>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-decide");
    ops.run_id = Some(checked);
    ops.decision = Some(req.decision.as_str().to_string());
    agentic_call(&state, ops).await
}

pub async fn agent_run_push(State(state): State<Arc<AppState>>, Path(run_id): Path<String>) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-push");
    ops.run_id = Some(checked);
    agentic_call(&state, ops).await
}

pub async fn agent_run_publish(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ValidJson(req): ValidJson<AgentPublishRequest>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-publish");
    ops.run_id = Some(checked);
    ops.reason = Some(req.reason.clone());
    ops.confirm = req.confirm;
    agentic_call(&state, ops).await
}

pub async fn agent_run_discard(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-discard");
    ops.run_id = Some(checked);
    agentic_call(&state, ops).await
}
