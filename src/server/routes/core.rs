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
use crate::server::compaction::{DEFAULT_REPLY_TOKENS, MAX_PROMPT_TOKENS, MIN_PROMPT_HEADROOM};
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::guards::retry_after_error;
use crate::server::prompts::{compose_system_prompt, PromptInputs};
use crate::server::schemas::*;
use crate::server::sessions::TokenTally;
use crate::server::state::{AppState, HARNESS_LOOP_TOOL};

const DEFAULT_TEMPERATURE: f64 = 0.3;

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
        "chat_tools_available": settings.web_enabled,
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
    Ok(Json(json!({
        "deleted_sessions": deleted,
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

    let mut loop_claimed = false;
    if req.loop_turn {
        if session.goal.trim().is_empty() {
            return Err(loop_error("LOOP_REQUIRES_GOAL", "set a /goal before /loop"));
        }
        let ip = peer.ip().to_string();
        if !state.loop_rate_limiter.allow(&ip) {
            return Err(retry_after_error(
                &state.loop_rate_limiter,
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
    let web_context = if cloud_selected {
        String::new()
    } else {
        state.web.context_text(settings.web_enabled, &owner)
    };
    let selections = req
        .selected_facts
        .as_ref()
        .map(|items| super::structured_memory::selections_from_items(items))
        .unwrap_or_else(|| session.selected_facts.clone());
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
            goal: Some(&session.goal),
            web_context: Some(&web_context),
            memory_context: Some(&pinned),
            selected_facts_context: Some(&facts),
            memory_budget,
            memory_enabled: settings.memory_enabled,
            web_enabled: settings.web_enabled,
        })
    };
    let max_tokens = if req.loop_turn {
        state.loop_max_tokens
    } else {
        state.cfg.u64_or("models.local_llm.max_tokens", DEFAULT_REPLY_TOKENS)
    };
    // Projection uses stored prompt history (up to SessionStore::MAX_MESSAGES).
    // Compaction owns overflow against chat.compact_prompt_tokens, floored at
    // reply+4096. The incoming user paste is never compacted.
    let mut history = if cloud_selected {
        Vec::new()
    } else {
        prompt_history(&session)
    };
    let mut compaction = None;
    if !cloud_selected {
        let configured_threshold = state.cfg.u64_or(
            "chat.compact_prompt_tokens",
            crate::server::compaction::DEFAULT_PROMPT_TOKENS,
        );
        let minimum_threshold = max_tokens.saturating_add(MIN_PROMPT_HEADROOM).min(MAX_PROMPT_TOKENS);
        let mut threshold = configured_threshold.clamp(minimum_threshold, MAX_PROMPT_TOKENS);
        if settings.web_enabled && !req.loop_turn {
            let web_room = state
                .web
                .limits
                .total_tokens
                .saturating_sub(max_tokens.saturating_mul(2));
            threshold = threshold.min(web_room).max(minimum_threshold);
        }
        let keep = state
            .cfg
            .u64_or(
                "chat.compact_keep_messages",
                crate::server::compaction::DEFAULT_KEEP_MESSAGES,
            )
            .clamp(2, 40) as usize;
        let projected =
            crate::server::compaction::projected_prompt_tokens(&system_prompt, &history, &req.message, max_tokens);
        if projected > threshold {
            let paste_alone =
                crate::server::compaction::projected_prompt_tokens(&system_prompt, &[], &req.message, max_tokens);
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
            let middle = crate::server::compaction::middle_turns(&session.messages, keep);
            let summary = if middle.is_empty() {
                crate::server::compaction::COMPACT_PREFIX.to_string()
            } else {
                crate::server::compaction::summarize_turns(&state.chat, &model, middle)
                    .await
                    .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?
            };
            let mut compacted = session.clone();
            compacted.messages = crate::server::compaction::compact_messages(&session.messages, keep, &summary);
            let compacted_history = prompt_history(&compacted);
            let compacted_projected = crate::server::compaction::projected_prompt_tokens(
                &system_prompt,
                &compacted_history,
                &req.message,
                max_tokens,
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
    let temperature = state.cfg.f64_or("models.local_llm.temperature", DEFAULT_TEMPERATURE);
    let (reply, web_tools) = if cloud_selected {
        (
            state
                .cloud_chat
                .chat(&model, &req.message)
                .await
                .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?,
            Vec::new(),
        )
    } else if settings.web_enabled && !req.loop_turn {
        crate::server::chat_web::run_stream(
            &state,
            &owner,
            &system_prompt,
            &history,
            &model,
            max_tokens,
            temperature,
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

    let usage = TokenTally {
        prompt_tokens: reply.prompt_tokens,
        completion_tokens: reply.completion_tokens,
        exchanges: 0,
    };
    let prompt_skills = selected_skills
        .iter()
        .map(|(id, body)| {
            use sha2::{Digest, Sha256};
            json!({"id":id,"outcome":"included_in_successful_chat","chars":body.chars().count(),"sha256":hex::encode(Sha256::digest(body.as_bytes()))})
        })
        .collect::<Vec<_>>();
    let recorded = match compaction {
        Some((keep, .., ref summary)) => state.store.record_compacted_exchange(
            &session.session_id,
            &req.message,
            &reply.body_text,
            &reply.model,
            &usage,
            &prompt_skills,
            keep,
            summary,
        ),
        None => state.store.record_exchange(
            &session.session_id,
            &req.message,
            &reply.body_text,
            &reply.model,
            &usage,
            &prompt_skills,
        ),
    };
    let updated = recorded.map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
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
