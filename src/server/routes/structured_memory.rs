//! Guarded structured-memory facts and proposals. Suggest ≠ mutate.
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::common::auth_store::UserSummary;
use crate::common::errors::HarnessError;
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::schemas::{
    StructuredConsolidationStartRequest, StructuredEpisodeSummaryRequest, StructuredFactAddRequest,
    StructuredFactDeactivateRequest, StructuredFactSelectRequest, StructuredMemoryDecisionRequest,
    StructuredMemoryProposeRequest, StructuredMemoryReasonRequest, ValidJson,
};
use crate::server::state::AppState;
use crate::server::structured_memory::{
    assemble_retrieval_facts, assemble_selected_facts, auto_consolidation_available, auto_retrieval_available,
    capture_available, consolidation_available, current_gates, disabled_status, enabled_status, merge_recall,
    recall_available, retrieval_available, valid_public_id, EpisodeDraft, FactSelection, Limits, ProposalDraft,
    RetrievalIntent, StructuredMemoryStore,
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
        "STRUCTURED_MEMORY_CHANGED"
        | "STRUCTURED_MEMORY_PROPOSAL_CHANGED"
        | "STRUCTURED_MEMORY_DISABLED"
        | "STRUCTURED_MEMORY_BUSY" => StatusCode::CONFLICT,
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

pub fn prompt_memory(
    state: &AppState,
    owner: &str,
    memory_enabled: bool,
    selections: &[FactSelection],
    intent: RetrievalIntent<'_>,
) -> (
    String,
    String,
    crate::server::prompts::MemoryBudget,
    crate::server::structured_memory::RecallResult,
    Option<String>,
) {
    let limits = match state.structured_memory.as_ref() {
        Some(store) => store.limits(),
        None => Limits::from_config(&state.cfg),
    };
    let pinned = if memory_enabled {
        state.notes.context_text()
    } else {
        String::new()
    };
    let gates = current_gates(state);
    let selected = assemble_selected_facts(state.structured_memory.as_ref(), &state.cfg, owner, selections, &gates);
    let (retrieved, retrieval_error) =
        assemble_retrieval_facts(state.structured_memory.as_ref(), &state.cfg, owner, &intent, &gates);
    let max = limits.max_selected_facts.max(limits.max_retrieval_results);
    let recalled = merge_recall(selected, retrieved, max);
    let facts = crate::server::structured_memory::format_selected_facts(&recalled.injected);
    let budget = crate::server::prompts::MemoryBudget::from_limits(
        limits.pinned_prompt_chars,
        limits.selected_fact_prompt_chars,
    );
    (pinned, facts, budget, recalled, retrieval_error)
}

pub fn selections_from_items(items: &[crate::server::schemas::StructuredFactSelection]) -> Vec<FactSelection> {
    items
        .iter()
        .map(|item| FactSelection {
            id: item.id.clone(),
            expected_revision: item.expected_revision,
        })
        .collect()
}

pub fn status_payload(state: &AppState, owner_id: &str) -> ApiResult<Value> {
    let limits = Limits::from_config(&state.cfg);
    let gates = current_gates(state);
    if let Some(store) = state.structured_memory.as_ref() {
        let (facts, pending) = store.counts(owner_id).map_err(|e| store_err(&e))?;
        let episodes = store.episode_count(owner_id).map_err(|e| store_err(&e))?;
        let mut payload = enabled_status(
            owner_id,
            &store.limits(),
            facts,
            pending,
            capture_available(&state.cfg, true, &gates),
            episodes,
            &store.episode_health(),
        );
        payload["explicit_recall"] = json!(recall_available(&state.cfg, true, &gates));
        payload["retrieval"] = json!(retrieval_available(&state.cfg, true, &gates));
        payload["auto_retrieval"] = json!(auto_retrieval_available(&state.cfg, true, &gates));
        payload["consolidation"] = json!(consolidation_available(&state.cfg, true, &gates));
        payload["auto_consolidation"] = json!(auto_consolidation_available(&state.cfg, true, &gates));
        payload["retrieval_health"] = store.retrieval_health().as_json();
        if let Ok(Some(run)) = store.running_consolidation(owner_id) {
            payload["consolidation_health"] = json!({
                "running": true,
                "run_id": run.id,
                "state": run.state,
                "error_class": Value::Null,
            });
        } else if let Ok(runs) = store.list_consolidation_runs(owner_id) {
            payload["consolidation_health"] = json!({
                "running": false,
                "run_id": runs.first().map(|r| r.id.clone()),
                "state": runs.first().map(|r| r.state.clone()),
                "error_class": runs.first().and_then(|r| r.error_class.clone()),
            });
        }
        payload["operator_gates"] = gates.as_json();
        Ok(payload)
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

#[derive(Debug, Default, serde::Deserialize)]
pub struct FactListQuery {
    q: Option<String>,
    category: Option<String>,
    limit: Option<u64>,
}

pub async fn list_facts(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Query(query): Query<FactListQuery>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let limits = store.limits();
    if query
        .q
        .as_ref()
        .is_some_and(|q| q.chars().count() > limits.max_search_query_chars)
    {
        return Err(error(
            "STRUCTURED_MEMORY_SEARCH",
            "search query exceeds the configured character bound",
        ));
    }
    let limit = query
        .limit
        .map(|n| n as usize)
        .unwrap_or(limits.max_search_results)
        .min(limits.max_search_results);
    let searching = query.q.as_ref().is_some_and(|q| !q.trim().is_empty())
        || query.category.as_ref().is_some_and(|c| !c.trim().is_empty())
        || query.limit.is_some();
    let facts = if searching {
        store.search_facts(&owner, query.q.as_deref(), query.category.as_deref(), limit)
    } else {
        store.list_facts(&owner)
    }
    .map_err(|e| store_err(&e))?;
    let payload: Vec<Value> = facts.into_iter().map(|f| f.to_json()).collect();
    Ok(private(json!({
        "owner_id": owner,
        "facts": payload,
        "count": payload.len(),
        "search": searching,
        "retrieval": false,
        "fts": false,
    })))
}

pub async fn selection(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(session_id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let session = state
        .store
        .get(&session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let gates = current_gates(&state);
    let available = recall_available(&state.cfg, state.structured_memory.is_some(), &gates);
    let recalled = assemble_selected_facts(
        state.structured_memory.as_ref(),
        &state.cfg,
        &owner,
        &session.selected_facts,
        &gates,
    );
    Ok(private(json!({
        "session_id": session.session_id,
        "owner_id": owner,
        "explicit_recall": available,
        "selected": session.selected_facts.iter().map(|f| json!({"id": f.id, "expected_revision": f.expected_revision})).collect::<Vec<_>>(),
        "injected": recalled.preview_json()["injected"],
        "dropped": recalled.preview_json()["dropped"],
        "type": "untrusted_background_context",
        "scope": "explicit selection only; never authorizes tools, coding, or network",
    })))
}

pub async fn select(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(session_id): Path<String>,
    ValidJson(req): ValidJson<StructuredFactSelectRequest>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let max = store.limits().max_selected_facts;
    if req.facts.len() > max {
        return Err(error(
            "STRUCTURED_MEMORY_SELECTION",
            "too many selected facts for this configuration",
        ));
    }
    let mut selected = Vec::new();
    for item in &req.facts {
        if !valid_public_id(&item.id) || item.expected_revision < 1 {
            return Err(error(
                "STRUCTURED_MEMORY_SELECTION",
                "each selected fact needs a stable public id and expected revision",
            ));
        }
        selected.push(FactSelection {
            id: item.id.clone(),
            expected_revision: item.expected_revision,
        });
    }
    state
        .store
        .select_facts(&session_id, &selected)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    audit(
        &state,
        "structured_memory_facts_selected",
        &owner,
        json!({
            "session_id": session_id,
            "count": selected.len(),
            "ids": selected.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        }),
    );
    let gates = current_gates(&state);
    let recalled = assemble_selected_facts(Some(store), &state.cfg, &owner, &selected, &gates);
    Ok(private(json!({
        "session_id": session_id,
        "owner_id": owner,
        "explicit_recall": recall_available(&state.cfg, true, &gates),
        "selected": selected.iter().map(|f| json!({"id": f.id, "expected_revision": f.expected_revision})).collect::<Vec<_>>(),
        "injected": recalled.preview_json()["injected"],
        "dropped": recalled.preview_json()["dropped"],
        "effect": "included in subsequent chat and /prompt preview only when explicit_recall is on; no canonical fact mutation",
    })))
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
                source_episode_ids: &req.source_episode_ids,
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

pub fn stage_after_exchange(
    state: &AppState,
    owner: &str,
    model: &str,
    user_chars: usize,
    assistant_chars: usize,
    sensitivity: &str,
) -> Value {
    let gates = current_gates(state);
    if !capture_available(&state.cfg, state.structured_memory.is_some(), &gates) {
        return json!({"available": false, "staged": false});
    }
    let Some(store) = state.structured_memory.as_ref() else {
        return json!({"available": false, "staged": false});
    };
    match store.stage_episode(
        owner,
        EpisodeDraft {
            model_id: if model.is_empty() { "unknown" } else { model },
            outcome: "completed",
            user_chars,
            assistant_chars,
            sensitivity,
        },
    ) {
        Ok(episode) => {
            audit(
                state,
                "structured_memory_episode_staged",
                owner,
                json!({
                    "id": episode.id,
                    "outcome": episode.outcome,
                    "summary_chars": episode.privacy_summary.chars().count(),
                    "byte_len": episode.byte_len
                }),
            );
            json!({
                "available": true,
                "staged": true,
                "id": episode.id,
                "health": {"ok": true, "error_class": Value::Null}
            })
        }
        Err(err) => {
            audit(
                state,
                "structured_memory_episode_stage_failed",
                owner,
                json!({"error_class": err.code}),
            );
            json!({
                "available": true,
                "staged": false,
                "health": {"ok": false, "error_class": err.code}
            })
        }
    }
}

pub async fn list_episodes(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let episodes: Vec<Value> = store
        .list_episodes(&owner)
        .map_err(|e| store_err(&e))?
        .into_iter()
        .map(|e| e.to_json())
        .collect();
    Ok(private(json!({
        "owner_id": owner,
        "episodes": episodes,
        "count": episodes.len(),
        "available": capture_available(&state.cfg, true, &current_gates(&state))
    })))
}

pub async fn search_facts(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Query(query): Query<FactListQuery>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let gates = current_gates(&state);
    if !retrieval_available(&state.cfg, true, &gates) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "STRUCTURED_MEMORY_RETRIEVAL_DISABLED",
            "structured retrieval is disabled; FTS search requires the retrieval gate",
        ));
    }
    let q = query.q.as_deref().unwrap_or("").trim();
    if q.chars().count() > store.limits().max_search_query_chars {
        return Err(error(
            "STRUCTURED_MEMORY_SEARCH",
            "search query exceeds the configured character bound",
        ));
    }
    let limit = query
        .limit
        .map(|n| n as usize)
        .unwrap_or(store.limits().max_retrieval_results)
        .min(store.limits().max_search_results);
    let hits = match store.search_facts_fts(&owner, q, limit) {
        Ok(hits) => hits,
        Err(err) if err.code == "STRUCTURED_MEMORY_FTS" => {
            return Ok(private(json!({
                "owner_id": owner,
                "hits": [],
                "count": 0,
                "retrieval": true,
                "fts": true,
                "health": store.retrieval_health().as_json(),
                "error_class": err.code,
            })));
        }
        Err(err) => return Err(store_err(&err)),
    };
    let payload: Vec<Value> = hits.iter().map(|h| h.to_json()).collect();
    audit(
        &state,
        "structured_memory_fts_search",
        &owner,
        json!({"count": payload.len(), "query_chars": q.chars().count()}),
    );
    Ok(private(json!({
        "owner_id": owner,
        "hits": payload,
        "count": payload.len(),
        "retrieval": true,
        "fts": true,
        "health": store.retrieval_health().as_json(),
        "type": "untrusted_background_context",
        "scope": "search candidates only; not injected unless retrieve/auto_retrieval picks them",
    })))
}

pub async fn set_gates(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<crate::server::schemas::StructuredMemoryGateRequest>,
) -> ApiResult<PrivateJson> {
    let _store = require_store(&state)?;
    let owner = owner(user);
    let snapshot = {
        let mut gates = state.structured_gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.set(&req.gate, req.enabled).map_err(|e| store_err(&e))?;
        gates.clone()
    };
    snapshot
        .save(&state.home)
        .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?;
    if req.gate == "retrieval" && req.enabled {
        if let Some(store) = state.structured_memory.as_ref() {
            if let Err(err) = store.rebuild_facts_fts() {
                audit(
                    &state,
                    "structured_memory_fts_backfill_failed",
                    &owner,
                    json!({"error_class": err.code}),
                );
            }
        }
    }
    audit(
        &state,
        "structured_memory_gate_set",
        &owner,
        json!({"gate": req.gate, "enabled": req.enabled}),
    );
    Ok(private(json!({
        "owner_id": owner,
        "operator_gates": snapshot.as_json(),
        "episode_capture": capture_available(&state.cfg, true, &snapshot),
        "explicit_recall": recall_available(&state.cfg, true, &snapshot),
        "retrieval": retrieval_available(&state.cfg, true, &snapshot),
        "auto_retrieval": auto_retrieval_available(&state.cfg, true, &snapshot),
        "consolidation": consolidation_available(&state.cfg, true, &snapshot),
        "auto_consolidation": auto_consolidation_available(&state.cfg, true, &snapshot),
        "memory_on_unchanged": true,
    })))
}

pub async fn get_episode(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    if !valid_public_id(&id) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "STRUCTURED_MEMORY_NOT_FOUND",
            "unknown episode",
        ));
    }
    let store = require_store(&state)?;
    Ok(private(
        store.get_episode(&owner, &id).map_err(|e| store_err(&e))?.to_json(),
    ))
}

