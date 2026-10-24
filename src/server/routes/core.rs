//! Status, sessions, soul/model toggles, chat + cancel.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::net::SocketAddr;

use crate::common::tool_broker::assert_allowed;
use crate::llm::openai_chat::ChatMessage;
use crate::server::attachments;
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::guards::retry_after_error;
use crate::server::prompts::{compose_system_prompt, PromptInputs};
use crate::server::schemas::*;
use crate::server::sessions::TokenTally;
use crate::server::slash::{parse_line, SlashParseRequest};
use crate::server::state::{AppState, HARNESS_LOOP_TOOL};

pub async fn slash_parse(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<SlashParseRequest>,
) -> Json<Value> {
    let parsed = parse_line(&req.line).to_json();
    if state.audit.redactors().contains_secret_json(&parsed) || req.line.contains('•') {
        return Json(
            crate::server::slash::suggest_only(
                "Credentials belong in Settings → API Keys; command not dispatched.",
                &[],
            )
            .to_json(),
        );
    }
    Json(parsed)
}

const DEFAULT_TEMPERATURE: f64 = 0.3;

pub async fn reload_config(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(_req): ValidJson<ConfigReloadRequest>,
) -> ApiResult<Json<Value>> {
    if user.is_none_or(|user| user.role != "admin" || user.must_change_password) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AUTH_PERMISSION_DENIED",
            "configuration reload requires an administrator account",
        ));
    }
    crate::server::config_reload::reload(state, "http")
        .await
        .map(Json)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> Json<Value> {
    let owner = super::auth::context_owner(user.clone());
    if state.auth.is_some()
        && user
            .as_ref()
            .is_none_or(|u| u.role == "audit" || u.must_change_password)
    {
        return Json(
            json!({"version":crate::VERSION,"auth_enabled":true,"api_key_optional":true,"status":"login required for operational details"}),
        );
    }
    // portal_auditor learns which model it chats with, not paths, backend URL,
    // persona or memory state.
    if user
        .as_ref()
        .is_some_and(|u| u.role == crate::common::authn::PORTAL_AUDITOR_ROLE)
    {
        let model = state.current_model();
        let sessions = state.store.for_owner(&owner).list();
        return Json(json!({
            "version": crate::VERSION,
            "auth_enabled": state.auth.is_some(),
            "model": model,
            "provider": state.provider_for(&model),
            "chat_mode": "conversation",
            "soul_enabled": false,
            "memory_enabled": false,
            "chat_tools_available": false,
            "sessions": sessions.len(),
        }));
    }
    let sessions = state.store.for_owner(&owner).list();
    let total_tokens: u64 = sessions.iter().filter_map(|s| s["tokens"]["total"].as_u64()).sum();
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    // One settings snapshot, so model and saved-override notice agree.
    // An automatic fallback is an effective backend, not a saved selection.
    let model = if settings.selected_model.trim().is_empty() {
        state.backend.model.clone()
    } else {
        settings.selected_model.clone()
    };
    let cloud = state.cloud_chat.is_cloud_selection(&model);
    let model_status = model_status(&state, &owner, &model);
    let loop_limits = state.runtime_limits().loop_limits;
    Json(json!({
        "version": crate::VERSION,
        "model": model_status["model"],
        "configured_model": state.cfg.str_or("models.local_llm.model", &state.backend.model),
        "selection_overrides_config": !settings.selected_model.trim().is_empty()
            && settings.selected_model != state.cfg.str_or("models.local_llm.model", &state.backend.model).trim(),
        "provider": state.provider_for(&model),
        "model_status": model_status,
        "api_key_optional": state.api_key_optional,
        "base_url": if cloud { Value::Null } else { json!(state.backend.base_url) },
        "soul_enabled": settings.soul_enabled,
        "soul": soul_status(&state, settings.soul_enabled),
        "memory_enabled": settings.memory_enabled,
        "home": state.home.root.display().to_string(),
        "repo_root": Value::Null,
        "chat_mode": "conversation",
        "chat_tools_available": settings.web_enabled && !cloud && !state.chat_tools_unsupported(&model),
        "loop": {
            "max_turns": loop_limits.max_turns,
            "max_total_completion_tokens": loop_limits.max_total_completion_tokens,
            "max_failures": loop_limits.max_failures,
            "cooldown_ms": loop_limits.cooldown_ms,
        },
        "server_loop": true,
        "sessions": sessions.len(),
        "total_tokens": total_tokens,
        "layout": {
            "sessions": state.home.sessions_dir().display().to_string(),
            "skills": state.home.skills_dir().display().to_string(),
            "tools": state.home.tools_dir().display().to_string(),
            "memory": state.home.memory_dir().display().to_string(),
        },
    }))
}

pub async fn spend_summary(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let path = state.spend_file.clone();
    tokio::task::spawn_blocking(move || crate::llm::spend::summarize_file(&path))
        .await
        .map(Json)
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SPEND_UNAVAILABLE",
                "spend summary could not be read",
            )
        })
}

/// Compose existing reads without importing the agentic implementation or
/// reading its records in the server. A disabled/failed child is unavailable,
/// never evidence of zero runs. Listing retains its normal reconciliation.
pub async fn analytics_summary(State(state): State<Arc<AppState>>, user: Caller) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    let snapshot = state.clone();
    let local = tokio::task::spawn_blocking(move || {
        let sessions = snapshot.store.for_owner(&owner).list();
        let mut days = std::collections::BTreeMap::<String, u64>::new();
        let mut unknown_dates = 0_u64;
        let mut total_tokens = 0_u64;
        for session in &sessions {
            total_tokens = total_tokens.saturating_add(session["tokens"]["total"].as_u64().unwrap_or(0));
            let date = session["created_ts"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0)
                .and_then(|v| time::OffsetDateTime::from_unix_timestamp(v.floor() as i64).ok());
            if let Some(date) = date {
                *days.entry(date.date().to_string()).or_default() += 1;
            } else {
                unknown_dates += 1;
            }
        }
        json!({
            "spend":crate::llm::spend::summarize_file(&snapshot.spend_file),
            "sessions":{"sessions":sessions},
            "session_days":days.into_iter().map(|(day,count)| json!({"day":day,"count":count})).collect::<Vec<_>>(),
            "sessions_without_created_date":unknown_dates,
            "status":{"model":snapshot.current_model(),"provider":snapshot.current_provider(),"total_tokens":total_tokens},
        })
    });
    let (local, runs) = tokio::join!(local, super::agent::agent_runs(State(state)));
    let mut data = local.map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ANALYTICS_UNAVAILABLE",
            "analytics snapshot could not be read",
        )
    })?;
    data["code"] = match runs {
        Ok(Json(envelope)) if envelope["ok"] == true && envelope["parsed"]["runs"].is_array() => {
            let mut code = envelope["parsed"].clone();
            let mut outcomes = std::collections::BTreeMap::<String, u64>::new();
            for run in code["runs"].as_array().expect("array checked above") {
                *outcomes
                    .entry(run["status"].as_str().unwrap_or("unknown").to_string())
                    .or_default() += 1;
            }
            code["outcomes"] = json!(outcomes);
            code
        }
        Ok(_) => json!({"error":{"code":"AGENTIC_UNAVAILABLE","message":"Retained runs could not be read."}}),
        Err(error) => json!({"error":{"code":error.code,"message":error.message}}),
    };
    Ok(Json(data))
}

pub async fn spend_predict(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<SpendPredictRequest>,
) -> ApiResult<Json<crate::llm::cloud_chat::SpendPrediction>> {
    let owner = super::auth::context_owner(user.clone());
    let model = req.model.unwrap_or_else(|| state.current_model());
    if !state.cloud_chat.is_cloud_selection(&model) {
        return Err(ApiError::bad_request(
            "SPEND_LOCAL_UNPRICED",
            "local inference is unpriced; select a cloud model to estimate its cost",
        ));
    }
    require_cloud_consent(&state, &owner, &model)?;
    let _gate = state.generation_gate.claim("chat").ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "CHAT_BUSY",
            "a model operation is already running",
        )
    })?;
    let lease = state
        .chat_owner
        .start(&owner)
        .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))?;
    let prediction = tokio::select! {
        biased;
        _ = lease.token.cancelled() => return Err(ApiError::new(StatusCode::BAD_GATEWAY,"WEB_CANCELLED","prediction cancelled")),
        result = state.cloud_chat.predict(&model, &req.message) => result.map_err(|e| ApiError::from_err(llm_status(&e), &e))?,
    };
    state.audit.log(
        json!({"event":"cloud_chat_prediction", "owner":super::auth::context_owner(user),
        "provider":prediction.provider, "model":prediction.model, "estimate_source":prediction.estimate_source,
        "budget_exceeded":prediction.budget_exceeded}),
    );
    Ok(Json(prediction))
}

