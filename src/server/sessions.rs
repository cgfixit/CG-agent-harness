//! JSON-backed chat session store with per-session token tallies.
//! Version 1 records carry an owner; older unassigned records stay quarantined.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::atomic::write_json_atomic_mode;
use crate::common::errors::{HarnessError, Result};

pub const SESSION_ERROR_CODE: &str = "HARNESS_SESSION_ERROR";
pub const PERSIST_ERROR_CODE: &str = "HARNESS_SESSION_PERSIST_ERROR";
pub const MAX_MESSAGES: usize = 500;
pub const PROMPT_HISTORY_LIMIT: usize = 50;
pub const MAX_PINNED_ATTACHMENTS: usize = 12;
const SESSION_ID_CHARS: usize = 12;

fn id_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\A[0-9a-f]{12}\z").expect("static regex"))
}

/// Twelve lowercase hex chars. Export paths must use this, not a deserialized field blindly.
pub fn session_id_ok(id: &str) -> bool {
    id_re().is_match(id)
}

/// Parse twelve lowercase hex digits into an integer. The integer, not the
/// original string, is what export uses to build a filename.
pub fn session_id_u64(raw: &str) -> Result<u64> {
    if raw.len() != SESSION_ID_CHARS {
        return Err(session_error("invalid session id", raw));
    }
    let mut n = 0u64;
    for b in raw.bytes() {
        let digit = match b {
            b'0'..=b'9' => u64::from(b - b'0'),
            b'a'..=b'f' => u64::from(b - b'a' + 10),
            _ => return Err(session_error("invalid session id", raw)),
        };
        n = (n << 4) | digit;
    }
    Ok(n)
}

