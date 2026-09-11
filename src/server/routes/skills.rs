//! Explicit runtime context selection and fixed-check staging; never execute prose.
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::prompts::{load_skill, valid_skill_id};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRequest {
    ids: Vec<String>,
}
impl Validate for SelectionRequest {
    fn validate(&self) -> Vec<String> {
        if self.ids.len() > 4 || self.ids.iter().any(|id| !valid_skill_id(id)) {
            vec!["ids".into()]
        } else {
            vec![]
        }
    }
}
pub fn resolve(state: &AppState, ids: &[String]) -> ApiResult<Vec<(String, String)>> {
    if ids.len() > 4 {
        return Err(ApiError::bad_request(
            "SKILL_LIMIT",
            "Select at most four prompt skills",
        ));
    }
    let per_skill = state
        .cfg
        .u64_or("personality.prompt_skill_max_chars", 6000)
        .clamp(1, 32768) as usize;
    let total = state
        .cfg
        .u64_or("personality.prompt_skills_total_chars", 16000)
        .clamp(1, 65536) as usize;
    let mut out = Vec::new();
    let mut used = 0;
    for id in ids {
        if !valid_skill_id(id) {
            return Err(ApiError::bad_request("SKILL_ID", "Unknown or unsafe runtime skill ID"));
        }
        if out.iter().any(|(key, _)| key == id) {
            continue;
        }
        let load = load_skill(&state.home.skills_dir(), id, per_skill);
        if !load.loaded {
            return Err(ApiError::bad_request(
                "SKILL_UNAVAILABLE",
                "Selected prompt skill is missing, empty, unreadable, or outside the skill directory",
            ));
        }
        used += load.text.chars().count();
        if used > total {
            return Err(ApiError::bad_request(
                "SKILL_BUDGET",
                "Selected skills exceed the aggregate prompt budget",
            ));
        }
        out.push((id.clone(), load.text));
    }
    Ok(out)
}
pub async fn selection(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let session = state
        .store
        .get(&id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let available = resolve(&state, &session.selected_skills);
    Ok(Json(
        json!({"selected":session.selected_skills,"ready":available.is_ok(),
        "unavailable_reason":available.err().map(|e|e.code),"last_result":session.last_prompt_skills,
        "type":"prompt_context","scope":"session chat only; no executable authority"}),
    ))
}
pub async fn select(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<SelectionRequest>,
) -> ApiResult<Json<Value>> {
    let resolved = resolve(&state, &req.ids)?;
    let ids: Vec<String> = resolved.iter().map(|(id, _)| id.clone()).collect();
    state
        .store
        .select_skills(&id, &ids)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    Ok(Json(
        json!({"selected":ids,"effect":"included in subsequent chat turns; no code executed"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    id: String,
}
impl Validate for CheckRequest {
    fn validate(&self) -> Vec<String> {
        if self.id.len() > 100 {
            vec!["id".into()]
        } else {
            vec![]
        }
    }
}
pub async fn check(ValidJson(req): ValidJson<CheckRequest>) -> ApiResult<Json<Value>> {
    let name = req
        .id
        .strip_prefix("check:")
        .ok_or_else(|| ApiError::bad_request("SKILL_TYPE", "Expected a check:<profile> ID"))?;
    let checks = crate::server::agent_policy::resolve_check_profiles(&[name.to_string()])
        .map_err(|_| ApiError::bad_request("SKILL_ID", "Unknown fixed check skill"))?;
    Ok(Json(
        json!({"id":req.id,"type":"fixed_check","profile":name,"adapter":checks[0],
        "executed":false,"requires":"staged coding request, reason, confirmation and fresh execution-time policy",
        "execution":"/api/agent/jobs","results":"/api/agent/jobs/{job_id}","limits":"existing coding-job iteration, time, output and sandbox bounds"}),
    ))
}
