//! Registry/tools/skills views, /web, /memory, /api keys, harness runs.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequest, State};
use axum::http::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::common::errors::HarnessError;
use crate::server::env_keys;
use crate::server::errors::{ApiError, ApiResult};
use crate::server::headers::NO_STORE;
use crate::server::memory_notes::rag_flags;
use crate::server::schemas::*;
use crate::server::state::AppState;
use crate::server::views;

const MAX_RUNS: usize = 50;

pub async fn registry(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(views::full_registry(&state.home))
}

pub async fn tools(State(state): State<Arc<AppState>>) -> Json<Value> {
    let mut report = views::list_wired_tools(&super::registered_paths());
    let config = crate::common::config::AppConfig::load(&state.home.config_path());
    for row in report["tools"].as_array_mut().unwrap() {
        let path = row["path"].as_str().unwrap_or("");
        if path.starts_with("/api/agent/") && path != "/api/agent/checks" {
            let enabled = config.as_ref().is_ok_and(|cfg| cfg.flag_is_true("agentic.enabled"));
            row["enabled"] = json!(enabled);
            if !enabled {
                row["ready"] = json!(false);
                row["unavailable_reason"] = json!("agentic layer disabled or config unreadable");
            }
        }
    }
    Json(report)
}

pub async fn skills(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(views::list_wired_skills(&state.home))
}

// ---------------------------------------------------------------- web

fn web_err(e: &HarnessError) -> ApiError {
    let status = match e.code.as_str() {
        "WEB_DISABLED" | "WEB_ALLOWLIST_EMPTY" | "WEB_BUSY" => StatusCode::CONFLICT,
        "WEB_PERMISSION_DENIED" => StatusCode::FORBIDDEN,
        "WEB_FETCH_FAILED" | "WEB_DNS" | "WEB_CLEAR_FAILED" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::BAD_REQUEST,
    };
    // Code only: never the exception text.
    ApiError::new(status, &e.code, e.code.clone())
}

fn web_enabled(state: &AppState) -> bool {
    state.settings.lock().unwrap_or_else(|p| p.into_inner()).web_enabled
}

pub async fn web_status(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .web
        .status(web_enabled(&state), &owner)
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_toggle(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<ToggleRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let snapshot = {
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        s.web_enabled = req.enabled;
        s.clone()
    };
    snapshot
        .save(&state.home)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    state
        .web
        .status(snapshot.web_enabled, &owner)
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_allow(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebRuleRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .web
        .allow_rule(&req.url, &req.group, &req.seeds, web_enabled(&state))
        .and_then(|_| state.web.status(web_enabled(&state), &owner))
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_deny(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebUrlRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .web
        .deny(&req.url, web_enabled(&state))
        .and_then(|_| state.web.status(web_enabled(&state), &owner))
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_fetch(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebUrlRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let enabled = web_enabled(&state);
    state
        .web
        .fetch(&req.url, enabled, &state.audit, &owner)
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_search(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<WebSearchRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let enabled = web_enabled(&state);
    state
        .web
        .search(&req.query, req.group.as_deref(), enabled, &state.audit, &owner)
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_inject(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .web
        .inject(web_enabled(&state), &owner)
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_forget(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .web
        .forget(web_enabled(&state), &owner)
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_research(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    let ValidJson(body) = ValidJson::<WebSearchRequest>::from_request(req, &()).await?;
    crate::server::web_research::run(state, &owner, &body.query, body.group.as_deref())
        .await
        .map(Json)
        .map_err(|e| web_err(&e))
}

pub async fn web_cancel(State(state): State<Arc<AppState>>, req: Request<Body>) -> ApiResult<Json<Value>> {
    let owner = super::auth::web_owner(&state, &req)?;
    state
        .web
        .research
        .cancel(&owner)
        .map(|cancelled| Json(json!({"cancelled":cancelled})))
        .map_err(|e| web_err(&e))
}

// ---------------------------------------------------------------- memory

fn memory_payload(state: &AppState) -> ApiResult<Value> {
    let enabled = state.settings.lock().unwrap_or_else(|p| p.into_inner()).memory_enabled;
    let mut payload = state
        .notes
        .status(enabled)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    payload["rag"] = rag_flags();
    Ok(payload)
}

pub async fn memory_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    memory_payload(&state).map(Json)
}

pub async fn memory_toggle(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ToggleRequest>,
) -> ApiResult<Json<Value>> {
    let snapshot = {
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        s.memory_enabled = req.enabled;
        s.clone()
    };
    snapshot
        .save(&state.home)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    memory_payload(&state).map(Json)
}

pub async fn memory_add(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<MemoryNoteRequest>,
) -> ApiResult<Json<Value>> {
    let added = state
        .notes
        .add(&req.text)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    let mut payload = memory_payload(&state)?;
    payload["added"] = added;
    Ok(Json(payload))
}

pub async fn memory_forget(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<MemoryForgetRequest>,
) -> ApiResult<Json<Value>> {
    let forgotten = state
        .notes
        .forget(&req.id)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    let mut payload = memory_payload(&state)?;
    merge(&mut payload, forgotten);
    Ok(Json(payload))
}

pub async fn memory_clear(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let cleared = state
        .notes
        .clear()
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    let mut payload = memory_payload(&state)?;
    merge(&mut payload, cleared);
    Ok(Json(payload))
}

fn merge(target: &mut Value, extra: Value) {
    if let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            t.insert(k.clone(), v.clone());
        }
    }
}

// ---------------------------------------------------------------- keys

pub async fn api_keys_status(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let path = state.home.env_path();
    let body = json!({"keys": env_keys::read_status(&path, &state.key_file_sources).map_err(|e| ApiError::from_err(StatusCode::INTERNAL_SERVER_ERROR, &e))?, "env_file": path.display().to_string()});
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    Ok(resp)
}

pub async fn api_keys_set(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ApiKeysRequest>,
) -> ApiResult<Json<Value>> {
    let path = state.home.env_path();
    let written = env_keys::update_keys(&path, &req.keys, &req.clear).map_err(|e| {
        if e.code == env_keys::ENV_KEY_ERROR {
            ApiError::new(StatusCode::BAD_REQUEST, "ENV_KEY_REJECTED", e.message.clone())
        } else {
            tracing::warn!("writing {} failed: {}", path.display(), e.message);
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ENV_KEY_WRITE_FAILED",
                "could not write the key file",
            )
        }
    })?;
    state
        .audit
        .log(json!({"event": "harness_api_keys_updated", "keys": written["written"]}));
    let mut out = written;
    out["keys"] = json!(env_keys::read_status(&path, &state.key_file_sources)
        .map_err(|e| ApiError::from_err(StatusCode::INTERNAL_SERVER_ERROR, &e))?);
    Ok(Json(out))
}

// ---------------------------------------------------------------- harness runs

pub async fn harness_runs(State(state): State<Arc<AppState>>) -> Json<Value> {
    let accepted = state.home.optimizer_runs_dir().join("accepted");
    let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&accepted) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") || !p.is_file() {
                continue;
            }
            let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            files.push((mtime, p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()));
        }
    }
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    // run_id only: the absolute path embeds the operator's home directory.
    let runs: Vec<Value> = files
        .into_iter()
        .take(MAX_RUNS)
        .map(|(_, stem)| json!({"run_id": stem}))
        .collect();
    Json(json!({"count": runs.len(), "runs": runs}))
}
