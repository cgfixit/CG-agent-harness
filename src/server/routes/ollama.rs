//! Live Ollama inventory, abortable pull, and keep_alive warmup status.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use axum::Json;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::llm::ollama::{self, PullLimits};
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{OllamaPullRequest, ValidJson};
use crate::server::state::AppState;

pub async fn inventory(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let refresh = ollama::clamped(&state.cfg, "models.local_llm.inventory.refresh_sec", 30, 1, 3600);
    if let Some(cached) = state.ollama.cached_inventory(refresh) {
        return Ok(Json(cached));
    }
    let value = refresh_inventory(&state).await?;
    Ok(Json(value))
}

async fn refresh_inventory(state: &AppState) -> ApiResult<Value> {
    if state.backend.provider != "ollama" {
        let value = json!({
            "endpoint": Value::Null,
            "configured_model": state.backend.model,
            "configured_state": "not_probed",
            "models": [],
            "detail": "Native Ollama inventory is available only for provider ollama.",
        });
        state.ollama.store_inventory(value.clone());
        return Ok(value);
    }
    let value = ollama::snapshot(
        &state.backend.base_url,
        &state.backend.model,
        &state.backend.api_key,
        &state.cfg,
    )
    .await
    .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    state.ollama.store_inventory(value.clone());
    Ok(value)
}

pub async fn pull(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ValidJson(req): ValidJson<OllamaPullRequest>,
) -> Response {
    let streaming = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|part| part.trim() == "text/event-stream"));
    if !streaming {
        return pull_inner(state, req, None).await.into_response();
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Value>(16);
    let task = tokio::spawn(async move {
        let result = pull_inner(state, req, Some(&sender)).await;
        let event = match result {
            Ok(Json(data)) => json!({"type":"done","data":data}),
            Err(error) => {
                json!({"type":"error","status":error.status.as_u16(),"error":error.body()})
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

async fn pull_inner(
    state: Arc<AppState>,
    req: OllamaPullRequest,
    sender: Option<&tokio::sync::mpsc::Sender<Value>>,
) -> ApiResult<Json<Value>> {
    if state.backend.provider != "ollama" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "OLLAMA_PROVIDER_REQUIRED",
            "native Ollama pull requires models.local_llm.provider ollama",
        ));
    }
    let Some(native) = ollama::native_base_url(&state.backend.base_url) else {
        return Err(ApiError::bad_request(
            ollama::OLLAMA_ERROR,
            "configured model endpoint is not a loopback Ollama origin",
        ));
    };
    let Some(_gate) = state.ollama.pull_gate.claim("ollama-pull") else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "OLLAMA_PULL_BUSY",
            "an Ollama pull is already running",
        )
        .details(json!({"cancel":"/api/ollama/pull/cancel"})));
    };
    let cancel = CancellationToken::new();
    state.ollama.register_pull(cancel.clone());
    struct ClearAbort(Arc<AppState>);
    impl Drop for ClearAbort {
        fn drop(&mut self) {
            let _ = self.0.ollama.abort_pull();
        }
    }
    let _clear = ClearAbort(state.clone());
    state
        .audit
        .log(json!({"event":"ollama_pull_started","model":req.model}));
    let limits = PullLimits::from_config(&state.cfg);
    let send = sender.cloned();
    let result = ollama::pull_model(&native, &req.model, limits, cancel.clone(), |progress| {
        if let Some(tx) = &send {
            let _ = tx.try_send(json!({"type":"progress","data":progress}));
        }
    })
    .await;
    match result {
        Ok(()) => {
            state.ollama.invalidate_inventory();
            let inventory = refresh_inventory(&state)
                .await
                .unwrap_or_else(|_| json!({"configured_state":"installed"}));
            state
                .audit
                .log(json!({"event":"ollama_pull_finished","model":req.model,"ok":true}));
            Ok(Json(json!({
                "model": req.model,
                "state": "installed",
                "inventory": inventory,
            })))
        }
        Err(err) if err.message.contains("aborted") => {
            state
                .audit
                .log(json!({"event":"ollama_pull_aborted","model":req.model}));
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "OLLAMA_PULL_ABORTED",
                "the Ollama pull was aborted",
            ))
        }
        Err(_) => {
            state
                .audit
                .log(json!({"event":"ollama_pull_finished","model":req.model,"ok":false}));
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                ollama::OLLAMA_ERROR,
                "the Ollama pull did not complete",
            ))
        }
    }
}

pub async fn cancel_pull(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let cancelled = state.abort_ollama_pull();
    Ok(Json(json!({"cancelled": cancelled})))
}
