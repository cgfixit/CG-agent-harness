//! Users, sessions, lockout and bootstrap backed by transactional SQLite.
//! Legacy auth.json records are migrated once; failures never create a new admin.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[path = "auth_sqlite.rs"]
mod sqlite;
use super::authn;
use super::authz;
use super::config::AppConfig;
use super::errors::{HarnessError, Result};

pub const BOOTSTRAP_USERNAME: &str = "admin";
const DEFAULT_IDLE_TIMEOUT_SEC: f64 = 43200.0;
const DEFAULT_ABSOLUTE_TIMEOUT_SEC: f64 = 604800.0;
const DEFAULT_RESTRICTED_IDLE_TIMEOUT_SEC: f64 = 600.0;
const DEFAULT_RESTRICTED_ABSOLUTE_TIMEOUT_SEC: f64 = 1800.0;
/// Session timeouts an operator may set, in seconds: one minute to 30 days.
/// 30 days is NIST SP 800-63B-4's longest reauthentication interval for a
/// single-factor (AAL1) session. Sessions only gate requests; detached jobs
/// and research runs belong to the account and outlive any one session.
const SESSION_TIMEOUT_RANGE_SEC: std::ops::RangeInclusive<f64> = 60.0..=2_592_000.0;

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
    /// How many times each account's sign-ins were revoked since this process
    /// started. In memory only: see `AuthManager::sign_in_revocations`.
    #[serde(skip)]
    revocations: BTreeMap<String, u64>,
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

/// Whether `current` still carries what a password was checked against in
/// `checked`: the same account, password record and setup-code state (which
/// `AuthManager::credential` reads). A password check runs with the account
/// lock released, so its verdict holds only while this does.
fn same_credential(checked: &UserRow, current: &UserRow) -> bool {
    current.user_id == checked.user_id
        && current.password_hash == checked.password_hash
        && current.must_change_password == checked.must_change_password
}

/// Most usernames without an account whose failures are remembered at once.
/// A fake-name flood stops growing memory here (about 2 MiB); see
/// `AuthManager::record_unknown_failure` for what is forgotten first.
const UNKNOWN_FAILURE_ENTRIES: usize = 16_384;

/// Sign-in failures for a username with no account, kept only in memory. They
/// follow the arithmetic of `UserRow::failed_count` and `locked_until_ts`, so a
/// name with no account locks out exactly when a real one would: otherwise the
/// first `AUTH_ACCOUNT_LOCKED` would tell a guesser which names exist.
#[derive(Clone, Copy)]
struct UnknownFailures {
    count: u32,
    locked_until_ts: f64,
    last_ts: f64,
}

/// Longest interval between idle-window slides of a live session.
const SESSION_TOUCH_INTERVAL_SEC: f64 = 60.0;

