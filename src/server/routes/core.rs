//! Status, sessions, soul/model toggles, chat + cancel.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use axum::Json;
use serde_json::{json, Value};
use std::net::SocketAddr;

use crate::common::tool_broker::assert_allowed;
use crate::llm::openai_chat::ChatMessage;
use crate::server::attachments;
use crate::server::compaction::{DEFAULT_REPLY_TOKENS, MAX_PROMPT_TOKENS, MIN_PROMPT_HEADROOM};
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::guards::retry_after_error;
use crate::server::prompts::{compose_system_prompt, PromptInputs};
use crate::server::schemas::*;
use crate::server::sessions::TokenTally;
use crate::server::slash::{parse_line, SlashParseRequest};
use crate::server::state::{AppState, HARNESS_LOOP_TOOL};

pub async fn slash_parse(ValidJson(req): ValidJson<SlashParseRequest>) -> Json<Value> {
    Json(parse_line(&req.line).to_json())
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
    if state.auth.is_some() && user.is_none_or(|u| u.role == "audit" || u.must_change_password) {
        return Json(
            json!({"version":crate::VERSION,"auth_enabled":true,"api_key_optional":true,"status":"login required for operational details"}),
        );
    }
    let sessions = state.store.list();
    let total_tokens: u64 = sessions.iter().filter_map(|s| s["tokens"]["total"].as_u64()).sum();
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Json(json!({
        "version": crate::VERSION,
        "model": state.current_model(),
        "provider": state.current_provider(),
        "api_key_optional": state.api_key_optional,
        "base_url": if state.cloud_chat.is_cloud_selection(&state.current_model()) { Value::Null } else { json!(state.backend.base_url) },
        "soul_enabled": settings.soul_enabled,
        "soul": soul_status(&state, settings.soul_enabled),
        "memory_enabled": settings.memory_enabled,
        "home": state.home.root.display().to_string(),
        "repo_root": Value::Null,
        "chat_mode": "conversation",
        "chat_tools_available": settings.web_enabled && !state.cloud_chat.is_cloud_selection(&state.current_model()),
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

pub async fn spend_summary(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(crate::llm::spend::summarize_file(&state.spend_file))
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

pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"sessions": state.store.list()}))
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<SessionCreateRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let session = state
        .store
        .create(&state.current_model(), &req.title)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok((StatusCode::CREATED, Json(session.summary())))
}

pub async fn clear_sessions(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<SessionClearRequest>,
) -> ApiResult<Json<Value>> {
    crate::server::structured_memory_suggest::clear_chat_queue(&state);
    state.abort_chat();
    let deleted = state
        .store
        .clear()
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    let owner = super::auth::context_owner(user);
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

pub async fn get_session(State(state): State<Arc<AppState>>, Path(session_id): Path<String>) -> ApiResult<Json<Value>> {
    let session = state
        .store
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
    out["style"] = json!(session.style);
    Ok(Json(out))
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<RenameRequest>,
) -> ApiResult<Json<Value>> {
    let session = state
        .store
        .rename(&session_id, Some(&req.title), None)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    Ok(Json(session.summary()))
}

pub async fn session_goal(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<GoalRequest>,
) -> ApiResult<Json<Value>> {
    let session = state
        .store
        .rename(&session_id, None, Some(&req.goal))
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let mut out = session.summary();
    out["goal"] = json!(session.goal);
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
    ValidJson(req): ValidJson<ModelSelectRequest>,
) -> ApiResult<Json<Value>> {
    let snapshot = {
        let mut s = state.settings.lock().unwrap_or_else(|p| p.into_inner());
        s.selected_model = req.model.trim().to_string();
        s.clone()
    };
    snapshot
        .save(&state.home)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(
        json!({"model": state.current_model(), "provider": state.current_provider()}),
    ))
}

/// Prior user/assistant turns for the next prompt. Compaction owns overflow.
/// The persist cap is `MAX_MESSAGES`. There is no 20-turn or 8000-char clip.
pub fn prompt_history(session: &crate::server::sessions::Session) -> Vec<ChatMessage> {
    session
        .messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.text.clone(),
        })
        .collect()
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
        return chat_inner(state, peer, user, req, None).await.into_response();
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Value>(16);
    let task = tokio::spawn(async move {
        let result = chat_inner(state, peer, user, req, Some(&sender)).await;
        let event = match result {
            Ok(Json(data)) => json!({"type":"done","data":data}),
            Err(error) => {
                json!({"type":"error","status":error.status.as_u16(),"error":error.body(),"headers":error.headers})
            }
        };
        let _ = sender.send(event).await;
    });
    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let abort = AbortOnDrop(task.abort_handle());
    let stream = futures_util::stream::unfold((receiver, abort), |(mut receiver, abort)| async move {
        let event = receiver.recv().await?;
        Some((
            Ok::<_, std::convert::Infallible>(Event::default().data(event.to_string())),
            (receiver, abort),
        ))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn chat_inner(
    state: Arc<AppState>,
    peer: SocketAddr,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    req: ChatRequest,
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
) -> ApiResult<Json<Value>> {
    if req.loop_turn && req.session_id.as_deref().unwrap_or("").is_empty() {
        return Err(loop_error(
            "LOOP_REQUIRES_SESSION",
            "loop turns require an existing session",
        ));
    }
    let session = match req.session_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => state
            .store
            .get(id)
            .map_err(|e| ApiError::from_err(session_status(&e), &e))?,
        None => state
            .store
            .create(&state.current_model(), "")
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?,
    };
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| state.current_model());
    let cloud_selected = state.cloud_chat.is_cloud_selection(&model);
    if cloud_selected && req.loop_turn {
        return Err(loop_error(
            "CLOUD_CHAT_LOOP",
            "cloud chat is unavailable for /loop turns",
        ));
    }
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let selected_skills = if cloud_selected {
        Vec::new()
    } else {
        super::skills::resolve(&state, &session.selected_skills)?
    };

    let live = state.runtime_limits();
    let mut web = state.web.clone();
    web.limits = live.web.clone();
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

    let _gate = match state.generation_gate.claim_or_busy_owner("chat") {
        Ok(gate) => gate,
        Err(busy_owner) => {
            drop(release);
            let mut details =
                json!({"session_id": session.session_id, "timeout_sec": state.chat_timeout_sec(&model) as u64});
            if busy_owner == "chat" {
                details["cancel"] = json!("/api/chat/cancel");
            } else if busy_owner == "consolidation" {
                details["busy"] = json!("consolidation");
            }
            state.audit.log(json!({
                "event": "chat_busy",
                "session_id": session.session_id,
                "owner": busy_owner,
            }));
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "CHAT_BUSY",
                "a local model turn is already running",
            )
            .details(details));
        }
    };

    let owner = super::auth::context_owner(user);
    let surface = if cloud_selected {
        crate::server::retrieval::RetrievalSurface::CloudChat
    } else if req.loop_turn {
        crate::server::retrieval::RetrievalSurface::Loop
    } else {
        crate::server::retrieval::RetrievalSurface::LocalChat
    };
    let live_pins: Vec<String> = session
        .pinned_ids_for(&owner)
        .into_iter()
        .filter(|id| state.attachments.owned_blob(&owner, id).is_some())
        .collect();
    crate::server::retrieval::refuse_forbidden_attachments(surface, &req.attachment_ids, &live_pins)
        .map_err(|e| attachments::upload_error(&e))?;
    let web_context = if cloud_selected {
        String::new()
    } else {
        web.context_text(settings.web_enabled, &owner)
    };
    let selections = req
        .selected_facts
        .as_ref()
        .map(|items| super::structured_memory::selections_from_items(items))
        .unwrap_or_else(|| session.selected_facts.clone());
    let ids = crate::server::retrieval::attachment_ids_for_prompt(&live_pins, &req.attachment_ids);
    let attachment_fence = crate::server::retrieval::assemble_local_untrusted(
        surface,
        &state.attachments,
        &state.notes_corpus,
        &owner,
        &ids,
        Some(req.message.as_str()),
    )
    .map_err(|e| attachments::upload_error(&e))?;
    let (pinned, facts, memory_budget, recalled, retrieval_error) = super::structured_memory::prompt_memory(
        &state,
        &owner,
        settings.memory_enabled,
        &selections,
        crate::server::structured_memory::RetrievalIntent {
            force: req.retrieve,
            query: req.retrieve_query.as_deref(),
            message: Some(req.message.as_str()),
        },
    );
    let system_prompt = if cloud_selected {
        String::new()
    } else {
        compose_system_prompt(&PromptInputs {
            selected_skills: &selected_skills,
            soul_enabled: settings.soul_enabled,
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
            memory_enabled: settings.memory_enabled,
            chat_tools_enabled: settings.web_enabled && !req.loop_turn,
            web_enabled: settings.web_enabled,
            attachment_fence: Some(attachment_fence.as_str()),
        })
    };
    let max_tokens = if req.loop_turn {
        live.loop_max_tokens
    } else {
        state.cfg.u64_or("models.local_llm.max_tokens", DEFAULT_REPLY_TOKENS)
    };
    let reservation = crate::server::compaction::reply_reservation(&state.backend, max_tokens);
    let ratio =
        crate::server::compaction::token_ratio(session.token_calibration.as_ref(), &state.chat.base_url, &model);
    let tool_tokens = if !cloud_selected && settings.web_enabled && !req.loop_turn {
        crate::server::compaction::estimate_tokens(&serde_json::to_string(&crate::server::chat_web::tools()).unwrap())
    } else {
        0
    };
    // Projection uses stored prompt history (up to SessionStore::MAX_MESSAGES).
    // Compaction owns overflow against chat.compact_prompt_tokens, floored at
    // effective reply reservation+4096. The incoming paste is never compacted.
    let mut history = if cloud_selected {
        Vec::new()
    } else {
        prompt_history(&session)
    };
    let mut compaction = None;
    let mut summary_prompt_tokens = 0u64;
    let mut summary_completion_tokens = 0u64;
    if !cloud_selected {
        let configured_threshold = state.cfg.u64_or(
            "chat.compact_prompt_tokens",
            crate::server::compaction::DEFAULT_PROMPT_TOKENS,
        );
        let minimum_threshold = reservation
            .saturating_add(MIN_PROMPT_HEADROOM)
            .saturating_add(crate::server::compaction::calibrated_tokens(tool_tokens, ratio))
            .min(MAX_PROMPT_TOKENS);
        let mut threshold = configured_threshold.clamp(minimum_threshold, MAX_PROMPT_TOKENS);
        if settings.web_enabled && !req.loop_turn {
            let web_room = web.limits.total_tokens.saturating_sub(reservation.saturating_mul(2));
            threshold = threshold.min(web_room).max(minimum_threshold);
        }
        let keep = state
            .cfg
            .u64_or(
                "chat.compact_keep_messages",
                crate::server::compaction::DEFAULT_KEEP_MESSAGES,
            )
            .clamp(2, 40) as usize;
        let projected = crate::server::compaction::projected_prompt_tokens(
            &system_prompt,
            &history,
            &req.message,
            reservation,
            ratio,
            tool_tokens,
        );
        if projected > threshold {
            let paste_alone = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &[],
                &req.message,
                reservation,
                ratio,
                tool_tokens,
            );
            if paste_alone > threshold {
                state.audit.log(json!({
                    "event": "chat_prompt_too_large",
                    "session_id": session.session_id,
                    "projected": projected,
                    "compacted_projected": paste_alone,
                    "threshold": threshold,
                }));
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "CHAT_PROMPT_TOO_LARGE",
                    "the next prompt still exceeds the configured limit after history compaction",
                )
                .details(json!({
                    "projected_tokens": projected,
                    "compacted_tokens": paste_alone,
                    "limit_tokens": threshold,
                })));
            }
            let before = session.messages.len();
            let retained = crate::server::compaction::retained_messages(&session.messages, keep);
            let retained_history: Vec<ChatMessage> = retained
                .iter()
                .filter(|m| m.role == "user" || m.role == "assistant")
                .map(|m| ChatMessage {
                    role: m.role.clone(),
                    content: m.text.clone(),
                })
                .collect();
            let retained_projected = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &retained_history,
                &req.message,
                reservation,
                ratio,
                tool_tokens,
            );
            if retained_projected > threshold {
                state.audit.log(json!({
                    "event": "chat_prompt_too_large",
                    "session_id": session.session_id,
                    "projected": projected,
                    "compacted_projected": retained_projected,
                    "threshold": threshold,
                }));
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "CHAT_PROMPT_TOO_LARGE",
                    "the next prompt still exceeds the configured limit after history compaction",
                )
                .details(json!({
                    "projected_tokens": projected,
                    "compacted_tokens": retained_projected,
                    "limit_tokens": threshold,
                })));
            }
            let middle = crate::server::compaction::middle_turns(&session.messages, keep);
            let summary = if middle.is_empty() {
                crate::server::compaction::COMPACT_PREFIX.to_string()
            } else {
                let summary_tokens = crate::server::compaction::summary_max_tokens(&state.cfg);
                let (text, p, c) = crate::server::compaction::summarize_turns(
                    &state.chat,
                    &model,
                    middle,
                    summary_tokens,
                    crate::server::compaction::reply_reservation(&state.backend, summary_tokens),
                    ratio,
                )
                .await
                .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
                summary_prompt_tokens = p;
                summary_completion_tokens = c;
                text
            };
            let mut compacted = session.clone();
            compacted.messages = crate::server::compaction::compact_messages(&session.messages, keep, &summary);
            let compacted_history = prompt_history(&compacted);
            let compacted_projected = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &compacted_history,
                &req.message,
                reservation,
                ratio,
                tool_tokens,
            );
            if compacted_projected > threshold {
                state.audit.log(json!({
                    "event": "chat_prompt_too_large",
                    "session_id": session.session_id,
                    "projected": projected,
                    "compacted_projected": compacted_projected,
                    "threshold": threshold,
                }));
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "CHAT_PROMPT_TOO_LARGE",
                    "the next prompt still exceeds the configured limit after history compaction",
                )
                .details(json!({
                    "projected_tokens": projected,
                    "compacted_tokens": compacted_projected,
                    "limit_tokens": threshold,
                })));
            }
            let after = compacted.messages.len();
            compaction = Some((keep, before, after, projected, compacted_projected, threshold, summary));
            history = compacted_history;
        }
    }
    history.push(ChatMessage {
        role: "user".into(),
        content: req.message.clone(),
    });
    let estimated_input =
        crate::server::compaction::projected_prompt_tokens(&system_prompt, &history, "", 0, 1.0, tool_tokens);
    let temperature = state.cfg.f64_or("models.local_llm.temperature", DEFAULT_TEMPERATURE);
    let spend_source = if req.loop_turn { "loop" } else { "chat" };
    let (mut reply, web_tools) = if cloud_selected {
        (
            state
                .cloud_chat
                .chat_with_source(&model, &req.message, spend_source)
                .await
                .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?,
            Vec::new(),
        )
    } else if settings.web_enabled && !req.loop_turn {
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
            output,
        )
        .await
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?
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

    let inventory =
        crate::server::tool_inventory::chat_callable_names(!cloud_selected && settings.web_enabled && !req.loop_turn);
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
            use sha2::{Digest, Sha256};
            json!({"id":id,"outcome":"included_in_successful_chat","chars":body.chars().count(),"sha256":hex::encode(Sha256::digest(body.as_bytes()))})
        })
        .collect::<Vec<_>>();
    let calibration = if cloud_selected {
        None
    } else {
        crate::server::compaction::calibrate(
            session.token_calibration.as_ref(),
            &state.chat.base_url,
            &model,
            estimated_input,
            reply.initial_prompt_tokens,
        )
    };
    let recorded = state.store.record_exchange_inner(
        &session.session_id,
        &req.message,
        &reply.body_text,
        &reply.model,
        &usage,
        &prompt_skills,
        compaction.as_ref().map(|(keep, .., summary)| (*keep, summary.clone())),
        calibration,
    );
    let updated = recorded.map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    if !cloud_selected && !req.loop_turn {
        let live = |id: &str| state.attachments.owned_blob(&owner, id).is_some();
        state
            .store
            .merge_attachment_pins(&session.session_id, &owner, &req.attachment_ids, live)
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    }
    if let Some((_, before, after, projected, compacted_projected, threshold, _)) = compaction {
        state.audit.log(json!({
            "event": "chat_session_compacted",
            "session_id": session.session_id,
            "before": before,
            "after": after,
            "projected": projected,
            "compacted_projected": compacted_projected,
            "threshold": threshold,
        }));
    }
    let sensitivity = {
        let scanner = crate::common::injection::Scanner::core();
        if scanner.count_matches(&req.message) > 0 || scanner.count_matches(&reply.body_text) > 0 {
            "sensitive"
        } else {
            "normal"
        }
    };
    let episode = super::structured_memory::stage_after_exchange(
        &state,
        &owner,
        &reply.model,
        req.message.chars().count(),
        reply.body_text.chars().count(),
        sensitivity,
    );
    let memory_suggestion = crate::server::structured_memory_suggest::enqueue(
        &state,
        &owner,
        crate::server::structured_memory_suggest::Source::Chat,
        episode["id"].as_str(),
        &req.message,
        &reply.body_text,
    );
    Ok(Json(json!({
        "session_id": session.session_id,
        "reply": reply.body_text,
        "web_tools": web_tools,
        "model": reply.model,
        "usage": {"prompt_tokens": reply.prompt_tokens, "completion_tokens": reply.completion_tokens},
        "tally": updated.tally.to_json(),
        "episode": episode,
        "memory_suggestion": memory_suggestion,
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
    state
        .web
        .chat_turn
        .cancel(&super::auth::context_owner(user))
        .map_err(|e| ApiError::from_err(StatusCode::FORBIDDEN, &e))?;
    state.abort_chat();
    Ok(Json(json!({"cancelled": true})))
}