pub async fn upload_attachments(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    req: axum::extract::Request,
) -> ApiResult<Json<Value>> {
    let account = user.as_ref().map(|u| u.0.clone());
    let owner = super::auth::context_owner(user);
    // Take the buffering permit before a single body byte is read: with the
    // route rate limit alone, one client could hold dozens of maximum-size
    // bodies in memory while the quota check waits behind the store lock.
    let Ok(permit) = state.upload_permits.clone().try_acquire_owned() else {
        return Err(attachments::upload_error(&crate::common::errors::HarnessError::new(
            "ATTACHMENT_BUSY",
            "too many attachment uploads are in flight; retry shortly",
        )));
    };
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let declared = attachments::parse_content_length(
        req.headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
    )
    .map_err(|e| attachments::upload_error(&e))?;
    if let Some(n) = declared {
        if n > attachments::MAX_REQUEST_BYTES {
            return Err(attachments::upload_error(&crate::common::errors::HarnessError::new(
                "ATTACHMENT_TOO_LARGE",
                "Content-Length exceeds the attachment request limit",
            )));
        }
    }
    let limit = attachments::MAX_REQUEST_BYTES as usize;
    // The body read is bounded so a client that trickles or never finishes
    // its upload cannot hold the permit indefinitely.
    let bytes = tokio::time::timeout(
        state.upload_body_timeout,
        axum::body::to_bytes(req.into_body(), limit.saturating_add(1)),
    )
    .await
    .map_err(|_| {
        attachments::upload_error(&crate::common::errors::HarnessError::new(
            "ATTACHMENT_TIMEOUT",
            "request body was not received within the upload deadline",
        ))
    })?
    .map_err(|_| {
        attachments::upload_error(&crate::common::errors::HarnessError::new(
            "ATTACHMENT_TOO_LARGE",
            "request body exceeds the attachment request limit",
        ))
    })?;
    attachments::check_content_length(declared, bytes.len() as u64).map_err(|e| attachments::upload_error(&e))?;
    let boundary = attachments::multipart_boundary(&content_type).map_err(|e| attachments::upload_error(&e))?;
    // Parsing, hashing, blob writes and fsync are synchronous and run under
    // the store mutex, so they go to the blocking pool rather than a Tokio
    // worker. The permit moves with the work: a client that disconnects
    // does not release it before the store has finished.
    let st = state.clone();
    let task_owner = owner.clone();
    let blobs = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let files = attachments::parse_multipart(&bytes, &boundary)?;
        // Re-check the account under the store lock: a deletion that
        // completed while this body was in flight must not gain an orphaned
        // blob.
        let owner_live = || match (st.auth.as_ref(), account.as_ref()) {
            (Some(manager), Some(account)) => manager
                .get_user(&account.username)
                .is_some_and(|current| current.user_id == account.user_id && !current.disabled),
            _ => true,
        };
        st.attachments.store_guarded(&task_owner, &files, &owner_live)
    })
    .await
    .map_err(|_| {
        attachments::upload_error(&crate::common::errors::HarnessError::new(
            "IO_ERROR",
            "attachment storage task failed",
        ))
    })?
    .map_err(|e| attachments::upload_error(&e))?;
    state.audit.log(attachments::audit_record(&owner, &blobs));
    Ok(Json(json!({
        "attachments": blobs.iter().map(|b| json!({
            "id": b.id,
            "sha256": b.sha256,
            "magic_mime": b.magic_mime,
            "byte_len": b.byte_len,
        })).collect::<Vec<_>>(),
        "count": blobs.len(),
        "bytes": blobs.iter().map(|b| b.byte_len).sum::<u64>(),
    })))
}

pub async fn list_sessions(State(state): State<Arc<AppState>>, user: Caller) -> Json<Value> {
    let owner = super::auth::context_owner(user.clone());
    Json(json!({"sessions": state.store.for_owner(&owner).list()}))
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    user: Caller,
    ValidJson(req): ValidJson<SessionCreateRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let owner = super::auth::context_owner(user.clone());
    let session = state
        .store
        .for_owner(&owner)
        .create(&state.current_model(), &req.title)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok((StatusCode::CREATED, Json(session.summary())))
}

pub async fn clear_sessions(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<SessionClearRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    crate::server::structured_memory_suggest::clear_chat_queue(&state, &owner);
    state.diag.forget(&owner);
    let _ = state.chat_owner.cancel_with(&owner, || state.abort_chat());
    let deleted = state
        .store
        .for_owner(&owner)
        .clear()
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    let mut derived_deleted = 0usize;
    let mut derived_retained = 0usize;
    if req.delete_derived_episodes {
        if let Some(store) = state.structured_memory.as_ref() {
            let (deleted_eps, retained) = store
                .delete_unreferenced_episodes(&owner, &req.reason)
                .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
            derived_deleted = deleted_eps;
            derived_retained = retained;
        }
    } else if let Some(store) = state.structured_memory.as_ref() {
        derived_retained = store.episode_count(&owner).unwrap_or(0);
    }
    // Session clear is the operator's recovery path for a full attachment
    // quota, so an incomplete blob cleanup is reported rather than hidden.
    let _ = state.notes_corpus.unlink_owner(&owner);
    let attachment_cleanup = match state.attachments.unlink_owner(&owner) {
        Ok(removed) => json!({"removed": removed}),
        Err(e) => {
            state.audit.log(json!({
                "event": "chat_attachments_cleanup_incomplete",
                "owner": owner,
                "code": e.code,
                "message": e.message,
                "details": e.details,
            }));
            json!({
                "removed": e.details.get("removed").cloned().unwrap_or(json!(0)),
                "retained": e.details.get("retained").cloned().unwrap_or(json!(0)),
                "error": e.code,
            })
        }
    };
    Ok(Json(json!({
        "deleted_sessions": deleted,
        "attachment_cleanup": attachment_cleanup,
        "derived_episodes_deleted": derived_deleted,
        "derived_episodes_retained": derived_retained,
        "delete_derived_episodes": req.delete_derived_episodes,
        "note": if req.delete_derived_episodes {
            "Session history was cleared. This owner's unreferenced derived episodes were deleted. Facts, proposals, and episodes referenced by pending proposals were kept."
        } else {
            "Session history was cleared. Derived structured-memory episodes remain unless delete_derived_episodes is confirmed. Facts and proposals are never cleared here."
        }
    })))
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    let session = state
        .store
        .for_owner(&owner)
        .get(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let messages: Vec<Value> = session
        .messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.text, "ts": m.ts}))
        .collect();
    let mut out = session.summary();
    out["messages"] = json!(messages);
    out["prompt_history"] = json!(session.prompt_history);
    out["goal"] = json!(session.goal);
    out["goal_web_mode"] = json!(session.goal_web_mode);
    out["style"] = json!(session.style);
    out["live_attachment_pins"] = json!(session
        .pinned_ids_for(&owner)
        .iter()
        .filter(|id| state.attachments.owned_blob(&owner, id).is_some())
        .count());
    Ok(Json(out))
}

pub async fn clear_attachment_pins(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .store
        .for_owner(&owner)
        .get(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    // A completing turn must not re-pin attachments after this response.
    let _gate = state.generation_gate.claim("attachment_unpin").ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "CHAT_BUSY",
            "stop the active turn before /session unpin",
        )
    })?;
    let removed = state
        .store
        .for_owner(&owner)
        .clear_attachment_pins(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    state
        .audit
        .log(json!({"event": "chat_attachment_pins_cleared", "session_id": session_id, "removed": removed}));
    Ok(Json(json!({"removed": removed, "blobs_retained": true})))
}

