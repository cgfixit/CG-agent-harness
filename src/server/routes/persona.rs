//! Guarded operator persona editing. No model output is applied automatically.
use std::io::Read;
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::common::atomic::write_atomic;
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::prompts::{compose_system_prompt, load_text, PromptInputs, DISCIPLINE_SKILLS};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;

static EDIT_LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 32;
type PrivateJson = ([(header::HeaderName, &'static str); 1], Json<Value>);
fn private(value: Value) -> PrivateJson {
    ([(header::CACHE_CONTROL, crate::server::headers::NO_STORE)], Json(value))
}
fn error(code: &str, message: &str) -> ApiError {
    ApiError::bad_request(code, message)
}
fn io_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "SOUL_IO",
        "Persona storage failed; inspect the current document and proposal status before retrying",
    )
}
fn revision(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}
fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn read(path: &FsPath) -> ApiResult<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => (),
        _ => {
            return Err(error(
                "SOUL_PATH",
                "Persona files must be regular files, not links or directories",
            ))
        }
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|_| io_error())?
        .take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| io_error())?;
    if text.len() > MAX_BYTES {
        return Err(error("SOUL_TOO_LARGE", "Existing persona exceeds the editing bound"));
    }
    Ok(Some(text))
}
fn records(state: &AppState) -> ApiResult<std::path::PathBuf> {
    let dir = state.home.root.join("soul-history");
    if !dir.exists() {
        std::fs::create_dir(&dir).map_err(|_| io_error())?;
    }
    if !std::fs::symlink_metadata(&dir)
        .map_err(|_| io_error())?
        .file_type()
        .is_dir()
    {
        return Err(error("SOUL_PATH", "History must be a real directory"));
    }
    Ok(dir)
}
fn max_chars(state: &AppState) -> usize {
    state.cfg.u64_or("personality.soul_max_chars", 8000).min(65536) as usize
}
fn validate_content(state: &AppState, text: &str) -> ApiResult<()> {
    if text.trim().is_empty() || text.chars().count() > max_chars(state) || text.contains('\0') {
        return Err(error(
            "SOUL_CONTENT",
            "Persona must be nonempty and within the configured character bound",
        ));
    }
    if crate::common::injection::Scanner::core().count_matches(text) > 0 {
        return Err(error(
            "SOUL_INJECTION",
            "Persona matches a critical instruction-override pattern",
        ));
    }
    Ok(())
}
fn current(state: &AppState) -> ApiResult<(String, String)> {
    let content = read(&state.home.soul_path())?;
    let rev = content.as_deref().map(revision).unwrap_or_else(|| "missing".into());
    Ok((content.unwrap_or_default(), rev))
}
fn save(state: &AppState, content: &str, expected: &str) -> ApiResult<String> {
    validate_content(state, content)?;
    let (old, actual) = current(state)?;
    if expected != actual {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "SOUL_CHANGED",
            "Persona changed since preview; reload and review it",
        ));
    }
    if actual != "missing" {
        let dir = records(state)?;
        let backup = dir.join(format!("{actual}.md"));
        if let Some(existing) = read(&backup)? {
            if existing != old {
                return Err(error("SOUL_BACKUP", "Existing backup does not match its revision"));
            }
        } else {
            if std::fs::read_dir(&dir).map_err(|_| io_error())?.count() >= MAX_RECORDS {
                return Err(error(
                    "SOUL_HISTORY_FULL",
                    "History limit reached; archive old records before saving",
                ));
            }
            write_atomic(&backup, old.as_bytes(), Some(0o600)).map_err(|_| io_error())?;
        }
    }
    write_atomic(&state.home.soul_path(), content.as_bytes(), Some(0o600)).map_err(|_| io_error())?;
    Ok(revision(content))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DocumentQuery {
    version: Option<String>,
}
pub async fn document(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DocumentQuery>,
) -> ApiResult<PrivateJson> {
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (content, rev) = if let Some(id) = query.version {
        if !valid_id(&id) {
            return Err(error("SOUL_VERSION", "Invalid version"));
        }
        let text = read(&records(&state)?.join(format!("{id}.md")))?
            .ok_or_else(|| error("SOUL_VERSION", "No such version"))?;
        (text, id)
    } else {
        current(&state)?
    };
    let dir = state.home.root.join("soul-history");
    let versions: Vec<String> = if dir.exists() {
        std::fs::read_dir(records(&state)?)
            .map_err(|_| io_error())?
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".md"))
                    .filter(|n| valid_id(n))
                    .map(str::to_string)
            })
            .take(MAX_RECORDS)
            .collect()
    } else {
        Vec::new()
    };
    Ok(private(
        json!({"content":content,"revision":rev,"max_chars":max_chars(&state),"versions":versions,"scope":"chat persona only; fixed contracts and coding planner are separate"}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    content: String,
    base_revision: String,
    reason: String,
    #[serde(default)]
    confirm: bool,
}
impl Validate for EditRequest {
    fn validate(&self) -> Vec<String> {
        if self.content.len() > MAX_BYTES
            || self.reason.trim().is_empty()
            || self.reason.len() > 1000
            || !(self.base_revision == "missing" || valid_id(&self.base_revision))
        {
            vec!["content, base_revision, reason".into()]
        } else {
            vec![]
        }
    }
}
pub async fn edit(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<EditRequest>,
) -> ApiResult<PrivateJson> {
    if !req.confirm {
        return Err(error("SOUL_CONFIRM", "Explicit confirmation is required"));
    }
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let rev = save(&state, &req.content, &req.base_revision)?;
    state.audit.log(json!({"event":"soul_human_edit","revision":rev}));
    Ok(private(json!({"revision":rev,"saved":true})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    session_id: Option<String>,
    soul_content: Option<String>,
}
impl Validate for PreviewRequest {
    fn validate(&self) -> Vec<String> {
        if self.soul_content.as_ref().is_some_and(|s| s.len() > MAX_BYTES) {
            vec!["soul_content".into()]
        } else {
            vec![]
        }
    }
}
pub async fn preview(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<PreviewRequest>,
) -> ApiResult<PrivateJson> {
    let session = req
        .session_id
        .as_deref()
        .map(|id| state.store.get(id))
        .transpose()
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    if let Some(content) = &req.soul_content {
        validate_content(&state, content)?;
    }
    let settings = state.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let web = state.web.context_text();
    let memory = if settings.memory_enabled {
        state.notes.context_text()
    } else {
        String::new()
    };
    let skill_dir = state.home.skills_dir();
    let soul_path = state.home.soul_path();
    let inputs = PromptInputs {
        skills_dir: &skill_dir,
        soul_enabled: settings.soul_enabled,
        soul_path: &soul_path,
        soul_max_chars: max_chars(&state),
        soul_override: req.soul_content.as_deref(),
        goal: session.as_ref().map(|s| s.goal.as_str()),
        web_context: Some(&web),
        memory_context: Some(&memory),
    };
    let sections: Vec<Value> = DISCIPLINE_SKILLS
        .iter()
        .map(|id| {
            let load = load_text(&skill_dir, &FsPath::new(id).join("SKILL.md"), true, usize::MAX);
            json!({"origin":format!("skills/{id}/SKILL.md"), "state":load})
        })
        .collect();
    Ok(private(
        json!({"prompt":compose_system_prompt(&inputs),"discipline_sections":sections,
        "soul":load_text(&state.home.root, FsPath::new("soul.md"), settings.soul_enabled, max_chars(&state)),"candidate":req.soul_content.is_some(),
        "limits":{"goal":2000,"web":4000,"memory":3000,"soul":max_chars(&state)},
        "scope":"Next chat system prompt snapshot; not the coding planner. Persona and goal never authorize execution."}),
    ))
}

pub async fn propose(
    State(state): State<Arc<AppState>>,
    ValidJson(req): ValidJson<EditRequest>,
) -> ApiResult<PrivateJson> {
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    validate_content(&state, &req.content)?;
    if current(&state)?.1 != req.base_revision {
        return Err(error("SOUL_CHANGED", "Reload current persona before proposing"));
    }
    let dir = records(&state)?;
    if std::fs::read_dir(&dir).map_err(|_| io_error())?.count() >= MAX_RECORDS {
        return Err(error("SOUL_HISTORY_FULL", "History limit reached"));
    }
    let id = crate::common::random_hex(32);
    let proposal = json!({"id":id,"content":req.content,"base_revision":req.base_revision,"revision":revision(&req.content),"status":"pending","origin":"operator-submitted proposal; may contain model-authored text"});
    write_atomic(
        &dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&proposal).unwrap().as_bytes(),
        Some(0o600),
    )
    .map_err(|_| io_error())?;
    Ok(private(proposal))
}
fn proposal(state: &AppState, id: &str) -> ApiResult<Value> {
    if !valid_id(id) {
        return Err(error("SOUL_PROPOSAL", "Invalid proposal identifier"));
    }
    serde_json::from_str(
        &read(&records(state)?.join(format!("{id}.json")))?
            .ok_or_else(|| error("SOUL_PROPOSAL", "No such proposal"))?,
    )
    .map_err(|_| io_error())
}
pub async fn review(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<PrivateJson> {
    Ok(private(proposal(&state, &id)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    revision: String,
    reason: String,
    #[serde(default)]
    confirm: bool,
    apply: bool,
}
impl Validate for DecisionRequest {
    fn validate(&self) -> Vec<String> {
        if !valid_id(&self.revision) || self.reason.trim().is_empty() || self.reason.len() > 1000 {
            vec!["revision, reason".into()]
        } else {
            vec![]
        }
    }
}
pub async fn decide(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<DecisionRequest>,
) -> ApiResult<PrivateJson> {
    if !req.confirm {
        return Err(error("SOUL_CONFIRM", "Explicit confirmation is required"));
    }
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut record = proposal(&state, &id)?;
    let content = record["content"].as_str().ok_or_else(io_error)?;
    if record["status"] != "pending" || revision(content) != req.revision {
        return Err(error(
            "SOUL_PROPOSAL_CHANGED",
            "Proposal is no longer the reviewed pending revision",
        ));
    }
    if req.apply {
        save(&state, content, record["base_revision"].as_str().ok_or_else(io_error)?)?;
    }
    record["status"] = json!(if req.apply { "applied" } else { "rejected" });
    write_atomic(
        &records(&state)?.join(format!("{id}.json")),
        serde_json::to_string_pretty(&record).unwrap().as_bytes(),
        Some(0o600),
    )
    .map_err(|_| io_error())?;
    state
        .audit
        .log(json!({"event":"soul_proposal_decision","id":id,"status":record["status"]}));
    Ok(private(record))
}
