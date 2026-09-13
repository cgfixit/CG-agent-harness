//! Account-private structured facts and governed proposals.
//!
//! Distinct from pinned `/memory` notes in [`super::memory_notes`]. This store
//! is created only when `structured_memory.enabled` is the literal YAML boolean
//! `true`. Models and jobs may suggest through proposals; they never write
//! canonical facts. File mode 0600 is OS access control, not encryption.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};

use crate::common::atomic::write_atomic;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::injection::Scanner;

pub const SCHEMA_VERSION: i64 = 1;
pub const PUBLIC_ID_LEN: usize = 32;
const MARKER_BODY: &[u8] = b"sqlite3-v1\n";

const DEFAULT_MAX_FACTS: u64 = 64;
const DEFAULT_MAX_FACT_CHARS: u64 = 1000;
const DEFAULT_MAX_CATEGORY_CHARS: u64 = 64;
const DEFAULT_MAX_PROPOSALS: u64 = 32;
const DEFAULT_MAX_REASON_CHARS: u64 = 1000;

const SCHEMA: &str = "
CREATE TABLE facts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  public_id TEXT UNIQUE NOT NULL,
  owner_id TEXT NOT NULL,
  content TEXT NOT NULL,
  category TEXT NOT NULL DEFAULT '',
  content_digest TEXT NOT NULL,
  revision INTEGER NOT NULL CHECK(revision >= 1),
  active INTEGER NOT NULL CHECK(active IN (0,1)),
  created_ts REAL NOT NULL,
  updated_ts REAL NOT NULL,
  CHECK(length(public_id) = 32),
  CHECK(length(owner_id) BETWEEN 1 AND 64),
  CHECK(length(content) BETWEEN 1 AND 16000),
  CHECK(length(category) <= 256),
  CHECK(length(content_digest) = 64)
) STRICT;
CREATE TABLE proposals (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  public_id TEXT UNIQUE NOT NULL,
  owner_id TEXT NOT NULL,
  action TEXT NOT NULL CHECK(action IN ('add','update','deactivate')),
  content TEXT,
  category TEXT,
  target_fact_id TEXT,
  expected_revision INTEGER,
  expected_digest TEXT,
  proposal_revision TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected','stale')),
  created_ts REAL NOT NULL,
  decided_ts REAL,
  CHECK(length(public_id) = 32),
  CHECK(length(owner_id) BETWEEN 1 AND 64),
  CHECK(length(proposal_revision) = 64)
) STRICT;
CREATE INDEX facts_owner_active ON facts(owner_id, active);
CREATE INDEX proposals_owner_status ON proposals(owner_id, status);
PRAGMA user_version=1;
";

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_facts_per_owner: usize,
    pub max_fact_chars: usize,
    pub max_category_chars: usize,
    pub max_proposals_per_owner: usize,
    pub max_reason_chars: usize,
}

impl Limits {
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            max_facts_per_owner: clamp_u64(
                cfg.u64_or("structured_memory.max_facts_per_owner", DEFAULT_MAX_FACTS),
                1,
                256,
            ) as usize,
            max_fact_chars: clamp_u64(
                cfg.u64_or("structured_memory.max_fact_chars", DEFAULT_MAX_FACT_CHARS),
                1,
                4000,
            ) as usize,
            max_category_chars: clamp_u64(
                cfg.u64_or("structured_memory.max_category_chars", DEFAULT_MAX_CATEGORY_CHARS),
                1,
                64,
            ) as usize,
            max_proposals_per_owner: clamp_u64(
                cfg.u64_or("structured_memory.max_proposals_per_owner", DEFAULT_MAX_PROPOSALS),
                1,
                128,
            ) as usize,
            max_reason_chars: clamp_u64(
                cfg.u64_or("structured_memory.max_reason_chars", DEFAULT_MAX_REASON_CHARS),
                1,
                1000,
            ) as usize,
        }
    }

    pub fn as_json(&self) -> Value {
        json!({
            "max_facts_per_owner": self.max_facts_per_owner,
            "max_fact_chars": self.max_fact_chars,
            "max_category_chars": self.max_category_chars,
            "max_proposals_per_owner": self.max_proposals_per_owner,
            "max_reason_chars": self.max_reason_chars,
        })
    }
}