/// Saves one submitted composer line for ArrowUp recall in this session,
/// whatever happened to it. `/api` values and URL userinfo are dropped here as
/// well as in the console (`sessions::redacted_prompt`).
pub async fn record_prompt(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<PromptHistoryRequest>,
) -> ApiResult<Json<Value>> {
    if state
        .audit
        .redactors()
        .contains_secret(&crate::server::sessions::redacted_prompt(&req.text))
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROMPT_HISTORY_REJECTED",
            "credentials cannot be saved in prompt recall",
        ));
    }
    let owner = super::auth::context_owner(user);
    state
        .store
        .for_owner(&owner)
        .record_prompt(&session_id, req.text.trim())
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    Ok(Json(json!({"recorded": true})))
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<RenameRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    let session = state
        .store
        .for_owner(&owner)
        .rename(&session_id, Some(&req.title), None)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    Ok(Json(session.summary()))
}

pub async fn session_goal(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<GoalRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    let web_mode = req.web_mode.unwrap_or(state.runtime_limits().web.goal_web_default);
    let session = state
        .store
        .for_owner(&owner)
        .set_goal(&session_id, &req.goal, web_mode)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let mut out = session.summary();
    out["goal"] = json!(session.goal);
    out["goal_web_mode"] = json!(session.goal_web_mode);
    Ok(Json(out))
}

fn soul_status(state: &AppState, enabled: bool) -> Value {
    json!(crate::server::prompts::load_text(
        &state.home.root,
        std::path::Path::new("soul.md"),
        enabled,
        crate::server::prompts::effective_soul_max_chars(state.cfg.u64_or("personality.soul_max_chars", 8000))
    ))
}

pub async fn soul_state(State(state): State<Arc<AppState>>) -> Json<Value> {
    let enabled = state.settings.lock().unwrap_or_else(|p| p.into_inner()).soul_enabled;
    Json(soul_status(&state, enabled))
}

pub async fn soul_toggle(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ToggleRequest>,
) -> ApiResult<Json<Value>> {
    let snapshot = {
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        s.soul_enabled = req.enabled;
        s.clone()
    };
    snapshot
        .save(&state.home)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(soul_status(&state, snapshot.soul_enabled)))
}

pub async fn model_select(
    State(state): State<Arc<AppState>>,
    user: Caller,
    ValidJson(req): ValidJson<ModelSelectRequest>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let selected = req.model.trim();
    if state.audit.redactors().contains_secret(selected) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "MODEL_SELECTION_REJECTED",
            "credentials are not model names; use Settings → API Keys",
        ));
    }
    let cloud = state.cloud_chat.selection_status(selected);
    let mut from_provider = state.current_provider();
    let model = cloud
        .as_ref()
        .and_then(|c| c["model"].as_str())
        .unwrap_or(if selected.eq_ignore_ascii_case("local") {
            &state.backend.model
        } else {
            selected
        })
        .to_string();
    let to_provider = cloud
        .as_ref()
        .and_then(|c| c["provider"].as_str())
        .unwrap_or(&state.backend.provider)
        .to_string();
    let result = (|| -> ApiResult<()> {
        if cloud.is_some() {
            check_cloud_ready(&state, selected)?;
            if !req.confirm {
                require_cloud_consent(&state, &owner, selected)?;
            }
        }
        state
            .chat_owner
            .cancel_with(&owner, || state.abort_chat())
            .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))?;
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        from_provider = state.provider_for(&s.selected_model);
        let mut snapshot = s.clone();
        snapshot.selected_model = if selected.eq_ignore_ascii_case("local") {
            String::new()
        } else if cloud.is_some() {
            // Persist provider identity, so changing its model ID cannot turn a saved cloud selection into a local tag.
            to_provider.clone()
        } else {
            model.clone()
        };
        snapshot.selected_model_config = Some(
            state
                .cfg
                .str_or("models.local_llm.model", &state.backend.model)
                .trim()
                .to_string(),
        );
        snapshot
            .save(&state.home)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
        *s = snapshot;
        let mut consents = state.cloud_consents.lock().unwrap_or_else(|p| p.into_inner());
        if cloud.is_some() {
            consents.insert((owner.clone(), to_provider.clone()));
        } else {
            consents.retain(|(account, _)| account != &owner);
        }
        state.set_tuning(None);
        Ok(())
    })();
    state.audit.log(
        json!({"event":"model_switched","from_provider":from_provider,"to_provider":to_provider,
        "model":model,"cloud":cloud.is_some(),"tuning_profile":cloud.as_ref().map(|c| &c["tuning_profile"]),
        "result":if result.is_ok() {"selected"} else {"refused"}}),
    );
    result?;
    let tuning = if cloud.is_some() {
        "profile"
    } else if state.cfg.flag_is_true("models.local_llm.auto_tune") {
        // tune() also clears stale limits for a cloud selection, but measures
        // (and loads) only a local Ollama model.
        tokio::spawn(crate::server::model_limits::tune(state.clone(), model.clone()));
        if state.cloud_chat.is_cloud_selection(&model) || state.backend.provider != "ollama" {
            "skipped"
        } else {
            "pending"
        }
    } else {
        "off"
    };
    Ok(Json(json!({"model": model, "provider": to_provider, "tuning": tuning,
            "egress_notice":cloud.as_ref().map(|c| format!("Only your newly typed message leaves this machine for {}.",c["egress_host"].as_str().unwrap_or(""))),
            "model_status":model_status(&state, &owner, if cloud.is_some() {&to_provider} else {&model})})))
}

fn cloud_presence(state: &AppState, cloud: &Value) -> Value {
    crate::server::env_keys::active_presence(
        cloud["key_name"].as_str().unwrap_or(""),
        &state.key_file_sources,
        !state.cfg.flag_is_true("security.allow_plaintext_key_file"),
    )
}

fn check_cloud_ready(state: &AppState, model: &str) -> ApiResult<()> {
    state
        .cloud_chat
        .check_selection(model)
        .map_err(|e| ApiError::from_err(StatusCode::CONFLICT, &e))?;
    let cloud = state
        .cloud_chat
        .selection_status(model)
        .expect("configured cloud selection");
    if cloud_presence(state, &cloud)["active_configured"] != true {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "MODEL_PROVIDER_NOT_READY",
            format!(
                "{} is not active; save it in Settings → API Keys and restart; selection unchanged",
                cloud["key_name"].as_str().unwrap_or("provider key")
            ),
        ));
    }
    Ok(())
}

fn require_cloud_consent(state: &AppState, owner: &str, model: &str) -> ApiResult<()> {
    check_cloud_ready(state, model)?;
    let cloud = state
        .cloud_chat
        .selection_status(model)
        .expect("configured cloud selection");
    let provider = cloud["provider"].as_str().unwrap_or("");
    if !state
        .cloud_consents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&(owner.to_string(), provider.to_string()))
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "CLOUD_CHAT_CONFIRM_REQUIRED",
            format!(
                "Only your newly typed message leaves this machine for {}. Confirm /model use {provider} first.",
                cloud["egress_host"].as_str().unwrap_or("")
            ),
        )
        .details(json!({"provider":provider,"egress_host":cloud["egress_host"]})));
    }
    Ok(())
}

fn model_status(state: &AppState, owner: &str, model: &str) -> Value {
    if let Some(mut cloud) = state.cloud_chat.selection_status(model) {
        cloud["key"] = cloud_presence(state, &cloud);
        cloud["key_present"] = json!(cloud["key"]["active_configured"] == true && cloud["key_present"] == true);
        cloud["consent_required"] = json!(!state
            .cloud_consents
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&(owner.to_string(), state.provider_for(model))));
        return cloud;
    }
    let tuning = state.tuning_for(model);
    let source = |key: &str, tuned: bool| {
        if tuned {
            "profile"
        } else if state.cfg.get(key).is_some() {
            "config"
        } else {
            "default"
        }
    };
    json!({"model":model,"provider":state.provider_for(model),"cloud":false,"key_present":null,
        "tuning_profile":{"context_window":tuning.as_ref().map(|t| t.window),"max_output_tokens":state.chat_max_tokens(model),
            "temperature":state.cfg.f64_or("models.local_llm.temperature",DEFAULT_TEMPERATURE),"timeout_sec":state.chat_timeout_sec(model),
            "count_timeout_sec":null,"tool_support":!state.chat_tools_unsupported(model),"rpm":null,
            "sources":{"model":"config","context_window":if tuning.is_some() {"profile"} else {"default"},
                "max_output_tokens":source("models.local_llm.max_tokens", tuning.as_ref().is_some_and(|t| t.max_tokens <= state.cfg.u64_or("models.local_llm.max_tokens", super::super::compaction::DEFAULT_REPLY_TOKENS))),
                "temperature":source("models.local_llm.temperature",false),
                "timeout_sec":source("models.local_llm.timeout_sec",tuning.as_ref().is_some_and(|t| t.timeout_sec.is_some())),"count_timeout_sec":"default","tool_support":if tuning.is_some() {"profile"} else {"default"},"rpm":"default"}}})
}

