//! Versioned authoritative account storage. The owner process serializes writes;
//! every mutation is committed before its in-memory snapshot becomes visible.
use super::{authn, authz, AuthDb, SessionRow, UserRow};
use crate::common::{
    atomic::write_atomic,
    errors::{HarnessError, Result},
    private_sqlite::{self, present},
};
use rusqlite::{params, Connection};
use std::{io::Read, path::Path};

fn invalid(message: impl Into<String>) -> HarnessError {
    HarnessError::new("AUTH_CONFIG_INVALID", message)
}
fn sql(error: rusqlite::Error) -> HarnessError {
    invalid(format!("account database refused: {error}"))
}

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The `LIMIT` for reading a table capped at `max` rows: one past the cap, so
/// a store over it reaches [`validate`] and is refused instead of cut to fit.
fn read_limit(max: usize) -> i64 {
    max as i64 + 1
}

/// `(user_id, username, old_role)` for one account folded into `auditor` (#965).
type RoleMigration = (String, String, String);
/// The audit-facing part of a [`RoleMigration`]: `(user_id, old_role)`.
pub(super) type RoleMigrationReport = (String, String);

/// The outbox for #965 role folds. Each fold's report is written in the same
/// transaction as the fold and stays until the server confirms its
/// `auth.role_migrated` audit row reached the log, so a crash between the two
/// re-reports it on the next boot instead of losing it (at least once). No
/// foreign key: schema 3 recreates `users` in the same transaction.
const ROLE_MIGRATIONS: &str = "
CREATE TABLE IF NOT EXISTS role_migrations (
 user_id TEXT PRIMARY KEY NOT NULL,
 old_role TEXT NOT NULL CHECK(old_role IN('audit','portal_auditor'))
) STRICT;";

/// Record the folds in the outbox, inside the caller's transaction.
fn record_migrations(conn: &Connection, migrated: &[RoleMigration]) -> rusqlite::Result<()> {
    if migrated.is_empty() {
        return Ok(());
    }
    conn.execute_batch(ROLE_MIGRATIONS)?;
    let mut row = conn.prepare("INSERT OR REPLACE INTO role_migrations VALUES (?1,?2)")?;
    for (user_id, _, old_role) in migrated {
        row.execute(params![user_id, old_role])?;
    }
    Ok(())
}

fn has_outbox(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='role_migrations'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 1)
    .map_err(sql)
}

