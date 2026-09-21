//! Private, bounded metadata outbox. Dispatch attempts persist before networking.
use super::notifications::Completion;
use crate::common::errors::{HarnessError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub delivery_id: String,
    pub event_id: String,
    pub owner: String,
    pub destination_id: String,
    pub destination_revision: String,
    pub completion: Completion,
    pub state: String,
    pub attempts: u64,
    pub replays: u64,
    pub next_attempt_at: f64,
    pub last_code: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub version: u32,
    pub deliveries: BTreeMap<String, Delivery>,
    pub next_send_at: BTreeMap<String, f64>,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            version: 1,
            deliveries: BTreeMap::new(),
            next_send_at: BTreeMap::new(),
        }
    }
}

pub struct Outbox {
    pub data: Data,
    path: PathBuf,
}
fn invalid() -> HarnessError {
    HarnessError::config("invalid notification outbox; preserve the file for inspection")
}
pub fn digest(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value))
}

/// Bounded regular-file read; refuses links and FIFOs rather than blocking.
pub fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(bytes)
}

impl Outbox {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or_else(invalid)?;
        std::fs::create_dir_all(parent)?;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let data: Data = match std::fs::symlink_metadata(path) {
            Ok(_) => serde_json::from_slice(&read_private(path, MAX_BYTES)?).map_err(|_| invalid())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(e.into()),
        };
        if data.version != 1 || data.deliveries.len() > 512 || data.next_send_at.len() > 8 {
            return Err(invalid());
        }
        for (id, d) in &data.deliveries {
            if id != &d.delivery_id
                || id.len() != 64
                || !id.bytes().all(|b| b.is_ascii_hexdigit())
                || !super::structured_memory::valid_owner(&d.owner)
                || d.completion.owner != d.owner
                || !d.completion.valid()
                || d.event_id != d.completion.event_id()
                || d.destination_id.is_empty()
                || d.destination_id.len() > 64
                || d.destination_revision.len() != 64
                || !d.destination_revision.bytes().all(|b| b.is_ascii_hexdigit())
                || d.delivery_id
                    != digest(format!("{}:{}:{}", d.event_id, d.destination_id, d.destination_revision).as_bytes())
                || d.last_code.as_ref().is_some_and(|s| s.len() > 64)
                || !d.next_attempt_at.is_finite()
                || d.next_attempt_at < 0.0
                || d.attempts > 3
                || d.replays > 10
                || !["pending", "delivered", "failed", "revoked"].contains(&d.state.as_str())
            {
                return Err(invalid());
            }
        }
        if data.next_send_at.values().any(|t| !t.is_finite() || *t < 0.0) {
            return Err(invalid());
        }
        let mut out = Self {
            data: Data::default(),
            path: path.into(),
        };
        out.commit(data)?;
        Ok(out)
    }
    pub fn commit(&mut self, next: Data) -> Result<()> {
        if next == self.data && self.path.is_file() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&next)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid());
        }
        crate::common::atomic::write_atomic(&self.path, &bytes, Some(0o600))?;
        // Rename has committed the visible state even if directory sync later
        // fails. Keep memory aligned; callers still refuse network on failure.
        self.data = next;
        // Publish directory ordering on Unix; power-loss guarantees still depend
        // on the filesystem and storage device.
        #[cfg(unix)]
        std::fs::File::open(self.path.parent().ok_or_else(invalid)?)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_oversized_or_linked_store_is_preserved_and_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbox.json");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(Outbox::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        std::fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(Outbox::open(&path).is_err());
        std::fs::write(&path, br#"{"version":2,"deliveries":{},"next_send_at":{}}"#).unwrap();
        assert!(Outbox::open(&path).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("linked.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(Outbox::open(&link).is_err());
        }
    }
    #[test]
    fn failed_atomic_commit_keeps_old_memory_and_records_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbox.json");
        let mut store = Outbox::open(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut changed = store.data.clone();
        changed.next_send_at.insert("fixture".into(), 123.0);
        assert!(store.commit(changed).is_err());
        assert!(store.data.next_send_at.is_empty());
    }
}