/// One turn's compaction: what was kept and summarized, for persistence,
/// audit and the response.
struct Compaction {
    keep: usize,
    before: usize,
    after: usize,
    summarized: usize,
    projected: u64,
    compacted_projected: u64,
    threshold: u64,
    summary: String,
    omitted_turns: usize,
    clipped_turns: usize,
    repairs: crate::server::compaction::RepairCounts,
}

/// Prior user/assistant turns for the next prompt, as the model receives
/// them: a stored compaction summary is rendered as a labeled harness note
/// ([`crate::server::compaction::prompt_messages`]). Compaction owns overflow.
/// The persist cap is `MAX_MESSAGES`. There is no 20-turn or 8000-char clip.
pub fn prompt_history(messages: &[crate::server::sessions::Message]) -> Vec<ChatMessage> {
    crate::server::compaction::prompt_messages(messages)
}

fn loop_error(code: &str, message: &str) -> ApiError {
    ApiError::bad_request(code, message)
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    headers: HeaderMap,
    ValidJson(req): ValidJson<ChatRequest>,
) -> Response {
    let streaming = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|part| part.trim() == "text/event-stream"));
    if !streaming {
        return chat_inner(state, peer, user, req, None, None).await.into_response();
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Value>(16);
    let task = tokio::spawn(async move {
        let result = chat_inner(state, peer, user, req, Some(&sender), None).await;
        let event = match result {
            Ok(Json(data)) => json!({"type":"done","data":data}),
            Err(error) => {
                json!({"type":"error","status":error.status.as_u16(),"error":error.body(),"headers":error.headers})
            }
        };
        let _ = sender.send(event).await;
    });
    super::sse_from_channel(receiver, task)
}

/// `portal_auditor` chats with the selected local model only: no cloud, loop,
/// model override, attachments or memory selection. Context the account owned
/// under an earlier role is withheld in `chat_inner`, not merely unreachable.
fn refuse_portal_auditor_extras(req: &ChatRequest, current_model: &str, cloud_selected: bool) -> ApiResult<()> {
    let model_override = req
        .model
        .as_deref()
        .is_some_and(|m| !m.trim().is_empty() && m != current_model);
    if cloud_selected
        || req.loop_turn
        || model_override
        || !req.attachment_ids.is_empty()
        || req.retrieve
        || req.retrieve_query.is_some()
        || req.selected_facts.is_some()
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "AUTH_PERMISSION_DENIED",
            "portal_auditor chat is local-only, without loops, model selection, attachments or memory",
        ));
    }
    Ok(())
}

/// How long a chat turn waits for a preempted memory suggestion to release the
/// local model. Preemption aborts the suggestion's HTTP call, so the release
/// normally takes milliseconds; the bound only covers a stalled store write.
const SUGGESTION_YIELD_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What a background generation-gate owner is doing, in the operator's words.
fn busy_label(owner: &str) -> &str {
    match owner {
        "consolidation" => "memory consolidation",
        "web_research" => "web research",
        "agent" => "a coding run",
        crate::server::structured_memory_suggest::GATE_OWNER => "a memory suggestion",
        crate::server::model_limits::TUNE_OWNER => "a model speed measurement",
        other => other,
    }
}

