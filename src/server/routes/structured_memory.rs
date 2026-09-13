//! Guarded structured-memory facts and proposals. Suggest ≠ mutate.
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::common::auth_store::UserSummary;
use crate::common::errors::HarnessError;
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{
    StructuredFactAddRequest, StructuredFactDeactivateRequest, StructuredMemoryDecisionRequest,
    StructuredMemoryProposeRequest, ValidJson,
};
use crate::server::state::AppState;
use crate::server::structured_memory::{
    disabled_status, enabled_status, valid_public_id, Limits, ProposalDraft, StructuredMemoryStore,
};

type PrivateJson = ([(header::HeaderName, &'static str); 1], Json<Value>);

fn private(value: Value) -> PrivateJson {
    ([(header::CACHE_CONTROL, crate::server::headers::NO_STORE)], Json(value))
}

fn error(code: &str, message: &str) -> ApiError {
    ApiError::bad_request(code, message)
}

fn store_err(err: &HarnessError) -> ApiError {
    let status = match err.code.as_str() {
        "STRUCTURED_MEMORY_IO" => StatusCode::BAD_GATEWAY,
        "STRUCTURED_MEMORY_CHANGED" | "STRUCTURED_MEMORY_PROPOSAL_CHANGED" | "STRUCTURED_MEMORY_DISABLED" => {
            StatusCode::CONFLICT
        }
        "STRUCTURED_MEMORY_NOT_FOUND" | "STRUCTURED_MEMORY_PROPOSAL" => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::from_err(status, err)
}

fn require_confirm(confirm: bool) -> ApiResult<()> {
    if confirm {
        Ok(())
    } else {
        Err(error(
            "STRUCTURED_MEMORY_CONFIRM",
            "Explicit confirmation is required to mutate structured memory",
        ))
    }
}

fn owner(user: Option<axum::Extension<UserSummary>>) -> String {
    super::auth::context_owner(user)
}

fn require_store(state: &AppState) -> ApiResult<&StructuredMemoryStore> {
    state.structured_memory.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "STRUCTURED_MEMORY_DISABLED",
            "structured memory is disabled; models may not create a store or mutate facts",
        )
    })
}

fn audit(state: &AppState, event: &str, owner: &str, extra: Value) {
    let mut payload = json!({"event": event, "owner_id": owner});
    if let Some(map) = extra.as_object() {
        for (key, value) in map {
            payload[key] = value.clone();
        }
    }
    state.audit.log(payload);
}

pub fn status_payload(state: &AppState, owner_id: &str) -> ApiResult<Value> {
    let limits = Limits::from_config(&state.cfg);
    if let Some(store) = state.structured_memory.as_ref() {
        let (facts, pending) = store.counts(owner_id).map_err(|e| store_err(&e))?;
        Ok(enabled_status(owner_id, &store.limits(), facts, pending))
    } else {
        Ok(disabled_status(owner_id, &limits))
    }
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    Ok(private(status_payload(&state, &owner(user))?))
}

pub async fn list_facts(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let facts: Vec<Value> = store
        .list_facts(&owner)
        .map_err(|e| store_err(&e))?
        .into_iter()
        .map(|f| f.to_json())
        .collect();
    Ok(private(
        json!({"owner_id": owner, "facts": facts, "count": facts.len()}),
    ))
}

pub async fn get_fact(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    if !valid_public_id(&id) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "STRUCTURED_MEMORY_NOT_FOUND",
            "unknown fact",
        ));
    }
    let store = require_store(&state)?;
    Ok(private(
        store.get_fact(&owner, &id).map_err(|e| store_err(&e))?.to_json(),
    ))
}

pub async fn add_fact(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StructuredFactAddRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let fact = store
        .add_fact(&owner, &req.content, req.category.as_deref().unwrap_or(""), &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_fact_added",
        &owner,
        json!({"id": fact.id, "revision": fact.revision, "content_chars": fact.content.chars().count()}),
    );
    Ok(private(fact.to_json()))
}

pub async fn deactivate_fact(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<StructuredFactDeactivateRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let fact = store
        .deactivate_fact(&owner, &id, req.expected_revision, &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_fact_deactivated",
        &owner,
        json!({"id": fact.id, "revision": fact.revision, "content_chars": fact.content.chars().count()}),
    );
    Ok(private(fact.to_json()))
}

pub async fn list_proposals(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let proposals: Vec<Value> = store
        .list_proposals(&owner)
        .map_err(|e| store_err(&e))?
        .into_iter()
        .map(|p| p.to_json())
        .collect();
    Ok(private(
        json!({"owner_id": owner, "proposals": proposals, "count": proposals.len()}),
    ))
}

pub async fn propose(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StructuredMemoryProposeRequest>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let proposal = store
        .create_proposal(
            &owner,
            ProposalDraft {
                action: &req.action,
                content: req.content.as_deref(),
                category: req.category.as_deref(),
                target_fact_id: req.target_fact_id.as_deref(),
                expected_revision: req.expected_revision,
                expected_digest: req.expected_digest.as_deref(),
            },
        )
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_proposal_created",
        &owner,
        json!({
            "id": proposal.id,
            "action": proposal.action,
            "revision": proposal.revision,
            "status": proposal.status
        }),
    );
    Ok(private(proposal.to_json()))
}

pub async fn review(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    Ok(private(
        store.get_proposal(&owner, &id).map_err(|e| store_err(&e))?.to_json(),
    ))
}

pub async fn decide(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<StructuredMemoryDecisionRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let proposal = store
        .decide_proposal(&owner, &id, &req.revision, req.apply, &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_proposal_decision",
        &owner,
        json!({"id": proposal.id, "status": proposal.status, "action": proposal.action}),
    );
    Ok(private(proposal.to_json()))
}
