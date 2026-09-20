//! Session output-style selection. Context only; never writes `soul.md`.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::prompts::effective_soul_max_chars;
use crate::server::schemas::{ValidJson, Validate};
use crate::server::sessions::session_id_ok;
use crate::server::state::AppState;
use crate::server::style::{list_catalog, load_style, suggest_builtin, StyleLoad};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct StyleQuery {
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleRequest {
    session_id: String,
    name: String,
}

impl Validate for StyleRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !session_id_ok(&self.session_id) {
            bad.push("session_id".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            bad.push("name".into());
        }
        bad
    }
}

fn max_chars(state: &AppState) -> usize {
    effective_soul_max_chars(state.cfg.u64_or("personality.soul_max_chars", 8000))
}

fn catalog_json(state: &AppState) -> Vec<Value> {
    list_catalog(&state.home.root)
        .into_iter()
        .map(|(id, origin)| json!({"id": id, "origin": origin}))
        .collect()
}

fn load_active(state: &AppState, name: Option<&str>) -> StyleLoad {
    match name {
        Some(name) if !name.is_empty() && name != "off" => load_style(&state.home.root, name, max_chars(state)),
        _ => StyleLoad {
            name: String::new(),
            origin: None,
            loaded: false,
            truncated: false,
            unavailable_reason: Some("off"),
            text: String::new(),
        },
    }
}

fn payload(state: &AppState, session_id: Option<&str>, name: Option<&str>, effect: &str) -> Value {
    let load = load_active(state, name);
    let style = name.filter(|n| !n.is_empty() && *n != "off");
    json!({
        "styles": catalog_json(state),
        "session_id": session_id,
        "style": style,
        "origin": load.origin,
        "loaded": load.loaded,
        "truncated": load.truncated,
        "unavailable_reason": load.unavailable_reason.filter(|r| *r != "off"),
        "effect": effect,
        "scope": "session chat output style; context only; never writes soul.md; cannot override harness capabilities"
    })
}

pub async fn catalog(State(state): State<Arc<AppState>>, Query(query): Query<StyleQuery>) -> ApiResult<Json<Value>> {
    let session = match query.session_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => Some(
            state
                .store
                .get(id)
                .map_err(|e| ApiError::from_err(session_status(&e), &e))?,
        ),
        None => None,
    };
    let style = session.as_ref().and_then(|s| s.style.clone());
    Ok(Json(payload(
        &state,
        session.as_ref().map(|s| s.session_id.as_str()),
        style.as_deref(),
        "read-only catalog; POST /api/style to select",
    )))
}

pub async fn select(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<StyleRequest>,
) -> ApiResult<Json<Value>> {
    let name = req.name.trim();
    if name.eq_ignore_ascii_case("off") {
        state
            .store
            .set_style(&req.session_id, None)
            .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
        return Ok(Json(payload(
            &state,
            Some(&req.session_id),
            None,
            "cleared; subsequent local chat turns omit the style section",
        )));
    }
    let load = load_style(&state.home.root, name, max_chars(&state));
    if !load.loaded {
        let suggestion = suggest_builtin(name);
        let message = match (load.unavailable_reason, suggestion) {
            (Some("injection"), _) => "Style file matches an injection pattern and was not selected".to_string(),
            (_, Some(s)) => format!("Unknown style '{name}'; closest built-in is '{s}'"),
            _ => format!("Unknown style '{name}'"),
        };
        let mut err = ApiError::bad_request(
            if load.unavailable_reason == Some("injection") {
                "STYLE_INJECTION"
            } else {
                "STYLE_UNKNOWN"
            },
            message,
        );
        if let Some(s) = suggestion {
            err = err.details(json!({"suggestion": s, "reason": load.unavailable_reason}));
        }
        return Err(err);
    }
    state
        .store
        .set_style(&req.session_id, Some(load.name.as_str()))
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    Ok(Json(payload(
        &state,
        Some(&req.session_id),
        Some(load.name.as_str()),
        "included in subsequent local chat turns and /prompt; cloud chat still sends only the new user message",
    )))
}
