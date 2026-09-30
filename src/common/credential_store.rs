//! OS credential store for managed provider keys, plus the one-time `.env` migration.
//!
//! Read order at startup is an inherited environment variable, then this store.
//! A legacy home `.env` is input to [`migrate_text`] only, unless
//! `security.allow_plaintext_key_file` is the literal boolean `true`.
//! Values are never included in errors, warnings, or `Debug` output from this module.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use subtle::ConstantTimeEq;

pub const SERVICE: &str = "CGagentHarness";
pub const PLAINTEXT_KEY_FILE: &str = "security.allow_plaintext_key_file";
pub const OS_STORE_LABEL: &str = "os-credential-store";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Unavailable,
    WriteFailed,
    VerifyMismatch,
    ReadFailed,
}

/// Platform credential store. Unit tests supply a mock; production uses [`OsCredentialStore`].
pub trait CredentialStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError>;
    fn set(&self, name: &str, value: &str) -> Result<(), StoreError>;
    fn delete(&self, name: &str) -> Result<(), StoreError>;
}

pub struct MigrationReport {
    pub migrated: Vec<String>,
    pub failed: Vec<(String, StoreError)>,
    pub text: String,
}

/// Write each managed assignment, read it back, and drop only the lines that verified.
/// Unknown lines stay. A failed name is left in `text` and is not reported as migrated.
pub fn migrate_text(
    text: &str,
    assignments: &BTreeMap<String, String>,
    store: &dyn CredentialStore,
) -> MigrationReport {
    let mut remove = BTreeSet::new();
    let mut migrated = Vec::new();
    let mut failed = Vec::new();
    for (name, value) in assignments {
        match store_and_verify(store, name, value) {
            Ok(()) => {
                remove.insert(name.clone());
                migrated.push(name.clone());
            }
            Err(error) => failed.push((name.clone(), error)),
        }
    }
    MigrationReport {
        migrated,
        failed,
        text: strip_assignments(text, &remove),
    }
}

fn store_and_verify(store: &dyn CredentialStore, name: &str, value: &str) -> Result<(), StoreError> {
    store.set(name, value)?;
    match store.get(name) {
        Ok(Some(got)) if secret_eq(&got, value) => Ok(()),
        Ok(_) => {
            let _ = store.delete(name);
            Err(StoreError::VerifyMismatch)
        }
        Err(StoreError::Unavailable) => Err(StoreError::Unavailable),
        Err(_) => {
            let _ = store.delete(name);
            Err(StoreError::VerifyMismatch)
        }
    }
}

pub fn secret_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

pub fn migration_warning(name: &str, error: StoreError) -> String {
    let why = match error {
        StoreError::Unavailable => "the OS credential store is unavailable",
        StoreError::WriteFailed => "the OS credential store refused the write",
        StoreError::VerifyMismatch => "the OS credential store read-back did not match",
        StoreError::ReadFailed => "the OS credential store could not be read",
    };
    format!(
        "{name} remains in the private .env file because {why}. It was not loaded. Unlock the OS credential store (macOS Keychain, Linux Secret Service, or Windows Credential Manager), or set {PLAINTEXT_KEY_FILE}: true and restart."
    )
}

pub fn save_refusal(error: StoreError) -> &'static str {
    match error {
        StoreError::Unavailable => {
            "OS credential store is unavailable, so the key was not saved. On Linux, start a Secret Service provider in a session that sets DBUS_SESSION_BUS_ADDRESS. On macOS, unlock Keychain. On Windows, use Credential Manager. To keep the legacy 0600 .env file, set security.allow_plaintext_key_file: true and restart."
        }
        StoreError::VerifyMismatch => {
            "OS credential store read-back did not match, so the key was not saved."
        }
        StoreError::WriteFailed | StoreError::ReadFailed => {
            "OS credential store refused the update, so the key was not saved."
        }
    }
}

/// macOS Keychain, Windows Credential Manager, or Linux Secret Service.
/// Entries are scoped by the canonical home path so two homes do not share keys.
pub struct OsCredentialStore {
    target: String,
    unavailable: AtomicBool,
}

impl OsCredentialStore {
    pub fn for_home(home: &Path) -> Self {
        let target = std::fs::canonicalize(home)
            .unwrap_or_else(|_| home.to_path_buf())
            .display()
            .to_string();
        Self {
            target,
            unavailable: AtomicBool::new(platform_store_missing()),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, StoreError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(StoreError::Unavailable);
        }
        keyring::Entry::new_with_target(&self.target, SERVICE, name).map_err(|error| self.classify(error))
    }