pub(crate) async fn chat_inner(
    state: Arc<AppState>,
    peer: SocketAddr,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    req: ChatRequest,
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
    expected_loop_goal: Option<&str>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user.clone());
    if req.loop_turn && req.session_id.as_deref().unwrap_or("").is_empty() {
        return Err(loop_error(
            "LOOP_REQUIRES_SESSION",
            "loop turns require an existing session",
        ));
    }
    let session = match req.session_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => state
            .store
            .for_owner(&owner)
            .get(id)
            .map_err(|e| ApiError::from_err(session_status(&e), &e))?,
        None => state
            .store
            .for_owner(&owner)
            .create(&state.current_model(), "")
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?,
    };
    // Validate the same snapshot used below for system context, not a separate load.
    if expected_loop_goal.is_some_and(|goal| goal != session.goal) {
        return Err(loop_error(
            "LOOP_GOAL_CHANGED",
            "session goal changed; start a new loop for the current goal",
        ));
    }
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| state.current_model());
    let cloud_selected = state.cloud_chat.is_cloud_selection(&model);
    let portal_auditor = user
        .as_ref()
        .is_some_and(|u| u.role == crate::common::authn::PORTAL_AUDITOR_ROLE);
    if portal_auditor {
        refuse_portal_auditor_extras(&req, &state.current_model(), cloud_selected)?;
    }
    if cloud_selected && req.loop_turn {
        return Err(loop_error(
            "CLOUD_CHAT_LOOP",
            "cloud chat is unavailable for /loop turns",
        ));
    }
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    // Web tools need web on and a model that can take them (auto_tune may have
    // found it declares none, or that no web-chat prompt fits its window).
    let chat_web_mode = if portal_auditor {
        crate::server::sessions::GoalWebMode::Off
    } else {
        session.chat_web_mode(settings.web_enabled && !state.chat_tools_unsupported(&model))
    };
    let chat_tools = chat_web_mode.enabled();
    let selected_skills = if cloud_selected || portal_auditor {
        Vec::new()
    } else {
        super::skills::resolve(&state, &session.selected_skills)?
    };

    let sized_tuned = state.tuning_for(&model).is_some();
    // A tuned model's caps follow the window Ollama reports right now, read
    // fresh before they are computed (a no-op for an untuned model).
    if !cloud_selected {
        state.verify_window(&model).await;
    }
    let live = state.runtime_limits();
    let mut web = state.web.clone();
    web.limits = state.model_web_limits(&model, live.web.clone());
    let mut loop_claimed = false;
    if req.loop_turn {
        if session.goal.trim().is_empty() {
            return Err(loop_error("LOOP_REQUIRES_GOAL", "set a /goal before /loop"));
        }
        let ip = peer.ip().to_string();
        if !state
            .loop_rate_limiter
            .allow_with_limits(&ip, live.loop_rate.max_requests, live.loop_rate.window_seconds)
        {
            return Err(retry_after_error(
                &state.loop_rate_limiter,
                &live.loop_rate,
                &ip,
                "LOOP_RATE_LIMIT",
                "loop rate limit exceeded",
            ));
        }
        if !state.claim_loop_inflight(&session.session_id) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "LOOP_IN_FLIGHT",
                "a loop turn is already in flight for this session",
            )
            .details(json!({"session_id": session.session_id})));
        }
        loop_claimed = true;
        if let Err(e) = assert_allowed(
            HARNESS_LOOP_TOOL,
            std::slice::from_ref(&session.session_id),
            &state.loop_tool_allowlist(),
            &state.audit,
        ) {
            state.release_loop_inflight(&session.session_id);
            return Err(ApiError::from_err(StatusCode::FORBIDDEN, &e));
        }
    }
    let release = LoopRelease {
        state: state.clone(),
        session_id: session.session_id.clone(),
        active: loop_claimed,
    };

    let busy = |busy_owner: &str| {
        let mut details =
            json!({"session_id": session.session_id, "timeout_sec": state.chat_timeout_sec(&model) as u64});
        let message = if busy_owner == "chat" {
            details["cancel"] = json!("/api/chat/cancel");
            "a local model turn is already running".to_string()
        } else {
            details["busy"] = json!(busy_owner);
            format!(
                "the local model is busy with {}; try again when it finishes",
                busy_label(busy_owner)
            )
        };
        state.audit.log(json!({
            "event": "chat_busy",
            "session_id": session.session_id,
            "owner": busy_owner,
        }));
        ApiError::new(StatusCode::CONFLICT, "CHAT_BUSY", message).details(details)
    };
    let cancelled = || ApiError::new(StatusCode::BAD_GATEWAY, "WEB_CANCELLED", "chat cancelled");
    // Register the turn before waiting for the model, so /api/chat/cancel reaches
    // it while a preempted suggestion is still letting go. The lease is held by
    // one turn at a time, like the generation gate.
    let Ok(chat_owner) = state.chat_owner.start(&owner) else {
        drop(release);
        return Err(busy("chat"));
    };
    // A best-effort memory suggestion yields to the operator's turn: it is
    // stopped and requeued, and this turn takes the model once it lets go.
    let claimed = tokio::select! {
        biased;
        _ = chat_owner.token.cancelled() => return Err(cancelled()),
        claimed = state.generation_gate.claim_preempting(
            "chat",
            crate::server::structured_memory_suggest::GATE_OWNER,
            SUGGESTION_YIELD_WAIT,
            || {
                crate::server::structured_memory_suggest::preempt(&state);
            },
        ) => claimed,
    };
    let _gate = match claimed {
        Ok(gate) => gate,
        Err(busy_owner) => {
            drop(release);
            return Err(busy(&busy_owner));
        }
    };
    // With the gate held, wake a tuned model whose keep_alive expired, so this
    // turn is sized for the window it will serve, not the 32768 defaults. A
    // cold load can take the whole warmup timeout, so /api/chat/cancel ends it.
    let loaded = tokio::select! {
        biased;
        _ = chat_owner.token.cancelled() => return Err(cancelled()),
        loaded = async {
            if cloud_selected {
                Ok(false)
            } else {
                state.load_if_absent(&model, 0).await
            }
        } => loaded,
    };
    let loaded = loaded.map_err(|e| ApiError::from_err(llm_status(&e), &e))?;
    if loaded {
        web.limits = state.model_web_limits(&model, live.web.clone());
    }

    tokio::select! {
        biased;
        _ = chat_owner.token.cancelled() => Err(cancelled()),
        result = async {
    let surface = if cloud_selected {
        crate::server::retrieval::RetrievalSurface::CloudChat
    } else if req.loop_turn {
        crate::server::retrieval::RetrievalSurface::Loop
    } else {
        crate::server::retrieval::RetrievalSurface::LocalChat
    };
    let live_pins: Vec<String> = if portal_auditor {
        Vec::new()
    } else {
        session
            .pinned_ids_for(&owner)
            .into_iter()
            .filter(|id| state.attachments.owned_blob(&owner, id).is_some())
            .collect()
    };
    crate::server::retrieval::refuse_forbidden_attachments(surface, &req.attachment_ids, &live_pins)
        .map_err(|e| attachments::upload_error(&e))?;
    // A staged /diag excerpt: consumed by this local turn, withheld from /loop,
    // refused on cloud chat (never silently dropped).
    let diag_surface = match surface {
        crate::server::retrieval::RetrievalSurface::CloudChat => crate::server::diag_reader::Surface::CloudChat,
        crate::server::retrieval::RetrievalSurface::Loop => crate::server::diag_reader::Surface::Loop,
        _ => crate::server::diag_reader::Surface::LocalChat,
    };
    let diag = state
        .diag
        .for_turn(&owner, &session.session_id, diag_surface)
        .map_err(|e| super::diag::diag_error(&e))?;
    let web_context = if cloud_selected || portal_auditor {
        String::new()
    } else {
        web.context_text(session.chat_web_mode(settings.web_enabled).enabled(), &owner)
    };
    let selections = if portal_auditor {
        Vec::new()
    } else {
        req.selected_facts
            .as_ref()
            .map(|items| super::structured_memory::selections_from_items(items))
            .unwrap_or_else(|| session.selected_facts.clone())
    };
    let ids = crate::server::retrieval::attachment_ids_for_prompt(&live_pins, &req.attachment_ids);
    // Clients may abbreviate loop reminders; retrieve against the full stored goal.
    let attachment_query = if req.loop_turn {
        format!("{}\n{}", session.goal, req.message)
    } else {
        req.message.clone()
    };
    // portal_auditor gets no attachment or notes-corpus bytes, including any it
    // owned under an earlier role.
    let attachment_fence = if portal_auditor {
        String::new()
    } else {
        crate::server::retrieval::assemble_local_untrusted(
            surface,
            &state.attachments,
            &state.notes_corpus,
            &owner,
            &ids,
            Some(attachment_query.as_str()),
        )
        .map_err(|e| attachments::upload_error(&e))?
    };
    // The bundled manual and live state reach local chat turns only: never
    // cloud chat (which sends only the new message) and never /loop.
    // portal_auditor gets the manual but not live state (home path, soul state).
    let local_turn = !cloud_selected && !req.loop_turn;
    let live_block = (local_turn && !portal_auditor)
        .then(|| {
            state.live_state_block(
                &model,
                settings.web_enabled,
                chat_web_mode,
                session.style.as_deref(),
                settings.soul_enabled,
                Some(&session.session_id),
            )
        })
        .flatten();
    let manual = local_turn.then(|| state.self_knowledge.select(&req.message)).flatten();
    if let (Some(selection), Some(index)) = (&manual, state.self_knowledge.manual.as_ref()) {
        state.audit.log(json!({
            "event": "self_manual_injected",
            "chunks": selection.ids,
            "chars": selection.chars,
            "manual_sha256": index.sha256,
        }));
    }
    // Fact recall and retrieval read SQLite: run them off the async worker.
    let (pinned, facts, memory_budget, recalled, retrieval_error) = {
        let (state, owner, selections) = (state.clone(), owner.clone(), selections.clone());
        // A loop turn's message is framing ("Loop turn 2 of 5. Session goal: …"),
        // which would spend the retrieval token cap on "loop" and "turn".
        let message = if req.loop_turn && !session.goal.trim().is_empty() {
            session.goal.clone()
        } else {
            req.message.clone()
        };
        let (force, query) = (req.retrieve, req.retrieve_query.clone());
        let memory_enabled = settings.memory_enabled && !portal_auditor;
        super::structured_memory::off_worker(move || {
            super::structured_memory::prompt_memory(
                &state,
                &owner,
                memory_enabled,
                // Withheld context, as for cloud: portal_auditor reads no memory.
                cloud_selected || portal_auditor,
                &selections,
                crate::server::structured_memory::RetrievalIntent {
                    force,
                    query: query.as_deref(),
                    message: Some(message.as_str()),
                },
            )
        })
        .await
    };
    let system_prompt = if cloud_selected {
        String::new()
    } else {
        compose_system_prompt(&PromptInputs {
            selected_skills: &selected_skills,
            soul_enabled: settings.soul_enabled && !portal_auditor,
            soul_override: None,
            soul_path: &state.home.soul_path(),
            soul_max_chars: crate::server::prompts::effective_soul_max_chars(
                state.cfg.u64_or("personality.soul_max_chars", 8000),
            ),
            style_name: session.style.as_deref(),
            goal: Some(&session.goal),
            web_context: Some(&web_context),
            memory_context: Some(&pinned),
            selected_facts_context: Some(&facts),
            memory_budget,
            memory_enabled: settings.memory_enabled && !portal_auditor,
            auto_suggest_chat: !portal_auditor && crate::server::structured_memory_suggest::available(&state, crate::server::structured_memory_suggest::Source::Chat),
            auto_suggest_coding: !portal_auditor && crate::server::structured_memory_suggest::available(&state, crate::server::structured_memory_suggest::Source::Coding),
            chat_web_mode,
            web_enabled: settings.web_enabled,
            attachment_fence: Some(attachment_fence.as_str()),
            live_state: live_block.as_deref(),
            self_manual: manual.as_ref().map(|selection| selection.block.as_str()),
            diag_context: diag.as_ref().map(|staged| staged.block.as_str()),
        })
    };
    let max_tokens = if req.loop_turn {
        // Only a tuned model's window lowers the loop cap; untuned, it is unchanged.
        live.loop_max_tokens
            .min(state.tuning_for(&model).map_or(u64::MAX, |tuning| tuning.max_tokens))
    } else {
        state.chat_max_tokens(&model)
    };
    let reservation = crate::server::compaction::reply_reservation(&state.backend, max_tokens);
    let ratio =
        crate::server::compaction::token_ratio(session.token_calibration.as_ref(), &state.chat.base_url, &model);
    let tool_tokens = if !cloud_selected && chat_tools {
        crate::server::compaction::estimate_tokens(&serde_json::to_string(&crate::server::chat_web::tools_for(chat_web_mode)).unwrap())
    } else {
        0
    };
    // Projection uses stored prompt history (up to SessionStore::MAX_MESSAGES).
    // Compaction owns overflow against chat.compact_prompt_tokens, floored at
    // one reply+4096 (the hard limit at the effective reservation+4096). The
    // incoming paste is never compacted.
    let mut history = if cloud_selected {
        Vec::new()
    } else {
        prompt_history(&session.messages)
    };
    let mut compaction = None;
    // Why this turn was sent uncompacted although it passed the trigger.
    let mut compaction_skipped: Option<&'static str> = None;
    let mut summary_prompt_tokens = 0u64;
    let mut summary_completion_tokens = 0u64;
    // The prompt limit this turn was sized for, checked again before the reply.
    let mut turn_prompt_limit = 0u64;
    if !cloud_selected {
        let configured_threshold = state.compact_prompt_tokens(&model, max_tokens);
        let prompt_cap = state.prompt_cap(&model);
        // The reply budget of this turn: /loop turns reserve their own.
        let reply_setting = if req.loop_turn {
            "api.harness_loop_rate_limit.max_tokens"
        } else {
            "models.local_llm.max_tokens"
        };
        let web_chat = chat_tools;
        // The trigger's floor counts one reply, like its projection below.
        let (threshold, limit_source) = crate::server::compaction::turn_prompt_limit(
            configured_threshold,
            prompt_cap,
            web_chat.then_some(web.limits.total_tokens),
            max_tokens,
            crate::server::compaction::calibrated_tokens(tool_tokens, ratio),
            crate::server::compaction::calibrated_tokens(crate::server::web_search::MIN_EVIDENCE_TOKENS, ratio),
            reply_setting,
        );
        // The trigger starts compaction; the hard limit (the prompt cap, or the
        // web room for web chat) is the most a turn may send. History that
        // compaction cannot bring under the trigger, or a summary the model
        // could not produce usably, is sent while it fits the hard limit.
        let (hard_limit, hard_source) = crate::server::compaction::turn_prompt_limit(
            prompt_cap,
            prompt_cap,
            web_chat.then_some(web.limits.total_tokens),
            reservation,
            crate::server::compaction::calibrated_tokens(tool_tokens, ratio),
            crate::server::compaction::calibrated_tokens(crate::server::web_search::MIN_EVIDENCE_TOKENS, ratio),
            reply_setting,
        );
        turn_prompt_limit = hard_limit;
        // What can raise the web budget, so the remedy never suggests a change that cannot help.
        let web_raise = crate::server::compaction::WebRaise::for_budget(
            web.limits.total_tokens,
            state.web_total_ceiling(&model),
            state.verified_window(&model),
        );
        // Name the setting that actually bounds this prompt: "start a new session"
        // cannot help when the system prompt and reply reservation fill the limit.
        // `reply` is what the bound counts: one reply for the trigger, the reservation otherwise.
        let too_large = |what: &str, projected: u64, compacted: u64, threshold: u64, limit_source: &'static str, reply: u64| {
            state.audit.log(json!({
                "event": "chat_prompt_too_large",
                "session_id": session.session_id,
                "projected": projected,
                "compacted_projected": compacted,
                "threshold": threshold,
                "limit_source": limit_source,
            }));
            let remedy =
                crate::server::compaction::prompt_limit_remedy(
                limit_source,
                threshold,
                // Raising the setting helps only below every ceiling it is held to.
                state.compact_ceiling(&model),
                web_raise,
                reply_setting,
                web_chat,
            );
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CHAT_PROMPT_TOO_LARGE",
                format!(
                    "{what} the {threshold}-token prompt limit (set by {limit_source}; {reply_setting} reserves \
                     {reply} of those tokens for the reply); {remedy}"
                ),
            )
            .details(json!({
                "projected_tokens": projected,
                "compacted_tokens": compacted,
                "limit_tokens": threshold,
                "limit_source": limit_source,
                "reply_reservation_tokens": reply,
            }))
        };
        let keep_max = state
            .cfg
            .u64_or(
                "chat.compact_keep_messages",
                crate::server::compaction::DEFAULT_KEEP_MESSAGES,
            )
            .clamp(2, 40) as usize;
        // Every backend reaches the trigger at the same input size: it counts one
        // reply. The hard limit, the window checks and dispatch keep the
        // effective reservation, twice where hidden reasoning may spend it, so
        // a doubled one can compact sooner, at the hard limit.
        let project = |history: &[ChatMessage], reply: u64| {
            crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                history,
                &req.message,
                reply,
                ratio,
                tool_tokens,
            )
        };
        let projected = project(&history, reservation);
        let projected_trigger = project(&history, max_tokens);
        if projected_trigger > threshold || projected > hard_limit {
            let (paste_trigger, paste_alone) = (project(&[], max_tokens), project(&[], reservation));
            if paste_trigger > threshold || paste_alone > hard_limit {
                // Past the hard limit a larger trigger cannot help: name the hard
                // limit. Each size counts the reply of the limit it is held to.
                let (limit, source, reply, whole, size) = if paste_alone > hard_limit {
                    (hard_limit, hard_source, reservation, projected, paste_alone)
                } else {
                    (threshold, limit_source, max_tokens, projected_trigger, paste_trigger)
                };
                return Err(too_large(
                    "the new message and system prompt alone exceed",
                    whole,
                    size,
                    limit,
                    source,
                    reply,
                ));
            }
            let before = session.messages.len();
            // The verbatim tail is whole exchanges sized in tokens, so the
            // compacted prompt lands well under the trigger: at most
            // chat.compact_tail_percent of it, and no more than what is left
            // after the system prompt, first message, new message, reply and a
            // full summary. Never less than the last exchange.
            let summary_tokens = crate::server::compaction::summary_max_tokens(&state.cfg);
            let summary_send = crate::server::compaction::reply_reservation(&state.backend, summary_tokens);
            let scaffold =
                crate::server::compaction::calibrated_tokens(crate::server::compaction::scaffold_tokens(), ratio);
            let pinned_only: Vec<_> = session.messages.iter().find(|m| m.role == "user").cloned().into_iter().collect();
            let fixed = project(&prompt_history(&pinned_only), max_tokens);
            let room = threshold.saturating_sub(fixed + summary_send + scaffold);
            let tail_budget = (threshold * crate::server::compaction::tail_percent(&state.cfg) / 100).min(room);
            let keep = crate::server::compaction::tail_keep(&session.messages, keep_max, tail_budget, ratio);
            let retained = crate::server::compaction::retained_messages(&session.messages, keep);
            let retained_history = prompt_history(&retained);
            let middle = crate::server::compaction::middle_turns(&session.messages, keep);
            let retained_projected = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &retained_history,
                &req.message,
                reservation,
                ratio,
                tool_tokens,
            ) + if middle.is_empty() { 0 } else { scaffold };
            let fits_uncompacted = projected <= hard_limit;
            // A middle smaller than the scaffold makes compaction grow the
            // prompt: refuse only when the uncompacted turn does not fit either
            // (when it does, that middle is never worth summarizing).
            if retained_projected > hard_limit && !fits_uncompacted {
                return Err(too_large(
                    "the next prompt still exceeds, after history compaction,",
                    projected,
                    retained_projected,
                    hard_limit,
                    hard_source,
                    reservation,
                ));
            }
            let mut summary_meta = crate::server::compaction::Summary::default();
            let summary = if fits_uncompacted
                && !crate::server::compaction::worth_summarizing(middle, summary_send + scaffold, ratio)
            {
                // Too little to summarize to be worth a model call.
                compaction_skipped = Some("no_gain");
                None
            } else if middle.is_empty() {
                Some(crate::server::compaction::COMPACT_PREFIX.to_string())
            } else {
                // The summary is a model call too: check the window first and
                // size its input to the model's cap, not the 32768 default.
                state
                    .ensure_window_allows(&model, turn_prompt_limit, 0, sized_tuned)
                    .await
                    .map_err(|e| ApiError::from_err(llm_status(&e), &e))?;
                let pinned = crate::server::compaction::PinnedContext {
                    goal: &session.goal,
                    first_user: session.messages.iter().find(|m| m.role == "user").map(|m| m.text.as_str()),
                };
                match crate::server::compaction::summarize_turns(
                    &state.chat,
                    &model,
                    pinned,
                    middle,
                    summary_tokens,
                    summary_send,
                    ratio,
                    state.prompt_cap(&model),
                )
                .await
                {
                    Ok(meta) => {
                        summary_meta = meta;
                        summary_prompt_tokens = summary_meta.prompt_tokens;
                        summary_completion_tokens = summary_meta.completion_tokens;
                        Some(std::mem::take(&mut summary_meta.text))
                    }
                    // A summary the model could not produce usably leaves the
                    // session unrewritten; the turn goes uncompacted if it fits.
                    // The attempt was still a model call, so the tally counts it.
                    Err(e) if e.code == crate::server::compaction::SUMMARY_UNUSABLE && fits_uncompacted => {
                        summary_prompt_tokens = e.details["prompt_tokens"].as_u64().unwrap_or(0);
                        summary_completion_tokens = e.details["completion_tokens"].as_u64().unwrap_or(0);
                        compaction_skipped = Some("summary_unusable");
                        None
                    }
                    Err(e) => return Err(ApiError::from_err(StatusCode::BAD_GATEWAY, &e)),
                }
            };
            if let Some(summary) = summary {
            let compacted = crate::server::compaction::compact_messages(&session.messages, keep, &summary);
            let compacted_history = prompt_history(&compacted);
            let compacted_projected = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &compacted_history,
                &req.message,
                reservation,
                ratio,
                tool_tokens,
            );
            if compacted_projected > hard_limit {
                return Err(too_large(
                    "the next prompt still exceeds, after history compaction,",
                    projected,
                    compacted_projected,
                    hard_limit,
                    hard_source,
                    reservation,
                ));
            }
            compaction = Some(Compaction {
                keep,
                before,
                after: compacted.len(),
                summarized: middle.len(),
                projected,
                compacted_projected,
                threshold,
                summary,
                omitted_turns: summary_meta.omitted_turns,
                clipped_turns: summary_meta.clipped_turns,
                repairs: summary_meta.repairs,
            });
            history = compacted_history;
            }
        }
    }
    history.push(ChatMessage {
        role: "user".into(),
        content: req.message.clone(),
    });
    let temperature = state.cfg.f64_or("models.local_llm.temperature", DEFAULT_TEMPERATURE);
    let spend_source = if req.loop_turn { "loop" } else { "chat" };
    // Compaction can take a while: the window must still allow what this turn
    // was sized for before the reply is requested, even if tuning was cleared.
    if !cloud_selected {
        let web_total = if chat_tools { web.limits.total_tokens } else { 0 };
        state
            .ensure_window_allows(&model, turn_prompt_limit, web_total, sized_tuned)
            .await
            .map_err(|e| ApiError::from_err(llm_status(&e), &e))?;
    }
    let (mut reply, web_tools) = if cloud_selected {
        require_cloud_consent(&state, &owner, &model)?;
        (
            state
                .cloud_chat
                .chat_with_source(&model, &req.message, spend_source)
                .await
                .map_err(|e| {
                    if e.code == "CLOUD_CHAT_BUDGET" {
                        state
                            .audit
                            .log(json!({"event":"cloud_chat_budget_refused", "owner":owner, "model":model}));
                        ApiError::from_err(StatusCode::UNPROCESSABLE_ENTITY, &e)
                    } else {
                        ApiError::from_err(llm_status(&e), &e)
                    }
                })?,
            Vec::new(),
        )
    } else if chat_tools {
        crate::server::chat_web::run_stream(
            &state,
            &web,
            &owner,
            &system_prompt,
            &history,
            &model,
            max_tokens,
            temperature,
            ratio,
            chat_web_mode,
            sized_tuned,
            output,
        )
        .await
        .map_err(|e| ApiError::from_err(llm_status(&e), &e))?
    } else {
        (
            state
                .chat
                .chat_stream(
                    &system_prompt,
                    &history,
                    Some(&model),
                    max_tokens,
                    temperature,
                    output.map(|sender| crate::llm::openai_stream::Output {
                        sender,
                        validate: &|| Ok(()),
                    }),
                )
                .await
                .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?,
            Vec::new(),
        )
    };
    drop(release);

    // Ground against the tools the answering request offered: web chat withholds
    // them when no tool round fits, and withdraws them after a refused batch.
    let inventory = crate::server::tool_inventory::chat_callable_names(
        if !cloud_selected && reply.final_prompt_tools { chat_web_mode } else { crate::server::sessions::GoalWebMode::Off },
    );
    reply.body_text = crate::server::tool_inventory::ground_assistant_text(&reply.body_text, &inventory);

    if !cloud_selected {
        let tokens = if reply.usage_reported {
            crate::llm::spend::UsageTokens {
                input_tokens: Some(reply.prompt_tokens),
                output_tokens: Some(reply.completion_tokens),
                ..crate::llm::spend::UsageTokens::default()
            }
        } else {
            crate::llm::spend::UsageTokens::default()
        };
        let empty = reply.body_text.trim().is_empty();
        crate::llm::spend::record(
            Some(&state.audit),
            &state.spend_file,
            crate::common::bounded_log::limit(&state.cfg),
            crate::llm::spend::SpendEvent {
                provider: "local",
                model: &reply.model,
                served_model: None,
                source: spend_source,
                tokens,
                outcome: empty.then_some("failed_after_billing"),
            },
        );
        if empty {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "EMPTY_MODEL_RESPONSE",
                "local model returned HTTP 2xx with empty text after billing",
            ));
        }
    }

    let usage = TokenTally {
        prompt_tokens: reply.prompt_tokens.saturating_add(summary_prompt_tokens),
        completion_tokens: reply.completion_tokens.saturating_add(summary_completion_tokens),
        exchanges: 0,
    };
    let prompt_skills = selected_skills
        .iter()
        .map(|(id, body)| {
            json!({"id":id,"outcome":"included_in_successful_chat","chars":body.chars().count(),"sha256":crate::common::sha256_hex(body)})
        })
        .collect::<Vec<_>>();
    let calibration = if cloud_selected {
        None
    } else {
        // Calibrate against what the first call sent: web chat leaves out the tool
        // definitions when a tool round cannot fit.
        let sent_tools = if reply.initial_prompt_tools { tool_tokens } else { 0 };
        let estimated_input =
            crate::server::compaction::projected_prompt_tokens(&system_prompt, &history, "", 0, 1.0, sent_tools);
        crate::server::compaction::calibrate(
            session.token_calibration.as_ref(),
            &state.chat.base_url,
            &model,
            estimated_input,
            reply.initial_prompt_tokens,
        )
    };
    // Ollama keeps the system prompt and the newest message of an oversized
    // prompt and drops whole older messages, this session's first one first,
    // without an error (its MLX engine does not truncate). The harness sends
    // no num_ctx, so compare this turn with the window Ollama actually loaded.
    let context_window = if cloud_selected || state.backend.provider != "ollama" {
        None
    } else {
        // This turn's own usage may already show denser text than `ratio`.
        let ratio = calibration.as_ref().map_or(ratio, |c| {
            crate::server::compaction::token_ratio(Some(c), &state.chat.base_url, &model)
        });
        let project = |extra| {
            crate::server::compaction::projected_prompt_tokens(&system_prompt, &history, "", reservation, ratio, extra)
        };
        // The turn's largest request: the first one's tool definitions, or a later
        // web round's definitions plus the tool calls and results sent so far.
        let first = if reply.initial_prompt_tools { tool_tokens } else { 0 };
        let projected = project(first.max(reply.peak_prompt_extra_tokens));
        loaded_window(&state, &model)
            .await
            .filter(|window| projected > *window)
            .map(|window| {
                state.audit.log(json!({"event":"chat_context_window_exceeded","session_id":session.session_id,
                    "model":model,"window":window,"projected":projected}));
                json!({"model":model,"window_tokens":window,"projected_tokens":projected,
                    "message":format!("Ollama loaded {model} with a {window}-token context window, but this turn needed about \
                        {projected} tokens (prompt plus reply reserve). Ollama keeps the system prompt and silently drops whole \
                        older messages, starting with this session's first message. Restart Ollama with a larger OLLAMA_CONTEXT_LENGTH (32768 is recommended; see \
                        docs/MODELS.md), or start a new session. A Modelfile PARAMETER num_ctx wins over that variable, so \
                        a derived tag needs its own num_ctx raised; the harness sends no num_ctx.")})
            })
    };
    let recorded = state.store.for_owner(&owner).record_exchange_inner(
        &session.session_id,
        &req.message,
        &reply.body_text,
        &reply.model,
        &usage,
        &prompt_skills,
        compaction.as_ref().map(|c| (c.keep, c.summary.clone())),
        calibration,
    );
    let updated = recorded.map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    if !cloud_selected {
        let live = |id: &str| state.attachments.owned_blob(&owner, id).is_some();
        state
            .store
            .for_owner(&owner)
            .merge_attachment_pins(&session.session_id, &owner, &req.attachment_ids, live)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    }
    // Counts only, never bodies: the audit and the response say what this
    // turn's compaction kept, summarized and repaired.
    let compaction_report = compaction.as_ref().map(|c| {
        json!({
            "before": c.before,
            "after": c.after,
            "kept_messages": c.keep.min(c.before),
            "summarized_messages": c.summarized,
            "projected": c.projected,
            "compacted_projected": c.compacted_projected,
            "threshold": c.threshold,
            "omitted_turns": c.omitted_turns,
            "clipped_turns": c.clipped_turns,
            "masked_identifiers": c.repairs.masked_identifiers,
            "dropped_sentences": c.repairs.dropped_sentences,
            "dropped_corrections": c.repairs.dropped_corrections,
        })
    });
    if let Some(reason) = compaction_skipped {
        state.audit.log(json!({"event": "chat_compaction_skipped", "session_id": session.session_id, "reason": reason}));
    }
    if let Some(report) = &compaction_report {
        let mut event = json!({"event": "chat_session_compacted", "session_id": session.session_id});
        if let (Some(event), Some(report)) = (event.as_object_mut(), report.as_object()) {
            event.extend(report.clone());
        }
        state.audit.log(event);
    }
    let sensitivity = {
        let scanner = crate::common::injection::Scanner::core();
        if scanner.count_matches(&req.message) > 0 || scanner.count_matches(&reply.body_text) > 0 {
            "sensitive"
        } else {
            "normal"
        }
    };
    let episode = if portal_auditor {
        json!({"available": false, "staged": false, "reason": "portal_auditor"})
    } else {
        let (state, owner, model) = (state.clone(), owner.clone(), reply.model.clone());
        let (user_chars, assistant_chars) = (req.message.chars().count(), reply.body_text.chars().count());
        super::structured_memory::off_worker(move || {
            super::structured_memory::stage_after_exchange(&state, &owner, &model, user_chars, assistant_chars, sensitivity)
        })
        .await
    };
    // Intermediate loop turns repeat generated framing. Only the terminal reply
    // supplies completion evidence, paired with the human goal, never that framing.
    let memory_suggestion = if req.loop_turn && !reply_has_goal_done(&reply.body_text) {
        json!({"queued": false, "reason": "loop_turn"})
    } else if portal_auditor {
        json!({"queued": false, "reason": "portal_auditor"})
    } else {
        crate::server::structured_memory_suggest::enqueue_for_model(
            &state,
            &owner,
            crate::server::structured_memory_suggest::Source::Chat,
            episode["id"].as_str(),
            if req.loop_turn { &session.goal } else { &req.message },
            &reply.body_text,
            &model,
        )
    };
    Ok(Json(json!({
        "session_id": session.session_id,
        "reply": reply.body_text,
        "web_tools": web_tools,
        "model": reply.model,
        "usage": {"prompt_tokens": reply.prompt_tokens, "completion_tokens": reply.completion_tokens},
        "tally": updated.tally.to_json(),
        "episode": episode,
        "memory_suggestion": memory_suggestion,
        "context_window": context_window,
        "compaction": compaction_report,
        "compaction_skipped": compaction_skipped,
        "structured_facts": {
            "explicit_recall": crate::server::structured_memory::recall_available(
                &state.cfg,
                state.structured_memory.is_some(),
                &crate::server::structured_memory::current_gates(&state),
            ),
            "retrieval": crate::server::structured_memory::retrieval_available(
                &state.cfg,
                state.structured_memory.is_some(),
                &crate::server::structured_memory::current_gates(&state),
            ),
            "auto_retrieval": crate::server::structured_memory::auto_retrieval_available(
                &state.cfg,
                state.structured_memory.is_some(),
                &crate::server::structured_memory::current_gates(&state),
            ),
            "retrieve": req.retrieve,
            "retrieval_error": retrieval_error,
            "requested": selections.iter().map(|f| json!({"id": f.id, "expected_revision": f.expected_revision})).collect::<Vec<_>>(),
            "injected": recalled.preview_json()["injected"],
            "dropped": recalled.preview_json()["dropped"],
        },
    })))
        } => result,
    }
}

