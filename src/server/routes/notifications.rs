//! Guarded owner-scoped delivery status and explicit bounded replay.
use crate::server::{
    errors::{ApiError, ApiResult},
    schemas::{StructuredMemoryReasonRequest, ValidJson},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

type Caller = Option<axum::Extension<crate::common::auth_store::UserSummary>>;

/// Status and replay reread authority files and replay fsyncs the outbox, so
/// both run on the blocking pool rather than an async worker.
pub async fn status(State(state): State<Arc<AppState>>, user: Caller) -> Json<Value> {
    let owner = super::auth::context_owner(user);
    super::structured_memory::off_worker(move || Json(state.jobs.notification_status(&owner))).await
}
pub async fn replay(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(id): Path<String>,
    ValidJson(body): ValidJson<StructuredMemoryReasonRequest>,
) -> ApiResult<Json<Value>> {
    if !body.confirm || crate::common::is_visibly_blank(&body.reason) {
        return Err(ApiError::bad_request(
            "CONFIRM_REQUIRED",
            "Replay requires explicit confirmation and a reason",
        ));
    }
    if !crate::common::is_lower_hex(&id, 64) {
        return Err(ApiError::bad_request(
            "INVALID_DELIVERY_ID",
            "delivery_id must be 64 lowercase hex characters",
        ));
    }
    let owner = super::auth::context_owner(user);
    let reason_chars = body.reason.trim().chars().count();
    super::structured_memory::off_worker(move || {
        state.jobs.replay_notification(&owner, &id, reason_chars).map_err(|e| {
            ApiError::from_err(
                if e.code == "DELIVERY_NOT_FOUND" {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::CONFLICT
                },
                &e,
            )
        })?;
        Ok(Json(json!({"delivery_id":id,"state":"pending","job_restarted":false})))
    })
    .await
}
