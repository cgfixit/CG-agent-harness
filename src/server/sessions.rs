//! JSON-backed chat session store with per-session token tallies.
//! Port of `harness/sessions.py`; on-disk shape unchanged.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::atomic::write_json_atomic;
use crate::common::errors::{HarnessError, Result};

pub const SESSION_ERROR_CODE: &str = "HARNESS_SESSION_ERROR";
pub const PERSIST_ERROR_CODE: &str = "HARNESS_SESSION_PERSIST_ERROR";
pub const MAX_MESSAGES: usize = 500;
const SESSION_ID_CHARS: usize = 12;

fn id_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\A[0-9a-f]{12}\z").expect("static regex"))
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
    #[serde(default)]
    pub tally: TokenTally,
    /// Operator /goal. Never in `summary()` because GET /api/sessions is open.
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub selected_skills: Vec<String>,
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
            tally: TokenTally::default(),
            goal: String::new(),
            selected_skills: Vec::new(),
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
        session.goal = session.goal.trim().to_string();
        Ok(session)
    }

    /// Summaries newest-first; corrupt or stray files are skipped.
    pub fn list(&self) -> Vec<Value> {
        let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if p.extension().and_then(|s| s.to_str()) != Some("json") || !id_re().is_match(stem) {
                    continue;
                }
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                entries.push((mtime, p));
            }
        }
        entries.sort_by_key(|a| std::cmp::Reverse(a.0));
        let mut out = Vec::new();
        for (_, p) in entries {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(session) = serde_json::from_str::<Session>(&text) else {
                continue;
            };
            out.push(session.summary());
        }
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
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = self.get(session_id)?;
        let now = crate::common::now_ts();
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
        write_json_atomic(&path, &payload)
            .map_err(|_| HarnessError::new(PERSIST_ERROR_CODE, "could not persist session"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

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
}
