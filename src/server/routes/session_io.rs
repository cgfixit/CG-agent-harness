//! Guarded session Markdown export and transcript search.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::headers::NO_STORE;
use crate::server::schemas::{SessionSearchRequest, ValidJson};
use crate::server::session_export::{self, write_export};
use crate::server::session_search::search_sessions;
use crate::server::state::AppState;

pub async fn export_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Response, ApiError> {
    let session = state
        .store
        .get(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    write_export(&state.home, &session).map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    let body = session_export::render(&session);
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    resp.headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(resp)
}

pub async fn search_session_transcripts(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<SessionSearchRequest>,
) -> ApiResult<Json<Value>> {
    let hits =
        search_sessions(&state.store, &req.query).map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    Ok(Json(json!({
        "index": "tantivy-bm25",
        "hits": hits.iter().map(|h| json!({
            "session_id": h.session_id,
            "title": h.title,
            "snippet": h.snippet,
            "role": h.role,
            "ts": h.ts,
            "score": h.score,
        })).collect::<Vec<_>>(),
    })))
}