/// Keep window, quota and context refusals distinct from upstream failures (502).
fn llm_status(error: &crate::common::errors::HarnessError) -> StatusCode {
    if error.code == "CLOUD_RATE_LIMITED" {
        StatusCode::TOO_MANY_REQUESTS
    } else if error.code == "CLOUD_CHAT_CONTEXT" {
        StatusCode::UNPROCESSABLE_ENTITY
    } else if error.code == crate::server::state::WINDOW_CHANGED {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_GATEWAY
    }
}

/// The window to check a finished turn against: the live `/api/ps` read
/// ([`AppState::live_window`]), else the window `auto_tune` recorded at
/// selection. The live read wins because the recorded one goes stale once
/// Ollama restarts with the larger window this warning asks for.
async fn loaded_window(state: &AppState, model: &str) -> Option<u64> {
    state
        .live_window(model)
        .await
        .or_else(|| state.tuning_for(model).map(|tuning| tuning.window))
}

/// Drops the per-session loop in-flight claim on every exit path.
struct LoopRelease {
    state: Arc<AppState>,
    session_id: String,
    active: bool,
}

impl Drop for LoopRelease {
    fn drop(&mut self) {
        if self.active {
            self.state.release_loop_inflight(&self.session_id);
        }
    }
}

pub async fn cancel_chat(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let cancelled = state
        .chat_owner
        .cancel_with(&owner, || state.abort_chat())
        .map_err(|e| ApiError::from_err(StatusCode::FORBIDDEN, &e))?;
    state
        .web
        .chat_turn
        .cancel(&owner)
        .map_err(|e| ApiError::from_err(StatusCode::FORBIDDEN, &e))?;
    Ok(Json(json!({"cancelled": cancelled})))
}

