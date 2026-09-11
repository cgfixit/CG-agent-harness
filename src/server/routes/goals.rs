//! Explicit goal-to-coding staging, durable association and evidence-based status.
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::schemas::{AgentRunRequest, ValidJson, Validate};
use crate::server::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageRequest {
    branch: String,
}
impl Validate for StageRequest {
    fn validate(&self) -> Vec<String> {
        if self.branch.len() > 88
            || !crate::common::identity::identity().is_ok_and(|id| id.branch_is_valid(&self.branch))
        {
            vec!["branch".into()]
        } else {
            vec![]
        }
    }
}
pub async fn stage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<StageRequest>,
) -> ApiResult<Json<Value>> {
    let session = state
        .store
        .get(&id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    if session.goal.trim().is_empty() {
        return Err(ApiError::bad_request(
            "GOAL_REQUIRED",
            "Set a goal before staging coding work",
        ));
    }
    if let Some(job_id) = session.goal_stage.as_ref().and_then(|s| s["job_id"].as_str()) {
        if state.jobs.get(job_id).is_some_and(|j| j["status"] == "running") {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "GOAL_RUNNING",
                "Inspect or stop the current goal job before staging another",
            ));
        }
    }
    let stage_id = crate::common::random_hex(16);
    let request = json!({"instruction":session.goal,"branch":req.branch,"commit_message":crate::common::clip_chars(&session.goal,72),
        "checks":null,"read_files":[],"plan":null,"max_iterations":1,"goal_stage":{"session_id":id,"stage_id":stage_id}});
    let record = json!({"stage_id":stage_id,"goal":session.goal,"request":request,"job_id":null,"created_at":crate::common::now_ts()});
    state
        .store
        .stage_goal(&id, &session.goal, session.goal_stage.as_ref(), &record)
        .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))?;
    Ok(Json(
        json!({"stage":record,"request":request,"executed":false,"next":"Review request/checks; /agent confirm <reason>. Approval, push and publication remain separate."}),
    ))
}
pub fn validate_binding(state: &AppState, req: &AgentRunRequest) -> ApiResult<()> {
    let Some(binding) = &req.goal_stage else { return Ok(()) };
    if !req.confirm || req.reason.trim().is_empty() {
        return Err(ApiError::bad_request(
            "GOAL_CONFIRM",
            "Goal execution requires explicit confirmation and reason",
        ));
    }
    state
        .store
        .validate_goal_binding(&binding.session_id, &binding.stage_id, &req.instruction, &req.branch)
        .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))
}
pub fn completion_state(record: &Value) -> &'static str {
    match record["status"].as_str() {
        Some("pending_decision") if record["acceptance_digest"].as_str().is_some_and(|s| s.len() == 64) => {
            "awaiting_review"
        }
        Some("approved")
            if record["approved_commit"].as_str().is_some_and(|s| s.len() == 40)
                && record["acceptance_digest"].as_str().is_some_and(|s| s.len() == 64) =>
        {
            "completed_local"
        }
        Some("failed" | "exhausted" | "rejected" | "discarded") => "not_completed",
        _ => "unverified",
    }
}
pub async fn status(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let session = state
        .store
        .get(&id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let Some(stage) = session.goal_stage else {
        return Ok(Json(json!({"status":"not_staged"})));
    };
    let mut out = json!({"stage":stage,"status":"staged","completion":"Model text never establishes completion; local completion requires checked candidate evidence and operator approval.","auto_resume":false});
    if let Some(job_id) = stage["job_id"].as_str() {
        match state.jobs.get(job_id) {
            Some(job) => {
                out["status"] = job["status"].clone();
                if let Some(run_id) = job["result"]["parsed"]["run_id"].as_str() {
                    if crate::server::agent_policy::run_id_re().is_match(run_id) {
                        match super::agent::agent_run_status(State(state.clone()), Path(run_id.to_string())).await {
                            Ok(Json(record)) if record["ok"] == true => {
                                out["status"] = json!(completion_state(&record["parsed"]));
                                out["run"] = record["parsed"].clone();
                            }
                            _ => {
                                out["status"] = json!("evidence_unavailable");
                            }
                        }
                    }
                } else if job["status"] == "finished" {
                    out["status"] = json!("unverified");
                }
                out["job"] = job;
            }
            None => out["status"] = json!("evidence_unavailable"),
        }
    } else if stage["goal"] != session.goal {
        out["status"] = json!("stale");
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_needs_checked_candidate_and_operator_commit_evidence() {
        assert_eq!(
            completion_state(&json!({"status":"finished","reply":"GOAL_DONE"})),
            "unverified"
        );
        assert_eq!(completion_state(&json!({"status":"approved"})), "unverified");
        assert_eq!(
            completion_state(&json!({"status":"pending_decision","acceptance_digest":"a".repeat(64)})),
            "awaiting_review"
        );
        assert_eq!(
            completion_state(
                &json!({"status":"approved","acceptance_digest":"a".repeat(64),"approved_commit":"b".repeat(40)})
            ),
            "completed_local"
        );
    }
}
