//! Dedicated machine credentials; never console users, sessions, or memory rows.
use crate::common::{atomic::write_atomic, authn::hash_token, config::AppConfig, home::Home, now_ts, random_hex};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use subtle::ConstantTimeEq;

const SCHEMA: &str = "CREATE TABLE mcp_keys (
 key_id TEXT PRIMARY KEY NOT NULL, token_hash TEXT UNIQUE NOT NULL,
 owner_id TEXT NOT NULL, label TEXT NOT NULL,
 scopes TEXT NOT NULL CHECK(scopes='memory:read'), created_ts REAL NOT NULL,
 last_used_ts REAL, disabled INTEGER NOT NULL CHECK(disabled IN(0,1)),
 issued_by_user_id TEXT
) STRICT; PRAGMA user_version=1;";
const MARKER: &[u8] = b"mcp-keys-v1\n";
fn refused() -> anyhow::Error {
    anyhow::anyhow!("MCP_KEYS_REFUSED: restore a valid private machine-key store")
}
fn hex_id(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn present(path: &Path) -> anyhow::Result<bool> {
    // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
    let raw = path.to_string_lossy();
    if raw.contains("..") {
        return Err(refused());
    }
    let path = Path::new(raw.as_ref());
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(refused()),
    }
}
fn private_file(path: &Path) -> anyhow::Result<std::fs::File> {
    // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
    let raw = path.to_string_lossy();
    if raw.contains("..") {
        return Err(refused());
    }
    let path = Path::new(raw.as_ref());
    if std::fs::symlink_metadata(path)
        .map_err(|_| refused())?
        .file_type()
        .is_symlink()
    {
        return Err(refused());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = opts.open(path).map_err(|_| refused())?;
    let meta = file.metadata().map_err(|_| refused())?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err(refused());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(refused());
        }
    }
    Ok(file)
}
fn connect(path: &Path) -> anyhow::Result<Connection> {
    private_file(path)?;
    let parent = path
        .parent()
        .ok_or_else(refused)?
        .canonicalize()
        .map_err(|_| refused())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = parent.metadata().map_err(|_| refused())?;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err(refused());
        }
    }
    let conn = Connection::open_with_flags(
        parent.join(path.file_name().ok_or_else(refused)?),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| refused())?;
    conn.busy_timeout(Duration::from_millis(100)).map_err(|_| refused())?;
    conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(|_| refused())?;
    Ok(conn)
}
#[derive(Clone, Serialize)]
pub struct KeyInfo {
    pub key_id: String,
    pub owner_id: String,
    pub label: String,
    pub scopes: String,
    pub created_ts: f64,
    pub last_used_ts: Option<f64>,
    pub disabled: bool,
    pub issued_by_user_id: Option<String>,
}
#[derive(Clone)]
pub struct Principal {
    pub key_id: String,
    pub owner_id: String,
}
#[derive(Clone)]
pub struct KeyStore {
    path: PathBuf,
    max_keys: usize,
}

/// Who may receive a new machine key.
pub enum OwnerCheck<'a> {
    /// `auth.enabled` is off. The only owner is `local`.
    LocalOnly,
    /// `auth.enabled` is on. The owner must be an existing enabled account id.
    EnabledOwners(&'a [String]),
}
impl OwnerCheck<'_> {
    fn allows(&self, owner: &str) -> bool {
        match self {
            Self::LocalOnly => owner == "local",
            Self::EnabledOwners(owners) => owners.iter().any(|id| id == owner),
        }
    }
}