// Account identity comes only from the guarded request extension.
type Caller = Option<axum::Extension<crate::common::auth_store::UserSummary>>;

// Keep marker recognition aligned with console replyHasGoalDone; this is
// unverified model advice, never proof of execution or authority to apply facts.
pub(crate) fn reply_has_goal_done(reply: &str) -> bool {
    static MARKER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let marker = MARKER.get_or_init(|| {
        regex::Regex::new(r"^(?:(?:[#>*_`~\-]+|[0-9]+[.)])\s*)*GOAL_DONE(?:\s*[*_`~.!:]+)*$")
            .expect("valid goal completion marker")
    });
    reply.lines().any(|line| marker.is_match(line.trim()))
}

#[cfg(test)]
mod loop_completion_tests {
    #[test]
    fn completion_marker_matches_console_decorations_not_mentions() {
        for text in [
            "GOAL_DONE",
            "Summary.\n**GOAL_DONE**",
            "`GOAL_DONE`",
            "## GOAL_DONE",
            "1. GOAL_DONE",
            "> GOAL_DONE.",
            "  GOAL_DONE  ",
        ] {
            assert!(super::reply_has_goal_done(text), "{text}");
        }
        for text in [
            "",
            "Not GOAL_DONE yet",
            "GOAL_DONE eventually",
            "GOAL_DONE?",
            "goal_done",
        ] {
            assert!(!super::reply_has_goal_done(text), "{text}");
        }
    }
}
