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
use crate::server::session_export::{self, write_export, ExportAttachmentPin};
use crate::server::session_search::search_sessions;
use crate::server::state::AppState;

fn export_pins_for_owner(
    session: &crate::server::sessions::Session,
    owner: &str,
    attachments: &crate::server::attachments::AttachmentStore,
) -> Vec<ExportAttachmentPin> {
    session
        .attachment_pins
        .iter()
        .filter(|pin| pin.owner == owner)
        .map(|pin| match attachments.owned_blob(owner, &pin.id) {
            Some(blob) => ExportAttachmentPin {
                id: pin.id.clone(),
                magic_mime: blob.magic_mime,
                sha256_prefix: blob.sha256.chars().take(12).collect(),
                omitted: false,
            },
            None => ExportAttachmentPin {
                id: pin.id.clone(),
                magic_mime: String::new(),
                sha256_prefix: String::new(),
                omitted: true,
            },
        })
        .collect()
}

pub async fn export_session(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    Path(session_id): Path<String>,
) -> Result<Response, ApiError> {
    let id = crate::server::sessions::canonical_session_id(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let session = state
        .store
        .get(&id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    if session.session_id != id {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "HARNESS_SESSION_ERROR",
            "session id mismatch",
        ));
    }
    let owner = super::auth::context_owner(user);
    let pins = export_pins_for_owner(&session, &owner, &state.attachments);
    write_export(&state.home, &session, pins.clone()).map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    let body = session_export::render(&session, pins);
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