fn rows(conn: &Connection) -> anyhow::Result<Vec<KeyInfo>> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|_| refused())?;
    if version != 1 {
        return Err(refused());
    }
    let strict: i64 = conn
        .query_row(
            "SELECT strict FROM pragma_table_list WHERE name='mcp_keys' AND type='table'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| refused())?;
    if strict != 1 {
        return Err(refused());
    }
    let mut stmt=conn.prepare("SELECT key_id,owner_id,label,scopes,created_ts,last_used_ts,disabled,issued_by_user_id,token_hash FROM mcp_keys ORDER BY created_ts,key_id LIMIT 33").map_err(|_|refused())?;
    let values = stmt
        .query_map([], |r| {
            Ok((
                KeyInfo {
                    key_id: r.get(0)?,
                    owner_id: r.get(1)?,
                    label: r.get(2)?,
                    scopes: r.get(3)?,
                    created_ts: r.get(4)?,
                    last_used_ts: r.get(5)?,
                    disabled: r.get(6)?,
                    issued_by_user_id: r.get(7)?,
                },
                r.get::<_, String>(8)?,
            ))
        })
        .map_err(|_| refused())?;
    let mut result = Vec::new();
    for value in values {
        let (info, hash) = value.map_err(|_| refused())?;
        if !hex_id(&info.key_id, 32)
            || !hex_id(&hash, 64)
            || !super::structured_memory::valid_owner(&info.owner_id)
            || info.label.trim().is_empty()
            || info.label.chars().count() > 80
            || info.label.chars().any(char::is_control)
            || info.scopes != "memory:read"
            || !info.created_ts.is_finite()
            || !(0.0..1e12).contains(&info.created_ts)
            || info
                .last_used_ts
                .is_some_and(|t| !t.is_finite() || !(0.0..1e12).contains(&t))
            || info.issued_by_user_id.as_ref().is_some_and(|s| !hex_id(s, 32))
        {
            return Err(refused());
        }
        result.push(info);
    }
    if result.len() > 32 {
        return Err(refused());
    }
    Ok(result)
}
impl KeyStore {
    pub fn open(home: &Home, max_keys: usize) -> anyhow::Result<Self> {
        if !(1..=32).contains(&max_keys) {
            return Err(refused());
        }
        let path = home.root.join("mcp_keys.sqlite3");
        let marker = home.root.join("mcp_keys.initialized");
        if present(&marker)? {
            let mut bytes = Vec::new();
            private_file(&marker)?
                .take(64)
                .read_to_end(&mut bytes)
                .map_err(|_| refused())?;
            if bytes != MARKER || !present(&path)? {
                return Err(refused());
            }
        }
        if !present(&path)? {
            let temp = tempfile::Builder::new()
                .prefix(".mcp-keys.")
                .tempfile_in(&home.root)
                .map_err(|_| refused())?;
            let conn = connect(temp.path())?;
            conn.execute_batch(SCHEMA).map_err(|_| refused())?;
            rows(&conn)?;
            conn.close().map_err(|_| refused())?;
            temp.as_file().sync_all().map_err(|_| refused())?;
            temp.persist_noclobber(&path).map_err(|_| refused())?;
        }
        let store = Self { path, max_keys };
        store.list()?;
        if !present(&marker)? {
            write_atomic(&marker, MARKER, Some(0o600)).map_err(|_| refused())?;
        }
        Ok(store)
    }
    pub fn list(&self) -> anyhow::Result<Vec<KeyInfo>> {
        rows(&connect(&self.path)?)
    }
    /// Trusted local CLI authority supplies the namespace. No HTTP caller can choose it.
    /// The owner must exist and be enabled. Auth-disabled homes may mint only `local`.
    pub fn mint(&self, owner: &str, label: &str, owners: OwnerCheck<'_>) -> anyhow::Result<(KeyInfo, String)> {
        if !super::structured_memory::valid_owner(owner)
            || label.trim().is_empty()
            || label.chars().count() > 80
            || label.chars().any(char::is_control)
        {
            anyhow::bail!("MCP_KEY_INVALID: supply a valid owner ID and a label of 1-80 printable characters");
        }
        if !owners.allows(owner) {
            anyhow::bail!("MCP_KEY_OWNER_REFUSED: owner must exist and be enabled");
        }
        let mut conn = connect(&self.path)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| refused())?;
        let current = rows(&tx)?;
        if current.iter().filter(|k| !k.disabled).count() >= self.max_keys {
            anyhow::bail!("MCP_KEY_LIMIT: revoke an existing key before minting");
        }
        // Keep a bounded table across rotations; revocation remains in content-free audit.
        let needed = (current.len() + 1).saturating_sub(self.max_keys);
        for key in current.iter().filter(|k| k.disabled).take(needed) {
            tx.execute("DELETE FROM mcp_keys WHERE key_id=?1 AND disabled=1", [&key.key_id])
                .map_err(|_| refused())?;
        }
        let id = random_hex(16);
        let secret = random_hex(32);
        let now = now_ts();
        tx.execute(
            "INSERT INTO mcp_keys VALUES(?1,?2,?3,?4,'memory:read',?5,NULL,0,NULL)",
            params![id, hash_token(&secret), owner, label.trim(), now],
        )
        .map_err(|_| refused())?;
        tx.commit().map_err(|_| refused())?;
        let info = KeyInfo {
            key_id: id.clone(),
            owner_id: owner.into(),
            label: label.trim().into(),
            scopes: "memory:read".into(),
            created_ts: now,
            last_used_ts: None,
            disabled: false,
            issued_by_user_id: None,
        };
        Ok((info, format!("cgamcp_{id}.{secret}")))
    }
    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        if !hex_id(id, 32) {
            return Ok(false);
        }
        let conn = connect(&self.path)?;
        rows(&conn)?;
        Ok(conn
            .execute("UPDATE mcp_keys SET disabled=1 WHERE key_id=?1", [id])
            .map_err(|_| refused())?
            > 0)
    }

    /// Disable every key for this owner. Key-id revocation stays available.
    /// A read that already authenticated may finish; this is not an in-flight kill.
    pub fn disable_owner(&self, owner: &str) -> anyhow::Result<usize> {
        if !super::structured_memory::valid_owner(owner) || !present(&self.path)? {
            return Ok(0);
        }
        let conn = connect(&self.path)?;
        rows(&conn)?;
        conn.execute(
            "UPDATE mcp_keys SET disabled=1 WHERE owner_id=?1 AND disabled=0",
            [owner],
        )
        .map_err(|_| refused())
    }

    /// No-op when this home has never initialized a machine-key database.
    pub fn disable_owner_if_initialized(home: &Home, owner: &str) -> anyhow::Result<usize> {
        let path = home.root.join("mcp_keys.sqlite3");
        let marker = home.root.join("mcp_keys.initialized");
        if !present(&path)? || !present(&marker)? {
            return Ok(0);
        }
        Self { path, max_keys: 1 }.disable_owner(owner)
    }

    pub fn authenticate(&self, token: &str) -> anyhow::Result<Principal> {
        // Always hash and compare, including unknown, disabled and malformed inputs.
        let (id, secret) = token
            .strip_prefix("cgamcp_")
            .and_then(|s| s.split_once('.'))
            .unwrap_or(("", ""));
        let computed = hash_token(secret);
        let conn = connect(&self.path)?;
        let all = rows(&conn)?;
        let found = all.into_iter().find(|k| k.key_id == id);
        let stored: Option<String> = conn
            .query_row("SELECT token_hash FROM mcp_keys WHERE key_id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(|_| refused())?;
        let dummy = hash_token("MCP unknown high entropy token");
        let equal = bool::from(
            computed
                .as_bytes()
                .ct_eq(stored.as_deref().unwrap_or(&dummy).as_bytes()),
        );
        let key = found
            .filter(|k| !k.disabled && k.scopes == "memory:read" && equal && hex_id(id, 32) && hex_id(secret, 64))
            .ok_or_else(|| anyhow::anyhow!("MCP_AUTH_REQUIRED"))?;
        // This UPDATE also observes revocation committed after the lookup.
        if conn
            .execute(
                "UPDATE mcp_keys SET last_used_ts=?2 WHERE key_id=?1 AND disabled=0 AND scopes='memory:read'",
                params![id, now_ts()],
            )
            .map_err(|_| refused())?
            != 1
        {
            anyhow::bail!("MCP_AUTH_REQUIRED");
        }
        Ok(Principal {
            key_id: key.key_id,
            owner_id: key.owner_id,
        })
    }
    pub fn still_authorized(&self, principal: &Principal) -> bool {
        self.list().is_ok_and(|keys| {
            keys.iter().any(|k| {
                k.key_id == principal.key_id
                    && k.owner_id == principal.owner_id
                    && !k.disabled
                    && k.scopes == "memory:read"
            })
        })
    }
}