pub async fn summarize_episode(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<StructuredEpisodeSummaryRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let episode = store
        .set_episode_summary(&owner, &id, &req.summary, &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_episode_summarized",
        &owner,
        json!({"id": episode.id, "summary_chars": episode.semantic_summary.as_ref().map(|s| s.chars().count()).unwrap_or(0)}),
    );
    Ok(private(episode.to_json()))
}

pub async fn delete_episode(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<StructuredMemoryReasonRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    store
        .delete_episode(&owner, &id, &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(&state, "structured_memory_episode_deleted", &owner, json!({"id": id}));
    Ok(private(json!({"deleted": id, "owner_id": owner})))
}

pub async fn purge_expired(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StructuredMemoryReasonRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let deleted = store
        .purge_expired_episodes(&owner, &req.reason)
        .map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_episodes_purged",
        &owner,
        json!({"deleted": deleted}),
    );
    Ok(private(json!({"owner_id": owner, "deleted": deleted})))
}

pub async fn purge_owner(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StructuredMemoryReasonRequest>,
) -> ApiResult<PrivateJson> {
    require_confirm(req.confirm)?;
    let owner = owner(user);
    let store = require_store(&state)?;
    let body = store.purge_owner(&owner, &req.reason).map_err(|e| store_err(&e))?;
    audit(
        &state,
        "structured_memory_owner_purged",
        &owner,
        json!({"counts": body["deleted"]}),
    );
    Ok(private(body))
}