pub struct AuthManager {
    path: PathBuf,
    idle_timeout_sec: f64,
    absolute_timeout_sec: f64,
    /// Limits for a session whose account must still replace its password.
    /// Never longer than the normal limits.
    restricted_idle_timeout_sec: f64,
    restricted_absolute_timeout_sec: f64,
    state: Mutex<(rusqlite::Connection, AuthDb)>,
    /// Keyed by a hash of the canonical name, so no typed username (or a
    /// password typed into the username field) is held. Never held together
    /// with `state`.
    unknown_failures: Mutex<HashMap<String, UnknownFailures>>,
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
        // Refuse an out-of-range timeout before the store is opened or migrated.
        let idle_timeout_sec = cfg.f64_in(
            "auth.session.idle_timeout_sec",
            DEFAULT_IDLE_TIMEOUT_SEC,
            SESSION_TIMEOUT_RANGE_SEC,
        )?;
        let absolute_timeout_sec = cfg.f64_in(
            "auth.session.absolute_timeout_sec",
            DEFAULT_ABSOLUTE_TIMEOUT_SEC,
            SESSION_TIMEOUT_RANGE_SEC,
        )?;
        let restricted_idle_timeout_sec = cfg
            .f64_in(
                "auth.session.password_change_idle_timeout_sec",
                DEFAULT_RESTRICTED_IDLE_TIMEOUT_SEC,
                SESSION_TIMEOUT_RANGE_SEC,
            )?
            .min(idle_timeout_sec);
        let restricted_absolute_timeout_sec = cfg
            .f64_in(
                "auth.session.password_change_absolute_timeout_sec",
                DEFAULT_RESTRICTED_ABSOLUTE_TIMEOUT_SEC,
                SESSION_TIMEOUT_RANGE_SEC,
            )?
            .min(absolute_timeout_sec);
        let (mut connection, mut db, fresh) = sqlite::open(path)?;
        // Two older first-run states move to a setup code. Homes created before
        // setup codes shipped `admin` / `admin`: while that account still must
        // replace its password and its username still verifies, anyone on this
        // machine could take it (one scrypt check, only in that state). Older
        // homes migrated from auth.json may hold a `pending$` admin record that
        // only the retired public bootstrap route could set; it now signs in
        // with a code and must replace it like a fresh home.
        if !fresh {
            let shipped = db.users.get_key_value(BOOTSTRAP_USERNAME).is_some_and(|(name, u)| {
                !u.disabled
                    && (authn::is_pending_password_record(&u.password_hash)
                        || (u.must_change_password && authn::verify_password(name, &u.password_hash).0))
            });
            if shipped {
                let code = authn::new_setup_code();
                super::atomic::write_atomic(&setup_code_path(path), format!("{code}\n").as_bytes(), Some(0o600))?;
                let mut rotated = db.clone();
                if let Some(u) = rotated.users.get_mut(BOOTSTRAP_USERNAME) {
                    u.password_hash = authn::hash_bootstrap_password(&code)?;
                    u.must_change_password = true;
                    u.failed_count = 0;
                    u.locked_until_ts = None;
                }
                revoke_sessions_for(&mut rotated, BOOTSTRAP_USERNAME);
                sqlite::persist(&mut connection, &rotated)?;
                db = rotated;
            }
        }
        let mgr = Self {
            path: path.to_path_buf(),
            idle_timeout_sec,
            absolute_timeout_sec,
            restricted_idle_timeout_sec,
            restricted_absolute_timeout_sec,
            state: Mutex::new((connection, db)),
            unknown_failures: Mutex::new(HashMap::new()),
            fresh_bootstrap: std::sync::atomic::AtomicBool::new(fresh),
            clock: Box::new(super::now_ts),
        };
        // Warm the timing-equalization dummy before any login can arrive.
        let _ = dummy_record();
        Ok(mgr)
    }

    /// Test hook: inject a clock. Session expiry reads it, so production code
    /// must not reach it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> f64 + Send + Sync>) {
        self.clock = clock;
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    /// How stale a live session's recorded activity may get: a minute, or 1% of
    /// its idle timeout when that is shorter.
    fn touch_interval_sec(idle_timeout_sec: f64) -> f64 {
        (idle_timeout_sec / 100.0).min(SESSION_TOUCH_INTERVAL_SEC)
    }

    /// `(idle, absolute)` limits for a session of an account in this state.
    fn session_limits(&self, must_change_password: bool) -> (f64, f64) {
        if must_change_password {
            (self.restricted_idle_timeout_sec, self.restricted_absolute_timeout_sec)
        } else {
            (self.idle_timeout_sec, self.absolute_timeout_sec)
        }
    }

    fn persist(&self, stored: &mut (rusqlite::Connection, AuthDb), db: &AuthDb) -> Result<()> {
        sqlite::persist(&mut stored.0, db)?;
        stored.1 = db.clone();
        Ok(())
    }

    /// The stored account, copied out so a password can be checked against it
    /// with the lock released: one scrypt run under the lock would stall every
    /// request's session check for its whole duration.
    fn user_row(&self, canonical: &str) -> Option<UserRow> {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        stored.1.users.get(canonical).cloned()
    }

    fn locked(row: &UserRow, now: f64, canonical: &str) -> HarnessError {
        let retry_after = (row.locked_until_ts.unwrap_or(now) - now).max(0.0);
        HarnessError::auth_locked(retry_after, canonical)
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

    /// `(user_id, old_role)` for each account folded into `auditor` (#965)
    /// whose `auth.role_migrated` audit row is not yet confirmed. The fold and
    /// this report commit together, so a crash before the audit write leaves
    /// the report here for the next boot: reported at least once, never lost.
    pub fn pending_role_migrations(&self) -> Result<Vec<(String, String)>> {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        sqlite::pending_role_migrations(&stored.0)
    }

    /// The fold for `user_id` is in the audit log: drop its pending report.
    pub fn acknowledge_role_migration(&self, user_id: &str) -> Result<()> {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        sqlite::acknowledge_role_migration(&stored.0, user_id)
    }

    /// True while the enabled `admin` account still signs in with its setup
    /// code: it must replace its password and the private code file exists.
    /// The sign-in screen then names the code, never shows it. Answers without
    /// hashing, so the public setup-status route stays cheap.
    pub fn setup_code_active(&self) -> bool {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        stored
            .1
            .users
            .get(BOOTSTRAP_USERNAME)
            .is_some_and(|u| !u.disabled && u.must_change_password)
            && setup_code_path(&self.path).is_file()
    }

    /// What to verify for `username`: while `admin` still signs in with its
    /// setup code, a code-shaped entry is checked in canonical form, so a
    /// pasted trailing period or capitals do not reject the right code.
    fn credential<'a>(username: &str, row: &UserRow, password: &'a str) -> std::borrow::Cow<'a, str> {
        if username == BOOTSTRAP_USERNAME && row.must_change_password {
            if let Some(code) = authn::normalize_setup_code(password) {
                return code.into();
            }
        }
        password.into()
    }

    /// The code file outlives its use only until `admin` gets a real password.
    fn forget_setup_code(&self, username: &str) {
        if username == BOOTSTRAP_USERNAME {
            let _ = std::fs::remove_file(setup_code_path(&self.path));
        }
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

    /// How many times this account's sign-ins have been revoked since the
    /// server started: a role change, a password set or change, a disable and
    /// a delete each count. An admin password reset leaves no other trace on
    /// the account (it clears `must_change_password`), and a session's
    /// `revoked` flag is also set by ordinary expiry.
    pub fn sign_in_revocations(&self, username: &str) -> u64 {
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        stored
            .1
            .revocations
            .get(&Self::canonical(username))
            .copied()
            .unwrap_or(0)
    }

    /// Whether `account`, as read when some work began, is still the same
    /// enabled account. With `revocations` (the `sign_in_revocations` value
    /// read then), also whether its sign-ins have not been revoked since.
    /// One lock, so an account change cannot land between the two answers.
    pub fn account_unchanged(&self, account: &UserSummary, revocations: Option<u64>) -> bool {
        let canonical = Self::canonical(&account.username);
        let stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let db = &stored.1;
        db.users
            .get(&canonical)
            .is_some_and(|current| current.user_id == account.user_id && !current.disabled)
            && revocations.is_none_or(|seen| db.revocations.get(&canonical).copied().unwrap_or(0) == seen)
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
            Some(u) if authz::is_role(&u.role, authz::Role::Admin) && !u.disabled => count_enabled_admins(db) <= 1,
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
        if !authz::is_role(&canonical_role, authz::Role::Admin) && Self::is_last_enabled_admin(&db, &canonical) {
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
        *db.revocations.entry(canonical.clone()).or_default() += 1;
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

    /// The account stays present and must set a new password before automation runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn require_password_change(&self, username: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if let Some(user) = db.users.get_mut(&canonical) {
            user.must_change_password = true;
        }
        revoke_sessions_for(&mut db, &canonical);
        self.persist(&mut stored, &db)
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
        self.persist(&mut stored, &db)?;
        self.forget_setup_code(&canonical);
        Ok(())
    }

    pub fn change_password(&self, username: &str, current: &str, password: &str) -> Result<LoginResult> {
        self.change_password_with(username, current, password, authn::verify_password)
    }

    /// `change_password` with the check of the current password supplied. The
    /// check runs between two short lock scopes, never inside one; a test
    /// supplies its own to act while it runs.
    fn change_password_with(
        &self,
        username: &str,
        current: &str,
        password: &str,
        verify: impl Fn(&str, &str) -> (bool, bool),
    ) -> Result<LoginResult> {
        let canonical = Self::canonical(username);
        let record = authn::hash_password(password)?;
        let now = self.now();
        let checked = self.user_row(&canonical).ok_or_else(HarnessError::auth_login_failed)?;
        let current = Self::credential(&canonical, &checked, current);
        if checked.disabled || !verify(&current, &checked.password_hash).0 {
            return Err(HarnessError::auth_login_failed());
        }
        // A forced change must actually change the credential: the setup code
        // has been on screen and in a file, so it may not become the password.
        if Self::credential(&canonical, &checked, password) == current {
            return Err(HarnessError::password_policy(
                "the new password must differ from the current one",
            ));
        }
        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        // An account deleted, disabled or given another password while the
        // check ran keeps that change: the checked password no longer speaks
        // for it.
        let Some(user) = db
            .users
            .get_mut(&canonical)
            .filter(|user| !user.disabled && same_credential(&checked, user))
        else {
            return Err(HarnessError::auth_login_failed());
        };
        user.password_hash = record;
        user.must_change_password = false;
        user.failed_count = 0;
        user.locked_until_ts = None;
        revoke_sessions_for(&mut db, &canonical);
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut stored, &db)?;
        self.forget_setup_code(&canonical);
        Ok(result)
    }

    fn create_session(&self, db: &mut AuthDb, username: &str, now: f64) -> Result<LoginResult> {
        let (_, absolute_timeout_sec) =
            self.session_limits(db.users.get(username).is_some_and(|u| u.must_change_password));
        db.sessions
            .retain(|_, row| !row.revoked && row.expires_ts > now && row.last_seen_ts + self.idle_timeout_sec > now);
        if db.sessions.len() >= 4096 {
            return Err(HarnessError::new("AUTH_LIMIT", "active session limit reached"));
        }
        let session_id = authn::new_session_id();
        // The sessions schema still requires a csrf_hash. Nothing returns or
        // checks it: request CSRF is the process token in the console page.
        let csrf_hash = authn::hash_token(&authn::new_csrf_token());
        let expires_ts = now + absolute_timeout_sec;
        db.sessions.insert(
            authn::hash_token(&session_id),
            SessionRow {
                username: username.to_string(),
                csrf_hash,
                created_ts: now,
                last_seen_ts: now,
                expires_ts,
                revoked: false,
            },
        );
        Ok(LoginResult {
            username: username.to_string(),
            session_id,
            expires_ts,
        })
    }

    /// Unknown username, wrong password and disabled account all raise the
    /// same `AUTH_LOGIN_FAILED`; an active lockout raises `AUTH_ACCOUNT_LOCKED`,
    /// for a name with no account as for a real one (`fail_unknown`).
    pub fn login(&self, username: &str, password: &str) -> Result<LoginResult> {
        self.login_with(username, password, authn::verify_password)
    }

    /// `login` with the password check supplied. The check runs between two
    /// short lock scopes, never inside one; a test supplies its own to act
    /// while it runs. Every path that reaches a check runs exactly one, so an
    /// unknown username costs what a wrong password does.
    fn login_with(
        &self,
        username: &str,
        password: &str,
        verify: impl Fn(&str, &str) -> (bool, bool),
    ) -> Result<LoginResult> {
        let canonical = Self::canonical(username);
        let now = self.now();
        let Some(checked) = self.user_row(&canonical) else {
            return self.fail_unknown(&canonical, password, verify);
        };
        if authn::is_locked(checked.locked_until_ts, now) {
            return Err(Self::locked(&checked, now, &canonical));
        }
        let (ok, needs_rehash) = verify(
            &Self::credential(&canonical, &checked, password),
            &checked.password_hash,
        );
        // The upgrade of an older record is scrypt too: also before the lock.
        let rehashed = (ok && needs_rehash && !checked.disabled)
            .then(|| authn::hash_password(password).ok())
            .flatten();

        let mut stored = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut db = stored.1.clone();
        // The outcome is recorded at the time read here, under the lock, so
        // recorded times follow lock order. A failure below the lockout
        // threshold stores a lock that ends when it was recorded; judged
        // against the earlier pre-check time it would refuse this attempt.
        let now = self.now();
        // Deleted, or its password record replaced, while the check ran. The
        // attempt is refused whatever the check found and nothing is recorded:
        // the change already revoked the account's sign-ins.
        let Some(user) = db
            .users
            .get_mut(&canonical)
            .filter(|user| same_credential(&checked, user))
        else {
            return Err(HarnessError::auth_login_failed());
        };
        // A failure recorded while this check ran may have started a lockout.
        // Refuse as for any attempt made during one, whatever this check
        // found, so parallel attempts learn no more than serial ones.
        if authn::is_locked(user.locked_until_ts, now) {
            return Err(Self::locked(user, now, &canonical));
        }
        // The count and the disabled flag are read now, not from the copy: a
        // failure or a disable that landed meanwhile is kept.
        if user.disabled || !ok {
            user.failed_count = user.failed_count.saturating_add(1);
            user.locked_until_ts = Some(authn::next_lock_until(user.failed_count, now));
            self.persist(&mut stored, &db)?;
            return Err(HarnessError::auth_login_failed());
        }
        user.failed_count = 0;
        user.locked_until_ts = None;
        user.last_login_ts = Some(now);
        if let Some(record) = rehashed {
            user.password_hash = record;
        }
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut stored, &db)?;
        Ok(result)
    }

    /// A sign-in for a name with no account, answered as a wrong password for
    /// a real one would be: refused as locked before any check while its
    /// remembered failures say so, otherwise one check against the dummy
    /// record and one counted failure. Status, body and lockout timing then
    /// do not say whether the name exists. It writes nothing to the store, as
    /// before: a real failure's one store rewrite (a few milliseconds) is the
    /// remaining difference, measured within sign-in noise.
    fn fail_unknown(
        &self,
        canonical: &str,
        password: &str,
        verify: impl Fn(&str, &str) -> (bool, bool),
    ) -> Result<LoginResult> {
        let key = authn::hash_token(canonical);
        let now = self.now();
        if let Some(until) = self.unknown_locked_until(&key, now) {
            return Err(HarnessError::auth_locked((until - now).max(0.0), canonical));
        }
        let _ = verify(password, dummy_record());
        // An account created under this name while the check ran: the check
        // spoke for no record of it, so nothing is recorded.
        if self.user_row(canonical).is_some() {
            return Err(HarnessError::auth_login_failed());
        }
        // The time is read again at record time, as `login_with` does, and a
        // lockout that started while this check ran refuses it.
        let now = self.now();
        if let Err(until) = self.record_unknown_failure(key, now) {
            return Err(HarnessError::auth_locked((until - now).max(0.0), canonical));
        }
        Err(HarnessError::auth_login_failed())
    }

    /// When the remembered failures of a name with no account lock it out.
    fn unknown_locked_until(&self, key: &str, now: f64) -> Option<f64> {
        let failures = self.unknown_failures.lock().unwrap_or_else(|p| p.into_inner());
        let until = failures.get(key)?.locked_until_ts;
        authn::is_locked(Some(until), now).then_some(until)
    }

    /// Counts one failure for `key` as `login_with` counts one for a real
    /// account, or returns the end of a lockout that started while the check
    /// ran: the check and the count are one step under the table's lock, so
    /// overlapping attempts count like serial ones. At the cap, names idle longer than
    /// the longest lockout are forgotten first, then the least recently failed
    /// name that is not locked right now, and a locked one only when every
    /// remembered name is locked: forgetting a locked name would answer it 401
    /// where a real account answers 423. A forgotten name starts again from
    /// zero, which a real account never does, so the cap is set well above
    /// what the rate limit lets one client reach in an hour.
    fn record_unknown_failure(&self, key: String, now: f64) -> std::result::Result<(), f64> {
        let mut failures = self.unknown_failures.lock().unwrap_or_else(|p| p.into_inner());
        if !failures.contains_key(&key) && failures.len() >= UNKNOWN_FAILURE_ENTRIES {
            failures.retain(|_, f| now - f.last_ts < authn::LOCKOUT_CEILING_SEC);
            if failures.len() >= UNKNOWN_FAILURE_ENTRIES {
                let oldest = |locked: bool| {
                    failures
                        .iter()
                        .filter(|(_, f)| authn::is_locked(Some(f.locked_until_ts), now) == locked)
                        .min_by(|a, b| a.1.last_ts.total_cmp(&b.1.last_ts))
                        .map(|(k, _)| k.clone())
                };
                let oldest = oldest(false).or_else(|| oldest(true));
                if let Some(oldest) = oldest {
                    failures.remove(&oldest);
                }
            }
        }
        let entry = failures.entry(key).or_insert(UnknownFailures {
            count: 0,
            locked_until_ts: 0.0,
            last_ts: now,
        });
        if authn::is_locked(Some(entry.locked_until_ts), now) {
            return Err(entry.locked_until_ts);
        }
        entry.count = entry.count.saturating_add(1);
        entry.locked_until_ts = authn::next_lock_until(entry.count, now);
        entry.last_ts = now;
        Ok(())
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
        let row = stored.1.sessions.get(&key).filter(|row| !row.revoked)?;
        // Limits follow the account's current state, so a restricted account's
        // older session cannot outlive the restricted window either.
        let restricted = stored
            .1
            .users
            .get(&row.username)
            .is_some_and(|u| u.must_change_password);
        let (idle_timeout_sec, absolute_timeout_sec) = self.session_limits(restricted);
        let expired = now >= row.expires_ts
            || now >= row.created_ts + absolute_timeout_sec
            || now >= row.last_seen_ts + idle_timeout_sec;
        let username = row.username.clone();
        // Every guarded request lands here, so slide the idle window at most once
        // per interval instead of writing SQLite each time. A skipped slide can
        // only end a session sooner, never later; an expiry is always written.
        if expired || now - row.last_seen_ts >= Self::touch_interval_sec(idle_timeout_sec) {
            let last_seen_ts = if expired { row.last_seen_ts } else { now };
            // Committed before visible, like `persist`, but only this row is written.
            sqlite::touch_session(&stored.0, &key, last_seen_ts, expired).ok()?;
            let row = stored.1.sessions.get_mut(&key)?;
            row.last_seen_ts = last_seen_ts;
            row.revoked = expired;
        }
        if expired || stored.1.users.get(&username).is_none_or(|user| user.disabled) {
            return None;
        }
        Some(SessionInfo {
            session_id: session_id.to_string(),
            username,
        })
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

/// `<home>/setup-code`, beside the account store.
fn setup_code_path(store: &Path) -> PathBuf {
    store
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(authn::SETUP_CODE_FILE)
}

fn count_enabled_admins(db: &AuthDb) -> usize {
    db.users
        .values()
        .filter(|u| authz::is_role(&u.role, authz::Role::Admin) && !u.disabled)
        .count()
}

fn revoke_sessions_for(db: &mut AuthDb, username: &str) {
    for row in db.sessions.values_mut() {
        if row.username == username {
            row.revoked = true;
        }
    }
    *db.revocations.entry(username.to_string()).or_default() += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every action that revokes an account's sign-ins moves its counter, a
    /// password reset included; signing in, logging out and other accounts'
    /// changes do not.
    #[test]
    fn each_sign_in_revocation_is_counted_for_its_account_only() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let auth = AuthManager::open(&dir.path().join("auth.db"), &cfg).unwrap();
        let password = crate::common::random_hex(24);
        auth.create_user("second-admin", &password, "admin").unwrap();
        auth.create_user("alice", &password, "operator").unwrap();
        let alice = auth.get_user("alice").unwrap();
        let seen = auth.sign_in_revocations("alice");
        let session = auth.login("alice", &password).unwrap().session_id;
        assert!(auth.logout(&session).unwrap());
        auth.set_role("second-admin", "operator").unwrap();
        assert_eq!(auth.sign_in_revocations("Alice "), seen, "not alice's sign-ins");
        assert!(auth.account_unchanged(&alice, Some(seen)));

        let mut count = seen;
        let next = crate::common::random_hex(24);
        let changes: [(&str, &dyn Fn()); 4] = [
            ("role", &|| auth.set_role("alice", "auditor").unwrap()),
            ("password reset", &|| auth.set_password("alice", &next).unwrap()),
            ("password change", &|| {
                auth.change_password("alice", &next, &password).unwrap();
            }),
            ("disable", &|| auth.disable_user("alice").unwrap()),
        ];
        for (what, change) in changes {
            change();
            assert_eq!(auth.sign_in_revocations("alice"), count + 1, "{what}");
            count += 1;
            assert!(!auth.account_unchanged(&alice, Some(seen)), "{what}");
        }
        // Without a count the answer is the earlier rule: same enabled account.
        assert!(!auth.account_unchanged(&alice, None), "disabled");
        auth.enable_user("alice").unwrap();
        assert!(auth.account_unchanged(&alice, None));
        assert!(!auth.account_unchanged(&alice, Some(seen)));
        auth.delete_user("alice").unwrap();
        assert_eq!(auth.sign_in_revocations("alice"), count + 1, "delete");
        assert!(!auth.account_unchanged(&alice, None));
    }

    fn store() -> (tempfile::TempDir, AuthManager) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let auth = AuthManager::open(&dir.path().join("auth.db"), &cfg).unwrap();
        (dir, auth)
    }

    /// A password check that runs `change` before it answers, as another
    /// request can while the lock is released.
    fn check_then<'a>(change: impl Fn() + 'a) -> impl Fn(&str, &str) -> (bool, bool) + 'a {
        move |password, record| {
            let verdict = authn::verify_password(password, record);
            change();
            verdict
        }
    }

    fn failed_count(auth: &AuthManager, username: &str) -> u32 {
        auth.get_user(username).unwrap().failed_count
    }

    fn live_sessions(auth: &AuthManager, username: &str) -> usize {
        let stored = auth.state.lock().unwrap();
        let sessions = stored.1.sessions.values();
        sessions.filter(|s| s.username == username && !s.revoked).count()
    }

    /// scrypt under the account lock stalls every request's session check, so
    /// each check runs with the lock free. Unknown, wrong and disabled each pay
    /// exactly one check: the timing an attacker sees does not name accounts.
    #[test]
    fn each_password_check_runs_once_with_the_account_lock_released() {
        let (_dir, auth) = store();
        let (password, wrong, next) = (
            crate::common::random_hex(24),
            crate::common::random_hex(24),
            crate::common::random_hex(24),
        );
        auth.create_user("alice", &password, "operator").unwrap();
        auth.create_user("gone", &password, "operator").unwrap();
        auth.disable_user("gone").unwrap();
        let checks = std::cell::Cell::new(0);
        let unlocked = |password: &str, record: &str| {
            checks.set(checks.get() + 1);
            assert!(auth.state.try_lock().is_ok(), "a password check held the account lock");
            authn::verify_password(password, record)
        };
        for (name, typed) in [("nobody", &password), ("alice", &wrong), ("gone", &password)] {
            let error = auth.login_with(name, typed, unlocked).unwrap_err();
            assert_eq!(error.code, "AUTH_LOGIN_FAILED", "{name}");
            assert_eq!(checks.replace(0), 1, "{name}");
        }
        auth.login_with("alice", &password, unlocked).unwrap();
        assert_eq!(checks.replace(0), 1, "sign-in");
        auth.change_password_with("alice", &password, &next, unlocked).unwrap();
        assert_eq!(checks.replace(0), 1, "password change");
    }

    /// A verdict is recorded only against the record it was checked on: a
    /// reset, disable or delete that lands while a check runs is never undone
    /// or bypassed by it.
    #[test]
    fn an_account_change_during_a_password_check_wins() {
        let (_dir, auth) = store();
        let [password, next, third] = [(); 3].map(|_| crate::common::random_hex(24));
        auth.create_user("alice", &password, "operator").unwrap();

        // The right password, replaced while it was checked.
        let reset = check_then(|| auth.set_password("alice", &next).unwrap());
        let error = auth.login_with("alice", &password, reset).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        assert_eq!(live_sessions(&auth, "alice"), 0);
        assert_eq!(failed_count(&auth, "alice"), 0, "a stale check records nothing");

        // The right password for an account disabled meanwhile: a counted failure.
        let disable = check_then(|| auth.disable_user("alice").unwrap());
        let error = auth.login_with("alice", &next, disable).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        assert_eq!(live_sessions(&auth, "alice"), 0);
        assert_eq!(failed_count(&auth, "alice"), 1);
        auth.enable_user("alice").unwrap();

        // A password change whose current password was reset meanwhile.
        let reset = check_then(|| auth.set_password("alice", &third).unwrap());
        let error = auth.change_password_with("alice", &next, &password, reset).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        assert_eq!(live_sessions(&auth, "alice"), 0);
        auth.login("alice", &third).expect("the reset password stands");

        let delete = check_then(|| auth.delete_user("alice").unwrap());
        let error = auth.login_with("alice", &third, delete).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        assert!(auth.get_user("alice").is_none());
    }

    /// Checks that overlap count every failure, and a lockout that starts
    /// while a check runs refuses that attempt even when it had the right
    /// password: running checks side by side buys no extra guesses.
    #[test]
    fn overlapping_attempts_count_and_lock_out_like_serial_ones() {
        let (_dir, auth) = store();
        let (password, wrong) = (crate::common::random_hex(24), crate::common::random_hex(24));
        auth.create_user("alice", &password, "operator").unwrap();
        let another_failure = || {
            let error = auth.login("alice", &wrong).unwrap_err();
            assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        };
        for expected in [2, 4] {
            let error = auth
                .login_with("alice", &wrong, check_then(another_failure))
                .unwrap_err();
            assert_eq!(error.code, "AUTH_LOGIN_FAILED");
            assert_eq!(
                failed_count(&auth, "alice"),
                expected,
                "both overlapping failures count"
            );
        }
        // The fifth failure starts the lockout during a check of the right password.
        let error = auth
            .login_with("alice", &password, check_then(another_failure))
            .unwrap_err();
        assert_eq!(error.code, "AUTH_ACCOUNT_LOCKED");
        assert_eq!(live_sessions(&auth, "alice"), 0);
        assert_eq!(
            failed_count(&auth, "alice"),
            5,
            "the refused attempt did not clear the count"
        );
    }

    #[test]
    fn session_timeouts_outside_one_minute_to_thirty_days_refuse_before_the_store_opens() {
        let dir = tempfile::tempdir().unwrap();
        for (key, value) in [
            ("idle_timeout_sec", "59"),
            ("absolute_timeout_sec", "2592001"),
            ("password_change_idle_timeout_sec", "0"),
            ("password_change_absolute_timeout_sec", "-1"),
        ] {
            let yaml = format!("auth: {{session: {{{key}: {value}}}}}");
            let cfg = AppConfig::from_str(&yaml, Path::new("config.yaml")).unwrap();
            let error = AuthManager::open(&dir.path().join("auth.json"), &cfg).unwrap_err();
            assert_eq!(error.code, "CONFIG_ERROR", "{yaml}");
            assert_eq!(
                error.message,
                format!("auth.session.{key} must be a finite number from 60 to 2592000")
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "a refused timeout creates no account store"
        );
    }

    /// What one sign-in attempt showed a client.
    #[derive(Debug, PartialEq)]
    struct Seen {
        code: String,
        retry_after: Option<f64>,
        checks: u32,
    }

    fn attempt(auth: &AuthManager, name: &str, password: &str) -> Seen {
        let checks = std::cell::Cell::new(0);
        let counted = |password: &str, record: &str| {
            checks.set(checks.get() + 1);
            authn::verify_password(password, record)
        };
        let error = auth.login_with(name, password, counted).unwrap_err();
        Seen {
            code: error.code.clone(),
            retry_after: error.details["retry_after_sec"].as_f64(),
            checks: checks.get(),
        }
    }

    /// A guesser must not learn which names exist from the lockout: a name
    /// with no account answers every attempt as a real account with a wrong
    /// password does (same code, same wait, same password checks), through
    /// the threshold, the lockout and the longer lockout after it.
    #[test]
    fn a_name_with_no_account_locks_out_exactly_like_a_real_one() {
        let (_dir, mut auth) = store();
        let clock = std::sync::Arc::new(std::sync::Mutex::new(1_000.0_f64));
        let read = clock.clone();
        auth.set_clock(Box::new(move || *read.lock().unwrap()));
        let [password, wrong] = [(); 2].map(|_| crate::common::random_hex(24));
        auth.create_user("alice", &password, "operator").unwrap();
        let tick = |by: f64| *clock.lock().unwrap() += by;

        let mut real = Vec::new();
        let mut unknown = Vec::new();
        // Five counted failures, then refusals during the 2 s lockout, then
        // one more failure once it ends, which starts a 4 s lockout.
        for step in 0..9 {
            if step == 7 {
                tick(2.5);
            }
            real.push(attempt(&auth, "alice", &wrong));
            unknown.push(attempt(&auth, "nobody", &wrong));
            tick(0.1);
        }
        assert_eq!(real, unknown, "the two names must look the same");
        let codes: Vec<&str> = real.iter().map(|seen| seen.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "AUTH_LOGIN_FAILED",
                "AUTH_LOGIN_FAILED",
                "AUTH_LOGIN_FAILED",
                "AUTH_LOGIN_FAILED",
                "AUTH_LOGIN_FAILED",
                "AUTH_ACCOUNT_LOCKED",
                "AUTH_ACCOUNT_LOCKED",
                "AUTH_LOGIN_FAILED",
                "AUTH_ACCOUNT_LOCKED",
            ]
        );
        for seen in &real {
            let counted = seen.code == "AUTH_LOGIN_FAILED";
            assert_eq!(seen.checks, u32::from(counted), "a locked name pays no check");
        }
        assert!(real[8].retry_after.unwrap() > 3.0, "the sixth failure locks for 4 s");
        assert_eq!(failed_count(&auth, "alice"), 6);
        // The unknown name was never written to the account store.
        assert!(auth.get_user("nobody").is_none());
        // The table holds a hash of the name, never the name itself.
        let failures = auth.unknown_failures.lock().unwrap();
        assert!(failures.contains_key(&authn::hash_token("nobody")));
        assert!(!failures.contains_key("nobody"));
    }

    /// A flood of made-up names stops growing memory at the cap. Names idle
    /// past the longest lockout go first, then the least recently failed; a
    /// name that is locked right now survives a flood that arrives later.
    #[test]
    fn remembered_unknown_names_stay_within_the_cap() {
        let (_dir, auth) = store();
        let key = |i: usize| authn::hash_token(&format!("name{i}"));
        let _ = auth.record_unknown_failure(key(0), 0.0);
        for _ in 0..5 {
            let _ = auth.record_unknown_failure(key(1), 1_000.0);
        }
        for i in 2..UNKNOWN_FAILURE_ENTRIES {
            let _ = auth.record_unknown_failure(key(i), 1_000.0);
        }
        let full = || auth.unknown_failures.lock().unwrap().len();
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        // The next new name drops every name idle past the longest lockout.
        let _ = auth.record_unknown_failure(key(UNKNOWN_FAILURE_ENTRIES), 1_000.0);
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        assert!(!auth.unknown_failures.lock().unwrap().contains_key(&key(0)));
        // With nothing idle, the least recently failed unlocked name goes, one
        // at a time; the locked name, older than every other, is kept.
        for i in 1..=50 {
            let _ = auth.record_unknown_failure(key(UNKNOWN_FAILURE_ENTRIES + i), 1_000.0 + i as f64 * 0.01);
            assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        }
        assert!(
            auth.unknown_locked_until(&key(1), 1_000.6).is_some(),
            "a locked name survives the flood"
        );
        // A name already remembered is counted in place, never evicting another.
        let _ = auth.record_unknown_failure(key(UNKNOWN_FAILURE_ENTRIES + 50), 1_100.0);
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
    }
}
