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

pub async fn status(State(state): State<Arc<AppState>>, user: Caller) -> Json<Value> {
    Json(state.jobs.notification_status(&super::auth::context_owner(user)))
}
pub async fn replay(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(id): Path<String>,
    ValidJson(body): ValidJson<StructuredMemoryReasonRequest>,
) -> ApiResult<Json<Value>> {
    if !body.confirm || body.reason.trim().is_empty() {
        return Err(ApiError::bad_request(
            "CONFIRM_REQUIRED",
            "Replay requires explicit confirmation and a reason",
        ));
    }
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(ApiError::bad_request(
            "INVALID_DELIVERY_ID",
            "delivery_id must be 64 lowercase hex characters",
        ));
    }
    let owner = super::auth::context_owner(user);
    state.jobs.replay_notification(&owner, &id).map_err(|e| {
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
}