/// Rebuild a session id from an integer so filesystem names cannot carry `../`.
pub fn canonical_session_id(raw: &str) -> Result<String> {
    Ok(format!("{:012x}", session_id_u64(raw)?))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenTally {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub exchanges: u64,
}

impl TokenTally {
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }

    pub fn to_json(&self) -> Value {
        json!({
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "exchanges": self.exchanges,
            "total": self.total(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub text: String,
    #[serde(default = "crate::common::now_ts")]
    pub ts: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentPin {
    pub owner: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    /// Zero/missing identifies legacy shared data; adoption is explicit.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub owner: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created_ts: f64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Recent submitted prompts survive model-context compaction, in the same private session log.
    #[serde(default)]
    pub prompt_history: Vec<String>,
    #[serde(default)]
    pub tally: TokenTally,
    /// Local prompt-usage calibration is scoped to a backend URL and model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_calibration: Option<crate::server::compaction::TokenCalibration>,
    /// Operator /goal. Never returned in the metadata-only session inventory.
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub selected_skills: Vec<String>,
    /// Session output-style preset id. Default off. Never written to soul.md.
    #[serde(default)]
    pub style: Option<String>,
    /// Explicit structured-fact selection for this session. IDs only; content
    /// is re-read and revalidated at prompt assembly.
    #[serde(default)]
    pub selected_facts: Vec<crate::server::structured_memory::FactSelection>,
    #[serde(default)]
    pub last_prompt_skills: Vec<Value>,
    #[serde(default)]
    pub goal_stage: Option<Value>,
    /// Sticky per-owner blob ids. Preserve original pin ownership on legacy
    /// adoption; only the caller's live blobs enter local chat or preview.
    #[serde(default)]
    pub attachment_pins: Vec<AttachmentPin>,
}

impl Session {
    /// Metadata only; normal inventory is filtered to the caller owner.
    pub fn summary(&self) -> Value {
        json!({
            "session_id": self.session_id,
            "owner": self.owner,
            "legacy_shared": self.owner.is_none(),
            "title": self.title,
            "created_ts": self.created_ts,
            "model": self.model,
            "message_count": self.messages.len(),
            "tokens": self.tally.to_json(),
        })
    }

    pub fn pinned_ids_for(&self, owner: &str) -> Vec<String> {
        self.attachment_pins
            .iter()
            .filter(|pin| pin.owner == owner)
            .map(|pin| pin.id.clone())
            .collect()
    }
}

fn session_error(message: &str, session_id: &str) -> HarnessError {
    HarnessError::new(SESSION_ERROR_CODE, message).detail("session_id", session_id)
}

pub struct SessionStore {
    dir: PathBuf,
    lock: Mutex<()>,
    /// `list()` summaries keyed by path. Store writes explicitly invalidate
    /// their row; ordinary out-of-band changes use the (mtime, len) stamp.
    summaries: Mutex<HashMap<PathBuf, (std::time::SystemTime, u64, Value)>>,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStore").field("dir", &self.dir).finish()
    }
}

impl SessionStore {
    pub fn new(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            lock: Mutex::new(()),
            summaries: Mutex::new(HashMap::new()),
        })
    }

    fn path_for(&self, session_id: &str) -> Result<PathBuf> {
        if !id_re().is_match(session_id) {
            return Err(session_error("invalid session id", session_id));
        }
        Ok(self.dir.join(format!("{session_id}.json")))
    }

    fn get_raw(&self, session_id: &str) -> Result<Session> {
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Err(session_error("unknown session", session_id));
        }
        let text = std::fs::read_to_string(&path).map_err(|_| {
            HarnessError::new(
                SESSION_ERROR_CODE,
                format!("unreadable session file: {session_id}.json"),
            )
        })?;
        let mut session: Session = serde_json::from_str(&text).map_err(|_| {
            HarnessError::new(
                SESSION_ERROR_CODE,
                format!("unreadable session file: {session_id}.json"),
            )
        })?;
        if session.session_id != session_id
            || !matches!(
                (session.schema_version, session.owner.as_deref()),
                (0, None) | (1, Some(_))
            )
            || session
                .owner
                .as_deref()
                .is_some_and(|owner| !crate::server::structured_memory::valid_owner(owner))
        {
            return Err(session_error(
                "invalid session identity or ownership version",
                session_id,
            ));
        }
        if session.prompt_history.is_empty() {
            session.prompt_history = session
                .messages
                .iter()
                .rev()
                .filter(|m| m.role == "user")
                .take(PROMPT_HISTORY_LIMIT)
                .map(|m| m.text.clone())
                .collect();
            session.prompt_history.reverse();
        }
        let excess = session.prompt_history.len().saturating_sub(PROMPT_HISTORY_LIMIT);
        session.prompt_history.drain(..excess);
        session.goal = session.goal.trim().to_string();
        if session
            .style
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s == "off")
        {
            session.style = None;
        }
        Ok(session)
    }

    /// Summaries newest-first; corrupt or stray files are skipped. Summaries
    /// are cached per path and re-parsed only when the file's (mtime, len)
    /// stamp changes, so the open list route does not re-read every session
    /// body on each poll.
    fn list_all(&self) -> Vec<Value> {
        let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if p.extension().and_then(|s| s.to_str()) != Some("json") || !id_re().is_match(stem) {
                    continue;
                }
                let (mtime, len) = e
                    .metadata()
                    .map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()))
                    .unwrap_or((std::time::UNIX_EPOCH, 0));
                entries.push((mtime, len, p));
            }
        }
        entries.sort_by_key(|a| std::cmp::Reverse(a.0));
        let mut cache = self.summaries.lock().unwrap_or_else(|p| p.into_inner());
        let mut live = std::collections::HashSet::new();
        let mut out = Vec::with_capacity(entries.len());
        for (mtime, len, p) in entries {
            live.insert(p.clone());
            if let Some((cached_mtime, cached_len, summary)) = cache.get(&p) {
                if *cached_mtime == mtime && *cached_len == len {
                    out.push(summary.clone());
                    continue;
                }
            }
            let summary = std::fs::read_to_string(&p)
                .ok()
                .and_then(|text| serde_json::from_str::<Session>(&text).ok())
                .filter(|session| {
                    session.session_id == p.file_stem().unwrap().to_string_lossy()
                        && match (session.schema_version, session.owner.as_deref()) {
                            (0, None) => true,
                            (1, Some(owner)) => crate::server::structured_memory::valid_owner(owner),
                            _ => false,
                        }
                })
                .map(|session| session.summary());
            match summary {
                Some(summary) => {
                    cache.insert(p, (mtime, len, summary.clone()));
                    out.push(summary);
                }
                None => {
                    cache.remove(&p);
                }
            }
        }
        // Drop rows for deleted sessions so the cache cannot grow unbounded.
        cache.retain(|p, _| live.contains(p));
        out
    }

    pub(crate) fn drop_attachment_pins_for_owner(&self, owner: &str) -> Result<usize> {
        self.filter_attachment_pins(|pin| pin.owner != owner)
    }

    pub(crate) fn drop_attachment_pins_unless(&self, live: impl Fn(&str) -> bool) -> Result<usize> {
        self.filter_attachment_pins(|pin| live(&pin.owner))
    }

    fn filter_attachment_pins(&self, keep: impl Fn(&AttachmentPin) -> bool) -> Result<usize> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut ids = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if p.extension().and_then(|s| s.to_str()) == Some("json") && id_re().is_match(stem) {
                    // Rebuild from the integer so a directory stem cannot carry `../`
                    // into `path_for` / `join`. Skip names the parser refuses.
                    let Ok(id) = canonical_session_id(stem) else {
                        continue;
                    };
                    ids.push(id);
                }
            }
        }
        let mut removed = 0usize;
        for id in ids {
            let Ok(mut session) = self.get_raw(&id) else {
                continue;
            };
            let before = session.attachment_pins.len();
            session.attachment_pins.retain(|pin| keep(pin));
            let n = before - session.attachment_pins.len();
            if n > 0 {
                self.write(&session)?;
                removed += n;
            }
        }
        Ok(removed)
    }

    fn write(&self, session: &Session) -> Result<()> {
        let path = self.path_for(&session.session_id)?;
        let payload = serde_json::to_value(session)?;
        // Chat history can carry pasted secrets; pin 0600 explicitly instead of
        // depending on the staging temp file's default permissions.
        write_json_atomic_mode(&path, &payload, 0o600)
            .map_err(|_| HarnessError::new(PERSIST_ERROR_CODE, "could not persist session"))?;
        self.summaries.lock().unwrap_or_else(|p| p.into_inner()).remove(&path);
        Ok(())
    }
    pub fn for_owner(&self, owner: &str) -> OwnedSessionStore<'_> {
        OwnedSessionStore {
            store: self,
            owner: owner.to_string(),
        }
    }

    /// Metadata only. The route permits administrators (or explicit local mode).
    pub fn legacy_summaries(&self) -> Vec<Value> {
        self.list_all()
            .into_iter()
            .filter(|row| row["owner"].is_null())
            .collect()
    }

    pub fn adopt_legacy(&self, id: &str, owner: &str) -> Result<Session> {
        if !crate::server::structured_memory::valid_owner(owner) {
            return Err(session_error("invalid session owner", id));
        }
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get_raw(id)?;
        if session.owner.is_some() {
            return Err(session_error("session is not unassigned legacy data", id));
        }
        session.schema_version = 1;
        session.owner = Some(owner.to_string());
        // Adoption transfers data, never another operator's execution approval.
        session.goal_stage = None;
        self.write(&session)?;
        Ok(session)
    }
}

