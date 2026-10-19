//! `/diag` routes: status, bounded log reads, and staging one excerpt for the
//! owner's next local chat turn. Every route needs an `admin` or
//! `portal_auditor` account and the literal `diagnostics.logs_enabled` gate.
//! The browser names an allowlisted file and fixed filters, never a path.
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::auth_store::UserSummary;
use crate::common::authn::PORTAL_AUDITOR_ROLE;
use crate::common::errors::HarnessError;
use crate::server::diag_reader::{self, ReadFilter};
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;

type PrivateJson = ([(header::HeaderName, &'static str); 1], Json<Value>);

fn private(value: Value) -> PrivateJson {
    ([(header::CACHE_CONTROL, crate::server::headers::NO_STORE)], Json(value))
}

fn status_for(code: &str) -> StatusCode {
    match code {
        "DIAG_REFUSED" => StatusCode::FORBIDDEN,
        "DIAG_DISABLED" | "DIAG_UNSUPPORTED" | "DIAG_NOTHING_TO_STAGE" => StatusCode::CONFLICT,
        "DIAG_NOT_FOUND" => StatusCode::NOT_FOUND,
        "DIAG_SCANNER_REFUSED" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// HTTP form of a `DIAG_*` refusal, for these routes and for chat.
pub fn diag_error(err: &HarnessError) -> ApiError {
    ApiError::from_err(status_for(&err.code), err)
}

/// Audit a refusal by code only, then return it.
fn refuse(state: &AppState, code: &str, message: &str) -> ApiError {
    state.audit.log(json!({"event": "diag_refused", "code": code}));
    ApiError::new(status_for(code), code, message)
}

/// Role first, then the gate, so a role that may never read logs learns nothing.
fn authorize(state: &AppState, user: &Option<axum::Extension<UserSummary>>) -> ApiResult<String> {
    let account = match user {
        Some(account)
            if matches!(account.role.as_str(), "admin" | PORTAL_AUDITOR_ROLE) && !account.must_change_password =>
        {
            account
        }
        _ => {
            return Err(refuse(
                state,
                "DIAG_REFUSED",
                "/diag needs an enabled admin or portal_auditor account",
            ))
        }
    };
    if !state.diag.config.logs_enabled {
        return Err(refuse(
            state,
            "DIAG_DISABLED",
            "/diag log reads are off: diagnostics.logs_enabled is not the literal boolean true",
        ));
    }
    Ok(account.user_id.clone())
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = authorize(&state, &user)?;
    let cfg = &state.diag.config;
    let logs = state.home.logs_dir();
    let files: Vec<_> = cfg
        .allowed_files
        .iter()
        .map(|name| diag_reader::file_status(&logs, name))
        .collect();
    let last = state.diag.last(&owner).map(|r| {
        json!({"file": r.file, "lines_returned": r.lines_returned, "truncated": r.truncated,
               "malformed": r.malformed, "generated_at": r.generated_at, "sha256": r.sha256})
    });
    Ok(private(json!({
        "logs_enabled": true,
        "model_tool": false,
        "limits": {"max_lines": cfg.max_lines, "max_bytes": cfg.max_bytes, "inject_max_chars": cfg.inject_max_chars},
        "files": files,
        "last": last,
        "staged": state.diag.staged(&owner),
        "surfaces": "staged excerpts reach the next local chat turn of their session only; cloud chat and /agent refuse while one is staged; /loop never receives it",
    })))
}

/// `file`, `last`, `event`, `since` and `errors`, each at most once.
fn parse_read_query(state: &AppState, pairs: &[(String, String)]) -> ApiResult<(String, ReadFilter)> {
    let bad = |message: &str| refuse(state, "DIAG_BAD_REQUEST", message);
    let mut seen = std::collections::BTreeSet::new();
    let (mut file, mut filter) = (None, ReadFilter::default());
    for (key, value) in pairs {
        if !seen.insert(key.as_str()) {
            return Err(bad("each query parameter may appear once"));
        }
        match key.as_str() {
            "file" => file = Some(value.clone()),
            "last" => {
                let n = (!value.is_empty() && value.len() <= 4 && value.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| value.parse::<usize>().ok())
                    .flatten()
                    .filter(|n| (1..=state.diag.config.max_lines).contains(n))
                    .ok_or_else(|| bad("last must be an integer from 1 to diagnostics.logs_max_lines"))?;
                filter.last = Some(n);
            }
            "event" => {
                if !diag_reader::valid_event_name(value) {
                    return Err(bad("event must match [a-z][a-z0-9_.]{0,63}"));
                }
                filter.event = Some(value.clone());
            }
            "since" => {
                filter.since_secs = Some(
                    diag_reader::parse_since(value)
                        .ok_or_else(|| bad("since must be 15m, 2h or 1d style, at most 30d"))?,
                );
            }
            "errors" => match value.as_str() {
                "true" => filter.errors_only = true,
                "false" => {}
                _ => return Err(bad("errors must be true or false")),
            },
            _ => return Err(bad("unknown query parameter")),
        }
    }
    let file = file.ok_or_else(|| bad("file is required"))?;
    let Some(name) = state.diag.config.allowed(&file) else {
        return Err(refuse(
            state,
            "DIAG_BAD_FILE",
            "file must be exactly one of diagnostics.logs_allowed_files",
        ));
    };
    Ok((name.to_string(), filter))
}

pub async fn read(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> ApiResult<PrivateJson> {
    let owner = authorize(&state, &user)?;
    let (name, filter) = parse_read_query(&state, &pairs)?;
    // Lines from earlier requests may still be queued for the writer thread.
    state.audit.flush_async().await;
    let worker = state.clone();
    let (file, worker_filter) = (name.clone(), filter.clone());
    let result = super::structured_memory::off_worker(move || {
        worker.diag.read(
            &worker.home.logs_dir(),
            &owner,
            &file,
            &worker_filter,
            worker.audit.redactors(),
        )
    })
    .await;
    match result {
        Ok(result) => {
            state.audit.log(json!({
                "event": "diag_read", "file": result.file, "lines_returned": result.lines_returned,
                "bytes_read": result.bytes_read, "truncated": result.truncated,
                "event_filter": filter.event, "since_secs": filter.since_secs,
                "errors_only": filter.errors_only, "outcome": "ok",
            }));
            let mut body = serde_json::to_value(&result).unwrap_or(Value::Null);
            body["source"] = json!("diag");
            Ok(private(body))
        }
        Err(err) if matches!(err.code.as_str(), "DIAG_SCANNER_REFUSED" | "DIAG_BAD_FILE") => {
            Err(refuse(&state, &err.code, &err.message))
        }
        Err(err) => {
            state
                .audit
                .log(json!({"event": "diag_read", "file": name, "outcome": err.code}));
            Err(diag_error(&err))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagInjectRequest {
    pub session_id: String,
}

impl Validate for DiagInjectRequest {
    fn validate(&self) -> Vec<String> {
        if crate::server::sessions::session_id_ok(&self.session_id) {
            Vec::new()
        } else {
            vec!["session_id".into()]
        }
    }
}

pub async fn inject(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
    ValidJson(req): ValidJson<DiagInjectRequest>,
) -> ApiResult<PrivateJson> {
    let owner = authorize(&state, &user)?;
    state
        .store
        .for_owner(&owner)
        .get(&req.session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let staged = state.diag.stage(&owner, &req.session_id).map_err(|e| diag_error(&e))?;
    state
        .audit
        .log(json!({"event": "diag_inject", "file": staged.file, "chars": staged.chars, "sha256": staged.sha256}));
    Ok(private(
        json!({"staged": true, "session_id": staged.session_id, "file": staged.file,
        "chars": staged.chars, "sha256": staged.sha256,
        "note": "the next local chat turn of this session carries it once; cloud chat and /agent refuse until then"}),
    ))
}

pub async fn forget(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<UserSummary>>,
) -> ApiResult<PrivateJson> {
    let owner = authorize(&state, &user)?;
    Ok(private(json!({"cleared": state.diag.forget(&owner)})))
}