fn clamp_u64(value: u64, min: u64, max: u64) -> u64 {
    value.clamp(min, max)
}

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub id: String,
    pub owner_id: String,
    pub content: String,
    pub category: String,
    pub content_digest: String,
    pub revision: i64,
    pub active: bool,
    pub created_ts: f64,
    pub updated_ts: f64,
}

impl Fact {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub owner_id: String,
    pub action: String,
    pub content: Option<String>,
    pub category: Option<String>,
    pub target_fact_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub expected_digest: Option<String>,
    pub revision: String,
    pub status: String,
    pub created_ts: f64,
    pub decided_ts: Option<f64>,
}

impl Proposal {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

pub struct StructuredMemoryStore {
    path: PathBuf,
    conn: Mutex<Connection>,
    limits: Limits,
    scanner: Scanner,
}

impl std::fmt::Debug for StructuredMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredMemoryStore")
            .field("path", &self.path)
            .finish()
    }
}

fn invalid(message: impl Into<String>) -> HarnessError {
    HarnessError::new("STRUCTURED_MEMORY_IO", message)
}

fn sql(error: rusqlite::Error) -> HarnessError {
    invalid(format!("structured memory database refused: {error}"))
}

fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return Err(invalid("structured memory file must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(invalid(
                "structured memory file must be owned, private, and not hard-linked",
            ));
        }
    }
    Ok(file)
}

fn connect(path: &Path) -> Result<Connection> {
    private_file(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing structured memory directory"))?
        .canonicalize()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = parent.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(invalid(
                "structured memory directory must be owned and not writable by other users",
            ));
        }
    }
    let path = parent.join(
        path.file_name()
            .ok_or_else(|| invalid("missing structured memory filename"))?,
    );
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql)?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql)?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;",
    )
    .map_err(sql)?;
    Ok(conn)
}

fn check_schema(conn: &Connection) -> Result<()> {
    if conn
        .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
        .map_err(sql)?
        != SCHEMA_VERSION
        || conn
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .map_err(sql)?
            != "ok"
    {
        return Err(invalid("unsupported or corrupt structured memory database"));
    }
    Ok(())
}

pub fn marker_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("initialized")
}

