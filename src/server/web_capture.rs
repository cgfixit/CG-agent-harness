//! Opt-in evaluation capture. Raw tool text stays outside the content-free audit.
use super::state::AppState;
use crate::common::{random_hex, sha256_hex};
use serde_json::json;
use std::path::Path;

const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
#[cfg(unix)]
const MAX_CAPTURE_BYTES: u64 = 128 * 1024 * 1024;

pub(super) struct Pending {
    pub channel: &'static str,
    pub text: String,
}

/// Called only after the model call succeeds, with the exact tool strings it received.
/// A capture failure changes no admission or model response; its audit row names no text.
pub(super) async fn delivered(state: &AppState, pending: Vec<Pending>) {
    if pending.is_empty() {
        return;
    }
    let directory = state.home.data_dir().join("web-capture");
    let rows = tokio::task::spawn_blocking(move || {
        pending
            .into_iter()
            .map(|item| {
                let source_id = random_hex(16);
                let digest = sha256_hex(&item.text);
                let policy = if item.channel == "search-listing" {
                    "agent-ingest"
                } else {
                    "agent-document"
                };
                let record = json!({
                    "schema_version": 1, "source_id": source_id, "channel": item.channel,
                    "name": "tool-result.json", "policy": policy, "text": item.text,
                    "text_sha256": digest, "extractor": "harness-tool-json",
                    "extractor_version": env!("CARGO_PKG_VERSION"), "actual_decision": "admitted"
                });
                let written = serde_json::to_vec(&record)
                    .ok()
                    .filter(|line| line.len() < MAX_LINE_BYTES)
                    .is_some_and(|line| append(&directory, &line).is_ok());
                json!({"event":"web_capture", "source_id":source_id, "channel":item.channel,
                    "text_sha256":digest, "written":written})
            })
            .collect::<Vec<_>>()
    })
    .await;
    match rows {
        Ok(rows) => {
            for row in rows {
                state.audit.log(row);
            }
        }
        Err(_) => {
            state.audit.log(json!({"event":"web_capture", "written":false}));
        }
    }
}

#[cfg(unix)]
fn append(directory: &Path, line: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    let refused = || std::io::Error::other("private capture storage unavailable");
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    let created = match builder.create(directory) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    // The operator controls the home. Do not repair an unsafe pre-existing capture sink.
    let meta = std::fs::symlink_metadata(directory)?;
    // SAFETY: geteuid has no arguments.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(refused());
    }
    #[cfg(target_os = "macos")]
    {
        use crate::common::credential_store::macos_acl;
        use std::os::fd::AsRawFd;
        let handle = std::fs::File::open(directory)?;
        if created {
            macos_acl::clear_inherited_acl(handle.as_raw_fd())?;
        }
        if macos_acl::fd_has_acl_entries(handle.as_raw_fd()) != Some(false) {
            return Err(refused());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = created;
    let _lease = crate::common::file_lease::FileLease::acquire(&directory.join("capture.lock"), true)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join("capture.jsonl"))?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != uid
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len().saturating_add(line.len() as u64 + 1) > MAX_CAPTURE_BYTES
    {
        return Err(refused());
    }
    #[cfg(target_os = "macos")]
    {
        use crate::common::credential_store::macos_acl;
        use std::os::fd::AsRawFd;
        if meta.len() == 0 {
            macos_acl::clear_inherited_acl(file.as_raw_fd())?;
        }
        if macos_acl::fd_has_acl_entries(file.as_raw_fd()) != Some(false) {
            return Err(refused());
        }
    }
    if let Err(error) = file.write_all(line).and_then(|()| file.write_all(b"\n")) {
        // A short append must not turn the next receipt into a malformed JSONL row.
        // The sink lease covers the append and any rollback.
        file.set_len(meta.len())?;
        return Err(error);
    }
    Ok(())
}

// The pilot requires owner-only storage; Unix modes cannot establish Windows ACLs.
#[cfg(not(unix))]
fn append(_directory: &Path, _line: &[u8]) -> std::io::Result<()> {
    Err(std::io::Error::other("private capture storage unsupported"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn capture_preserves_bytes_and_refuses_unsafe_or_full_storage() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join("capture");
        let line = br#"{"text":"quoted \\" example"}"#;
        append(&directory, line).unwrap();
        let path = directory.join("capture.jsonl");
        assert_eq!(std::fs::read(&path).unwrap(), [line.as_slice(), b"\n"].concat());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(append(&directory, line).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let linked = home.path().join("linked");
        std::fs::hard_link(&path, &linked).unwrap();
        assert!(append(&directory, line).is_err());
        std::fs::remove_file(linked).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_CAPTURE_BYTES)
            .unwrap();
        assert!(append(&directory, line).is_err());
        std::fs::remove_file(&path).unwrap();
        let target = home.path().join("target");
        std::fs::write(&target, b"unchanged").unwrap();
        symlink(&target, &path).unwrap();
        assert!(append(&directory, line).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"unchanged");
        let linked_directory = home.path().join("linked-directory");
        symlink(&directory, &linked_directory).unwrap();
        assert!(append(&linked_directory, line).is_err());
    }
}
