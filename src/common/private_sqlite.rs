//! Owner-only SQLite files: the one open path account storage and structured
//! memory share. `label` names the store in refusals ("account", ...) and
//! `invalid` supplies the caller's error code.
use crate::common::errors::{HarnessError, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Opens `path` read-only without following a link, and refuses anything but a
/// regular file of at most `max_bytes`, owned by this user, private, and not hard-linked.
pub fn private_file(
    path: &Path,
    max_bytes: u64,
    label: &str,
    invalid: fn(String) -> HarnessError,
) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return Err(invalid(format!("{label} file must be a bounded regular file")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(invalid(format!(
                "{label} file must be owned, private, and not hard-linked"
            )));
        }
    }
    Ok(file)
}

/// Checks the file with [`private_file`] and its directory for owner-only writes,
/// then opens it read-write with `SQLITE_OPEN_NOFOLLOW` and the shared pragmas.
pub fn connect(path: &Path, max_bytes: u64, label: &str, invalid: fn(String) -> HarnessError) -> Result<Connection> {
    private_file(path, max_bytes, label, invalid)?;
    // macOS exposes its temporary directory through /var -> /private/var.
    // Resolve the directory only; SQLite still refuses a linked database leaf.
    let parent = path
        .parent()
        .ok_or_else(|| invalid(format!("missing {label} directory")))?
        .canonicalize()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = parent.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(invalid(format!(
                "{label} directory must be owned and not writable by other users"
            )));
        }
    }
    let path = parent.join(
        path.file_name()
            .ok_or_else(|| invalid(format!("missing {label} filename")))?,
    );
    let sql = |error: rusqlite::Error| invalid(format!("{label} database refused: {error}"));
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