pub fn valid_owner(owner: &str) -> bool {
    owner == "local" || (owner.len() == 32 && owner.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn valid_public_id(id: &str) -> bool {
    id.len() == PUBLIC_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_action(action: &str) -> bool {
    matches!(action, "add" | "update" | "deactivate")
}

pub struct ProposalDraft<'a> {
    pub action: &'a str,
    pub content: Option<&'a str>,
    pub category: Option<&'a str>,
    pub target_fact_id: Option<&'a str>,
    pub expected_revision: Option<i64>,
    pub expected_digest: Option<&'a str>,
}

fn digest(content: &str) -> String {
    crate::common::sha256_hex(content)
}

fn proposal_revision(
    action: &str,
    content: Option<&str>,
    category: Option<&str>,
    target: Option<&str>,
    expected_revision: Option<i64>,
    expected_digest: Option<&str>,
) -> String {
    crate::common::sha256_hex(&format!(
        "v1|{action}|{}|{}|{}|{}|{}",
        content.unwrap_or(""),
        category.unwrap_or(""),
        target.unwrap_or(""),
        expected_revision.map(|n| n.to_string()).unwrap_or_default(),
        expected_digest.unwrap_or("")
    ))
}

impl StructuredMemoryStore {
    pub fn open(path: &Path, cfg: &AppConfig) -> Result<Self> {
        let marker = marker_path(path);
        let parent = path
            .parent()
            .ok_or_else(|| invalid("missing structured memory directory"))?;
        std::fs::create_dir_all(parent)?;
        let conn = if present(path)? {
            let conn = connect(path)?;
            check_schema(&conn)?;
            if !present(&marker)? {
                write_atomic(&marker, MARKER_BODY, Some(0o600))?;
            }
            conn
        } else {
            if present(&marker)? {
                return Err(invalid(
                    "initialized structured memory database is missing; restore it explicitly",
                ));
            }
            let staged = tempfile::Builder::new().prefix(".structured.").tempfile_in(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                staged
                    .as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            let mut conn = connect(staged.path())?;
            let tx = conn.transaction().map_err(sql)?;
            tx.execute_batch(SCHEMA).map_err(sql)?;
            tx.commit().map_err(sql)?;
            check_schema(&conn)?;
            conn.close().map_err(|(_, e)| sql(e))?;
            staged.as_file().sync_all()?;
            staged
                .persist_noclobber(path)
                .map_err(|e| invalid(format!("cannot install structured memory database: {}", e.error)))?;
            write_atomic(&marker, MARKER_BODY, Some(0o600))?;
            connect(path)?
        };
        Ok(Self {
            path: path.to_path_buf(),
            conn: Mutex::new(conn),
            limits: Limits::from_config(cfg),
            scanner: Scanner::core(),
        })
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn require_owner(owner: &str) -> Result<()> {
        if valid_owner(owner) {
            Ok(())
        } else {
            Err(HarnessError::new(
                "STRUCTURED_MEMORY_OWNER",
                "structured memory owner must be an authenticated user_id or the documented local namespace",
            ))
        }
    }

    fn clean_text(&self, text: &str, max_chars: usize, empty_ok: bool) -> Result<String> {
        if text.contains('\0') || text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CONTENT",
                "structured memory text must not contain NUL or control characters",
            ));
        }
        let cleaned = text.trim();
        if cleaned.is_empty() {
            if empty_ok {
                return Ok(String::new());
            }
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CONTENT",
                "structured memory text must be nonempty",
            ));
        }
        if cleaned.chars().count() > max_chars {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CONTENT",
                format!("structured memory text exceeds {max_chars} characters"),
            )
            .detail("max_chars", max_chars as u64));
        }
        let flags = self.scanner.count_matches(cleaned);
        if flags > 0 {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_INJECTION",
                "text matches a critical injection pattern",
            )
            .detail("injection_flag_count", flags as u64));
        }
        Ok(cleaned.to_string())
    }

    fn validate_reason(&self, reason: &str) -> Result<()> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > self.limits.max_reason_chars || reason.contains('\0') {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CONTENT",
                "reason must be nonempty and within the configured bound",
            ));
        }
        Ok(())
    }

    fn map_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
        Ok(Fact {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            content: row.get(2)?,
            category: row.get(3)?,
            content_digest: row.get(4)?,
            revision: row.get(5)?,
            active: row.get::<_, i64>(6)? == 1,
            created_ts: row.get(7)?,
            updated_ts: row.get(8)?,
        })
    }

    fn map_proposal(row: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
        Ok(Proposal {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            action: row.get(2)?,
            content: row.get(3)?,
            category: row.get(4)?,
            target_fact_id: row.get(5)?,
            expected_revision: row.get(6)?,
            expected_digest: row.get(7)?,
            revision: row.get(8)?,
            status: row.get(9)?,
            created_ts: row.get(10)?,
            decided_ts: row.get(11)?,
        })
    }

    pub fn counts(&self, owner: &str) -> Result<(usize, usize)> {
        Self::require_owner(owner)?;
        let conn = self.lock();
        let facts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE owner_id=?1 AND active=1",
                params![owner],
                |r| r.get(0),
            )
            .map_err(sql)?;
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proposals WHERE owner_id=?1 AND status='pending'",
                params![owner],
                |r| r.get(0),
            )
            .map_err(sql)?;
        Ok((facts as usize, pending as usize))
    }

    pub fn list_facts(&self, owner: &str) -> Result<Vec<Fact>> {
        Self::require_owner(owner)?;
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
                 FROM facts WHERE owner_id=?1 AND active=1 ORDER BY updated_ts DESC, public_id ASC LIMIT 256",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![owner], Self::map_fact)
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok(rows)
    }

    pub fn get_fact(&self, owner: &str, id: &str) -> Result<Fact> {
        Self::require_owner(owner)?;
        if !valid_public_id(id) {
            return Err(HarnessError::new("STRUCTURED_MEMORY_NOT_FOUND", "unknown fact"));
        }
        let conn = self.lock();
        conn.query_row(
            "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
             FROM facts WHERE owner_id=?1 AND public_id=?2",
            params![owner, id],
            Self::map_fact,
        )
        .optional()
        .map_err(sql)?
        .ok_or_else(|| HarnessError::new("STRUCTURED_MEMORY_NOT_FOUND", "unknown fact"))
    }

    pub fn add_fact(&self, owner: &str, content: &str, category: &str, reason: &str) -> Result<Fact> {
        Self::require_owner(owner)?;
        self.validate_reason(reason)?;
        let content = self.clean_text(content, self.limits.max_fact_chars, false)?;
        let category = self.clean_text(category, self.limits.max_category_chars, true)?;
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE owner_id=?1 AND active=1",
                params![owner],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if count >= self.limits.max_facts_per_owner as i64 {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CAP",
                format!("at most {} active facts per owner", self.limits.max_facts_per_owner),
            )
            .detail("max_facts", self.limits.max_facts_per_owner as u64));
        }
        let id = crate::common::random_hex(16);
        let now = crate::common::now_ts();
        let digest = digest(&content);
        tx.execute(
            "INSERT INTO facts(public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts)
             VALUES(?1,?2,?3,?4,?5,1,1,?6,?6)",
            params![id, owner, content, category, digest, now],
        )
        .map_err(sql)?;
        let fact = tx
            .query_row(
                "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
                 FROM facts WHERE public_id=?1",
                params![id],
                Self::map_fact,
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(fact)
    }

    pub fn deactivate_fact(&self, owner: &str, id: &str, expected_revision: i64, reason: &str) -> Result<Fact> {
        Self::require_owner(owner)?;
        self.validate_reason(reason)?;
        if !valid_public_id(id) || expected_revision < 1 {
            return Err(HarnessError::new("STRUCTURED_MEMORY_NOT_FOUND", "unknown fact"));
        }
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let current = tx
            .query_row(
                "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
                 FROM facts WHERE owner_id=?1 AND public_id=?2",
                params![owner, id],
                Self::map_fact,
            )
            .optional()
            .map_err(sql)?
            .ok_or_else(|| HarnessError::new("STRUCTURED_MEMORY_NOT_FOUND", "unknown fact"))?;
        if !current.active {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CHANGED",
                "fact is no longer the reviewed active revision",
            ));
        }
        if current.revision != expected_revision {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CHANGED",
                "fact changed since review; reload and confirm again",
            ));
        }
        let now = crate::common::now_ts();
        tx.execute(
            "UPDATE facts SET active=0, revision=revision+1, updated_ts=?1 WHERE owner_id=?2 AND public_id=?3 AND revision=?4 AND active=1",
            params![now, owner, id, expected_revision],
        )
        .map_err(sql)?;
        let fact = tx
            .query_row(
                "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
                 FROM facts WHERE owner_id=?1 AND public_id=?2",
                params![owner, id],
                Self::map_fact,
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(fact)
    }

    pub fn create_proposal(&self, owner: &str, draft: ProposalDraft<'_>) -> Result<Proposal> {
        Self::require_owner(owner)?;
        if !valid_action(draft.action) {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_ACTION",
                "proposal action must be add, update, or deactivate",
            ));
        }
        let content = match draft.content {
            Some(text) if draft.action != "deactivate" => {
                Some(self.clean_text(text, self.limits.max_fact_chars, false)?)
            }
            Some(_) => None,
            None if draft.action == "add" || draft.action == "update" => {
                return Err(HarnessError::new(
                    "STRUCTURED_MEMORY_CONTENT",
                    "add and update proposals require content",
                ))
            }
            None => None,
        };
        let category = match draft.category {
            Some(text) => Some(self.clean_text(text, self.limits.max_category_chars, true)?),
            None => None,
        };
        if matches!(draft.action, "update" | "deactivate") {
            let target = draft.target_fact_id.unwrap_or("");
            if !valid_public_id(target) || draft.expected_revision.unwrap_or(0) < 1 {
                return Err(HarnessError::new(
                    "STRUCTURED_MEMORY_PROPOSAL",
                    "update and deactivate proposals must bind a fact id and revision",
                ));
            }
            let digest = draft.expected_digest.unwrap_or("");
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(HarnessError::new(
                    "STRUCTURED_MEMORY_PROPOSAL",
                    "update and deactivate proposals must bind the expected content digest",
                ));
            }
            let _ = self.get_fact(owner, target)?;
        }
        let revision = proposal_revision(
            draft.action,
            content.as_deref(),
            category.as_deref(),
            draft.target_fact_id,
            draft.expected_revision,
            draft.expected_digest,
        );
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let pending: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM proposals WHERE owner_id=?1 AND status='pending'",
                params![owner],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if pending >= self.limits.max_proposals_per_owner as i64 {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CAP",
                format!(
                    "at most {} pending proposals per owner",
                    self.limits.max_proposals_per_owner
                ),
            )
            .detail("max_proposals", self.limits.max_proposals_per_owner as u64));
        }
        let id = crate::common::random_hex(16);
        let now = crate::common::now_ts();
        tx.execute(
            "INSERT INTO proposals(public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10)",
            params![
                id,
                owner,
                draft.action,
                content,
                category,
                draft.target_fact_id,
                draft.expected_revision,
                draft.expected_digest,
                revision,
                now
            ],
        )
        .map_err(sql)?;
        let proposal = tx
            .query_row(
                "SELECT public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts,decided_ts
                 FROM proposals WHERE public_id=?1",
                params![id],
                Self::map_proposal,
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(proposal)
    }

    pub fn list_proposals(&self, owner: &str) -> Result<Vec<Proposal>> {
        Self::require_owner(owner)?;
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts,decided_ts
                 FROM proposals WHERE owner_id=?1 ORDER BY created_ts DESC, public_id ASC LIMIT 128",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![owner], Self::map_proposal)
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok(rows)
    }

    pub fn get_proposal(&self, owner: &str, id: &str) -> Result<Proposal> {
        Self::require_owner(owner)?;
        if !valid_public_id(id) {
            return Err(HarnessError::new("STRUCTURED_MEMORY_PROPOSAL", "unknown proposal"));
        }
        let conn = self.lock();
        conn.query_row(
            "SELECT public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts,decided_ts
             FROM proposals WHERE owner_id=?1 AND public_id=?2",
            params![owner, id],
            Self::map_proposal,
        )
        .optional()
        .map_err(sql)?
        .ok_or_else(|| HarnessError::new("STRUCTURED_MEMORY_PROPOSAL", "unknown proposal"))
    }

    pub fn decide_proposal(
        &self,
        owner: &str,
        id: &str,
        revision: &str,
        apply: bool,
        reason: &str,
    ) -> Result<Proposal> {
        Self::require_owner(owner)?;
        self.validate_reason(reason)?;
        if !valid_public_id(id) || revision.len() != 64 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_PROPOSAL_CHANGED",
                "proposal is no longer the reviewed pending revision",
            ));
        }
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let record = tx
            .query_row(
                "SELECT public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts,decided_ts
                 FROM proposals WHERE owner_id=?1 AND public_id=?2",
                params![owner, id],
                Self::map_proposal,
            )
            .optional()
            .map_err(sql)?
            .ok_or_else(|| HarnessError::new("STRUCTURED_MEMORY_PROPOSAL", "unknown proposal"))?;
        let recomputed = proposal_revision(
            &record.action,
            record.content.as_deref(),
            record.category.as_deref(),
            record.target_fact_id.as_deref(),
            record.expected_revision,
            record.expected_digest.as_deref(),
        );
        if record.status != "pending" || record.revision != revision || recomputed != revision {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_PROPOSAL_CHANGED",
                "proposal is no longer the reviewed pending revision",
            ));
        }
        if apply {
            self.apply_pending(&tx, owner, &record)?;
        }
        let now = crate::common::now_ts();
        let status = if apply { "applied" } else { "rejected" };
        tx.execute(
            "UPDATE proposals SET status=?1, decided_ts=?2 WHERE owner_id=?3 AND public_id=?4 AND status='pending' AND proposal_revision=?5",
            params![status, now, owner, id, revision],
        )
        .map_err(sql)?;
        let decided = tx
            .query_row(
                "SELECT public_id,owner_id,action,content,category,target_fact_id,expected_revision,expected_digest,proposal_revision,status,created_ts,decided_ts
                 FROM proposals WHERE owner_id=?1 AND public_id=?2",
                params![owner, id],
                Self::map_proposal,
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(decided)
    }

    fn apply_pending(&self, tx: &rusqlite::Transaction<'_>, owner: &str, record: &Proposal) -> Result<()> {
        match record.action.as_str() {
            "add" => {
                let content = record.content.as_deref().unwrap_or("");
                let cleaned = self.clean_text(content, self.limits.max_fact_chars, false)?;
                let category = self.clean_text(
                    record.category.as_deref().unwrap_or(""),
                    self.limits.max_category_chars,
                    true,
                )?;
                let count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM facts WHERE owner_id=?1 AND active=1",
                        params![owner],
                        |r| r.get(0),
                    )
                    .map_err(sql)?;
                if count >= self.limits.max_facts_per_owner as i64 {
                    return Err(HarnessError::new(
                        "STRUCTURED_MEMORY_CAP",
                        format!("at most {} active facts per owner", self.limits.max_facts_per_owner),
                    ));
                }
                let id = crate::common::random_hex(16);
                let now = crate::common::now_ts();
                tx.execute(
                    "INSERT INTO facts(public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts)
                     VALUES(?1,?2,?3,?4,?5,1,1,?6,?6)",
                    params![id, owner, cleaned, category, digest(&cleaned), now],
                )
                .map_err(sql)?;
            }
            "update" => {
                let target = record.target_fact_id.as_deref().unwrap_or("");
                let content = self.clean_text(
                    record.content.as_deref().unwrap_or(""),
                    self.limits.max_fact_chars,
                    false,
                )?;
                let category = self.clean_text(
                    record.category.as_deref().unwrap_or(""),
                    self.limits.max_category_chars,
                    true,
                )?;
                self.bind_target(tx, owner, record)?;
                let now = crate::common::now_ts();
                tx.execute(
                    "UPDATE facts SET content=?1, category=?2, content_digest=?3, revision=revision+1, updated_ts=?4
                     WHERE owner_id=?5 AND public_id=?6 AND revision=?7 AND active=1",
                    params![
                        content,
                        category,
                        digest(&content),
                        now,
                        owner,
                        target,
                        record.expected_revision.unwrap_or(0)
                    ],
                )
                .map_err(sql)?;
            }
            "deactivate" => {
                self.bind_target(tx, owner, record)?;
                let now = crate::common::now_ts();
                tx.execute(
                    "UPDATE facts SET active=0, revision=revision+1, updated_ts=?1
                     WHERE owner_id=?2 AND public_id=?3 AND revision=?4 AND active=1",
                    params![
                        now,
                        owner,
                        record.target_fact_id.as_deref().unwrap_or(""),
                        record.expected_revision.unwrap_or(0)
                    ],
                )
                .map_err(sql)?;
            }
            _ => {
                return Err(HarnessError::new(
                    "STRUCTURED_MEMORY_ACTION",
                    "proposal action must be add, update, or deactivate",
                ))
            }
        }
        Ok(())
    }

    fn bind_target(&self, tx: &rusqlite::Transaction<'_>, owner: &str, record: &Proposal) -> Result<()> {
        let target = record.target_fact_id.as_deref().unwrap_or("");
        let current = tx
            .query_row(
                "SELECT public_id,owner_id,content,category,content_digest,revision,active,created_ts,updated_ts
                 FROM facts WHERE owner_id=?1 AND public_id=?2",
                params![owner, target],
                Self::map_fact,
            )
            .optional()
            .map_err(sql)?
            .ok_or_else(|| HarnessError::new("STRUCTURED_MEMORY_NOT_FOUND", "unknown fact"))?;
        if current.revision != record.expected_revision.unwrap_or(0)
            || current.content_digest != record.expected_digest.as_deref().unwrap_or("")
            || !current.active
        {
            return Err(HarnessError::new(
                "STRUCTURED_MEMORY_CHANGED",
                "fact changed since the proposal was reviewed",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn with_connection<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&Connection) -> T,
    {
        f(&self.lock())
    }
}

pub fn disabled_status(owner: &str, limits: &Limits) -> Value {
    json!({
        "enabled": false,
        "owner_id": owner,
        "facts": false,
        "proposals": false,
        "episodes": false,
        "retrieval": false,
        "retrieval_fusion": false,
        "consolidation": false,
        "rag": false,
        "writable_from_model": false,
        "at_rest_encryption": false,
        "at_rest": "Owner-private file mode is OS access control, not encryption. A process running as the home owner can read the SQLite bytes.",
        "pinned_notes": {
            "separate": true,
            "prompt_toggle": "harness.json.memory_enabled",
            "store": "memory/notes.json"
        },
        "fact_count": 0,
        "pending_proposal_count": 0,
        "limits": limits.as_json(),
    })
}

pub fn enabled_status(owner: &str, limits: &Limits, fact_count: usize, pending: usize) -> Value {
    let mut value = disabled_status(owner, limits);
    value["enabled"] = json!(true);
    value["facts"] = json!(true);
    value["proposals"] = json!(true);
    value["fact_count"] = json!(fact_count);
    value["pending_proposal_count"] = json!(pending);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config::AppConfig;
    use crate::common::home::Home;

    fn cfg(dir: &Path) -> AppConfig {
        AppConfig::from_str(AppConfig::embedded_default(), &dir.join("config.yaml")).unwrap()
    }

    fn store(dir: &Path) -> StructuredMemoryStore {
        StructuredMemoryStore::open(&dir.join("structured.sqlite3"), &cfg(dir)).unwrap()
    }

    #[test]
    fn fresh_open_and_reopen_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("structured.sqlite3");
        let first = StructuredMemoryStore::open(&path, &cfg(dir.path())).unwrap();
        assert!(path.is_file());
        assert!(marker_path(&path).is_file());
        drop(first);
        let again = StructuredMemoryStore::open(&path, &cfg(dir.path())).unwrap();
        assert_eq!(again.counts("local").unwrap(), (0, 0));
    }

    #[test]
    fn missing_initialized_database_refuses_instead_of_bootstrapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("structured.sqlite3");
        let _store = StructuredMemoryStore::open(&path, &cfg(dir.path())).unwrap();
        drop(_store);
        std::fs::remove_file(&path).unwrap();
        assert!(StructuredMemoryStore::open(&path, &cfg(dir.path())).is_err());
    }

    #[test]
    fn corrupt_and_unsupported_databases_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("structured.sqlite3");
        write_atomic(&path, b"not a database", Some(0o600)).unwrap();
        write_atomic(&marker_path(&path), MARKER_BODY, Some(0o600)).unwrap();
        assert!(StructuredMemoryStore::open(&path, &cfg(dir.path())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_permissive_files_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("structured.sqlite3");
        let _ = StructuredMemoryStore::open(&path, &cfg(dir.path())).unwrap();
        let link = dir.path().join("linked.sqlite3");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(StructuredMemoryStore::open(&link, &cfg(dir.path())).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(StructuredMemoryStore::open(&path, &cfg(dir.path())).is_err());
        }
    }

    #[test]
    fn owners_cannot_address_each_others_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let fact = store
            .add_fact(a, "Prefer metric units", "pref", "operator entry")
            .unwrap();
        let proposal = store
            .create_proposal(
                a,
                ProposalDraft {
                    action: "add",
                    content: Some("Keep tabs"),
                    category: Some("pref"),
                    target_fact_id: None,
                    expected_revision: None,
                    expected_digest: None,
                },
            )
            .unwrap();
        assert!(store.get_fact(b, &fact.id).is_err());
        assert!(store.get_proposal(b, &proposal.id).is_err());
        assert!(store.list_facts(b).unwrap().is_empty());
        assert!(store
            .deactivate_fact(b, &fact.id, fact.revision, "not my fact")
            .is_err());
        assert_eq!(store.list_facts(a).unwrap().len(), 1);
    }

    #[test]
    fn injected_failure_leaves_no_partial_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .with_connection(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER refuse_change BEFORE INSERT ON facts BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;",
                )
                .unwrap();
            });
        assert!(store.add_fact("local", "will not land", "", "try").is_err());
        store.with_connection(|conn| {
            conn.execute_batch("DROP TRIGGER refuse_change;").unwrap();
        });
        assert_eq!(store.counts("local").unwrap(), (0, 0));
    }

    #[test]
    fn home_layout_does_not_create_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("home"));
        home.ensure_layout().unwrap();
        assert!(!home.structured_memory_path().exists());
    }
}