pub async fn export_owner(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<axum::response::Response> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let export = store.export_owner(&owner).map_err(|e| store_err(&e))?;
    let html = export.html();
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, crate::server::headers::NO_STORE)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(axum::body::Body::from(html))
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty())))
}

fn require_consolidation(state: &AppState) -> ApiResult<&StructuredMemoryStore> {
    let store = require_store(state)?;
    let gates = current_gates(state);
    if !consolidation_available(&state.cfg, true, &gates) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "STRUCTURED_MEMORY_DISABLED",
            "structured consolidation is disabled; selected episodes are not summarized",
        ));
    }
    Ok(store)
}

pub async fn list_consolidation(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_store(&state)?;
    let gates = current_gates(&state);
    let runs: Vec<Value> = store
        .list_consolidation_runs(&owner)
        .map_err(|e| store_err(&e))?
        .into_iter()
        .map(|r| r.to_json())
        .collect();
    Ok(private(json!({
        "owner_id": owner,
        "runs": runs,
        "count": runs.len(),
        "consolidation": consolidation_available(&state.cfg, true, &gates),
        "auto_consolidation": false,
    })))
}

pub async fn get_consolidation(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    if !valid_public_id(&id) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "STRUCTURED_MEMORY_NOT_FOUND",
            "unknown consolidation run",
        ));
    }
    let store = require_store(&state)?;
    Ok(private(
        store
            .get_consolidation_run(&owner, &id)
            .map_err(|e| store_err(&e))?
            .to_json(),
    ))
}