    fn classify(&self, error: keyring::Error) -> StoreError {
        match error {
            keyring::Error::NoStorageAccess(_) => {
                self.unavailable.store(true, Ordering::Release);
                StoreError::Unavailable
            }
            keyring::Error::NoEntry => StoreError::ReadFailed,
            _ => StoreError::WriteFailed,
        }
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        let entry = self.entry(name)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(self.classify(error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        let entry = self.entry(name)?;
        entry.set_password(value).map_err(|error| self.classify(error))
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        let entry = self.entry(name)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(self.classify(error)),
        }
    }
}

/// Headless Linux without a session bus cannot reach Secret Service. Fail closed
/// before the client blocks. macOS and Windows always attempt the native store.
fn platform_store_missing() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|value| value.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

const QUOTE_ESCAPE: &str = r"'\''";

pub(crate) fn split_assignment(line: &str) -> Option<(String, String)> {
    let mut stripped = line.trim();
    if stripped.is_empty() || stripped.starts_with('#') {
        return None;
    }
    if let Some(rest) = stripped.strip_prefix("export ") {
        stripped = rest.trim_start();
    }
    let (name, raw) = stripped.split_once('=')?;
    Some((name.trim().to_string(), raw.to_string()))
}

pub(crate) fn unquote(raw: &str) -> String {
    let token = raw.trim();
    if token.len() >= 2 {
        let first = token.chars().next().unwrap();
        let last = token.chars().last().unwrap();
        if first == last && (first == '\'' || first == '"') {
            let inner = &token[1..token.len() - 1];
            return if first == '\'' {
                inner.replace(QUOTE_ESCAPE, "'")
            } else {
                inner.to_string()
            };
        }
    }
    token.to_string()
}

pub(crate) fn parse_managed(text: &str, is_managed: impl Fn(&str) -> bool) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in text.lines() {
        if let Some((name, raw)) = split_assignment(line) {
            if is_managed(&name) {
                found.insert(name, unquote(&raw));
            }
        }
    }
    found
}

