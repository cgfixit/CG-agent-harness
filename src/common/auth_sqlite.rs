//! Versioned authoritative account storage. The owner process serializes writes;
//! every mutation is committed before its in-memory snapshot becomes visible.
use super::{authn, AuthDb, SessionRow, UserRow};
use crate::common::{
    atomic::write_atomic,
    errors::{HarnessError, Result},
};
use rusqlite::{params, Connection, OpenFlags};
use std::{io::Read, path::Path};

fn invalid(message: impl Into<String>) -> HarnessError {
    HarnessError::new("AUTH_CONFIG_INVALID", message)
}
fn sql(error: rusqlite::Error) -> HarnessError {
    invalid(format!("account database refused: {error}"))
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
    if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
        return Err(invalid("account file must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(invalid("account file must be owned, private, and not hard-linked"));
        }
    }
    Ok(file)
}

fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn validate(db: &AuthDb) -> Result<()> {
    if db.users.is_empty() || db.users.len() > 128 || db.sessions.len() > 4096 || super::count_enabled_admins(db) == 0 {
        return Err(invalid(
            "account store must contain 1-128 valid users and an enabled administrator",
        ));
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

fn load(conn: &Connection) -> Result<AuthDb> {
    if conn
        .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
        .map_err(sql)?
        != 1
        || conn
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .map_err(sql)?
            != "ok"
    {
        return Err(invalid("unsupported or corrupt account database"));
    }
    let mut db = AuthDb::default();
    let mut statement = conn.prepare("SELECT username,password_hash,created_ts,disabled,last_login_ts,failed_count,locked_until_ts,role,must_change_password,user_id FROM users LIMIT 129").map_err(sql)?;
    for row in statement
        .query_map([], |r| {
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
    let mut statement = conn
        .prepare(
            "SELECT token_hash,username,csrf_hash,created_ts,last_seen_ts,expires_ts,revoked FROM sessions LIMIT 4097",
        )
        .map_err(sql)?;
    for row in statement
        .query_map([], |r| {
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
    validate(&db)?;
    Ok(db)
}

fn connect(path: &Path) -> Result<Connection> {
    private_file(path)?;
    // macOS exposes its temporary directory through /var -> /private/var.
    // Resolve the directory only; SQLite still refuses a linked database leaf.
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing account directory"))?
        .canonicalize()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = parent.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(invalid(
                "account directory must be owned and not writable by other users",
            ));
        }
    }
    let path = parent.join(path.file_name().ok_or_else(|| invalid("missing account filename"))?);
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

/// `legacy` remains an immutable private recovery source. A durable marker
/// distinguishes a genuinely fresh store from a deleted initialized database.
pub(super) fn open(legacy: &Path) -> Result<(Connection, AuthDb, bool)> {
    let path = legacy.with_extension("sqlite3");
    let marker = legacy.with_extension("initialized");
    if present(&path)? {
        let conn = connect(&path)?;
        let db = load(&conn)?;
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
    let db = if fresh {
        let mut db = AuthDb::default();
        db.users.insert(
            super::BOOTSTRAP_USERNAME.into(),
            UserRow {
                password_hash: authn::hash_bootstrap_password()?,
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
    let parent = path.parent().ok_or_else(|| invalid("missing account directory"))?;
    let staged = tempfile::Builder::new().prefix(".auth.").tempfile_in(parent)?;
    let mut conn = connect(staged.path())?;
    let tx = conn.transaction().map_err(sql)?;
    tx.execute_batch(SCHEMA).map_err(sql)?;
    insert(&tx, &db).map_err(sql)?;
    tx.commit().map_err(sql)?;
    load(&conn)?;
    conn.close().map_err(|(_, e)| sql(e))?;
    staged.as_file().sync_all()?;
    staged
        .persist_noclobber(&path)
        .map_err(|e| invalid(format!("cannot install account database: {}", e.error)))?;
    write_atomic(&marker, b"sqlite3-v1\n", Some(0o600))?;
    Ok((connect(&path)?, db, fresh))
}

pub(super) fn persist(conn: &mut Connection, db: &AuthDb) -> Result<()> {
    validate(db)?;
    // ponytail: <=128 users / 4096 live sessions, one transaction per mutation.
    // Use row-level updates if measured local account traffic warrants it.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    tx.execute_batch("DELETE FROM sessions; DELETE FROM users;")
        .map_err(sql)?;
    insert(&tx, db).map_err(sql)?;
    tx.commit().map_err(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{auth_store::AuthManager, config::AppConfig};

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
}
