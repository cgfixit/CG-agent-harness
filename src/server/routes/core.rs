//! Status, sessions, soul/model toggles, chat + cancel.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::net::SocketAddr;

use crate::common::tool_broker::assert_allowed;
use crate::llm::openai_chat::ChatMessage;
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::guards::retry_after_error;
use crate::server::prompts::{compose_system_prompt, PromptInputs};
use crate::server::schemas::*;
use crate::server::sessions::TokenTally;
use crate::server::state::{AppState, HARNESS_LOOP_TOOL};

const HISTORY_TURNS: usize = 20;
const LOOP_HISTORY_TURNS: usize = 8;
const LOOP_HISTORY_CHARS: usize = 4000;
const CHAT_HISTORY_CHARS: usize = 8000;
const DEFAULT_MAX_TOKENS: u64 = 4096;
const DEFAULT_TEMPERATURE: f64 = 0.3;

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let sessions = state.store.list();
    let total_tokens: u64 = sessions.iter().filter_map(|s| s["tokens"]["total"].as_u64()).sum();
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Json(json!({
        "version": crate::VERSION,
        "model": state.current_model(),
        "provider": state.backend.provider,
        "api_key_optional": state.api_key_optional,
        "base_url": state.backend.base_url,
        "soul_enabled": settings.soul_enabled,
        "soul": soul_status(&state, settings.soul_enabled),
        "memory_enabled": settings.memory_enabled,
        "home": state.home.root.display().to_string(),
        "repo_root": Value::Null,
        "chat_mode": "conversation",
        "chat_tools_available": false,
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
        state.cfg.u64_or("personality.soul_max_chars", 8000) as usize
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
    Ok(Json(json!({"model": state.current_model()})))
}

/// Keep the newest prior turns that fit `max_chars`; the newest message's tail survives.
pub fn clip_history(messages: &[ChatMessage], max_chars: usize) -> Vec<ChatMessage> {
    if max_chars == 0 {
        return Vec::new();
    }
    let mut clipped: Vec<ChatMessage> = Vec::new();
    let mut used = 0usize;
    for msg in messages.iter().rev() {
        if used >= max_chars {
            break;
        }
        let room = max_chars - used;
        let count = msg.content.chars().count();
        let text: String = if count > room {
            msg.content.chars().skip(count - room).collect()
        } else {
            msg.content.clone()
        };
        used += text.chars().count();
        clipped.push(ChatMessage {
            role: msg.role.clone(),
            content: text,
        });
    }
    clipped.reverse();
    clipped
}

fn loop_error(code: &str, message: &str) -> ApiError {
    ApiError::bad_request(code, message)
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ValidJson(req): ValidJson<ChatRequest>,
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
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let selected_skills = super::skills::resolve(&state, &session.selected_skills)?;

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

    let Some(_gate) = state.generation_gate.claim("chat") else {
        drop(release);
        let mut details = json!({"session_id": session.session_id, "timeout_sec": state.chat.timeout_sec as u64});
        if state.generation_gate.owner() == "chat" {
            details["cancel"] = json!("/api/chat/cancel");
        }
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "CHAT_BUSY",
            "a local model turn is already running",
        )
        .details(details));
    };

    let web_context = state.web.context_text();
    let memory_context = if settings.memory_enabled {
        Some(state.notes.context_text())
    } else {
        None
    };
    let system_prompt = compose_system_prompt(&PromptInputs {
        selected_skills: &selected_skills,
        soul_enabled: settings.soul_enabled,
        soul_override: None,
        soul_path: &state.home.soul_path(),
        soul_max_chars: state.cfg.u64_or("personality.soul_max_chars", 8000) as usize,
        goal: Some(&session.goal),
        web_context: Some(&web_context),
        memory_context: memory_context.as_deref(),
    });
    let (turns, chars) = if req.loop_turn {
        (LOOP_HISTORY_TURNS, LOOP_HISTORY_CHARS)
    } else {
        (HISTORY_TURNS, CHAT_HISTORY_CHARS)
    };
    let prior: Vec<ChatMessage> = session
        .messages
        .iter()
        .rev()
        .take(turns)
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.text.clone(),
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut history = clip_history(&prior, chars);
    history.push(ChatMessage {
        role: "user".into(),
        content: req.message.clone(),
    });
    let max_tokens = if req.loop_turn {
        state.loop_max_tokens
    } else {
        state.cfg.u64_or("models.local_llm.max_tokens", DEFAULT_MAX_TOKENS)
    };
    let temperature = state.cfg.f64_or("models.local_llm.temperature", DEFAULT_TEMPERATURE);
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| state.current_model());
    let reply = state
        .chat
        .chat(&system_prompt, &history, Some(&model), max_tokens, temperature)
        .await
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    drop(release);

    let updated = state
        .store
        .record_exchange(
            &session.session_id,
            &req.message,
            &reply.body_text,
            &reply.model,
            &TokenTally {
                prompt_tokens: reply.prompt_tokens,
                completion_tokens: reply.completion_tokens,
                exchanges: 0,
            },
            &selected_skills.iter().map(|(id,body)| {
                use sha2::{Digest, Sha256};
                json!({"id":id,"outcome":"included_in_successful_chat","chars":body.chars().count(),"sha256":hex::encode(Sha256::digest(body.as_bytes()))})
            }).collect::<Vec<_>>(),
        )
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(json!({
        "session_id": session.session_id,
        "reply": reply.body_text,
        "model": reply.model,
        "usage": {"prompt_tokens": reply.prompt_tokens, "completion_tokens": reply.completion_tokens},
        "tally": updated.tally.to_json(),
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

pub async fn cancel_chat(State(state): State<Arc<AppState>>) -> Json<Value> {
    state.chat.abort_in_flight();
    Json(json!({"cancelled": true}))
}
