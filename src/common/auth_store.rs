//! Users, sessions, lockout and bootstrap backed by transactional SQLite.
//! Legacy auth.json records are migrated once; failures never create a new admin.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

#[path = "auth_sqlite.rs"]
mod sqlite;
use super::authn;
use super::authz;
use super::config::AppConfig;
use super::errors::{HarnessError, Result};

pub const BOOTSTRAP_USERNAME: &str = "admin";
/// Accounts one store holds. `create_user` refuses the next one; loading or
/// persisting a store past it fails closed (`sqlite::validate`).
const MAX_USERS: usize = 128;
/// Live sign-in sessions across every account, split the same way:
/// `create_session` refuses the next one, `sqlite::validate` a store past it.
const MAX_LIVE_SESSIONS: usize = 4096;
/// Live sessions one account may hold (`auth.session.max_per_account`): the
/// next sign-in past it ends that account's least recently used session, never
/// another account's. The ceiling an operator may set is the largest value that
/// keeps every account at its limit inside [`MAX_LIVE_SESSIONS`], so no mix of
/// sign-ins can fill the table and lock out an account that holds fewer.
const DEFAULT_MAX_SESSIONS_PER_ACCOUNT: u64 = 8;
const MAX_SESSIONS_PER_ACCOUNT_CEILING: u64 = (MAX_LIVE_SESSIONS / MAX_USERS) as u64;
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

/// Why an account no longer permits work that began under it. The one
/// vocabulary for the mid-turn and per-occurrence rechecks, so each caller maps
/// it to its own error code and none restates the conditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLapse {
    /// The account no longer exists.
    Missing,
    Disabled,
    /// Still on an initial or reset password.
    MustChangePassword,
    /// The stored role no longer grants the capability (an unparseable role fails closed).
    Role(String),
}