/// All normal reads and mutations require an owner-bearing storage view. Raw
/// inventory is private; callers cannot accidentally search a shared store.
pub struct OwnedSessionStore<'a> {
    store: &'a SessionStore,
    owner: String,
}
impl std::ops::Deref for OwnedSessionStore<'_> {
    type Target = SessionStore;
    fn deref(&self) -> &Self::Target {
        self.store
    }
}
impl OwnedSessionStore<'_> {
    pub fn get(&self, id: &str) -> Result<Session> {
        let session = self.store.get_raw(id)?;
        if !crate::server::structured_memory::valid_owner(&self.owner)
            || session.owner.as_deref() != Some(self.owner.as_str())
        {
            return Err(session_error("unknown session", id));
        }
        Ok(session)
    }
    pub fn list(&self) -> Vec<Value> {
        if !crate::server::structured_memory::valid_owner(&self.owner) {
            return vec![];
        }
        self.store
            .list_all()
            .into_iter()
            .filter(|row| row["owner"].as_str() == Some(self.owner.as_str()))
            .collect()
    }
    fn write(&self, session: &Session) -> Result<()> {
        if session.schema_version != 1 || session.owner.as_deref() != Some(self.owner.as_str()) {
            return Err(session_error("session ownership changed", &session.session_id));
        }
        self.store.write(session)
    }
    pub fn create(&self, model: &str, title: &str) -> Result<Session> {
        if !crate::server::structured_memory::valid_owner(&self.owner) {
            return Err(session_error("invalid session owner", ""));
        }
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let now = time::OffsetDateTime::now_utc();
        let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
        let stamp = now.format(&fmt).unwrap_or_default();
        let title = if title.trim().is_empty() {
            format!("session {stamp}")
        } else {
            title.trim().to_string()
        };
        let session = Session {
            schema_version: 1,
            owner: Some(self.owner.clone()),
            session_id: crate::common::random_hex(SESSION_ID_CHARS / 2),
            title,
            created_ts: crate::common::now_ts(),
            model: model.to_string(),
            messages: Vec::new(),
            prompt_history: Vec::new(),
            tally: TokenTally::default(),
            token_calibration: None,
            goal: String::new(),
            selected_skills: Vec::new(),
            style: None,
            selected_facts: Vec::new(),
            last_prompt_skills: Vec::new(),
            goal_stage: None,
            attachment_pins: Vec::new(),
        };
        self.write(&session)?;
        Ok(session)
    }

    /// Serialize deletion with every writer. Late replies re-read the missing
    /// session and fail instead of restoring its history. Never recurse or follow
    /// a leaf symlink; remove only session files and our interrupted atomic writes.
    pub fn clear(&self) -> Result<usize> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let remove = || -> std::io::Result<usize> {
            if !std::fs::symlink_metadata(&self.dir)?.is_dir() {
                return Err(std::io::Error::other("session directory is not a directory"));
            }
            #[cfg(unix)]
            let dir = {
                use std::os::unix::fs::OpenOptionsExt;
                cap_std::fs::Dir::from_std_file(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                        .open(&self.dir)?,
                )
            };
            #[cfg(not(unix))]
            let dir = cap_std::fs::Dir::open_ambient_dir(&self.dir, cap_std::ambient_authority())?;
            let mut count = 0;
            for entry in dir.entries()? {
                let name = entry?.file_name();
                let Some(name) = name.to_str() else { continue };
                let session_file = name.strip_suffix(".json").is_some_and(|id| id_re().is_match(id));
                if session_file && dir.symlink_metadata(name)?.is_dir() {
                    return Err(std::io::Error::other("session file is a directory"));
                }
                // Unassigned/corrupt/interrupted writes are retained for explicit inspection.
                if session_file && self.get(name.strip_suffix(".json").unwrap()).is_ok() {
                    dir.remove_file(name)?;
                    count += usize::from(session_file);
                }
            }
            Ok(count)
        };
        remove().map_err(|_| HarnessError::new(PERSIST_ERROR_CODE,
            "Could not clear all session history; some files may already be deleted. Retry after resolving the storage error."))
    }

    pub fn record_exchange(
        &self,
        session_id: &str,
        user_text: &str,
        assistant_text: &str,
        model: &str,
        usage: &TokenTally,
        prompt_skills: &[Value],
    ) -> Result<Session> {
        self.record_exchange_inner(
            session_id,
            user_text,
            assistant_text,
            model,
            usage,
            prompt_skills,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_compacted_exchange(
        &self,
        session_id: &str,
        user_text: &str,
        assistant_text: &str,
        model: &str,
        usage: &TokenTally,
        prompt_skills: &[Value],
        keep_recent: usize,
        summary: &str,
    ) -> Result<Session> {
        self.record_exchange_inner(
            session_id,
            user_text,
            assistant_text,
            model,
            usage,
            prompt_skills,
            Some((keep_recent, summary.to_string())),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_exchange_inner(
        &self,
        session_id: &str,
        user_text: &str,
        assistant_text: &str,
        model: &str,
        usage: &TokenTally,
        prompt_skills: &[Value],
        compact: Option<(usize, String)>,
        calibration: Option<crate::server::compaction::TokenCalibration>,
    ) -> Result<Session> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        if let Some((keep_recent, summary)) = compact {
            session.messages = crate::server::compaction::compact_messages(&session.messages, keep_recent, &summary);
        }
        let now = crate::common::now_ts();
        session.prompt_history.push(user_text.to_string());
        let excess = session.prompt_history.len().saturating_sub(PROMPT_HISTORY_LIMIT);
        session.prompt_history.drain(..excess);
        session.messages.push(Message {
            role: "user".into(),
            text: user_text.to_string(),
            ts: now,
        });
        session.messages.push(Message {
            role: "assistant".into(),
            text: assistant_text.to_string(),
            ts: now,
        });
        if session.messages.len() > MAX_MESSAGES {
            let drop = session.messages.len() - MAX_MESSAGES;
            session.messages.drain(0..drop);
        }
        session.model = model.to_string();
        if let Some(calibration) = calibration {
            session.token_calibration = Some(calibration);
        }
        session.last_prompt_skills = prompt_skills.to_vec();
        session.tally.prompt_tokens += usage.prompt_tokens;
        session.tally.completion_tokens += usage.completion_tokens;
        session.tally.exchanges += 1;
        self.write(&session)?;
        Ok(session)
    }

    /// `title=None` leaves the title alone; `goal=Some("")` clears the goal.
    pub fn rename(&self, session_id: &str, title: Option<&str>, goal: Option<&str>) -> Result<Session> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        if let Some(t) = title {
            if !t.trim().is_empty() {
                session.title = t.trim().to_string();
            }
        }
        if let Some(g) = goal {
            session.goal = g.trim().to_string();
        }
        self.write(&session)?;
        Ok(session)
    }

    pub fn select_skills(&self, session_id: &str, ids: &[String]) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        session.selected_skills = ids.to_vec();
        self.write(&session)
    }

    pub fn set_style(&self, session_id: &str, style: Option<&str>) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        session.style = style
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "off")
            .map(str::to_string);
        self.write(&session)
    }

    pub fn select_facts(
        &self,
        session_id: &str,
        facts: &[crate::server::structured_memory::FactSelection],
    ) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        session.selected_facts = facts.to_vec();
        self.write(&session)
    }

    pub fn merge_attachment_pins(
        &self,
        session_id: &str,
        owner: &str,
        ids: &[String],
        live: impl Fn(&str) -> bool,
    ) -> Result<Session> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        let mut others = Vec::new();
        let mut mine = Vec::new();
        for pin in session.attachment_pins.drain(..) {
            if pin.owner == owner {
                if live(&pin.id) {
                    mine.push(pin);
                }
            } else {
                others.push(pin);
            }
        }
        for id in ids {
            if live(id) && !mine.iter().any(|pin| pin.id == *id) {
                mine.push(AttachmentPin {
                    owner: owner.to_string(),
                    id: id.clone(),
                });
            }
        }
        if mine.len() > MAX_PINNED_ATTACHMENTS {
            let drop = mine.len() - MAX_PINNED_ATTACHMENTS;
            mine.drain(..drop);
        }
        others.extend(mine);
        session.attachment_pins = others;
        self.write(&session)?;
        Ok(session)
    }

    pub fn stage_goal(
        &self,
        id: &str,
        expected_goal: &str,
        expected_stage: Option<&Value>,
        stage: &Value,
    ) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(id)?;
        if session.goal != expected_goal || session.goal_stage.as_ref() != expected_stage {
            return Err(session_error("Goal changed during staging", id));
        }
        session.goal_stage = Some(stage.clone());
        self.write(&session)
    }

    fn check_goal_binding(
        session: &Session,
        stage_id: &str,
        instruction: &str,
        branch: &str,
        allow_claimed: bool,
    ) -> Result<()> {
        let Some(stage) = &session.goal_stage else {
            return Err(session_error("No staged goal", &session.session_id));
        };
        if stage["stage_id"] != stage_id
            || stage["goal"] != session.goal
            || session.goal != instruction
            || stage["request"]["branch"] != branch
            || (!allow_claimed && !stage["job_id"].is_null())
        {
            return Err(session_error(
                "Goal stage changed, is stale, or was already submitted; inspect and stage again",
                &session.session_id,
            ));
        }
        Ok(())
    }
    pub fn validate_goal_binding(&self, id: &str, stage_id: &str, instruction: &str, branch: &str) -> Result<()> {
        Self::check_goal_binding(&self.get(id)?, stage_id, instruction, branch, false)
    }
    pub fn validate_goal_binding_recurring(
        &self,
        id: &str,
        stage_id: &str,
        instruction: &str,
        branch: &str,
    ) -> Result<()> {
        Self::check_goal_binding(&self.get(id)?, stage_id, instruction, branch, true)
    }
    pub fn claim_goal_stage(
        &self,
        id: &str,
        stage_id: &str,
        request: &crate::server::schemas::AgentRunRequest,
        job_id: &str,
    ) -> Result<()> {
        self.claim_goal_stage_inner(id, stage_id, request, job_id, false)
    }
    pub fn rebind_goal_stage(
        &self,
        id: &str,
        stage_id: &str,
        request: &crate::server::schemas::AgentRunRequest,
        job_id: &str,
    ) -> Result<()> {
        self.claim_goal_stage_inner(id, stage_id, request, job_id, true)
    }
    fn claim_goal_stage_inner(
        &self,
        id: &str,
        stage_id: &str,
        request: &crate::server::schemas::AgentRunRequest,
        job_id: &str,
        allow_claimed: bool,
    ) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(id)?;
        Self::check_goal_binding(&session, stage_id, &request.instruction, &request.branch, allow_claimed)?;
        let stage = session.goal_stage.as_mut().unwrap();
        stage["job_id"] = json!(job_id);
        stage["declared_checks"] = json!(request
            .checks
            .clone()
            .unwrap_or_else(|| vec![crate::server::agent_policy::DEFAULT_CHECK_PROFILE.to_string()]));
        stage["max_iterations"] = json!(request.max_iterations);
        self.write(&session)
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn canonical_session_id_rebuilds_from_an_integer() {
        assert_eq!(canonical_session_id("aaaaaaaaaaaa").unwrap(), "aaaaaaaaaaaa");
        assert_eq!(canonical_session_id("00000000000f").unwrap(), "00000000000f");
        assert!(canonical_session_id("../etc/passwd").is_err());
        assert!(canonical_session_id("AAAAAAAAAAAA").is_err());
        assert!(canonical_session_id("aaaa").is_err());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn clearing_never_follows_links_or_recurses_into_unrelated_data() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let private = outside.path().join("private.json");
        std::fs::write(&private, "keep").unwrap();
        let dir = home.path().join("sessions");
        let store_root = SessionStore::new(&dir).unwrap();
        let store = store_root.for_owner("local");
        symlink(&private, dir.join("aaaaaaaaaaaa.json")).unwrap();
        let mine = store.create("fixture", "mine").unwrap();
        let foreign = store_root
            .for_owner("user_foreign")
            .create("fixture", "foreign")
            .unwrap();
        assert_eq!(store.clear().unwrap(), 1);
        assert!(!dir.join(format!("{}.json", mine.session_id)).exists());
        assert!(dir.join(format!("{}.json", foreign.session_id)).exists());
        assert!(std::fs::symlink_metadata(dir.join("aaaaaaaaaaaa.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&private).unwrap(), "keep");
        std::fs::create_dir(dir.join("bbbbbbbbbbbb.json")).unwrap();
        assert!(store.clear().is_err());
        assert!(dir.join("bbbbbbbbbbbb.json").is_dir());
        std::fs::rename(&dir, home.path().join("original")).unwrap();
        symlink(outside.path(), &dir).unwrap();
        assert!(store.clear().is_err());
        assert_eq!(std::fs::read_to_string(private).unwrap(), "keep");
    }

    #[test]
    fn session_files_are_written_owner_only() {
        let home = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(&home.path().join("sessions")).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("m", "alpha").unwrap();
        let mode = std::fs::metadata(store.path_for(&session.session_id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn compacted_exchange_keeps_goal_and_first_user_message() {
        let home = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(&home.path().join("sessions")).unwrap();
        let store = store_root.for_owner("local");
        let mut session = store.create("m", "alpha").unwrap();
        store
            .rename(&session.session_id, None, Some("ship the parser"))
            .unwrap();
        let usage = TokenTally {
            prompt_tokens: 1,
            completion_tokens: 1,
            exchanges: 0,
        };
        for i in 0..6 {
            store
                .record_exchange(
                    &session.session_id,
                    &format!("user-{i}"),
                    &format!("asst-{i}"),
                    "m",
                    &usage,
                    &[],
                )
                .unwrap();
        }
        session = store
            .record_compacted_exchange(
                &session.session_id,
                "latest user",
                "latest reply",
                "m",
                &usage,
                &[],
                2,
                "[session-compacted]\nstub summary\n",
            )
            .unwrap();
        assert_eq!(session.goal, "ship the parser");
        assert_eq!(session.messages[0].text, "user-0");
        assert!(session
            .messages
            .iter()
            .any(|m| m.text.starts_with(crate::server::compaction::COMPACT_PREFIX)));
        let mode = std::fs::metadata(store.path_for(&session.session_id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[cfg(test)]
mod list_cache_tests {
    use super::*;

    #[test]
    fn summaries_are_cached_and_re_parsed_only_after_a_rewrite() {
        let home = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(&home.path().join("sessions")).unwrap();
        let store = store_root.for_owner("local");
        let a = store.create("m", "alpha").unwrap();
        let b = store.create("m", "beta").unwrap();
        assert_eq!(store.list().len(), 2);
        assert_eq!(store.summaries.lock().unwrap().len(), 2);
        // A store-level rewrite must invalidate even if the filesystem stamp
        // is restored and the new title has the same length.
        let a_path = store.path_for(&a.session_id).unwrap();
        let original = std::fs::read_to_string(&a_path).unwrap();
        let a_metadata = std::fs::metadata(&a_path).unwrap();
        store.rename(&a.session_id, Some("bravo"), None).unwrap();
        // `rename` re-serializes `created_ts`. A JSON f64 round-trip can shift
        // the file by a byte even when the title length is unchanged. Rebuild
        // from the original payload so the restored mtime is the only remaining
        // stamp cue.
        let rewritten = original.replacen("\"title\": \"alpha\"", "\"title\": \"bravo\"", 1);
        assert_ne!(rewritten, original, "title substitution must change the payload");
        assert_eq!(
            original.len(),
            rewritten.len(),
            "same-length titles must keep the JSON size"
        );
        std::fs::write(&a_path, rewritten).unwrap();
        std::fs::File::open(&a_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(a_metadata.modified().unwrap()))
            .unwrap();
        let listed = store.list();
        let titles: Vec<&str> = listed.iter().filter_map(|s| s["title"].as_str()).collect();
        assert!(titles.contains(&"bravo"), "titles={titles:?}");
        // An out-of-band rewrite of different length must also invalidate.
        let path = store.path_for(&b.session_id).unwrap();
        let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        raw["title"] = json!("external edit with a longer title");
        std::fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();
        assert!(store
            .list()
            .iter()
            .any(|s| s["title"] == "external edit with a longer title"));
        // Deleting a file drops its cache row.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.summaries.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_corrupt_session_file_is_skipped_and_not_cached() {
        let home = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(&home.path().join("sessions")).unwrap();
        let store = store_root.for_owner("local");
        let a = store.create("m", "alpha").unwrap();
        let path = store.path_for(&a.session_id).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(store.list().is_empty());
        assert!(store.summaries.lock().unwrap().is_empty());
        // Repairing the file out-of-band is picked up on the next list.
        std::fs::write(
            &path,
            format!("{{\"session_id\":\"{}\",\"title\":\"fixed\"}}", a.session_id),
        )
        .unwrap();
        assert!(
            store.list().is_empty(),
            "repair without an owner is quarantined, not silently assigned"
        );
        let legacy = store_root.legacy_summaries();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0]["title"], "fixed");
    }
}

#[cfg(test)]
mod prompt_history_tests {
    use super::*;
    #[test]
    fn last_fifty_prompts_persist_independently_and_legacy_logs_restore_history() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "history").unwrap();
        for i in 0..55 {
            store
                .record_exchange(
                    &session.session_id,
                    &format!("prompt {i}"),
                    "reply",
                    "fixture",
                    &TokenTally::default(),
                    &[],
                )
                .unwrap();
        }
        let mut saved = store.get(&session.session_id).unwrap();
        assert_eq!(saved.prompt_history.len(), 50);
        assert_eq!(saved.prompt_history.first().unwrap(), "prompt 5");
        assert_eq!(saved.prompt_history.last().unwrap(), "prompt 54");
        // Model-context compaction may replace messages but must not erase keyboard history.
        saved.messages.clear();
        store.write(&saved).unwrap();
        let reopened_root = SessionStore::new(dir.path()).unwrap();
        let reopened = reopened_root.for_owner("local");
        assert_eq!(
            reopened.get(&session.session_id).unwrap().prompt_history,
            saved.prompt_history
        );
        let legacy = store.create("fixture", "legacy").unwrap();
        let saved = store
            .record_exchange(
                &legacy.session_id,
                "old prompt",
                "old reply",
                "fixture",
                &TokenTally::default(),
                &[],
            )
            .unwrap();
        let mut value = serde_json::to_value(saved).unwrap();
        value.as_object_mut().unwrap().remove("prompt_history");
        std::fs::write(store.path_for(&legacy.session_id).unwrap(), value.to_string()).unwrap();
        assert_eq!(
            store.get(&legacy.session_id).unwrap().prompt_history,
            vec!["old prompt"]
        );
        assert!(store.create("fixture", "new").unwrap().prompt_history.is_empty());
        assert!(!store.list().iter().any(|row| row.get("prompt_history").is_some()));
        store.clear().unwrap();
        assert!(store.get(&session.session_id).is_err());
    }
}

#[cfg(test)]
mod attachment_pin_tests {
    use super::*;

    fn live_all(_: &str) -> bool {
        true
    }

    #[test]
    fn merge_keeps_other_owners_and_fifo_caps_this_owner() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "pins").unwrap();
        store
            .merge_attachment_pins(&session.session_id, "bob", &["b1".into()], live_all)
            .unwrap();
        let alice: Vec<String> = (0..13).map(|i| format!("a{i}")).collect();
        let saved = store
            .merge_attachment_pins(&session.session_id, "alice", &alice, live_all)
            .unwrap();
        let alice_ids = saved.pinned_ids_for("alice");
        assert_eq!(alice_ids.len(), MAX_PINNED_ATTACHMENTS);
        assert_eq!(alice_ids.first().unwrap(), "a1");
        assert_eq!(alice_ids.last().unwrap(), "a12");
        assert!(!alice_ids.contains(&"a0".to_string()));
        assert_eq!(saved.pinned_ids_for("bob"), vec!["b1".to_string()]);
        assert_eq!(store.drop_attachment_pins_for_owner("alice").unwrap(), 12);
        let after = store.get(&session.session_id).unwrap();
        assert!(after.pinned_ids_for("alice").is_empty());
        assert_eq!(after.pinned_ids_for("bob"), vec!["b1".to_string()]);
        assert_eq!(store.drop_attachment_pins_unless(|owner| owner == "alice").unwrap(), 1);
        assert!(store.get(&session.session_id).unwrap().attachment_pins.is_empty());
    }

    #[test]
    fn dead_blobs_are_dropped_and_missing_ids_are_not_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "pins").unwrap();
        store
            .merge_attachment_pins(&session.session_id, "local", &["live".into(), "gone".into()], |id| {
                id == "live"
            })
            .unwrap();
        let saved = store
            .merge_attachment_pins(&session.session_id, "local", &["gone".into()], |id| id == "live")
            .unwrap();
        assert_eq!(saved.pinned_ids_for("local"), vec!["live".to_string()]);
    }

    #[test]
    fn pins_round_trip_across_a_new_store_and_legacy_json() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "pins").unwrap();
        store
            .merge_attachment_pins(&session.session_id, "local", &["keep-me".into()], live_all)
            .unwrap();
        drop(store);
        let reopened_root = SessionStore::new(dir.path()).unwrap();
        let reopened = reopened_root.for_owner("local");
        let loaded = reopened.get(&session.session_id).unwrap();
        assert_eq!(loaded.pinned_ids_for("local"), vec!["keep-me".to_string()]);
        let mut value = serde_json::to_value(&loaded).unwrap();
        value.as_object_mut().unwrap().remove("attachment_pins");
        std::fs::write(reopened.path_for(&session.session_id).unwrap(), value.to_string()).unwrap();
        assert!(reopened.get(&session.session_id).unwrap().attachment_pins.is_empty());
        assert!(!reopened.list().iter().any(|row| row.get("attachment_pins").is_some()));
    }

    #[test]
    fn filter_skips_non_canonical_stems_and_does_not_escape_the_session_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "pins").unwrap();
        store
            .merge_attachment_pins(&session.session_id, "alice", &["keep".into()], live_all)
            .unwrap();

        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.json");
        let hostile_payload = r#"{"session_id":"deadbeefdead","attachment_pins":[{"owner":"alice","id":"leak"}]}"#;
        std::fs::write(&secret, hostile_payload).unwrap();

        // Names that match neither `id_re` nor `canonical_session_id`.
        let hostile_upper = dir.path().join("AAAAAAAAAAAA.json");
        let hostile_short = dir.path().join("aaaa.json");
        std::fs::write(&hostile_upper, hostile_payload).unwrap();
        std::fs::write(&hostile_short, hostile_payload).unwrap();

        assert!(canonical_session_id("../etc/passwd").is_err());
        assert!(canonical_session_id("AAAAAAAAAAAA").is_err());
        assert_eq!(store.drop_attachment_pins_for_owner("alice").unwrap(), 1);
        assert!(store.get(&session.session_id).unwrap().attachment_pins.is_empty());

        // Filter must not rewrite non-canonical siblings or anything outside `dir`.
        assert_eq!(std::fs::read_to_string(&hostile_upper).unwrap(), hostile_payload);
        assert_eq!(std::fs::read_to_string(&hostile_short).unwrap(), hostile_payload);
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), hostile_payload);
        assert!(std::fs::read_to_string(store.path_for(&session.session_id).unwrap())
            .unwrap()
            .contains(&session.session_id));
    }

    #[test]
    fn compacted_exchange_keeps_pins() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = SessionStore::new(dir.path()).unwrap();
        let store = store_root.for_owner("local");
        let session = store.create("fixture", "pins").unwrap();
        store
            .merge_attachment_pins(&session.session_id, "local", &["sticky".into()], live_all)
            .unwrap();
        let usage = TokenTally {
            prompt_tokens: 1,
            completion_tokens: 1,
            exchanges: 0,
        };
        for i in 0..6 {
            store
                .record_exchange(
                    &session.session_id,
                    &format!("user-{i}"),
                    &format!("asst-{i}"),
                    "fixture",
                    &usage,
                    &[],
                )
                .unwrap();
        }
        let saved = store
            .record_compacted_exchange(
                &session.session_id,
                "latest user",
                "latest reply",
                "fixture",
                &usage,
                &[],
                2,
                "[session-compacted]\nstub summary\n",
            )
            .unwrap();
        assert_eq!(saved.pinned_ids_for("local"), vec!["sticky".to_string()]);
        assert!(!saved.messages.iter().any(|m| m.text.contains("sticky")));
        assert_eq!(saved.goal, "");
    }
}