pub async fn start_consolidation(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<StructuredConsolidationStartRequest>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_consolidation(&state)?;
    if req.episode_ids.len() > store.limits().max_consolidation_episodes {
        return Err(error(
            "STRUCTURED_MEMORY_CAP",
            "too many episodes for this consolidation configuration",
        ));
    }
    let Some(_gate) = state.generation_gate.claim("consolidation") else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "STRUCTURED_MEMORY_BUSY",
            "a local model turn is already running",
        )
        .details(json!({
            "owner": state.generation_gate.owner(),
            "cancel": "/api/structured-memory/consolidation/{id}/cancel",
        })));
    };
    let run = crate::server::structured_memory_consolidate::run_manual(&state, store, &owner, &req.episode_ids)
        .await
        .map_err(|e| {
            let mut api = store_err(&e);
            if e.code == "STRUCTURED_MEMORY_BUSY" {
                api = ApiError::from_err(StatusCode::CONFLICT, &e);
            }
            if e.code == crate::llm::openai_chat::LLM_ERROR_CODE {
                api = ApiError::from_err(StatusCode::BAD_GATEWAY, &e);
            }
            api
        })?;
    audit(
        &state,
        "structured_memory_consolidation",
        &owner,
        json!({
            "id": run.id,
            "state": run.state,
            "episode_count": run.episode_ids.len(),
            "proposal_count": run.proposal_count,
            "candidate_count": run.candidate_count,
            "rejected_count": run.rejected_count,
            "error_class": run.error_class,
            "summarizer_version": run.summarizer_version,
        }),
    );
    Ok(private(run.to_json()))
}

pub async fn cancel_consolidation(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Path(id): Path<String>,
) -> ApiResult<PrivateJson> {
    let owner = owner(user);
    let store = require_consolidation(&state)?;
    let run = store.cancel_consolidation_run(&owner, &id).map_err(|e| store_err(&e))?;
    if state.generation_gate.owner() == "consolidation" {
        state.chat.abort_in_flight();
    }
    audit(
        &state,
        "structured_memory_consolidation_cancelled",
        &owner,
        json!({"id": run.id, "state": run.state, "error_class": run.error_class}),
    );
    Ok(private(run.to_json()))
}