impl UserSummary {
    /// Why this account may not use `capability` now, or `None` when it may.
    /// The single definition of "still permitted": enabled, past its setup
    /// password, and a role that grants the capability.
    pub fn lapse(&self, capability: authz::Capability) -> Option<OwnerLapse> {
        if self.disabled {
            Some(OwnerLapse::Disabled)
        } else if self.must_change_password {
            Some(OwnerLapse::MustChangePassword)
        } else if !authz::grants_stored(&self.role, capability) {
            Some(OwnerLapse::Role(self.role.clone()))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoginResult {
    pub username: String,
    pub session_id: String,
    pub expires_ts: f64,
    /// This account's older sessions ended to make room under
    /// `auth.session.max_per_account`; the caller audits a non-zero count.
    pub evicted: usize,
}

/// One judgement of a session row (`AuthManager::judge_session`).
struct SessionSeen {
    username: String,
    expired: bool,
    /// The `last_seen_ts` to commit, when this lookup must write.
    write: Option<f64>,
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
/// `count_unknown_failure` for what is forgotten first.
const UNKNOWN_FAILURE_ENTRIES: usize = 16_384;

/// Sign-in failures for a username with no account. They follow the arithmetic
/// of `UserRow::failed_count` and `locked_until_ts`, and are stored with it
/// (`auth_sqlite::record_login_failure`) so they survive a restart as an
/// account's do: a name with no account locks out exactly when a real one
/// would, otherwise the first `AUTH_ACCOUNT_LOCKED` would tell a guesser which
/// names exist. Forgetting at the cap is the one remaining difference. Only a
/// name that passes `authn::validate_username` is stored; one that fails can
/// never be an account, so it is counted in memory alone.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UnknownFailures {
    count: u32,
    locked_until_ts: f64,
    last_ts: f64,
}

/// Counts one failure for `key` at `now` and returns the names forgotten to
/// make room, with what they held, or returns the end of an active lockout;
/// the check and the count are one step, under the caller's lock.
/// At the cap, names idle longer than the longest lockout are forgotten
/// first, then the least recently failed name that is not locked at `now`,
/// and a locked one only when every remembered name is locked: forgetting a
/// locked name would answer it 401 where a real account answers 423. A
/// forgotten name starts again from zero, which a real account never does,
/// so the cap is set well above what the rate limit lets one client reach in
/// an hour.
fn count_unknown_failure(
    failures: &mut HashMap<String, UnknownFailures>,
    key: String,
    now: f64,
) -> std::result::Result<Vec<(String, UnknownFailures)>, f64> {
    let mut forgotten = Vec::new();
    if !failures.contains_key(&key) && failures.len() >= UNKNOWN_FAILURE_ENTRIES {
        failures.retain(|k, f| {
            let keep = now - f.last_ts < authn::LOCKOUT_CEILING_SEC;
            if !keep {
                forgotten.push((k.clone(), *f));
            }
            keep
        });
        if failures.len() >= UNKNOWN_FAILURE_ENTRIES {
            let oldest = |locked: bool| {
                failures
                    .iter()
                    .filter(|(_, f)| authn::is_locked(Some(f.locked_until_ts), now) == locked)
                    .min_by(|a, b| a.1.last_ts.total_cmp(&b.1.last_ts))
                    .map(|(k, _)| k.clone())
            };
            if let Some(oldest) = oldest(false).or_else(|| oldest(true)) {
                if let Some(f) = failures.remove(&oldest) {
                    forgotten.push((oldest, f));
                }
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
    Ok(forgotten)
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
    max_sessions_per_account: usize,
    /// The store's one SQLite connection. Every change takes it, reads
    /// `committed`, writes, commits, then publishes: changes are serialized
    /// and each starts from the last committed state.
    writer: Mutex<rusqlite::Connection>,
    /// The last committed state, which every reader reads. It is replaced
    /// only after a commit, so a reader never sees an uncommitted change, and
    /// held only for a copy or a pointer swap, never across I/O: a reader does
    /// not wait for a write in flight, so it may run on an async worker.
    committed: RwLock<Arc<AuthDb>>,
    /// Keyed by an HMAC of the canonical name under `name_key`, so no typed
    /// username (or a password typed into the username field) is held or
    /// stored. Taken before `writer` when both are held, never after, and
    /// never held together with `committed`.
    unknown_failures: Mutex<HashMap<String, UnknownFailures>>,
    /// The store's random key for `unknown_failures` names.
    name_key: [u8; 32],
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
        let max_sessions_per_account = cfg.u64_in(
            "auth.session.max_per_account",
            DEFAULT_MAX_SESSIONS_PER_ACCOUNT,
            1..=MAX_SESSIONS_PER_ACCOUNT_CEILING,
        )? as usize;
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
        let (name_key, remembered) = sqlite::load_login_failures(&mut connection, UNKNOWN_FAILURE_ENTRIES)?;
        let remembered = remembered
            .into_iter()
            .map(|(key, (count, locked_until_ts, last_ts))| {
                let failures = UnknownFailures {
                    count,
                    locked_until_ts,
                    last_ts,
                };
                (key, failures)
            })
            .collect();
        let mgr = Self {
            path: path.to_path_buf(),
            idle_timeout_sec,
            absolute_timeout_sec,
            restricted_idle_timeout_sec,
            restricted_absolute_timeout_sec,
            max_sessions_per_account,
            writer: Mutex::new(connection),
            committed: RwLock::new(Arc::new(db)),
            unknown_failures: Mutex::new(remembered),
            name_key,
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

    /// The committed state, as one consistent snapshot. Never waits on I/O.
    fn snapshot(&self) -> Arc<AuthDb> {
        self.committed.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Starts a change: the connection, held until the change is published
    /// or dropped, and a copy of the committed state to edit.
    fn begin(&self) -> (MutexGuard<'_, rusqlite::Connection>, AuthDb) {
        let conn = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        let db = (*self.snapshot()).clone();
        (conn, db)
    }

    /// Commits `db`, then makes it visible: committed before visible.
    fn persist(&self, conn: &mut rusqlite::Connection, db: AuthDb) -> Result<()> {
        sqlite::persist(conn, &db)?;
        *self.committed.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(db);
        Ok(())
    }

    /// The stored account, copied out so a password can be checked against it
    /// with the lock released: one scrypt run under the lock would stall every
    /// request's session check for its whole duration.
    fn user_row(&self, canonical: &str) -> Option<UserRow> {
        self.snapshot().users.get(canonical).cloned()
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
        let conn = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        sqlite::pending_role_migrations(&conn)
    }

    /// The fold for `user_id` is in the audit log: drop its pending report.
    pub fn acknowledge_role_migration(&self, user_id: &str) -> Result<()> {
        let conn = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        sqlite::acknowledge_role_migration(&conn, user_id)
    }

    /// True while the enabled `admin` account still signs in with its setup
    /// code: it must replace its password and the private code file exists.
    /// The sign-in screen then names the code, never shows it. Answers without
    /// hashing, so the public setup-status route stays cheap.
    pub fn setup_code_active(&self) -> bool {
        self.snapshot()
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
        let (mut conn, mut db) = self.begin();
        if db.users.len() >= MAX_USERS {
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
        self.persist(&mut conn, db)?;
        Ok(canonical)
    }

    pub fn get_user(&self, username: &str) -> Option<UserSummary> {
        let canonical = Self::canonical(username);
        let db = self.snapshot();
        db.users.get(&canonical).map(|u| Self::summary(&canonical, u))
    }

    /// How many times this account's sign-ins have been revoked since the
    /// server started: a role change, a password set or change, a disable and
    /// a delete each count. An admin password reset leaves no other trace on
    /// the account (it clears `must_change_password`), and a session's
    /// `revoked` flag is also set by ordinary expiry.
    pub fn sign_in_revocations(&self, username: &str) -> u64 {
        self.snapshot()
            .revocations
            .get(&Self::canonical(username))
            .copied()
            .unwrap_or(0)
    }

    /// Whether `account`, as read when some work began, is still the same
    /// enabled account. With `revocations` (the `sign_in_revocations` value
    /// read then), also whether its sign-ins have not been revoked since.
    /// One snapshot, so an account change cannot land between the two answers.
    pub fn account_unchanged(&self, account: &UserSummary, revocations: Option<u64>) -> bool {
        let canonical = Self::canonical(&account.username);
        let db = self.snapshot();
        db.users
            .get(&canonical)
            .is_some_and(|current| current.user_id == account.user_id && !current.disabled)
            && revocations.is_none_or(|seen| db.revocations.get(&canonical).copied().unwrap_or(0) == seen)
    }

    /// Why the account `user_id` may no longer use `capability`, read live from
    /// the last committed state, or `None` when it still may. Recheck with this
    /// after any wait and before a send or stored delivery; the callers own the
    /// error code. Do not rebuild the conditions from `list_users`
    /// (`invariant_guard` fails a copy).
    pub fn owner_lapse(&self, user_id: &str, capability: authz::Capability) -> Option<OwnerLapse> {
        match self.snapshot().users.iter().find(|(_, row)| row.user_id == user_id) {
            Some((username, row)) => Self::summary(username, row).lapse(capability),
            None => Some(OwnerLapse::Missing),
        }
    }

    /// [`Self::owner_lapse`] as a yes or no.
    pub fn owner_permits(&self, user_id: &str, capability: authz::Capability) -> bool {
        self.owner_lapse(user_id, capability).is_none()
    }

    pub fn list_users(&self) -> Vec<UserSummary> {
        let db = self.snapshot();
        db.users.iter().map(|(k, v)| Self::summary(k, v)).collect()
    }

    pub fn count_enabled_admins(&self) -> usize {
        let db = self.snapshot();
        count_enabled_admins(&db)
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
        let (mut conn, mut db) = self.begin();
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
        self.persist(&mut conn, db)
    }

    pub fn delete_user(&self, username: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let (mut conn, mut db) = self.begin();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if Self::is_last_enabled_admin(&db, &canonical) {
            return Err(HarnessError::auth_last_admin());
        }
        db.users.remove(&canonical);
        db.sessions.retain(|_, row| row.username != canonical);
        *db.revocations.entry(canonical.clone()).or_default() += 1;
        self.persist(&mut conn, db)
    }

    fn set_disabled(&self, username: &str, disabled: bool) -> Result<()> {
        let canonical = Self::canonical(username);
        let (mut conn, mut db) = self.begin();
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
        self.persist(&mut conn, db)
    }

    pub fn disable_user(&self, username: &str) -> Result<()> {
        self.set_disabled(username, true)
    }

    /// The account stays present and must set a new password before automation runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn require_password_change(&self, username: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let (mut conn, mut db) = self.begin();
        if !db.users.contains_key(&canonical) {
            return Err(HarnessError::auth_user_not_found(&canonical));
        }
        if let Some(user) = db.users.get_mut(&canonical) {
            user.must_change_password = true;
        }
        revoke_sessions_for(&mut db, &canonical);
        self.persist(&mut conn, db)
    }

    pub fn enable_user(&self, username: &str) -> Result<()> {
        self.set_disabled(username, false)
    }

    pub fn set_password(&self, username: &str, password: &str) -> Result<()> {
        let canonical = Self::canonical(username);
        let record = authn::hash_password(password)?;
        let (mut conn, mut db) = self.begin();
        let Some(u) = db.users.get_mut(&canonical) else {
            return Err(HarnessError::auth_user_not_found(&canonical));
        };
        u.password_hash = record;
        u.must_change_password = false;
        u.failed_count = 0;
        u.locked_until_ts = None;
        revoke_sessions_for(&mut db, &canonical);
        self.persist(&mut conn, db)?;
        self.forget_setup_code(&canonical);
        Ok(())
    }

    pub fn change_password(&self, username: &str, current: &str, password: &str) -> Result<LoginResult> {
        self.change_password_with(username, current, password, authn::verify_password)
    }

    /// `change_password` with the check of the current password supplied. The
    /// check runs between two short lock scopes, never inside one; a test
    /// supplies its own to act while it runs.
    ///
    /// The current password is a guess at the account's password, so it
    /// shares sign-in's failure count and lockout: a wrong one counts, and an
    /// active lockout refuses the change before any check. A session cookie
    /// therefore buys no more guesses than the sign-in form allows.
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
        if authn::is_locked(checked.locked_until_ts, now) {
            return Err(Self::locked(&checked, now, &canonical));
        }
        let current = Self::credential(&canonical, &checked, current);
        // One check on every path, a disabled account's included, as at sign-in.
        let (ok, _) = verify(&current, &checked.password_hash);
        let (mut conn, mut db) = self.begin();
        // Recorded at the time read under the lock, as `login_with` does.
        let now = self.now();
        // An account deleted or given another password while the check ran
        // keeps that change, and the attempt records nothing: the checked
        // password no longer speaks for it.
        let Some(user) = db
            .users
            .get_mut(&canonical)
            .filter(|user| same_credential(&checked, user))
        else {
            return Err(HarnessError::auth_login_failed());
        };
        // A lockout that started while this check ran refuses it, right
        // password or not, so overlapping attempts learn no more than serial ones.
        if authn::is_locked(user.locked_until_ts, now) {
            return Err(Self::locked(user, now, &canonical));
        }
        // The count and the disabled flag are read now: a failure or a
        // disable that landed meanwhile is kept.
        if user.disabled || !ok {
            user.failed_count = user.failed_count.saturating_add(1);
            user.locked_until_ts = Some(authn::next_lock_until(user.failed_count, now));
            self.persist(&mut conn, db)?;
            return Err(HarnessError::auth_login_failed());
        }
        // A forced change must actually change the credential: the setup code
        // has been on screen and in a file, so it may not become the password.
        // Refused only after the lockout recheck above, so a right guess made
        // while a lockout started is not told apart from a wrong one.
        if Self::credential(&canonical, &checked, password) == current {
            return Err(HarnessError::password_policy(
                "the new password must differ from the current one",
            ));
        }
        user.password_hash = record;
        user.must_change_password = false;
        user.failed_count = 0;
        user.locked_until_ts = None;
        revoke_sessions_for(&mut db, &canonical);
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut conn, db)?;
        self.forget_setup_code(&canonical);
        Ok(result)
    }

    fn create_session(&self, db: &mut AuthDb, username: &str, now: f64) -> Result<LoginResult> {
        let (_, absolute_timeout_sec) =
            self.session_limits(db.users.get(username).is_some_and(|u| u.must_change_password));
        db.sessions
            .retain(|_, row| !row.revoked && row.expires_ts > now && row.last_seen_ts + self.idle_timeout_sec > now);
        // Room for this account first, from its own sessions only, so a flood of
        // sign-ins from one account cannot spend the table other accounts need.
        let evicted = evict_for_room(db, username, self.max_sessions_per_account);
        if db.sessions.len() >= MAX_LIVE_SESSIONS {
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
            evicted,
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

        let (mut conn, mut db) = self.begin();
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
            self.persist(&mut conn, db)?;
            return Err(HarnessError::auth_login_failed());
        }
        user.failed_count = 0;
        user.locked_until_ts = None;
        user.last_login_ts = Some(now);
        if let Some(record) = rehashed {
            user.password_hash = record;
        }
        let result = self.create_session(&mut db, &canonical, now)?;
        self.persist(&mut conn, db)?;
        Ok(result)
    }

    /// A sign-in for a name with no account, answered as a wrong password for
    /// a real one would be: refused as locked before any check while its
    /// remembered failures say so, otherwise one check against the dummy
    /// record and one counted failure, stored before it is answered as a real
    /// failure is. Status, body and lockout timing then do not say whether
    /// the name exists, across restarts too. A real failure rewrites the
    /// account tables and this writes one row: both are one synchronous
    /// commit, so the difference is smaller than the earlier no-write path's.
    fn fail_unknown(
        &self,
        canonical: &str,
        password: &str,
        verify: impl Fn(&str, &str) -> (bool, bool),
    ) -> Result<LoginResult> {
        let key = self.name_hash(canonical);
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
        // Only a name that could be an account is stored: one that never can
        // reveals nothing by starting again at a restart, and a password
        // typed into the username field often fails validation, so it stays
        // off disk.
        let store = authn::validate_username(canonical).is_ok();
        // A lockout that started while this check ran refuses it.
        if let Err(retry_after) = self.record_unknown_failure(key, store)? {
            return Err(HarnessError::auth_locked(retry_after, canonical));
        }
        Err(HarnessError::auth_login_failed())
    }

    /// The `unknown_failures` key for a canonical name.
    fn name_hash(&self, canonical: &str) -> String {
        let digest = super::audit::hmac_sha256(&self.name_key, canonical.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// When the remembered failures of a name with no account lock it out.
    fn unknown_locked_until(&self, key: &str, now: f64) -> Option<f64> {
        let failures = self.unknown_failures.lock().unwrap_or_else(|p| p.into_inner());
        let until = failures.get(key)?.locked_until_ts;
        authn::is_locked(Some(until), now).then_some(until)
    }

    /// Counts one failure for `key` as `login_with` counts one for a real
    /// account, or returns the wait of a lockout that started while the check
    /// ran. The time is read after the table's lock is taken, as `login_with`
    /// reads it under the store lock: recorded times then follow lock order.
    /// Read before the lock, an attempt could find a later attempt's failure
    /// already recorded, and a failure below the threshold stores a lock that
    /// ends when it was recorded, so that later time would refuse this
    /// attempt as locked where a real account counts it. When `store` is set
    /// the count is stored before either lock is released; a refused write
    /// restores the table and fails the attempt as a refused account write
    /// does. Names forgotten at the cap leave the store either way.
    fn record_unknown_failure(&self, key: String, store: bool) -> Result<std::result::Result<(), f64>> {
        let mut failures = self.unknown_failures.lock().unwrap_or_else(|p| p.into_inner());
        let mut stored = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        let now = self.now();
        let before = failures.get(&key).copied();
        let forgotten = match count_unknown_failure(&mut failures, key.clone(), now) {
            Ok(forgotten) => forgotten,
            Err(until) => return Ok(Err((until - now).max(0.0))),
        };
        if !store && forgotten.is_empty() {
            return Ok(Ok(()));
        }
        let counted = failures[&key];
        let names: Vec<String> = forgotten.iter().map(|(k, _)| k.clone()).collect();
        let row = (counted.count, counted.locked_until_ts, counted.last_ts);
        let counted = store.then_some((key.as_str(), row));
        if let Err(error) = sqlite::record_login_failure(&mut stored, counted, &names) {
            match before {
                Some(before) => failures.insert(key, before),
                None => failures.remove(&key),
            };
            failures.extend(forgotten);
            return Err(error);
        }
        Ok(Ok(()))
    }

    /// Live means: exists, not revoked, within both the absolute expiry and the
    /// idle window. A valid lookup slides the idle window forward.
    ///
    /// Most lookups only read the committed snapshot. A slide or an expiry is
    /// a write: it takes the connection, judges the session again against the
    /// state committed by then, and is visible only once committed. That path
    /// does SQLite I/O, so callers stay on the blocking pool.
    pub fn validate_session(&self, session_id: &str) -> Option<SessionInfo> {
        if session_id.is_empty() {
            return None;
        }
        let key = authn::hash_token(session_id);
        let now = self.now();
        let mut db = self.snapshot();
        let mut seen = self.judge_session(&db, &key, now)?;
        if seen.write.is_some() {
            let conn = self.writer.lock().unwrap_or_else(|p| p.into_inner());
            // Another lookup or change may have committed meanwhile: judge
            // again against what is committed now, at the same `now`.
            db = self.snapshot();
            seen = self.judge_session(&db, &key, now)?;
            if let Some(last_seen_ts) = seen.write {
                // Committed before visible, like `persist`, but only this row is written.
                sqlite::touch_session(&conn, &key, last_seen_ts, seen.expired).ok()?;
                // Drop this lookup's own snapshot first, so the row is updated
                // in place unless a reader still holds the old state. Then the
                // copy is made outside `committed`'s lock, which readers wait
                // on, and only the pointer swap holds it; `writer` keeps any
                // other change out meanwhile.
                drop(db);
                let slide = |state: &mut AuthDb| {
                    if let Some(row) = state.sessions.get_mut(&key) {
                        row.last_seen_ts = last_seen_ts;
                        row.revoked = seen.expired;
                    }
                };
                let mut committed = self.committed.write().unwrap_or_else(|p| p.into_inner());
                if let Some(state) = Arc::get_mut(&mut committed) {
                    slide(state);
                    db = committed.clone();
                } else {
                    let shared = committed.clone();
                    drop(committed);
                    let mut next = (*shared).clone();
                    drop(shared);
                    slide(&mut next);
                    db = Arc::new(next);
                    *self.committed.write().unwrap_or_else(|p| p.into_inner()) = db.clone();
                }
            }
        }
        if seen.expired || db.users.get(&seen.username).is_none_or(|user| user.disabled) {
            return None;
        }
        Some(SessionInfo {
            session_id: session_id.to_string(),
            username: seen.username,
        })
    }

    /// The session `key` as `db` holds it at `now`, or `None` when it is
    /// missing or revoked. Limits follow the account's current state, so a
    /// restricted account's older session cannot outlive the restricted
    /// window either.
    fn judge_session(&self, db: &AuthDb, key: &str, now: f64) -> Option<SessionSeen> {
        let row = db.sessions.get(key).filter(|row| !row.revoked)?;
        let restricted = db.users.get(&row.username).is_some_and(|u| u.must_change_password);
        let (idle_timeout_sec, absolute_timeout_sec) = self.session_limits(restricted);
        let expired = now >= row.expires_ts
            || now >= row.created_ts + absolute_timeout_sec
            || now >= row.last_seen_ts + idle_timeout_sec;
        // Every guarded request lands here, so slide the idle window at most once
        // per interval instead of writing SQLite each time. A skipped slide can
        // only end a session sooner, never later; an expiry is always written.
        let write = (expired || now - row.last_seen_ts >= Self::touch_interval_sec(idle_timeout_sec))
            .then_some(if expired { row.last_seen_ts } else { now });
        Some(SessionSeen {
            username: row.username.clone(),
            expired,
            write,
        })
    }

    pub fn logout(&self, session_id: &str) -> Result<bool> {
        if session_id.is_empty() {
            return Ok(false);
        }
        let key = authn::hash_token(session_id);
        let (mut conn, mut db) = self.begin();
        let hit = match db.sessions.get_mut(&key) {
            Some(row) if !row.revoked => {
                row.revoked = true;
                true
            }
            _ => false,
        };
        if hit {
            self.persist(&mut conn, db)?;
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

/// End `username`'s least recently used sessions until one more fits under
/// `max`, and return how many ended. Ties break on creation time, then on the
/// stored hash, so the choice never depends on map order. Only this account's
/// rows are touched: an evicted browser signs in again and a revoked token is
/// not revived, since nothing here keeps it.
fn evict_for_room(db: &mut AuthDb, username: &str, max: usize) -> usize {
    let mut mine: Vec<(f64, f64, String)> = db
        .sessions
        .iter()
        .filter(|(_, row)| row.username == username)
        .map(|(hash, row)| (row.last_seen_ts, row.created_ts, hash.clone()))
        .collect();
    if mine.len() < max {
        return 0;
    }
    mine.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let ending = mine.len() + 1 - max;
    for (_, _, hash) in mine.into_iter().take(ending) {
        db.sessions.remove(&hash);
    }
    ending
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

    /// `owner_lapse` names why an account stopped being permitted, in the order
    /// the callers' messages rely on, and agrees with `owner_permits`.
    #[test]
    fn owner_lapse_names_why_an_account_no_longer_permits_work() {
        use authz::Capability::OwnData;
        let (_dir, auth) = store();
        let password = crate::common::random_hex(24);
        auth.create_user("alice", &password, "operator").unwrap();
        let id = auth.get_user("alice").unwrap().user_id;
        assert_eq!(auth.owner_lapse(&id, OwnData), None);
        assert!(auth.owner_permits(&id, OwnData));

        auth.set_role("alice", "auditor").unwrap();
        assert_eq!(auth.owner_lapse(&id, OwnData), Some(OwnerLapse::Role("auditor".into())));
        auth.set_role("alice", "operator").unwrap();
        assert!(auth.owner_permits(&id, OwnData));

        auth.require_password_change("alice").unwrap();
        assert_eq!(auth.owner_lapse(&id, OwnData), Some(OwnerLapse::MustChangePassword));
        // A disabled account reports that first, whatever else is true of it.
        auth.disable_user("alice").unwrap();
        assert_eq!(auth.owner_lapse(&id, OwnData), Some(OwnerLapse::Disabled));
        assert!(!auth.owner_permits(&id, OwnData));

        auth.delete_user("alice").unwrap();
        assert_eq!(auth.owner_lapse(&id, OwnData), Some(OwnerLapse::Missing));
        assert_eq!(auth.owner_lapse("not-an-id", OwnData), Some(OwnerLapse::Missing));
    }

    fn store() -> (tempfile::TempDir, AuthManager) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let auth = AuthManager::open(&dir.path().join("auth.db"), &cfg).unwrap();
        (dir, auth)
    }

    /// Admission stops exactly at each cap, before any write. The snapshot is
    /// filled directly: one scrypt hash, not one per account.
    #[test]
    fn admission_refuses_the_account_and_the_session_past_each_cap() {
        let (_dir, auth) = store();
        let password = crate::common::random_hex(24);
        auth.create_user("alice", &password, "admin").unwrap();
        let fill = |users: usize, sessions: usize| {
            let mut committed = auth.committed.write().unwrap();
            let db = Arc::make_mut(&mut committed);
            let template = db.users["alice"].clone();
            for n in 0..users - db.users.len() {
                let user_id = crate::common::random_hex(16);
                db.users.insert(
                    format!("user{n}"),
                    UserRow {
                        user_id,
                        ..template.clone()
                    },
                );
            }
            let now = auth.now();
            for _ in 0..sessions - db.sessions.len() {
                let row = SessionRow {
                    username: "alice".into(),
                    csrf_hash: crate::common::random_hex(32),
                    created_ts: now,
                    last_seen_ts: now,
                    expires_ts: now + 3600.0,
                    revoked: false,
                };
                db.sessions.insert(crate::common::random_hex(32), row);
            }
        };
        let code = |r: Result<String>| r.map(|_| ()).unwrap_err().code;

        fill(MAX_USERS - 1, 0);
        auth.create_user("last", &password, "operator").unwrap();
        assert_eq!(auth.snapshot().users.len(), MAX_USERS);
        assert_eq!(code(auth.create_user("one-more", &password, "operator")), "AUTH_LIMIT");
        assert!(auth.get_user("one-more").is_none(), "a refused account is not kept");

        // One account holds the whole table, as a store flooded before the
        // per-account cap would. Another account is refused at the global cap,
        // before any write; the flooding account's own next sign-in trims it to
        // its limit, after which the other can sign in.
        fill(MAX_USERS, MAX_LIVE_SESSIONS);
        let refused = auth.login("last", &password).map(|_| String::new());
        assert_eq!(code(refused), "AUTH_LIMIT");
        assert_eq!(auth.snapshot().sessions.len(), MAX_LIVE_SESSIONS);
        auth.login("alice", &password).unwrap();
        assert_eq!(live_sessions(&auth, "alice"), DEFAULT_MAX_SESSIONS_PER_ACCOUNT as usize);
        auth.login("last", &password).unwrap();
    }

    fn store_with_cap(cap: u64) -> (tempfile::TempDir, AuthManager) {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!("auth: {{session: {{max_per_account: {cap}}}}}");
        let cfg = AppConfig::from_str(&yaml, Path::new("config.yaml")).unwrap();
        let auth = AuthManager::open(&dir.path().join("auth.db"), &cfg).unwrap();
        (dir, auth)
    }

    /// The session past an account's cap ends that account's least recently
    /// used session, not its newest and not anyone else's. The ended token no
    /// longer validates; the others do.
    #[test]
    fn a_sign_in_past_the_per_account_cap_ends_that_accounts_least_recently_used_session() {
        let (_dir, auth) = store_with_cap(3);
        let password = crate::common::random_hex(24);
        auth.create_user("alice", &password, "operator").unwrap();
        auth.create_user("bob", &password, "operator").unwrap();
        let bob = auth.login("bob", &password).unwrap();
        let first = auth.login("alice", &password).unwrap();
        let second = auth.login("alice", &password).unwrap();
        let third = auth.login("alice", &password).unwrap();
        assert_eq!((first.evicted, second.evicted, third.evicted), (0, 0, 0));
        // Use the oldest sign-in, so the second is now the least recently used.
        {
            let mut committed = auth.committed.write().unwrap();
            let now = auth.now();
            let touched = authn::hash_token(&first.session_id);
            Arc::make_mut(&mut committed)
                .sessions
                .get_mut(&touched)
                .unwrap()
                .last_seen_ts = now + 5.0;
        }
        let fourth = auth.login("alice", &password).unwrap();
        assert_eq!(fourth.evicted, 1);
        assert_eq!(live_sessions(&auth, "alice"), 3);
        assert!(
            auth.validate_session(&second.session_id).is_none(),
            "the least recently used one ended"
        );
        for (position, kept) in [&first, &third, &fourth, &bob].into_iter().enumerate() {
            assert!(
                auth.validate_session(&kept.session_id).is_some(),
                "session {position} of the kept ones still validates"
            );
        }
    }

    /// One account signing in without end cannot fill the table: every other
    /// account still signs in, however many sign-ins it made.
    #[test]
    fn one_account_cannot_fill_the_session_table_against_the_others() {
        let (_dir, auth) = store_with_cap(MAX_SESSIONS_PER_ACCOUNT_CEILING);
        let password = crate::common::random_hex(24);
        auth.create_user("alice", &password, "operator").unwrap();
        auth.create_user("bob", &password, "operator").unwrap();
        let mut db = (*auth.snapshot()).clone();
        let now = auth.now();
        let mut evicted = 0;
        for _ in 0..MAX_LIVE_SESSIONS + 500 {
            evicted += auth.create_session(&mut db, "alice", now).unwrap().evicted;
        }
        assert_eq!(
            db.sessions.values().filter(|s| s.username == "alice").count(),
            MAX_SESSIONS_PER_ACCOUNT_CEILING as usize
        );
        assert_eq!(
            evicted,
            MAX_LIVE_SESSIONS + 500 - MAX_SESSIONS_PER_ACCOUNT_CEILING as usize
        );
        let bob = auth.create_session(&mut db, "bob", now).unwrap();
        assert_eq!(bob.evicted, 0, "another account's sessions are never ended");
    }

    /// No order of sign-ins by several accounts, at any allowed cap, holds more
    /// than the cap per account or refuses a sign-in at the global limit: the
    /// ceiling times the account limit fits the table.
    #[test]
    fn no_mix_of_sign_ins_exceeds_the_cap_or_reaches_the_global_limit() {
        assert!(MAX_USERS as u64 * MAX_SESSIONS_PER_ACCOUNT_CEILING <= MAX_LIVE_SESSIONS as u64);
        for cap in [1, 2, DEFAULT_MAX_SESSIONS_PER_ACCOUNT, MAX_SESSIONS_PER_ACCOUNT_CEILING] {
            let (_dir, auth) = store_with_cap(cap);
            // One account is hashed; the rest are copies of its row, which is
            // all `create_session` reads, so the run costs one scrypt.
            auth.create_user("user0", &crate::common::random_hex(24), "operator")
                .unwrap();
            let mut db = (*auth.snapshot()).clone();
            let template = db.users["user0"].clone();
            for n in 1..MAX_USERS {
                let user_id = crate::common::random_hex(16);
                db.users.insert(
                    format!("user{n}"),
                    UserRow {
                        user_id,
                        ..template.clone()
                    },
                );
            }
            let names: Vec<String> = (0..MAX_USERS).map(|n| format!("user{n}")).collect();
            let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
            for step in 0..6_000u32 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                // Skewed so a few accounts sign in far more often than the rest.
                let pick = ((seed >> 33) as usize % names.len()).min((seed >> 45) as usize % names.len());
                auth.create_session(&mut db, &names[pick], f64::from(step)).unwrap();
            }
            let mut per_account = std::collections::BTreeMap::<&str, usize>::new();
            for row in db.sessions.values() {
                *per_account.entry(row.username.as_str()).or_default() += 1;
            }
            let most = per_account.values().copied().max().unwrap_or(0);
            assert!(most <= cap as usize, "cap {cap}: one account held {most}");
            assert!(db.sessions.len() <= MAX_LIVE_SESSIONS);
        }
    }

    /// `auth.session.max_per_account` is 1 to the ceiling that keeps the table
    /// reachable only by accounts at their limits; anything else stops startup.
    #[test]
    fn the_per_account_cap_is_held_to_its_range_at_startup() {
        for (yaml, ok) in [
            ("{}", true),
            ("auth: {session: {max_per_account: 1}}", true),
            ("auth: {session: {max_per_account: 32}}", true),
            ("auth: {session: {max_per_account: 0}}", false),
            ("auth: {session: {max_per_account: 33}}", false),
            ("auth: {session: {max_per_account: abc}}", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let cfg = AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap();
            let opened = AuthManager::open(&dir.path().join("auth.db"), &cfg);
            assert_eq!(opened.is_ok(), ok, "{yaml}");
            if let Err(error) = opened {
                assert!(
                    error.to_string().contains("auth.session.max_per_account"),
                    "{yaml}: {error}"
                );
            }
        }
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
        let db = auth.snapshot();
        let sessions = db.sessions.values();
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
            assert!(
                auth.writer.try_lock().is_ok(),
                "a password check held the account store"
            );
            authn::verify_password(password, record)
        };
        for (name, typed) in [("nobody", &password), ("alice", &wrong), ("gone", &password)] {
            let error = auth.login_with(name, typed, unlocked).unwrap_err();
            assert_eq!(error.code, "AUTH_LOGIN_FAILED", "{name}");
            assert_eq!(checks.replace(0), 1, "{name}");
        }
        for (name, typed) in [("alice", &wrong), ("gone", &password)] {
            let error = auth.change_password_with(name, typed, &next, unlocked).unwrap_err();
            assert_eq!(error.code, "AUTH_LOGIN_FAILED", "password change for {name}");
            assert_eq!(checks.replace(0), 1, "password change for {name}");
        }
        auth.login_with("alice", &password, unlocked).unwrap();
        assert_eq!(checks.replace(0), 1, "sign-in");
        auth.change_password_with("alice", &password, &next, unlocked).unwrap();
        assert_eq!(checks.replace(0), 1, "password change");
    }

    /// Readers run on async workers (`/browse` and streamed-chat rechecks), so
    /// none may wait for a write in flight: with the connection held, as during
    /// a full rewrite, every reader still answers from the committed state,
    /// and a session lookup that needs no slide answers too.
    #[test]
    fn readers_never_wait_for_a_write_in_flight() {
        let (_dir, auth) = store();
        let password = crate::common::random_hex(24);
        auth.create_user("alice", &password, "operator").unwrap();
        let session = auth.login("alice", &password).unwrap().session_id;
        let account = auth.get_user("alice").unwrap();
        let before = (
            auth.list_users().len(),
            auth.count_enabled_admins(),
            auth.setup_code_active(),
        );
        let auth = Arc::new(auth);
        let in_flight = auth.writer.lock().unwrap();
        let (done, answered) = std::sync::mpsc::channel();
        let reader = auth.clone();
        std::thread::spawn(move || {
            let seen = reader.sign_in_revocations("alice");
            let answers = [
                reader.get_user("alice").is_some(),
                reader.account_unchanged(&account, Some(seen)),
                reader.owner_permits(&account.user_id, authz::Capability::OwnData),
                reader
                    .owner_lapse(&account.user_id, authz::Capability::OwnData)
                    .is_none(),
                (
                    reader.list_users().len(),
                    reader.count_enabled_admins(),
                    reader.setup_code_active(),
                ) == before,
                reader.validate_session(&session).is_some(),
            ];
            let _ = done.send(answers);
        });
        let answers = answered
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a reader waited for the write in flight");
        assert!(answers.iter().all(|ok| *ok), "{answers:?}");
        drop(in_flight);
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

    /// A password change from a signed-in session guesses the same password
    /// the sign-in form does, so the two share one failure count: wrong
    /// current passwords lock the account, the lockout refuses both before any
    /// check, and a change that succeeds clears the count like a sign-in.
    #[test]
    fn a_wrong_current_password_counts_toward_the_sign_in_lockout() {
        let (_dir, auth) = store();
        let [password, wrong, next] = [(); 3].map(|_| crate::common::random_hex(24));
        auth.create_user("alice", &password, "operator").unwrap();

        // Below the threshold a wrong current password is a counted failure,
        // and the right one still changes the password and clears the count.
        let error = auth.change_password("alice", &wrong, &next).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED");
        assert_eq!(failed_count(&auth, "alice"), 1);
        auth.change_password("alice", &password, &next).unwrap();
        assert_eq!(failed_count(&auth, "alice"), 0, "a successful change clears the count");

        // Sign-in and password-change failures add up to the one lockout.
        for attempt in 1..=4 {
            let error = if attempt % 2 == 0 {
                auth.login("alice", &wrong).unwrap_err()
            } else {
                auth.change_password("alice", &wrong, &password).unwrap_err()
            };
            assert_eq!(error.code, "AUTH_LOGIN_FAILED", "attempt {attempt}");
        }
        assert_eq!(failed_count(&auth, "alice"), 4);
        let error = auth.change_password("alice", &wrong, &password).unwrap_err();
        assert_eq!(error.code, "AUTH_LOGIN_FAILED", "the fifth failure starts the lockout");
        assert_eq!(failed_count(&auth, "alice"), 5);

        // Locked: the right password is refused on both paths, and refused
        // before a check, so neither path is a way to keep guessing.
        let checks = std::cell::Cell::new(0);
        let counted = |password: &str, record: &str| {
            checks.set(checks.get() + 1);
            authn::verify_password(password, record)
        };
        let error = auth
            .change_password_with("alice", &next, &password, counted)
            .unwrap_err();
        assert_eq!(error.code, "AUTH_ACCOUNT_LOCKED");
        let error = auth.login_with("alice", &next, counted).unwrap_err();
        assert_eq!(error.code, "AUTH_ACCOUNT_LOCKED");
        assert_eq!(checks.get(), 0, "a locked account pays no check");
        assert_eq!(
            failed_count(&auth, "alice"),
            5,
            "a refused attempt is not counted again"
        );

        // A lockout that starts while a change's check runs refuses that change.
        let (_dir, auth) = store();
        auth.create_user("bob", &password, "operator").unwrap();
        for _ in 0..4 {
            auth.login("bob", &wrong).unwrap_err();
        }
        let another_failure = || {
            auth.login("bob", &wrong).unwrap_err();
        };
        let error = auth
            .change_password_with("bob", &password, &next, check_then(another_failure))
            .unwrap_err();
        assert_eq!(error.code, "AUTH_ACCOUNT_LOCKED");
        assert_eq!(live_sessions(&auth, "bob"), 0);
        auth.login("bob", &password).unwrap_err();

        // The same overlap with the current password sent as the new one: a
        // right guess gets the lockout too, not "must differ", so the refusal
        // does not tell it apart from a wrong one.
        let (_dir, auth) = store();
        auth.create_user("carol", &password, "operator").unwrap();
        for _ in 0..4 {
            auth.login("carol", &wrong).unwrap_err();
        }
        let another_failure = || {
            auth.login("carol", &wrong).unwrap_err();
        };
        let error = auth
            .change_password_with("carol", &password, &password, check_then(another_failure))
            .unwrap_err();
        assert_eq!(error.code, "AUTH_ACCOUNT_LOCKED");
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
        assert!(failures.contains_key(&auth.name_hash("nobody")));
        assert!(!failures.contains_key("nobody"));
        assert!(
            !failures.contains_key(&authn::hash_token("nobody")),
            "the name key is keyed"
        );
    }

    /// A flood of made-up names stops growing memory at the cap. Names idle
    /// past the longest lockout go first, then the least recently failed; a
    /// name that is locked right now survives a flood that arrives later.
    /// A restart keeps the failures of a name with no account as it keeps an
    /// account's, so after one the two still answer alike: both re-lock on
    /// the next failure. Memory-only failures once restarted the unknown name
    /// at zero, answering 401 where the account answered 423.
    #[test]
    fn a_name_with_no_account_keeps_its_failures_across_a_restart() {
        let (dir, mut auth) = store();
        let path = dir.path().join("auth.db");
        let clock = std::sync::Arc::new(std::sync::Mutex::new(1_000.0_f64));
        let read = clock.clone();
        auth.set_clock(Box::new(move || *read.lock().unwrap()));
        let [password, wrong] = [(); 2].map(|_| crate::common::random_hex(24));
        auth.create_user("alice", &password, "operator").unwrap();
        let tick = |by: f64| *clock.lock().unwrap() += by;
        // Six counted failures each, stepping past every lockout in between.
        for _ in 0..6 {
            assert_eq!(attempt(&auth, "alice", &wrong).code, "AUTH_LOGIN_FAILED");
            assert_eq!(attempt(&auth, "zed", &wrong).code, "AUTH_LOGIN_FAILED");
            tick(authn::LOCKOUT_CEILING_SEC);
        }
        drop(auth);

        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let mut auth = AuthManager::open(&path, &cfg).unwrap();
        let read = clock.clone();
        auth.set_clock(Box::new(move || *read.lock().unwrap()));
        assert_eq!(failed_count(&auth, "alice"), 6, "the account's count survived");
        let seen = |name: &str| -> Vec<Seen> { (0..2).map(|_| attempt(&auth, name, &wrong)).collect() };
        let real = seen("alice");
        let codes: Vec<&str> = real.iter().map(|seen| seen.code.as_str()).collect();
        assert_eq!(codes, ["AUTH_LOGIN_FAILED", "AUTH_ACCOUNT_LOCKED"]);
        assert_eq!(seen("zed"), real, "the two names must still look the same");
        assert!(auth.get_user("zed").is_none());
    }

    /// A name that can never be an account (here, password-shaped) is counted
    /// and locks out like any other, but never reaches the store, so a
    /// restart forgets it; a valid name beside it is stored.
    #[test]
    fn a_name_that_cannot_be_an_account_is_counted_but_never_stored() {
        let (dir, mut auth) = store();
        let clock = std::sync::Arc::new(std::sync::Mutex::new(1_000.0_f64));
        let read = clock.clone();
        auth.set_clock(Box::new(move || *read.lock().unwrap()));
        let wrong = crate::common::random_hex(24);
        let typed = "Hunter2!pass";
        assert!(authn::validate_username(typed).is_err());
        let codes = |auth: &AuthManager, name: &str| -> Vec<String> {
            (0..6).map(|_| attempt(auth, name, &wrong).code).collect()
        };
        let locked = |n: usize| {
            let mut codes = vec!["AUTH_LOGIN_FAILED".to_string(); 5];
            codes.extend(vec!["AUTH_ACCOUNT_LOCKED".to_string(); n]);
            codes
        };
        assert_eq!(codes(&auth, typed), locked(1), "counted in memory");
        assert_eq!(codes(&auth, "zed"), locked(1));
        let rows = |auth: &AuthManager| -> Vec<String> {
            let stored = auth.writer.lock().unwrap();
            let mut statement = stored.prepare("SELECT name_hash FROM login_failures").unwrap();
            statement
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(rows(&auth), [auth.name_hash("zed")], "only the valid name is stored");
        drop(auth);

        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let mut auth = AuthManager::open(&dir.path().join("auth.db"), &cfg).unwrap();
        let read = clock.clone();
        auth.set_clock(Box::new(move || *read.lock().unwrap()));
        assert_eq!(attempt(&auth, "zed", &wrong).code, "AUTH_ACCOUNT_LOCKED");
        assert_eq!(
            attempt(&auth, typed, &wrong).code,
            "AUTH_LOGIN_FAILED",
            "forgotten at restart"
        );
        assert_eq!(rows(&auth), [auth.name_hash("zed")]);
    }

    /// A refused failure write fails the attempt as a refused account write
    /// does, and leaves the remembered count where the store has it.
    #[test]
    fn a_refused_unknown_failure_write_counts_nothing() {
        let (_dir, auth) = store();
        let wrong = crate::common::random_hex(24);
        assert_eq!(attempt(&auth, "zed", &wrong).code, "AUTH_LOGIN_FAILED");
        auth.writer
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse_failure BEFORE INSERT ON login_failures \
                 BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;",
            )
            .unwrap();
        assert_eq!(attempt(&auth, "zed", &wrong).code, "AUTH_CONFIG_INVALID");
        assert_eq!(attempt(&auth, "yan", &wrong).code, "AUTH_CONFIG_INVALID");
        let failures = auth.unknown_failures.lock().unwrap();
        assert_eq!(failures[&auth.name_hash("zed")].count, 1);
        assert!(!failures.contains_key(&auth.name_hash("yan")));
    }

    #[test]
    fn remembered_unknown_names_stay_within_the_cap() {
        let (_dir, auth) = store();
        let key = |i: usize| authn::hash_token(&format!("name{i}"));
        let count = |key: String, now: f64| {
            let _ = count_unknown_failure(&mut auth.unknown_failures.lock().unwrap(), key, now);
        };
        count(key(0), 0.0);
        for _ in 0..5 {
            count(key(1), 1_000.0);
        }
        for i in 2..UNKNOWN_FAILURE_ENTRIES {
            count(key(i), 1_000.0);
        }
        let full = || auth.unknown_failures.lock().unwrap().len();
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        // The next new name drops every name idle past the longest lockout.
        count(key(UNKNOWN_FAILURE_ENTRIES), 1_000.0);
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        assert!(!auth.unknown_failures.lock().unwrap().contains_key(&key(0)));
        // With nothing idle, the least recently failed unlocked name goes, one
        // at a time; the locked name, older than every other, is kept.
        for i in 1..=50 {
            count(key(UNKNOWN_FAILURE_ENTRIES + i), 1_000.0 + i as f64 * 0.01);
            assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
        }
        assert!(
            auth.unknown_locked_until(&key(1), 1_000.6).is_some(),
            "a locked name survives the flood"
        );
        // A name already remembered is counted in place, never evicting another.
        count(key(UNKNOWN_FAILURE_ENTRIES + 50), 1_100.0);
        assert_eq!(full(), UNKNOWN_FAILURE_ENTRIES);
    }

    /// Codex review on #1069: an attempt must count against a time no
    /// earlier than any failure already recorded for its name. A failure
    /// below the threshold stores a lock that ends when it was recorded, so
    /// counting at an earlier time than one already stored reads as locked.
    /// `record_unknown_failure` therefore reads the clock only after taking
    /// the table's lock, as `login_with` reads it under the store lock.
    #[test]
    fn a_failure_counted_at_or_after_the_last_one_is_never_refused_below_the_threshold() {
        let key = || authn::hash_token("nobody");
        let mut failures = HashMap::new();
        assert_eq!(count_unknown_failure(&mut failures, key(), 1_000.002), Ok(vec![]));
        assert_eq!(count_unknown_failure(&mut failures, key(), 1_000.002), Ok(vec![]));
        assert_eq!(count_unknown_failure(&mut failures, key(), 1_000.003), Ok(vec![]));
        assert_eq!(failures[&key()].count, 3);
        // The hazard the lock order removes: a time read before a later
        // failure was recorded would be refused, with nothing counted.
        assert_eq!(count_unknown_failure(&mut failures, key(), 1_000.001), Err(1_000.003));
        assert_eq!(failures[&key()].count, 3);
    }
}
