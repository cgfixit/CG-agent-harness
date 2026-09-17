//! JSON-backed chat session store with per-session token tallies.
//! Port of `harness/sessions.py`; on-disk shape unchanged.

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
const SESSION_ID_CHARS: usize = 12;

fn id_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\A[0-9a-f]{12}\z").expect("static regex"))
}

/// Twelve lowercase hex chars. Export paths must use this, not a deserialized field blindly.
pub fn session_id_ok(id: &str) -> bool {
    id_re().is_match(id)
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
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
    /// Operator /goal. Never in `summary()` because GET /api/sessions is open.
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub selected_skills: Vec<String>,
    /// Explicit structured-fact selection for this session. IDs only; content
    /// is re-read and revalidated at prompt assembly.
    #[serde(default)]
    pub selected_facts: Vec<crate::server::structured_memory::FactSelection>,
    #[serde(default)]
    pub last_prompt_skills: Vec<Value>,
    #[serde(default)]
    pub goal_stage: Option<Value>,
}

impl Session {
    /// Deliberately carries NO message content (the list route is open).
    pub fn summary(&self) -> Value {
        json!({
            "session_id": self.session_id,
            "title": self.title,
            "created_ts": self.created_ts,
            "model": self.model,
            "message_count": self.messages.len(),
            "tokens": self.tally.to_json(),
        })
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

    pub fn create(&self, model: &str, title: &str) -> Result<Session> {
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
            session_id: crate::common::random_hex(SESSION_ID_CHARS / 2),
            title,
            created_ts: crate::common::now_ts(),
            model: model.to_string(),
            messages: Vec::new(),
            prompt_history: Vec::new(),
            tally: TokenTally::default(),
            goal: String::new(),
            selected_skills: Vec::new(),
            selected_facts: Vec::new(),
            last_prompt_skills: Vec::new(),
            goal_stage: None,
        };
        self.write(&session)?;
        Ok(session)
    }

    pub fn get(&self, session_id: &str) -> Result<Session> {
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
        Ok(session)
    }

    /// Summaries newest-first; corrupt or stray files are skipped. Summaries
    /// are cached per path and re-parsed only when the file's (mtime, len)
    /// stamp changes, so the open list route does not re-read every session
    /// body on each poll.
    pub fn list(&self) -> Vec<Value> {
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
                let staged_file = name.starts_with(".staged.") && name.ends_with(".tmp");
                if session_file || staged_file {
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
        self.record_exchange_inner(session_id, user_text, assistant_text, model, usage, prompt_skills, None)
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
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_exchange_inner(
        &self,
        session_id: &str,
        user_text: &str,
        assistant_text: &str,
        model: &str,
        usage: &TokenTally,
        prompt_skills: &[Value],
        compact: Option<(usize, String)>,
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

    fn check_goal_binding(session: &Session, stage_id: &str, instruction: &str, branch: &str) -> Result<()> {
        let Some(stage) = &session.goal_stage else {
            return Err(session_error("No staged goal", &session.session_id));
        };
        if stage["stage_id"] != stage_id
            || stage["goal"] != session.goal
            || session.goal != instruction
            || stage["request"]["branch"] != branch
            || !stage["job_id"].is_null()
        {
            return Err(session_error(
                "Goal stage changed, is stale, or was already submitted; inspect and stage again",
                &session.session_id,
            ));
        }
        Ok(())
    }
    pub fn validate_goal_binding(&self, id: &str, stage_id: &str, instruction: &str, branch: &str) -> Result<()> {
        Self::check_goal_binding(&self.get(id)?, stage_id, instruction, branch)
    }
    pub fn claim_goal_stage(
        &self,
        id: &str,
        stage_id: &str,
        request: &crate::server::schemas::AgentRunRequest,
        job_id: &str,
    ) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(id)?;
        Self::check_goal_binding(&session, stage_id, &request.instruction, &request.branch)?;
        let stage = session.goal_stage.as_mut().unwrap();
        stage["job_id"] = json!(job_id);
        stage["declared_checks"] = json!(request
            .checks
            .clone()
            .unwrap_or_else(|| vec![crate::server::agent_policy::DEFAULT_CHECK_PROFILE.to_string()]));
        stage["max_iterations"] = json!(request.max_iterations);
        self.write(&session)
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
        let store = SessionStore::new(&dir).unwrap();
        symlink(&private, dir.join("aaaaaaaaaaaa.json")).unwrap();
        assert_eq!(store.clear().unwrap(), 1);
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
        let store = SessionStore::new(&home.path().join("sessions")).unwrap();
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
        let store = SessionStore::new(&home.path().join("sessions")).unwrap();
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
        let store = SessionStore::new(&home.path().join("sessions")).unwrap();
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
        let store = SessionStore::new(&home.path().join("sessions")).unwrap();
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
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["title"], "fixed");
    }
}

#[cfg(test)]
mod prompt_history_tests {
    use super::*;
    #[test]
    fn last_fifty_prompts_persist_independently_and_legacy_logs_restore_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
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
        let reopened = SessionStore::new(dir.path()).unwrap();
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
