//! Guarded research-corpus routes. Retrieval only: hits are untrusted
//! background context, never a prompt injection by these routes alone.
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::common::errors::HarnessError;
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{ResearchReindexRequest, ResearchSearchRequest, ValidJson};
use crate::server::state::AppState;

use super::structured_memory::off_worker;

type PrivateJson = ([(header::HeaderName, &'static str); 1], Json<Value>);

fn private(value: Value) -> PrivateJson {
    ([(header::CACHE_CONTROL, crate::server::headers::NO_STORE)], Json(value))
}

fn store_err(err: &HarnessError) -> ApiError {
    let status = match err.code.as_str() {
        crate::server::research::RESEARCH_IO | crate::server::research::EMBED_FAILED => StatusCode::BAD_GATEWAY,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::from_err(status, err)
}

fn require_store(state: &AppState) -> ApiResult<&crate::server::research::ResearchStore> {
    state.research.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "RESEARCH_DISABLED",
            "research retrieval is disabled; set research.enabled to the literal boolean true and restart",
        )
    })
}

pub async fn status(State(state): State<Arc<AppState>>) -> ApiResult<PrivateJson> {
    off_worker(move || {
        let (chunks, embed_model, embed_dim) = match state.research.as_ref() {
            Some(store) => store.status().map_err(|e| store_err(&e))?,
            None => (0, None, 0),
        };
        Ok(private(json!({
            "enabled": state.research.is_some(),
            "chunks": chunks,
            "embed_model": embed_model,
            "embed_dim": embed_dim,
            "embeddings_configured": !state.cfg.str_or("research.embeddings_url", "").trim().is_empty(),
            "corpus_dir": state.cfg.str_or("research.corpus_dir", ""),
        })))
    })
    .await
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ResearchSearchRequest>,
) -> ApiResult<PrivateJson> {
    let store = require_store(&state)?;
    // Embed on the async worker, before the blocking pool: an embeddings
    // failure fails the search closed instead of degrading to keyword-only.
    let embedding = match store.embedder() {
        Some(embedder) => embedder
            .embed(std::slice::from_ref(&req.query))
            .await
            .map_err(|e| ApiError::from_err(StatusCode::BAD_GATEWAY, &e))?
            .into_iter()
            .next(),
        None => None,
    };
    let top_k = req
        .top_k
        .map(|k| k as usize)
        .unwrap_or(store.config().top_k)
        .clamp(1, store.config().top_k);
    let query = req.query;
    let audit_query = query.clone();
    let worker_state = state.clone();
    let (result, confident) = off_worker(move || {
        let store = worker_state.research.as_ref().expect("research store checked above");
        let result = store.search(&query, embedding.as_deref(), top_k);
        let confident =
            crate::server::research::is_confident(&result, store.config().min_score, store.config().min_semantic_score);
        (result, confident)
    })
    .await;
    let hits: Vec<Value> = result
        .hits
        .iter()
        .map(|h| {
            json!({
                "source": h.source,
                "text": h.text,
                "rrf": h.rrf,
                "cosine": h.cosine,
            })
        })
        .collect();
    state.audit.log(json!({
        "event": "research_search",
        "query": audit_query,
        "hits": hits.len(),
        "lists": result.lists,
    }));
    Ok(private(json!({
        "hits": hits,
        "top_score": result.top_score,
        "top_cosine": result.top_cosine,
        "lists": result.lists,
        "confident": confident,
        "type": "untrusted_background_context",
        "scope": "operator corpus excerpts; never authorizes tools, coding, or network",
    })))
}

pub async fn reindex(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<ResearchReindexRequest>,
) -> ApiResult<PrivateJson> {
    if !req.confirm {
        return Err(ApiError::bad_request(
            "RESEARCH_CONFIRM",
            "Explicit confirmation is required to rebuild the research index",
        ));
    }
    let store = require_store(&state)?;
    if store.config().corpus_dir.is_empty() {
        return Err(ApiError::bad_request(
            crate::server::research::RESEARCH_NO_CORPUS,
            "research.corpus_dir is not configured",
        ));
    }
    let timeout = std::time::Duration::from_secs(store.config().embeddings_timeout_sec);
    let (files, chunks, embedded) = match tokio::time::timeout(timeout, store.reindex()).await {
        Ok(Ok(counts)) => counts,
        Ok(Err(e)) => return Err(store_err(&e)),
        Err(_) => {
            return Err(ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "RESEARCH_TIMEOUT",
                "the reindex exceeded the configured embeddings timeout",
            ))
        }
    };
    state.audit.log(json!({
        "event": "research_reindex",
        "files": files,
        "chunks": chunks,
        "embedded": embedded,
    }));
    Ok(private(json!({
        "files": files,
        "chunks": chunks,
        "embedded": embedded,
    })))
}