/// Folds not yet acknowledged as audited, in a stable order. A malformed row
/// fails closed: the store refuses rather than reports or drops it.
pub(super) fn pending_role_migrations(conn: &Connection) -> Result<Vec<RoleMigrationReport>> {
    if !has_outbox(conn)? {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare("SELECT user_id, old_role FROM role_migrations ORDER BY user_id LIMIT ?1")
        .map_err(sql)?;
    let rows = statement
        .query_map(params![read_limit(super::MAX_USERS)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(sql)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql)?;
    if rows.iter().any(|(user_id, old_role)| {
        user_id.len() != 32
            || !user_id.bytes().all(|b| b.is_ascii_hexdigit())
            || !authn::RETIRED_ROLES.contains(&old_role.as_str())
    }) {
        return Err(invalid("invalid role migration record in the account database"));
    }
    Ok(rows)
}

/// The fold for `user_id` is audited: drop it from the outbox.
pub(super) fn acknowledge_role_migration(conn: &Connection, user_id: &str) -> Result<()> {
    if has_outbox(conn)? {
        conn.execute("DELETE FROM role_migrations WHERE user_id=?1", params![user_id])
            .map_err(sql)?;
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<std::fs::File> {
    private_sqlite::private_file(path, MAX_FILE_BYTES, "account", invalid)
}

fn connect(path: &Path) -> Result<Connection> {
    private_sqlite::connect(path, MAX_FILE_BYTES, "account", invalid)
}

pub(super) fn validate(db: &AuthDb) -> Result<()> {
    if db.users.is_empty()
        || db.users.len() > super::MAX_USERS
        || db.sessions.len() > super::MAX_LIVE_SESSIONS
        || super::count_enabled_admins(db) == 0
    {
        return Err(invalid(format!(
            "account store must contain 1-{} valid users and an enabled administrator",
            super::MAX_USERS
        )));
    }
    let timestamp = |v: f64| v.is_finite() && (0.0..1e12).contains(&v);
    let mut identities = std::collections::BTreeSet::new();
    for (name, user) in &db.users {
        if user.user_id.len() != 32
            || !user.user_id.bytes().all(|b| b.is_ascii_hexdigit())
            || !identities.insert(&user.user_id)
            || authn::validate_username(name).ok().as_ref() != Some(name)
            || authn::validate_role(&user.role).ok().as_ref() != Some(&user.role)
            || !authn::valid_password_record(&user.password_hash)
            || !timestamp(user.created_ts)
            || !user.last_login_ts.is_none_or(timestamp)
            || !user.locked_until_ts.is_none_or(timestamp)
        {
            return Err(invalid(
                "invalid legacy or database account record; restore a valid recovery copy",
            ));
        }
    }
    for (id, session) in &db.sessions {
        if id.len() != 64
            || !id.bytes().all(|c| c.is_ascii_hexdigit())
            || !db.users.contains_key(&session.username)
            || session.csrf_hash.len() != 64
            || !session.csrf_hash.bytes().all(|c| c.is_ascii_hexdigit())
            || !timestamp(session.created_ts)
            || !timestamp(session.last_seen_ts)
            || !timestamp(session.expires_ts)
        {
            return Err(invalid("invalid account session record"));
        }
    }
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE users (
 username TEXT PRIMARY KEY NOT NULL, password_hash TEXT NOT NULL,
 created_ts REAL NOT NULL, disabled INTEGER NOT NULL CHECK(disabled IN(0,1)),
 last_login_ts REAL, failed_count INTEGER NOT NULL CHECK(failed_count>=0), locked_until_ts REAL,
 role TEXT NOT NULL CHECK(role IN('admin','operator','audit')),
 must_change_password INTEGER NOT NULL CHECK(must_change_password IN(0,1)),
 user_id TEXT UNIQUE NOT NULL
) STRICT;
CREATE TABLE sessions (
 token_hash TEXT PRIMARY KEY NOT NULL, username TEXT NOT NULL REFERENCES users(username), csrf_hash TEXT NOT NULL,
 created_ts REAL NOT NULL, last_seen_ts REAL NOT NULL, expires_ts REAL NOT NULL,
 revoked INTEGER NOT NULL CHECK(revoked IN(0,1))
) STRICT;
PRAGMA user_version=1;";

/// Version 3 narrows the role check to the current set: `auditor` replaces
/// the retired `audit` and `portal_auditor` (#965). It is installed by the
/// first commit that stores `auditor` on a version 1 or 2 home — including
/// the load-time role migration — so a home without one stays on its version
/// and an older binary can still open it. Older binaries refuse version 3.
const USERS_V3: &str = "
DROP TABLE users;
CREATE TABLE users (
 username TEXT PRIMARY KEY NOT NULL, password_hash TEXT NOT NULL,
 created_ts REAL NOT NULL, disabled INTEGER NOT NULL CHECK(disabled IN(0,1)),
 last_login_ts REAL, failed_count INTEGER NOT NULL CHECK(failed_count>=0), locked_until_ts REAL,
 role TEXT NOT NULL CHECK(role IN('admin','operator','auditor')),
 must_change_password INTEGER NOT NULL CHECK(must_change_password IN(0,1)),
 user_id TEXT UNIQUE NOT NULL
) STRICT;
PRAGMA user_version=3;";

fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
}

/// Retired role strings fold into `auditor` (#965): the account keeps its
/// password and history fields. A former `audit` account is also disabled:
/// `auditor` adds sessions, chat and `/diag` that `audit` never held, and an
/// account narrowed to `audit` may own transcripts from an earlier role, so
/// the wider authority waits for an admin to re-enable it. `portal_auditor`
/// held the same grants as `auditor` and stays as it was. Returns
/// `(user_id, username, old_role)` per migrated account; the caller revokes
/// the account's sessions (a role change does) and rewrites the store.
fn migrate_legacy_roles(db: &mut AuthDb) -> Vec<RoleMigration> {
    let mut migrated = Vec::new();
    for (name, user) in &mut db.users {
        if authn::RETIRED_ROLES.contains(&user.role.as_str()) {
            let old = std::mem::replace(&mut user.role, authn::AUDITOR_ROLE.to_string());
            if old == authn::RETIRED_AUDIT_ROLE {
                user.disabled = true;
            }
            migrated.push((user.user_id.clone(), name.clone(), old));
        }
    }
    // Deterministic order: the audit log replays the same events every boot.
    migrated.sort();
    migrated
}

/// Call with sessions and users already emptied: the drop then cascades nothing.
fn widen_roles_if_needed(conn: &Connection, db: &AuthDb) -> rusqlite::Result<()> {
    if matches!(schema_version(conn)?, 1 | 2)
        && db.users.values().any(|u| authz::is_role(&u.role, authz::Role::Auditor))
    {
        conn.execute_batch(USERS_V3)?;
    }
    Ok(())
}

fn insert(conn: &Connection, db: &AuthDb) -> rusqlite::Result<()> {
    let mut users = conn.prepare("INSERT INTO users VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?;
    for (name, u) in &db.users {
        users.execute(params![
            name,
            u.password_hash,
            u.created_ts,
            u.disabled,
            u.last_login_ts,
            u.failed_count,
            u.locked_until_ts,
            u.role,
            u.must_change_password,
            u.user_id
        ])?;
    }
    let mut sessions = conn.prepare("INSERT INTO sessions VALUES (?1,?2,?3,?4,?5,?6,?7)")?;
    for (id, s) in &db.sessions {
        sessions.execute(params![
            id,
            s.username,
            s.csrf_hash,
            s.created_ts,
            s.last_seen_ts,
            s.expires_ts,
            s.revoked
        ])?;
    }
    Ok(())
}

/// Reads the store, folding retired roles into `auditor` before validation
/// (#965). A fold rewrites the store at schema 3 with the migrated accounts'
/// sessions removed and their reports in the `role_migrations` outbox, all in
/// one transaction.
fn load(conn: &mut Connection) -> Result<AuthDb> {
    if !matches!(schema_version(conn).map_err(sql)?, 1..=3)
        || conn
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .map_err(sql)?
            != "ok"
    {
        return Err(invalid("unsupported or corrupt account database"));
    }
    let mut db = AuthDb::default();
    {
        let mut statement = conn.prepare("SELECT username,password_hash,created_ts,disabled,last_login_ts,failed_count,locked_until_ts,role,must_change_password,user_id FROM users LIMIT ?1").map_err(sql)?;
        for row in statement
            .query_map(params![read_limit(super::MAX_USERS)], |r| {
                Ok((
                    r.get(0)?,
                    UserRow {
                        password_hash: r.get(1)?,
                        created_ts: r.get(2)?,
                        disabled: r.get(3)?,
                        last_login_ts: r.get(4)?,
                        failed_count: r.get(5)?,
                        locked_until_ts: r.get(6)?,
                        role: r.get(7)?,
                        must_change_password: r.get(8)?,
                        user_id: r.get(9)?,
                    },
                ))
            })
            .map_err(sql)?
        {
            let (name, user) = row.map_err(sql)?;
            db.users.insert(name, user);
        }
    }
    {
        let mut statement = conn
            .prepare(
                "SELECT token_hash,username,csrf_hash,created_ts,last_seen_ts,expires_ts,revoked FROM sessions LIMIT ?1",
            )
            .map_err(sql)?;
        for row in statement
            .query_map(params![read_limit(super::MAX_LIVE_SESSIONS)], |r| {
                Ok((
                    r.get(0)?,
                    SessionRow {
                        username: r.get(1)?,
                        csrf_hash: r.get(2)?,
                        created_ts: r.get(3)?,
                        last_seen_ts: r.get(4)?,
                        expires_ts: r.get(5)?,
                        revoked: r.get(6)?,
                    },
                ))
            })
            .map_err(sql)?
        {
            let (id, session) = row.map_err(sql)?;
            db.sessions.insert(id, session);
        }
    }
    let migrated = migrate_legacy_roles(&mut db);
    if !migrated.is_empty() {
        // Role changes revoke sessions; deleting them first also frees the
        // foreign key before `persist` replaces the users table.
        let names: std::collections::BTreeSet<&str> = migrated.iter().map(|(_, name, _)| name.as_str()).collect();
        db.sessions.retain(|_, s| !names.contains(s.username.as_str()));
        persist_with(conn, &db, &migrated)?;
        return Ok(db);
    }
    validate(&db)?;
    Ok(db)
}

/// `legacy` remains an immutable private recovery source. A durable marker
/// distinguishes a genuinely fresh store from a deleted initialized database.
/// Role folds (#965) are reported through [`pending_role_migrations`].
pub(super) fn open(legacy: &Path) -> Result<(Connection, AuthDb, bool)> {
    let path = legacy.with_extension("sqlite3");
    let marker = legacy.with_extension("initialized");
    if present(&path)? {
        let mut conn = connect(&path)?;
        let db = load(&mut conn)?;
        if !present(&marker)? {
            write_atomic(&marker, b"sqlite3-v1\n", Some(0o600))?;
        }
        return Ok((conn, db, false));
    }
    if present(&marker)? {
        return Err(invalid(
            "initialized account database is missing; restore it explicitly",
        ));
    }
    let fresh = !present(legacy)?;
    let parent = path.parent().ok_or_else(|| invalid("missing account directory"))?;
    let mut migrated: Vec<RoleMigration> = Vec::new();
    let db = if fresh {
        // The code reaches disk before the database that accepts it, so a
        // crash between the two never leaves an admin with an unknown password.
        let code = authn::new_setup_code();
        write_atomic(
            &parent.join(authn::SETUP_CODE_FILE),
            format!("{code}\n").as_bytes(),
            Some(0o600),
        )?;
        let mut db = AuthDb::default();
        db.users.insert(
            super::BOOTSTRAP_USERNAME.into(),
            UserRow {
                password_hash: authn::hash_bootstrap_password(&code)?,
                created_ts: crate::common::now_ts(),
                disabled: false,
                last_login_ts: None,
                failed_count: 0,
                locked_until_ts: None,
                role: "admin".into(),
                must_change_password: true,
                user_id: crate::common::random_hex(16),
            },
        );
        db
    } else {
        let mut bytes = Vec::new();
        private_file(legacy)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        let mut db: AuthDb = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("legacy auth.json is malformed; no new credentials were created"))?;
        for user in db.users.values_mut() {
            if user.user_id.is_empty() {
                user.user_id = crate::common::random_hex(16);
            }
        }
        // Fold retired roles before validation rejects them; the legacy
        // sessions are cleared below either way.
        migrated = migrate_legacy_roles(&mut db);
        validate(&db)?;
        let backup = legacy.with_extension("json.pre-sqlite");
        if present(&backup)? {
            let mut saved = Vec::new();
            private_file(&backup)?
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut saved)?;
            if saved != bytes {
                return Err(invalid("existing account recovery copy differs from legacy auth.json"));
            }
        } else {
            write_atomic(&backup, &bytes, Some(0o600))?;
        }
        // Identity and role records survive; all legacy sessions require login.
        db.sessions.clear();
        db
    };
    validate(&db)?;
    let staged = tempfile::Builder::new().prefix(".auth.").tempfile_in(parent)?;
    let mut conn = connect(staged.path())?;
    let tx = conn.transaction().map_err(sql)?;
    tx.execute_batch(SCHEMA).map_err(sql)?;
    widen_roles_if_needed(&tx, &db).map_err(sql)?;
    insert(&tx, &db).map_err(sql)?;
    record_migrations(&tx, &migrated).map_err(sql)?;
    tx.commit().map_err(sql)?;
    load(&mut conn)?;
    conn.close().map_err(|(_, e)| sql(e))?;
    staged.as_file().sync_all()?;
    staged
        .persist_noclobber(&path)
        .map_err(|e| invalid(format!("cannot install account database: {}", e.error)))?;
    write_atomic(&marker, b"sqlite3-v1\n", Some(0o600))?;
    Ok((connect(&path)?, db, fresh))
}

pub(super) fn persist(conn: &mut Connection, db: &AuthDb) -> Result<()> {
    persist_with(conn, db, &[])
}

/// [`persist`], plus the outbox rows for `migrated` in the same transaction.
fn persist_with(conn: &mut Connection, db: &AuthDb, migrated: &[RoleMigration]) -> Result<()> {
    validate(db)?;
    // ponytail: at most MAX_USERS users / MAX_LIVE_SESSIONS sessions, one transaction per mutation.
    // Use row-level updates if measured local account traffic warrants it.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    tx.execute_batch("DELETE FROM sessions; DELETE FROM users;")
        .map_err(sql)?;
    widen_roles_if_needed(&tx, db).map_err(sql)?;
    insert(&tx, db).map_err(sql)?;
    record_migrations(&tx, migrated).map_err(sql)?;
    tx.commit().map_err(sql)
}

/// Session lookups slide `last_seen_ts` or revoke an expired row on every
/// guarded request; that one-row change is committed alone rather than through
/// the full rewrite above.
pub(super) fn touch_session(conn: &Connection, token_hash: &str, last_seen_ts: f64, revoked: bool) -> Result<()> {
    let changed = conn
        .execute(
            "UPDATE sessions SET last_seen_ts=?1, revoked=?2 WHERE token_hash=?3",
            params![last_seen_ts, revoked, token_hash],
        )
        .map_err(sql)?;
    if changed != 1 {
        return Err(invalid("session row is missing from the account database"));
    }
    Ok(())
}

/// Sign-in failures for usernames with no account (#1069), so they survive a
/// restart as an account's `failed_count` and `locked_until_ts` do. Rows are
/// keyed by an HMAC of the name under `login_failure_key`, a random key kept
/// in this file: it salts the hashes against precomputed tables, but anyone
/// who reads the file holds it too. Like the `role_migrations` outbox these
/// tables leave `user_version` alone: an older binary ignores them and
/// forgets the failures at restart, as it did before.
const LOGIN_FAILURES: &str = "
CREATE TABLE IF NOT EXISTS login_failure_key (
 id INTEGER PRIMARY KEY NOT NULL CHECK(id=0), key BLOB NOT NULL CHECK(length(key)=32)
) STRICT;
CREATE TABLE IF NOT EXISTS login_failures (
 name_hash TEXT PRIMARY KEY NOT NULL, failed_count INTEGER NOT NULL CHECK(failed_count>=0),
 locked_until_ts REAL NOT NULL, last_ts REAL NOT NULL
) STRICT;";

/// One remembered name: `(failed_count, locked_until_ts, last_ts)`.
pub(super) type LoginFailure = (u32, f64, f64);
/// The name key and every remembered `(name_hash, failure)`.
pub(super) type LoginFailures = ([u8; 32], Vec<(String, LoginFailure)>);

/// The name key and the remembered failures, creating both tables (and the
/// key) on first use. A malformed key or row, or more rows than `max`, fails
/// closed: the store refuses rather than drops a failure.
pub(super) fn load_login_failures(conn: &mut Connection, max: usize) -> Result<LoginFailures> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    tx.execute_batch(LOGIN_FAILURES).map_err(sql)?;
    let mut fresh = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut fresh);
    tx.execute(
        "INSERT OR IGNORE INTO login_failure_key VALUES (0, ?1)",
        params![fresh.as_slice()],
    )
    .map_err(sql)?;
    let key: Vec<u8> = tx
        .query_row("SELECT key FROM login_failure_key WHERE id=0", [], |r| r.get(0))
        .map_err(sql)?;
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| invalid("invalid sign-in failure key in the account database"))?;
    let rows = {
        let mut statement = tx
            .prepare("SELECT name_hash, failed_count, locked_until_ts, last_ts FROM login_failures LIMIT ?1")
            .map_err(sql)?;
        let rows = statement
            .query_map(params![read_limit(max)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            })
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        rows
    };
    tx.commit().map_err(sql)?;
    let timestamp = |v: f64| v.is_finite() && (0.0..1e12).contains(&v);
    if rows.len() > max {
        return Err(invalid("too many sign-in failure records in the account database"));
    }
    let rows = rows
        .into_iter()
        .map(|(name_hash, count, locked_until_ts, last_ts)| {
            let count = u32::try_from(count).ok().filter(|_| {
                name_hash.len() == 64
                    && name_hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                    && timestamp(locked_until_ts)
                    && timestamp(last_ts)
            });
            match count {
                Some(count) => Ok((name_hash, (count, locked_until_ts, last_ts))),
                None => Err(invalid("invalid sign-in failure record in the account database")),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((key, rows))
}

/// Stores one name's failures, when `counted` is given, and drops the names
/// `forgotten` at the cap, in one transaction.
pub(super) fn record_login_failure(
    conn: &mut Connection,
    counted: Option<(&str, LoginFailure)>,
    forgotten: &[String],
) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    {
        let mut forget = tx
            .prepare("DELETE FROM login_failures WHERE name_hash=?1")
            .map_err(sql)?;
        for name in forgotten {
            forget.execute(params![name]).map_err(sql)?;
        }
    }
    if let Some((name_hash, (count, locked_until_ts, last_ts))) = counted {
        tx.execute(
            "INSERT OR REPLACE INTO login_failures VALUES (?1,?2,?3,?4)",
            params![name_hash, count, locked_until_ts, last_ts],
        )
        .map_err(sql)?;
    }
    tx.commit().map_err(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{
        auth_store::{AuthManager, BOOTSTRAP_USERNAME},
        config::AppConfig,
    };

    /// A malformed or excess sign-in failure row refuses the store rather
    /// than drop a remembered lockout.
    #[test]
    fn malformed_sign_in_failure_rows_refuse_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("auth.db");
        let cfg = AppConfig::from_str("{}", &dir.path().join("config.yaml")).unwrap();
        drop(AuthManager::open(&legacy, &cfg).unwrap());
        let database = legacy.with_extension("sqlite3");
        let good = "a".repeat(64);
        for row in [
            format!("('{}', 1, 0.0, 1.0)", "A".repeat(64)),
            "('abc', 1, 0.0, 1.0)".to_string(),
            format!("('{good}', 4294967296, 0.0, 1.0)"),
            format!("('{good}', 1, 1e13, 1.0)"),
        ] {
            let conn = connect(&database).unwrap();
            conn.execute_batch(&format!(
                "DELETE FROM login_failures; INSERT INTO login_failures VALUES {row};"
            ))
            .unwrap();
            drop(conn);
            assert!(AuthManager::open(&legacy, &cfg).is_err(), "{row} must refuse");
        }
        let mut conn = connect(&database).unwrap();
        conn.execute_batch(&format!(
            "DELETE FROM login_failures; INSERT INTO login_failures VALUES ('{good}', 5, 0.0, 1.0);"
        ))
        .unwrap();
        assert_eq!(load_login_failures(&mut conn, 1).unwrap().1.len(), 1);
        conn.execute_batch(&format!(
            "INSERT INTO login_failures VALUES ('{}', 5, 0.0, 1.0);",
            "b".repeat(64)
        ))
        .unwrap();
        assert!(load_login_failures(&mut conn, 1).is_err(), "past the cap must refuse");
    }

    #[test]
    fn migration_preserves_records_and_failures_never_bootstrap_or_publish() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("auth.json");
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
        let hash = authn::hash_password("preserved-admin-password").unwrap();
        let raw=serde_json::to_vec(&serde_json::json!({"users":{"admin":{"password_hash":hash,"created_ts":123.0,"role":"admin","failed_count":7,"locked_until_ts":456.0},"reader":{"password_hash":hash,"created_ts":123.0,"role":"audit","disabled":true}},"sessions":{}})).unwrap();
        write_atomic(&legacy, &raw, Some(0o600)).unwrap();
        let mgr = AuthManager::open(&legacy, &cfg).unwrap();
        assert!(!mgr.bootstrap_if_empty().unwrap());
        assert_eq!(mgr.get_user("admin").unwrap().failed_count, 7);
        assert!(mgr.get_user("reader").unwrap().disabled);
        // The legacy file's retired `audit` role folds through the same outbox.
        let reader_id = mgr.get_user("reader").unwrap().user_id;
        assert_eq!(mgr.get_user("reader").unwrap().role, "auditor");
        assert_eq!(
            mgr.pending_role_migrations().unwrap(),
            [(reader_id, "audit".to_string())]
        );
        assert_eq!(std::fs::read(legacy.with_extension("json.pre-sqlite")).unwrap(), raw);
        let database = legacy.with_extension("sqlite3");
        let conn = connect(&database).unwrap();
        assert_eq!(
            conn.query_row("SELECT password_hash FROM users WHERE username='admin'", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            hash
        );
        conn.execute_batch("CREATE TRIGGER refuse_change BEFORE DELETE ON users BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;").unwrap();
        assert!(mgr.enable_user("reader").is_err());
        assert!(
            mgr.get_user("reader").unwrap().disabled,
            "failed transaction cannot publish changed memory state"
        );
        drop(mgr);
        drop(conn);
        std::fs::remove_file(&database).unwrap();
        assert!(
            AuthManager::open(&legacy, &cfg).is_err(),
            "missing initialized DB must not remigrate/reset"
        );
        write_atomic(&database, b"", Some(0o600)).unwrap();
        assert!(
            AuthManager::open(&legacy, &cfg).is_err(),
            "empty existing DB cannot bootstrap"
        );
        write_atomic(&database, b"corrupt database", Some(0o600)).unwrap();
        assert!(AuthManager::open(&legacy, &cfg).is_err());

        let other = tempfile::tempdir().unwrap();
        let legacy = other.path().join("auth.json");
        for bad in [b"{}".as_slice(), b"not JSON"] {
            write_atomic(&legacy, bad, Some(0o600)).unwrap();
            assert!(AuthManager::open(&legacy, &cfg).is_err());
            assert!(!legacy.with_extension("sqlite3").exists());
        }
        #[cfg(unix)]
        {
            std::fs::remove_file(&legacy).unwrap();
            std::os::unix::fs::symlink(other.path().join("missing"), &legacy).unwrap();
            assert!(AuthManager::open(&legacy, &cfg).is_err());
        }
    }

    #[test]
    fn auditor_widens_the_schema_only_when_first_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
        let mgr = AuthManager::open(&path, &cfg).unwrap();
        let database = path.with_extension("sqlite3");
        let version = || schema_version(&connect(&database).unwrap()).unwrap();
        let password = crate::common::random_urlsafe(24);
        mgr.create_user("reader", &password, "operator").unwrap();
        assert_eq!(version(), 1, "a home that never stores the role stays downgradeable");
        let session = mgr.login("reader", &password).unwrap();
        mgr.set_role("reader", "auditor").unwrap();
        assert_eq!(version(), 3);
        drop(mgr);
        let reopened = AuthManager::open(&path, &cfg).unwrap();
        assert_eq!(reopened.get_user("reader").unwrap().role, "auditor");
        assert!(
            reopened.validate_session(&session.session_id).is_none(),
            "role change revokes sessions"
        );
        assert!(reopened.login("reader", &password).is_ok());
        assert!(reopened.create_user("other", &password, "audit").is_err());
        assert!(reopened.pending_role_migrations().unwrap().is_empty());
    }

    /// A version 2 store holding the retired `audit` and `portal_auditor`
    /// roles opens as version 3 with both accounts folded into `auditor`,
    /// their sessions revoked and the migrations reported once (#965).
    #[test]
    fn legacy_roles_migrate_to_auditor_on_open() {
        /// The historical version 2 schema, pinned so the test can still build
        /// a store that holds retired roles.
        const USERS_V2: &str = "
DROP TABLE users;
CREATE TABLE users (
 username TEXT PRIMARY KEY NOT NULL, password_hash TEXT NOT NULL,
 created_ts REAL NOT NULL, disabled INTEGER NOT NULL CHECK(disabled IN(0,1)),
 last_login_ts REAL, failed_count INTEGER NOT NULL CHECK(failed_count>=0), locked_until_ts REAL,
 role TEXT NOT NULL CHECK(role IN('admin','operator','audit','portal_auditor')),
 must_change_password INTEGER NOT NULL CHECK(must_change_password IN(0,1)),
 user_id TEXT UNIQUE NOT NULL
) STRICT;
PRAGMA user_version=2;";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let database = path.with_extension("sqlite3");
        // Minted per run so no literal reaches the password hasher (CodeQL
        // rust/hard-coded-cryptographic-value, alert #1093 on this test).
        let password = crate::common::random_urlsafe(24);
        let password = password.as_str();
        let hash = authn::hash_password(password).unwrap();
        let session_id = authn::new_session_id();
        let (reader_id, portal_id) = (crate::common::random_hex(16), crate::common::random_hex(16));
        {
            write_atomic(&database, b"", Some(0o600)).unwrap();
            let conn = connect(&database).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(USERS_V2).unwrap();
            for (name, role, user_id) in [
                ("admin", "admin", crate::common::random_hex(16)),
                ("reader", "audit", reader_id.clone()),
                ("portal", "portal_auditor", portal_id.clone()),
            ] {
                conn.execute(
                    "INSERT INTO users VALUES (?1,?2,123.0,0,NULL,0,NULL,?3,0,?4)",
                    params![name, hash, role, user_id],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO sessions VALUES (?1,'reader',?2,123.0,123.0,999999999.0,0)",
                params![authn::hash_token(&session_id), authn::hash_token("csrf")],
            )
            .unwrap();
        }
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
        let mgr = AuthManager::open(&path, &cfg).unwrap();
        assert_eq!(mgr.get_user("reader").unwrap().role, "auditor");
        assert_eq!(mgr.get_user("portal").unwrap().role, "auditor");
        assert!(
            mgr.get_user("reader").unwrap().disabled,
            "a former audit account waits for an admin before gaining auditor authority"
        );
        assert!(
            !mgr.get_user("portal").unwrap().disabled,
            "a former portal_auditor account keeps its unchanged authority"
        );
        assert!(
            mgr.validate_session(&session_id).is_none(),
            "a migrated account's sessions are revoked"
        );
        {
            let conn = connect(&database).unwrap();
            assert_eq!(
                schema_version(&conn).unwrap(),
                3,
                "the rewrite installs the narrowed role check"
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0,
                "the migrated account's session row is deleted, not just expired"
            );
        }
        let mut expected = vec![
            (portal_id.clone(), "portal_auditor".to_string()),
            (reader_id.clone(), "audit".to_string()),
        ];
        expected.sort();
        assert_eq!(
            mgr.pending_role_migrations().unwrap(),
            expected,
            "each migrated account is reported with its old role"
        );
        // A migrated account keeps its password; the former audit account
        // signs in as auditor only once an admin re-enables it.
        assert!(mgr.login("reader", password).is_err());
        mgr.enable_user("reader").unwrap();
        mgr.login("reader", password).unwrap();
        // A crash before the audit write (no acknowledgement) keeps every
        // report for the next boot: at least once, never lost.
        drop(mgr);
        let reopened = AuthManager::open(&path, &cfg).unwrap();
        assert_eq!(reopened.pending_role_migrations().unwrap(), expected);
        assert_eq!(reopened.get_user("portal").unwrap().role, "auditor");
        // Acknowledged reports are gone, durably; the fold itself is not redone.
        reopened.acknowledge_role_migration(&reader_id).unwrap();
        drop(reopened);
        let reopened = AuthManager::open(&path, &cfg).unwrap();
        assert_eq!(
            reopened.pending_role_migrations().unwrap(),
            [(portal_id.clone(), "portal_auditor".to_string())]
        );
        reopened.acknowledge_role_migration(&portal_id).unwrap();
        assert!(reopened.pending_role_migrations().unwrap().is_empty());
        drop(reopened);
        let reopened = AuthManager::open(&path, &cfg).unwrap();
        assert!(reopened.pending_role_migrations().unwrap().is_empty());
        assert_eq!(reopened.get_user("reader").unwrap().role, "auditor");
    }

    #[test]
    fn password_change_persists_scrypt_and_returns_an_independent_session() {
        use base64::Engine;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
        let mgr = AuthManager::open(&path, &cfg).unwrap();
        // Minted per run so no literal reaches the password hasher.
        let old_password = crate::common::random_urlsafe(24);
        let new_password = crate::common::random_urlsafe(24);
        let (old_password, new_password) = (old_password.as_str(), new_password.as_str());
        mgr.create_user("tester", old_password, "operator").unwrap();
        let old_session = mgr.login("tester", old_password).unwrap();
        let changed = mgr.change_password("tester", old_password, new_password).unwrap();

        // CodeQL #839 treats change_password's return value as a password.
        // It is LoginResult: the session is freshly minted from 32 random bytes.
        let raw_token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&changed.session_id)
            .unwrap();
        assert_eq!(raw_token.len(), 32);
        assert_ne!(changed.session_id, old_session.session_id);
        assert_ne!(changed.session_id, new_password);
        let conn = connect(&path.with_extension("sqlite3")).unwrap();
        let password_record: String = conn
            .query_row("SELECT password_hash FROM users WHERE username='tester'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(password_record.starts_with("scrypt$131072$8$1$"));
        assert_eq!(authn::verify_password(new_password, &password_record), (true, false));
        assert_eq!(authn::verify_password(old_password, &password_record), (false, false));
        let token_hash: String = conn
            .query_row("SELECT token_hash FROM sessions WHERE revoked=0", [], |r| r.get(0))
            .unwrap();
        assert_eq!(token_hash, authn::hash_token(&changed.session_id));
        assert_ne!(token_hash, changed.session_id);

        drop(mgr);
        let reopened = AuthManager::open(&path, &cfg).unwrap();
        assert!(reopened.validate_session(&old_session.session_id).is_none());
        assert!(reopened.validate_session(new_password).is_none());
        assert_eq!(
            reopened.validate_session(&changed.session_id).unwrap().username,
            "tester"
        );
    }

    #[test]
    fn a_live_session_slides_its_idle_window_at_most_once_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
        let now = std::sync::Arc::new(std::sync::Mutex::new(1_000.0f64));
        let mut mgr = AuthManager::open(&path, &cfg).unwrap();
        let clock = now.clone();
        mgr.set_clock(Box::new(move || *clock.lock().unwrap()));
        // A random password keeps credentials out of the source.
        let password = authn::new_session_id();
        mgr.create_user("tester", &password, "operator").unwrap();
        let session = mgr.login("tester", &password).unwrap();
        let token = authn::hash_token(&session.session_id);
        let on_disk = || {
            connect(&path.with_extension("sqlite3"))
                .unwrap()
                .query_row(
                    "SELECT last_seen_ts, revoked FROM sessions WHERE token_hash=?1",
                    [&token],
                    |r| Ok((r.get::<_, f64>(0)?, r.get::<_, bool>(1)?)),
                )
                .unwrap()
        };
        let created = on_disk().0;
        // Within a minute of the recorded use, a request writes nothing.
        *now.lock().unwrap() += 59.0;
        assert!(mgr.validate_session(&session.session_id).is_some());
        assert_eq!(on_disk(), (created, false));
        // From a minute on, the next request slides the window on disk.
        *now.lock().unwrap() += 1.0;
        assert!(mgr.validate_session(&session.session_id).is_some());
        assert_eq!(on_disk(), (created + 60.0, false));
        // Expiry is never deferred: the first request past the idle timeout
        // (default 43200 s) is refused and revokes the row on disk.
        *now.lock().unwrap() += 43_200.0;
        assert!(mgr.validate_session(&session.session_id).is_none());
        assert_eq!(on_disk(), (created + 60.0, true));
    }

    /// A store written past a cap by anything but this code is refused on
    /// open, not cut to fit: each `LIMIT` reads one row past its cap. A store
    /// at the cap still opens, so the refusal is the cap and nothing else.
    #[test]
    fn a_store_past_either_cap_is_refused_on_open_and_one_at_the_cap_opens() {
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let open_with = |users: usize, sessions: usize| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.db");
            let mgr = AuthManager::open(&path, &cfg).unwrap();
            mgr.create_user("alice", &crate::common::random_hex(24), "admin")
                .unwrap();
            drop(mgr);
            let conn = connect(&path.with_extension("sqlite3")).unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            // A fresh store also holds the bootstrap admin: fill to a total.
            let count = |sql: &str| tx.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap() as usize;
            let held = count("SELECT COUNT(*) FROM users");
            for n in held..users {
                tx.execute(
                    "INSERT INTO users(username,password_hash,created_ts,disabled,last_login_ts,failed_count,
                     locked_until_ts,role,must_change_password,user_id)
                     SELECT ?1,password_hash,created_ts,disabled,last_login_ts,failed_count,locked_until_ts,role,
                     must_change_password,lower(hex(randomblob(16))) FROM users WHERE username='alice'",
                    params![format!("user{n}")],
                )
                .unwrap();
            }
            let live = count("SELECT COUNT(*) FROM sessions");
            for _ in live..sessions {
                tx.execute(
                    "INSERT INTO sessions(token_hash,username,csrf_hash,created_ts,last_seen_ts,expires_ts,revoked)
                     VALUES(lower(hex(randomblob(32))),'alice',lower(hex(randomblob(32))),1.0,1.0,2e9,0)",
                    [],
                )
                .unwrap();
            }
            tx.commit().unwrap();
            drop(conn);
            AuthManager::open(&path, &cfg).map(|_| ())
        };
        let (users, sessions) = (super::super::MAX_USERS, super::super::MAX_LIVE_SESSIONS);
        open_with(users, sessions).unwrap();
        for (users, sessions) in [(users + 1, 0), (1, sessions + 1)] {
            let refused = open_with(users, sessions).unwrap_err();
            assert_eq!(
                refused.code, "AUTH_CONFIG_INVALID",
                "{users} users, {sessions} sessions"
            );
        }
    }

    #[test]
    fn a_home_still_on_the_shipped_password_is_rotated_to_a_setup_code() {
        let open_with = |admin_hash: String, must_change: bool| {
            let dir = tempfile::tempdir().unwrap();
            let legacy = dir.path().join("auth.json");
            let cfg = AppConfig::from_str(AppConfig::embedded_default(), &dir.path().join("config.yaml")).unwrap();
            let raw = serde_json::to_vec(&serde_json::json!({"users":{"admin":{"password_hash":admin_hash,"created_ts":1.0,"role":"admin","must_change_password":must_change}},"sessions":{}})).unwrap();
            write_atomic(&legacy, &raw, Some(0o600)).unwrap();
            let mgr = AuthManager::open(&legacy, &cfg).unwrap();
            (dir, mgr)
        };
        // Passwords come from runtime values so no literal reaches the hasher
        // (CodeQL rust/hard-coded-cryptographic-value). The retired shipped
        // password was the bootstrap username itself; read it back from a store.
        let temporary = crate::common::random_urlsafe(24);
        let shipped = open_with(authn::hash_password(&temporary).unwrap(), false)
            .1
            .get_user(BOOTSTRAP_USERNAME)
            .unwrap()
            .username;
        // Still on the shipped password and never replaced: rotated on open.
        let (dir, mgr) = open_with(authn::hash_bootstrap_password(&shipped).unwrap(), true);
        let code = std::fs::read_to_string(dir.path().join(authn::SETUP_CODE_FILE)).unwrap();
        assert_eq!(
            mgr.login(BOOTSTRAP_USERNAME, &shipped).unwrap_err().code,
            "AUTH_LOGIN_FAILED"
        );
        mgr.login(BOOTSTRAP_USERNAME, code.trim()).unwrap();
        assert!(mgr.setup_code_active());
        // A real temporary password, or a replaced one, is left alone.
        for must_change in [true, false] {
            let (dir, mgr) = open_with(authn::hash_password(&temporary).unwrap(), must_change);
            assert!(!dir.path().join(authn::SETUP_CODE_FILE).exists());
            assert!(!mgr.setup_code_active());
            mgr.login(BOOTSTRAP_USERNAME, &temporary).unwrap();
        }
        // A `pending$` admin only the retired public bootstrap route could set
        // now signs in with a setup code and must replace it.
        let (dir, mgr) = open_with(format!("pending${}", authn::hash_password(&temporary).unwrap()), false);
        let code = std::fs::read_to_string(dir.path().join(authn::SETUP_CODE_FILE)).unwrap();
        assert!(mgr.get_user(BOOTSTRAP_USERNAME).unwrap().must_change_password);
        assert!(mgr.setup_code_active());
        mgr.login(BOOTSTRAP_USERNAME, code.trim()).unwrap();
    }
}
