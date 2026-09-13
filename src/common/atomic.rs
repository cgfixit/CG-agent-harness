//! Atomic file writes: stage into a temp file in the same directory, then rename.
//!
//! Port of CyClaw's `_atomic_write_json` (harness/config.py, sessions.py,
//! real_repo_run_store.py). `tempfile::NamedTempFile::persist` owns the
//! descriptor-and-rename dance the Python version documents at length; the
//! payload is serialized to a string FIRST so a serialization failure never
//! leaves a staged file behind, and the staged bytes are synced to stable
//! storage BEFORE the rename so a crash cannot publish a truncated
//! replacement of a file that held valid content. Durability of the directory
//! entry itself is left to the filesystem's ordering guarantees.

use std::io::Write;
use std::path::Path;

use super::errors::{HarnessError, Result};

/// Write `bytes` to `path` atomically. `mode` (unix only) sets the file mode
/// on the staged file before the rename so the secret is never world-readable.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| HarnessError::new("IO_ERROR", "atomic write target has no parent"))?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".staged.").suffix(".tmp");
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(m));
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut staged = builder.tempfile_in(parent).map_err(|e| {
        HarnessError::new("IO_ERROR", format!("cannot stage file: {e}")).detail("path", path.display().to_string())
    })?;
    staged
        .write_all(bytes)
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot write staged file: {e}")))?;
    staged
        .flush()
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot flush staged file: {e}")))?;
    // Durable before visible: `flush` only empties the userspace buffer, and a
    // rename that reaches disk ahead of the data leaves a zero-length or
    // partial auth session, job record, or run manifest after a crash.
    staged
        .as_file()
        .sync_all()
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot sync staged file: {e}")))?;
    // Close the handle before the rename: Windows refuses to rename a file with
    // an open handle, and persist() on a NamedTempFile keeps it open.
    let temp_path = staged.into_temp_path();
    temp_path.persist(path).map_err(|e| {
        HarnessError::new("IO_ERROR", format!("cannot replace file: {}", e.error))
            .detail("path", path.display().to_string())
    })?;
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m));
    }
    Ok(())
}

/// Serialize `value` as pretty JSON and write it atomically.
pub fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    write_atomic(path, text.as_bytes(), None)
}

/// Same, with an explicit mode (used for secret-bearing files).
pub fn write_json_atomic_mode(path: &Path, value: &serde_json::Value, mode: u32) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    write_atomic(path, text.as_bytes(), Some(mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_in_place_and_leaves_no_staged_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.json");
        write_atomic(&path, b"{\"v\":1}", None).unwrap();
        write_atomic(&path, b"{\"v\":2}", Some(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"v\":2}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".staged."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staged temp files must not survive: {leftovers:?}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(write_atomic(std::path::Path::new("/"), b"x", None).is_err());
    }
}
