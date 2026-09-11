//! Guarded operator persona editing. No model output is applied automatically.
use cap_std::fs::{Dir, OpenOptions};
use std::io::{Read, Write};
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::prompts::{compose_system_prompt, load_text, PromptInputs};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;

static EDIT_LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 32;
const APPLY_MARKER: &str = "soul-pending-apply.json";
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
fn read(dir: &Dir, name: &str) -> ApiResult<Option<String>> {
    match dir.symlink_metadata(name) {
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
    dir.open(name)
        .map_err(|_| io_error())?
        .take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| io_error())?;
    if text.len() > MAX_BYTES {
        return Err(error("SOUL_TOO_LARGE", "Existing persona exceeds the editing bound"));
    }
    Ok(Some(text))
}
fn home_dir(state: &AppState) -> ApiResult<Dir> {
    Dir::open_ambient_dir(&state.home.root, cap_std::ambient_authority()).map_err(|_| io_error())
}
fn records(state: &AppState) -> ApiResult<Dir> {
    let home = home_dir(state)?;
    match home.create_dir("soul-history") {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(io_error()),
    }
    if !home
        .symlink_metadata("soul-history")
        .map_err(|_| io_error())?
        .file_type()
        .is_dir()
    {
        return Err(error("SOUL_PATH", "History must be a real directory"));
    }
    home.open_dir("soul-history").map_err(|_| io_error())
}
fn write(dir: &Dir, name: &str, content: &[u8]) -> ApiResult<()> {
    let temporary = format!(".soul-staged-{}", crate::common::random_hex(16));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = dir.open_with(&temporary, &options)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        dir.rename(&temporary, dir, name)
    })();
    if result.is_err() {
        let _ = dir.remove_file(&temporary);
    }
    result.map_err(|_| io_error())
}
fn max_chars(state: &AppState) -> usize {
    crate::server::prompts::effective_soul_max_chars(state.cfg.u64_or("personality.soul_max_chars", 8000))
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
    let content = read(&home_dir(state)?, "soul.md")?;
    let rev = content.as_deref().map(revision).unwrap_or_else(|| "missing".into());
    Ok((content.unwrap_or_default(), rev))
}

// Caller holds EDIT_LOCK. Recovery updates bookkeeping only, never soul.md.
fn recover_apply(state: &AppState) -> ApiResult<()> {
    let home = home_dir(state)?;
    let Some(text) = read(&home, APPLY_MARKER)? else {
        return Ok(());
    };
    let marker: Value = serde_json::from_str(&text).map_err(|_| io_error())?;
    let id = marker["id"].as_str().filter(|id| valid_id(id)).ok_or_else(io_error)?;
    let base = marker["base_revision"].as_str().ok_or_else(io_error)?;
    let target = marker["revision"]
        .as_str()
        .filter(|rev| valid_id(rev))
        .ok_or_else(io_error)?;
    let mut record = proposal(state, id)?;
    if !(base == "missing" || valid_id(base))
        || record["id"] != id
        || record["base_revision"] != base
        || record["revision"] != target
        || revision(record["content"].as_str().ok_or_else(io_error)?) != target
        || !matches!(record["status"].as_str(), Some("pending" | "applied" | "interrupted"))
    {
        return Err(io_error());
    }
    let actual = current(state)?.1;
    record["status"] = json!(if actual == target {
        "applied"
    } else if actual == base {
        "pending"
    } else {
        "interrupted"
    });
    persist_proposal(state, id, &record)?;
    // Keep the marker on any failure so startup or the next persona operation retries.
    home.remove_file(APPLY_MARKER).map_err(|_| io_error())?;
    state
        .audit
        .log(json!({"event":"soul_apply_recovered","id":id,"status":record["status"]}));
    Ok(())
}

pub(crate) fn recover_on_startup(state: &AppState) -> ApiResult<()> {
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    recover_apply(state)
}