#[derive(clap::Subcommand)]
pub enum KeyCommand {
    /// Print public key metadata, never token hashes or secrets.
    List,
    /// Mint a dedicated memory:read key; the secret appears once on stdout.
    Create {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        reason: String,
    },
    /// Revoke a key immediately without restarting the server.
    Revoke {
        key_id: String,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        reason: String,
    },
}
pub fn run(command: KeyCommand) -> anyhow::Result<()> {
    let home = Home::resolve(None);
    home.ensure_layout()?;
    let cfg = AppConfig::load(&home.config_path())?;
    let settings = super::mcp_server_config::Settings::load(&cfg)?;
    let store = KeyStore::open(&home, settings.max_keys)?;
    let audit = crate::common::audit::Audit::from_home(&home.root, &cfg);
    let valid = |confirm: bool, reason: &str| -> anyhow::Result<()> {
        if !confirm || reason.trim().is_empty() || reason.chars().count() > 1000 {
            anyhow::bail!("MCP_KEY_CONFIRM_REQUIRED: --confirm and a bounded --reason are required");
        }
        Ok(())
    };
    match command {
        KeyCommand::List => println!("{}", serde_json::to_string_pretty(&store.list()?)?),
        KeyCommand::Create {
            owner,
            label,
            confirm,
            reason,
        } => {
            valid(confirm, &reason)?;
            let enabled = if cfg.flag_is_true("auth.enabled") {
                let accounts = crate::common::auth_store::AuthManager::open(&home.auth_path(), &cfg)
                    .map_err(|_| anyhow::anyhow!("MCP_KEY_OWNER_REFUSED: account directory is unavailable"))?;
                accounts
                    .list_users()
                    .into_iter()
                    .filter(|user| !user.disabled)
                    .map(|user| user.user_id)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let owners = if cfg.flag_is_true("auth.enabled") {
                OwnerCheck::EnabledOwners(&enabled)
            } else {
                OwnerCheck::LocalOnly
            };
            let (info, token) = store.mint(&owner, &label, owners)?;
            audit.log(serde_json::json!({"event":"mcp_key_created","key_id":info.key_id}));
            println!("{}", serde_json::json!({"key":info,"token":token}));
        }
        KeyCommand::Revoke {
            key_id,
            confirm,
            reason,
        } => {
            valid(confirm, &reason)?;
            if !store.revoke(&key_id)? {
                anyhow::bail!("MCP_KEY_NOT_FOUND");
            }
            audit.log(serde_json::json!({"event":"mcp_key_revoked","key_id":key_id}));
            println!("Key revoked.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_refuses_parent_components() {
        let err = present(Path::new("/tmp/harness/../mcp_keys.sqlite3")).unwrap_err();
        assert!(err.to_string().contains("MCP_KEYS_REFUSED"));
    }
}
