//! Ingest and list owner-jailed notes. Local chat retrieval only.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::attachments;
use crate::server::errors::ApiResult;
use crate::server::notes_corpus;
use crate::server::state::AppState;

pub async fn list_notes(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let notes = state
        .notes_corpus
        .list_for_owner(&owner)
        .map_err(|e| notes_corpus::ingest_error(&e))?;
    Ok(Json(notes_corpus::list_json(&notes)))
}

pub async fn ingest_notes(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    req: axum::extract::Request,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let Ok(permit) = state.notes_ingest_permits.clone().try_acquire_owned() else {
        return Err(notes_corpus::ingest_error(&crate::common::errors::HarnessError::new(
            "NOTES_BUSY",
            "too many notes ingests are in flight; retry shortly",
        )));
    };
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let limits = state.notes_corpus.limits();
    let limit = limits.max_request_bytes();
    let max_files = limits.max_files_per_request;
    let bytes = axum::body::to_bytes(req.into_body(), limit.saturating_add(1))
        .await
        .map_err(|_| {
            notes_corpus::ingest_error(&crate::common::errors::HarnessError::new(
                "NOTES_TOO_LARGE",
                "request body exceeds the notes request limit",
            ))
        })?;
    let boundary = attachments::multipart_boundary(&content_type).map_err(|e| notes_corpus::ingest_error(&e))?;
    let st = state.clone();
    let task_owner = owner.clone();
    let stored = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let files = match attachments::parse_multipart_limited(&bytes, &boundary, max_files) {
            Ok(files) => files,
            Err(e) if e.code == "ATTACHMENT_TOO_MANY" => {
                return Err(crate::common::errors::HarnessError::new(
                    "NOTES_TOO_MANY",
                    "too many notes files in this request",
                )
                .detail("limit", max_files as u64));
            }
            Err(e) => return Err(e),
        };
        st.notes_corpus.store(&task_owner, &files)
    })
    .await
    .map_err(|_| {
        notes_corpus::ingest_error(&crate::common::errors::HarnessError::new(
            "IO_ERROR",
            "notes ingest worker failed",
        ))
    })?
    .map_err(|e| notes_corpus::ingest_error(&e))?;
    Ok(Json(notes_corpus::list_json(&stored)))
}

#[derive(Deserialize)]
pub struct NoteId {
    id: String,
}

pub async fn delete_note(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    Path(NoteId { id }): Path<NoteId>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    state
        .notes_corpus
        .unlink_id(&owner, &id)
        .map_err(|e| notes_corpus::ingest_error(&e))?;
    Ok(Json(json!({"ok": true, "id": id})))
}