fn validate_save(state: &AppState, content: &str, expected: &str) -> ApiResult<(String, String)> {
    validate_content(state, content)?;
    let (old, actual) = current(state)?;
    if expected != actual {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "SOUL_CHANGED",
            "Persona changed since preview; reload and review it",
        ));
    }
    Ok((old, actual))
}
fn save(state: &AppState, content: &str, expected: &str) -> ApiResult<String> {
    let (old, actual) = validate_save(state, content, expected)?;
    if actual != "missing" {
        let dir = records(state)?;
        let backup = format!("{actual}.md");
        if let Some(existing) = read(&dir, &backup)? {
            if existing != old {
                return Err(error("SOUL_BACKUP", "Existing backup does not match its revision"));
            }
        } else {
            if dir.entries().map_err(|_| io_error())?.count() >= MAX_RECORDS {
                return Err(error(
                    "SOUL_HISTORY_FULL",
                    "History limit reached; archive old records before saving",
                ));
            }
            write(&dir, &backup, old.as_bytes())?;
        }
    }
    write(&home_dir(state)?, "soul.md", content.as_bytes())?;
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
    recover_apply(&state)?;
    let (content, rev) = if let Some(id) = query.version {
        if !valid_id(&id) {
            return Err(error("SOUL_VERSION", "Invalid version"));
        }
        let text =
            read(&records(&state)?, &format!("{id}.md"))?.ok_or_else(|| error("SOUL_VERSION", "No such version"))?;
        (text, id)
    } else {
        current(&state)?
    };
    let versions: Vec<String> = if home_dir(&state)?.symlink_metadata("soul-history").is_ok() {
        records(&state)?
            .entries()
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
    recover_apply(&state)?;
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
    let web = state.web.context_text(settings.web_enabled);
    let memory = if settings.memory_enabled {
        state.notes.context_text()
    } else {
        String::new()
    };
    let soul_path = state.home.soul_path();
    let selected = super::skills::resolve(
        &state,
        session.as_ref().map(|s| s.selected_skills.as_slice()).unwrap_or(&[]),
    )?;
    let inputs = PromptInputs {
        selected_skills: &selected,
        soul_enabled: settings.soul_enabled,
        soul_path: &soul_path,
        soul_max_chars: max_chars(&state),
        soul_override: req.soul_content.as_deref(),
        goal: session.as_ref().map(|s| s.goal.as_str()),
        web_context: Some(&web),
        memory_context: Some(&memory),
    };
    let sections: Vec<Value> = selected
        .iter()
        .map(|(id, body)| {
            json!({"id":id,"origin":format!("skills/{id}/SKILL.md"),
                "chars":body.chars().count(),"sha256":crate::common::sha256_hex(body)})
        })
        .collect();
    Ok(private(
        json!({"prompt":compose_system_prompt(&inputs),"discipline_sections":[],"selected_skill_sections":sections,
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
    recover_apply(&state)?;
    validate_content(&state, &req.content)?;
    if current(&state)?.1 != req.base_revision {
        return Err(error("SOUL_CHANGED", "Reload current persona before proposing"));
    }
    let dir = records(&state)?;
    if dir.entries().map_err(|_| io_error())?.count() >= MAX_RECORDS {
        return Err(error("SOUL_HISTORY_FULL", "History limit reached"));
    }
    let id = crate::common::random_hex(32);
    let proposal = json!({"id":id,"content":req.content,"base_revision":req.base_revision,"revision":revision(&req.content),"status":"pending","origin":"operator-submitted proposal; may contain model-authored text"});
    write(
        &dir,
        &format!("{id}.json"),
        serde_json::to_string_pretty(&proposal).unwrap().as_bytes(),
    )
    .map_err(|_| io_error())?;
    Ok(private(proposal))
}
fn proposal(state: &AppState, id: &str) -> ApiResult<Value> {
    if !valid_id(id) {
        return Err(error("SOUL_PROPOSAL", "Invalid proposal identifier"));
    }
    serde_json::from_str(
        &read(&records(state)?, &format!("{id}.json"))?.ok_or_else(|| error("SOUL_PROPOSAL", "No such proposal"))?,
    )
    .map_err(|_| io_error())
}
pub async fn review(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<PrivateJson> {
    let _guard = EDIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    recover_apply(&state)?;
    Ok(private(proposal(&state, &id)?))
}
fn persist_proposal(state: &AppState, id: &str, record: &Value) -> ApiResult<()> {
    write(
        &records(state)?,
        &format!("{id}.json"),
        serde_json::to_string_pretty(record).unwrap().as_bytes(),
    )
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
    recover_apply(&state)?;
    let mut record = proposal(&state, &id)?;
    let content = record["content"].as_str().ok_or_else(io_error)?;
    if record["status"] != "pending"
        || record["id"] != id
        || record["revision"] != req.revision
        || revision(content) != req.revision
    {
        return Err(error(
            "SOUL_PROPOSAL_CHANGED",
            "Proposal is no longer the reviewed pending revision",
        ));
    }
    if req.apply {
        validate_save(&state, content, record["base_revision"].as_str().ok_or_else(io_error)?)?;
        // Persist intent before replacing the document. No recovery path applies text.
        let marker = json!({"id":id,"base_revision":record["base_revision"],"revision":req.revision});
        write(
            &home_dir(&state)?,
            APPLY_MARKER,
            serde_json::to_string(&marker).unwrap().as_bytes(),
        )?;
        save(&state, content, record["base_revision"].as_str().ok_or_else(io_error)?)?;
    }
    record["status"] = json!(if req.apply { "applied" } else { "rejected" });
    persist_proposal(&state, &id, &record)?;
    if req.apply {
        home_dir(&state)?.remove_file(APPLY_MARKER).map_err(|_| io_error())?;
    }
    state
        .audit
        .log(json!({"event":"soul_proposal_decision","id":id,"status":record["status"]}));
    Ok(private(record))
}
