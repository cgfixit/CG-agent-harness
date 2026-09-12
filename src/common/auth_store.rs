//! Users, sessions, lockout and bootstrap backed by transactional SQLite.
//! Legacy auth.json records are migrated once; failures never create a new admin.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[path = "auth_sqlite.rs"]
mod sqlite;
use super::authn;
use super::config::AppConfig;
use super::errors::{HarnessError, Result};

pub const BOOTSTRAP_USERNAME: &str = "admin";
const DEFAULT_IDLE_TIMEOUT_SEC: f64 = 43200.0;
const DEFAULT_ABSOLUTE_TIMEOUT_SEC: f64 = 604800.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserRow {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    must_change_password: bool,
    password_hash: String,
    created_ts: f64,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    last_login_ts: Option<f64>,
    #[serde(default)]
    failed_count: u32,
    #[serde(default)]
    locked_until_ts: Option<f64>,
    #[serde(default = "default_role")]
    role: String,
}

fn default_role() -> String {
    authn::DEFAULT_ROLE.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRow {
    username: String,
    csrf_hash: String,
    created_ts: f64,
    last_seen_ts: f64,
    expires_ts: f64,
    #[serde(default)]
    revoked: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthDb {
    #[serde(default)]
    users: BTreeMap<String, UserRow>,
    /// Keyed by `hash_token(session_id)`; the raw id is never stored.
    #[serde(default)]
    sessions: BTreeMap<String, SessionRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSummary {
    pub user_id: String,
    pub username: String,
    pub created_ts: f64,
    pub disabled: bool,
    pub last_login_ts: Option<f64>,
    pub failed_count: u32,
    pub locked_until_ts: Option<f64>,
    pub role: String,
    pub must_change_password: bool,
}

#[derive(Debug, Clone)]
pub struct LoginResult {
    pub username: String,
    pub session_id: String,
    pub csrf_token: String,
    pub expires_ts: f64,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub username: String,
}

/// A record no real password matches, built once with current cost parameters
/// so an unknown username pays the same scrypt cost as a real check.
fn dummy_record() -> &'static str {
    static CELL: OnceLock<String> = OnceLock::new();
    CELL.get_or_init(|| {
        authn::hash_password_with_salt("dummy-timing-equalization-password", &[0u8; 16]).unwrap_or_else(|_| {
            "scrypt$131072$8$1$AAAAAAAAAAAAAAAAAAAAAA==$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()
        })
    })
}

pub struct AuthManager {
    path: PathBuf,
    idle_timeout_sec: f64,
    absolute_timeout_sec: f64,
    state: Mutex<(rusqlite::Connection, AuthDb)>,
    fresh_bootstrap: std::sync::atomic::AtomicBool,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager").field("path", &self.path).finish()
    }
}

impl AuthManager {
    pub fn open(path: &Path, cfg: &AppConfig) -> Result<Self> {
        let (connection, db, fresh) = sqlite::open(path)?;
        let mgr = Self {
            path: path.to_path_buf(),
            idle_timeout_sec: cfg.f64_or("auth.session.idle_timeout_sec", DEFAULT_IDLE_TIMEOUT_SEC),
            absolute_timeout_sec: cfg.f64_or("auth.session.absolute_timeout_sec", DEFAULT_ABSOLUTE_TIMEOUT_SEC),
            state: Mutex::new((connection, db)),
            fresh_bootstrap: std::sync::atomic::AtomicBool::new(fresh),
            clock: Box::new(super::now_ts),
        };
        // Warm the timing-equalization dummy before any login can arrive.
        let _ = dummy_record();
        Ok(mgr)
    }

    /// Test hook: inject a clock.
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> f64 + Send + Sync>) {
        self.clock = clock;
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    fn persist(&self, stored: &mut (rusqlite::Connection, AuthDb), db: &AuthDb) -> Result<()> {
        sqlite::persist(&mut stored.0, db)?;
        stored.1 = db.clone();
        Ok(())
    }

    fn summary(username: &str, row: &UserRow) -> UserSummary {
        UserSummary {
            user_id: row.user_id.clone(),
            username: username.to_string(),
            created_ts: row.created_ts,
            disabled: row.disabled,
            last_login_ts: row.last_login_ts,
            failed_count: row.failed_count,
            locked_until_ts: row.locked_until_ts,
            role: row.role.clone(),
            must_change_password: row.must_change_password,
        }
    }

    fn canonical(username: &str) -> String {
        username.trim().to_lowercase()
    }

    /// Creation happens atomically with the fresh database, never on an empty
    /// existing database. Retained for callers that report first-run status.
    pub fn bootstrap_if_empty(&self) -> Result<bool> {
        Ok(self.fresh_bootstrap.swap(false, std::sync::atomic::Ordering::SeqCst))
    }

    pub fn needs_password_setup(&self) -> bool {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let db = &stored.1;
        db.users
            .get(BOOTSTRAP_USERNAME)
            .map(|u| authn::is_pending_password_record(&u.password_hash))
            .unwrap_or(false)
    }

    pub fn bootstrap_set_password(&self, password: &str) -> Result<LoginResult> {
        let record = authn::hash_password(password)?;
        let now = self.now();
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let pending = db
            .users
            .get(BOOTSTRAP_USERNAME)
            .map(|u| authn::is_pending_password_record(&u.password_hash))
            .unwrap_or(false);
        if !pending {
            return Err(HarnessError::auth_bootstrap_complete().detail("username", BOOTSTRAP_USERNAME));
        }
        if let Some(u) = db.users.get_mut(BOOTSTRAP_USERNAME) {
            u.password_hash = record;
            u.failed_count = 0;
            u.locked_until_ts = None;
        }
        revoke_sessions_for(&mut db, BOOTSTRAP_USERNAME);
        let result = self.create_session(&mut db, BOOTSTRAP_USERNAME, now)?;
        self.persist(&mut stored, &db)?;
        Ok(result)
    }

    pub fn create_user(&self, username: &str, password: &str, role: &str) -> Result<String> {
        let canonical = authn::validate_username(username)?;
        let canonical_role = authn::validate_role(role)?;
        let record = authn::hash_password(password)?;
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        if db.users.len() >= 128 {
            return Err(HarnessError::new("AUTH_LIMIT", "account limit reached"));
        }
        if db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_exists(&canonical));
        }
        db.users.insert(
            canonical.clone(),
            UserRow {
                user_id: crate::common::random_hex(16),
                must_change_password: false,
                password_hash: record,
                created_ts: self.now(),
                disabled: false,
                last_login_ts: None,
                failed_count: 0,
                locked_until_ts: None,
                role: canonical_role,
            },
        );
        self.persist(&mut stored, &db)?;
        Ok(canonical)
    }

    pub fn get_user(&self, username: &str) -> Option<UserSummary> {
        let canonical = Self::canonical(username);
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let db = &stored.1;
        db.users.get(&canonical).map(|u| Self::summary(&canonical, u))
    }

    pub fn list_users(&self) -> Vec<UserSummary> {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let db = &stored.1;
        db.users.iter().map(|(k, v)| Self::summary(k, v)).collect()
    }

    pub fn count_enabled_admins(&self) -> usize {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let db = &stored.1;
        count_enabled_admins(db)
    }

    fn is_last_enabled_admin(db: &AuthDb, username: &str) -> bool {
        match db.users.get(username) {
            Some(u) if u.role == "admin" && !u.disabled => count_enabled_admins(db) <= 1,
            _ => false,
        }
    }

    pub fn set_role(&self, username: &str, role: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let canonical_role = authn::validate_role(role)?;
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if canonical_role != "admin" && Self::is_last_enabled_admin(&db, &canonical) {
            return Err(HarnessError::auth_last_admin());
        }
        if let Some(u) = db.users.get_mut(&canonical) {
            u.role = canonical_role;
        }
        revoke_sessions_for(&mut db, &canonical);
        self.persist(&mut stored, &db)
    }

    pub fn delete_user(&self, username: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if Self::is_last_enabled_admin(&db, &canonical) {
            return Err(HarnessError::auth_last_admin());
        }
        db.users.remove(&canonical);
        db.sessions.retain(|_, row| row.username != canonical);
        self.persist(&mut stored, &db)
    }

    fn set_disabled(&self, username: &str, disabled: bool) -> Result<()> {
        let canonical = Self::canonical(username);
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if disabled && Self::is_last_enabled_admin(&db, &canonical) {
            return Err(HarnessError::auth_last_admin());
        }
        if let Some(u) = db.users.get_mut(&canonical) {
            u.disabled = disabled;
            if !disabled {
                // Re-enabling is a deliberate decision to make the account usable NOW.
                u.failed_count = 0;
                u.locked_until_ts = None;
            }
        }
        if disabled {
            revoke_sessions_for(&mut db, &canonical);
        }
        self.persist(&mut stored, &db)
    }

    pub fn disable_user(&self, username: &str) -> Result<()> {
        self.set_disabled(username, true)
    }

    pub fn enable_user(&self, username: &str) -> Result<()> {
        self.set_disabled(username, false)
    }

    pub fn set_password(&self, username: &str, password: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let record = authn::hash_password(password)?;
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let Some(u) = db.users.get_mut(&canonical) else {
            return Err(HarnessError::auth_user_not_found(&canonical));
        };
        u.password_hash = record;
        u.must_change_password = false;
        u.failed_count = 0;
        u.locked_until_ts = None;
        revoke_sessions_for(&mut db, &canonical);
        self.persist(&mut stored, &db)
    }

    pub fn change_password(&self, username: &str, current: &str, password: &str) -> Result<LoginResult> {
        let canonical = Self::canonical(username);
        let record = authn::hash_password(password)?;
        let now = self.now();
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let user = db
            .users
            .get_mut(&canonical)
            .ok_or_else(HarnessError::auth_login_failed)?;
        if user.disabled || !authn::verify_password(current, &user.password_hash).0 {
            return Err(HarnessError::auth_login_failed());
        }
        user.password_hash = record;
        user.must_change_password = false;
        user.failed_count = 0;
        user.locked_until_ts = None;
        revoke_sessions_for(&mut db, &canonical);
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut stored, &db)?;
        Ok(result)
    }

    fn create_session(&self, db: &mut AuthDb, username: &str, now: f64) -> Result<LoginResult> {
        db.sessions
            .retain(|_, row| !row.revoked && row.expires_ts > now && row.last_seen_ts + self.idle_timeout_sec > now);
        if db.sessions.len() >= 4096 {
            return Err(HarnessError::new("AUTH_LIMIT", "active session limit reached"));
        }
        let session_id = authn::new_session_id();
        let csrf_token = authn::new_csrf_token();
        let expires_ts = now + self.absolute_timeout_sec;
        db.sessions.insert(
            authn::hash_token(&session_id),
            SessionRow {
                username: username.to_string(),
                csrf_hash: authn::hash_token(&csrf_token),
                created_ts: now,
                last_seen_ts: now,
                expires_ts,
                revoked: false,
            },
        );
        Ok(LoginResult {
            username: username.to_string(),
            session_id,
            csrf_token,
            expires_ts,
        })
    }

    /// Unknown username, wrong password and disabled account all raise the
    /// same `AUTH_LOGIN_FAILED`; an active lockout raises `AUTH_ACCOUNT_LOCKED`.
    pub fn login(&self, username: &str, password: &str) -> Result<LoginResult> {
        let canonical = Self::canonical(username);
        let now = self.now();
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let Some(row) = db.users.get(&canonical).cloned() else {
            let _ = authn::verify_password(password, dummy_record());
            return Err(HarnessError::auth_login_failed());
        };
        if authn::is_locked(row.locked_until_ts, now) {
            let retry_after = (row.locked_until_ts.unwrap_or(now) - now).max(0.0);
            return Err(HarnessError::auth_locked(retry_after, &canonical));
        }
        let (ok, needs_rehash) = authn::verify_password(password, &row.password_hash);
        if row.disabled || !ok {
            let new_count = row.failed_count.saturating_add(1);
            if let Some(u) = db.users.get_mut(&canonical) {
                u.failed_count = new_count;
                u.locked_until_ts = Some(authn::next_lock_until(new_count, now));
            }
            self.persist(&mut stored, &db)?;
            return Err(HarnessError::auth_login_failed());
        }
        if let Some(u) = db.users.get_mut(&canonical) {
            u.failed_count = 0;
            u.locked_until_ts = None;
            u.last_login_ts = Some(now);
            if needs_rehash {
                if let Ok(rec) = authn::hash_password(password) {
                    u.password_hash = rec;
                }
            }
        }
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut stored, &db)?;
        Ok(result)
    }

    /// Live means: exists, not revoked, within both the absolute expiry and the
    /// idle window. A valid lookup slides the idle window forward.
    pub fn validate_session(&self, session_id: &str) -> Option<SessionInfo> {
        if session_id.is_empty() {
            return None;
        }
        let key = authn::hash_token(session_id);
        let now = self.now();
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let idle = self.idle_timeout_sec;
        let outcome = match db.sessions.get_mut(&key) {
            None => return None,
            Some(row) if row.revoked => return None,
            Some(row) => {
                if now >= row.expires_ts || now >= row.last_seen_ts + idle {
                    row.revoked = true;
                    None
                } else {
                    row.last_seen_ts = now;
                    Some(SessionInfo {
                        session_id: session_id.to_string(),
                        username: row.username.clone(),
                    })
                }
            }
        };
        self.persist(&mut stored, &db).ok()?;
        outcome.filter(|info| db.users.get(&info.username).is_some_and(|user| !user.disabled))
    }

    pub fn logout(&self, session_id: &str) -> Result<bool> {
        if session_id.is_empty() {
            return Ok(false);
        }
        let key = authn::hash_token(session_id);
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        let hit = match db.sessions.get_mut(&key) {
            Some(row) if !row.revoked => {
                row.revoked = true;
                true
            }
            _ => false,
        };
        if hit {
            self.persist(&mut stored, &db)?;
        }
        Ok(hit)
    }
}

fn count_enabled_admins(db: &AuthDb) -> usize {
    db.users.values().filter(|u| u.role == "admin" && !u.disabled).count()
}

fn revoke_sessions_for(db: &mut AuthDb, username: &str) {
    for row in db.sessions.values_mut() {
        if row.username == username {
            row.revoked = true;
        }
    }
}