pub(crate) fn strip_assignments(text: &str, remove: &BTreeSet<String>) -> String {
    if remove.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let logical = line.trim_end_matches(['\n', '\r']);
        if let Some((name, _)) = split_assignment(logical) {
            if remove.contains(&name) {
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Private credential file: regular, owner-only, not a symlink, at most 64 KiB.
/// Missing is empty. The bytes are migration input or the plaintext opt-in, never a log line.
pub(crate) fn read_private_text(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => anyhow::bail!("credential file unreadable; check its owner, access and symlink status"),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        anyhow::bail!("credential file must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no arguments or memory effects.
        if metadata.uid() != unsafe { libc::getuid() } || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            anyhow::bail!("credential file must be private, singly linked, and owned by this user (0600)");
        }
    }
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("credential file unreadable or not UTF-8"))?;
    if text.len() > 65536 {
        anyhow::bail!("credential file exceeds 64 KiB");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockStore {
        values: Mutex<BTreeMap<String, String>>,
        fail_write: Mutex<BTreeSet<String>>,
        mismatch: Mutex<BTreeSet<String>>,
        unavailable: Mutex<bool>,
    }

    impl MockStore {
        fn new() -> Self {
            Self {
                values: Mutex::new(BTreeMap::new()),
                fail_write: Mutex::new(BTreeSet::new()),
                mismatch: Mutex::new(BTreeSet::new()),
                unavailable: Mutex::new(false),
            }
        }
    }

    impl CredentialStore for MockStore {
        fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
            if *self.unavailable.lock().unwrap() {
                return Err(StoreError::Unavailable);
            }
            if self.mismatch.lock().unwrap().contains(name) {
                return Ok(Some(format!("mismatch-{name}")));
            }
            Ok(self.values.lock().unwrap().get(name).cloned())
        }

        fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
            if *self.unavailable.lock().unwrap() {
                return Err(StoreError::Unavailable);
            }
            if self.fail_write.lock().unwrap().contains(name) {
                return Err(StoreError::WriteFailed);
            }
            self.values.lock().unwrap().insert(name.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<(), StoreError> {
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }

    fn sample() -> (String, BTreeMap<String, String>) {
        let text = "\
# keep me\n\
export OTHER='leave'\n\
export GROK_API_KEY='grok-secret-value-1111'\n\
export ANTHROPIC_API_KEY='anthropic-secret-value-2222'\n";
        let mut assignments = BTreeMap::new();
        assignments.insert("GROK_API_KEY".into(), "grok-secret-value-1111".into());
        assignments.insert("ANTHROPIC_API_KEY".into(), "anthropic-secret-value-2222".into());
        (text.to_string(), assignments)
    }

    #[test]
    fn migration_removes_only_verified_keys_and_keeps_unknown_lines() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.failed, Vec::new());
        assert_eq!(
            report.migrated,
            vec!["ANTHROPIC_API_KEY".to_string(), "GROK_API_KEY".to_string()]
        );
        assert!(report.text.contains("# keep me"));
        assert!(report.text.contains("export OTHER='leave'"));
        assert!(!report.text.contains("grok-secret-value-1111"));
        assert!(!report.text.contains("anthropic-secret-value-2222"));
        assert_eq!(
            store.values.lock().unwrap().get("GROK_API_KEY").map(String::as_str),
            Some("grok-secret-value-1111")
        );
        let warning = migration_warning("GROK_API_KEY", StoreError::WriteFailed);
        assert!(!warning.contains("grok-secret-value-1111"));
    }

    #[test]
    fn migration_keeps_the_line_when_the_write_fails() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.fail_write.lock().unwrap().insert("GROK_API_KEY".into());
        store.fail_write.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert!(report.text.contains("grok-secret-value-1111"));
        assert!(report.text.contains("anthropic-secret-value-2222"));
        assert!(store.values.lock().unwrap().is_empty());
        assert_eq!(report.failed.len(), 2);
        assert!(report.failed.iter().all(|(_, error)| *error == StoreError::WriteFailed));
    }

    #[test]
    fn migration_keeps_the_line_when_read_back_mismatches() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.mismatch.lock().unwrap().insert("GROK_API_KEY".into());
        store.mismatch.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert!(report.text.contains("grok-secret-value-1111"));
        assert!(store.values.lock().unwrap().get("GROK_API_KEY").is_none());
        assert!(report
            .failed
            .iter()
            .all(|(_, error)| *error == StoreError::VerifyMismatch));
        let warning = migration_warning("GROK_API_KEY", StoreError::VerifyMismatch);
        assert!(!warning.contains("grok-secret-value-1111"));
        assert!(!warning.contains("mismatch-GROK_API_KEY"));
    }

    #[test]
    fn migration_is_partial_when_only_one_key_verifies() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.fail_write.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.migrated, vec!["GROK_API_KEY".to_string()]);
        assert!(!report.text.contains("grok-secret-value-1111"));
        assert!(report.text.contains("anthropic-secret-value-2222"));
        assert!(report.text.contains("export OTHER='leave'"));
        assert_eq!(
            store.values.lock().unwrap().get("GROK_API_KEY").map(String::as_str),
            Some("grok-secret-value-1111")
        );
        assert!(store.values.lock().unwrap().get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(
            report.failed,
            vec![("ANTHROPIC_API_KEY".into(), StoreError::WriteFailed)]
        );
    }

    #[test]
    fn migration_leaves_every_line_when_the_store_is_unavailable() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        *store.unavailable.lock().unwrap() = true;
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.text, text);
        assert!(report.migrated.is_empty());
        assert!(report.failed.iter().all(|(_, error)| *error == StoreError::Unavailable));
        let warning = migration_warning("SERPAPI_API_KEY", StoreError::Unavailable);
        assert!(!warning.contains("secret"));
        assert!(warning.contains(PLAINTEXT_KEY_FILE));
    }

    #[test]
    fn os_store_round_trip_runs_only_when_requested() {
        if std::env::var("CGAGENTHARNESS_KEYRING_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = OsCredentialStore::for_home(dir.path());
        let name = "CGAGENTHARNESS_KEYRING_TEST_KEY";
        let value = "os-store-round-trip-value";
        store.delete(name).unwrap();
        store.set(name, value).unwrap();
        let got = store.get(name).unwrap().unwrap();
        assert!(secret_eq(&got, value));
        store.delete(name).unwrap();
        assert!(store.get(name).unwrap().is_none());
    }
}
