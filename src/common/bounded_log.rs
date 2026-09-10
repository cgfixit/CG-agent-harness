//! Two-generation JSONL retention, serialized between cooperating Unix writers.
use super::{config::AppConfig, file_lease::FileLease};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn limit(cfg: &AppConfig) -> u64 {
    cfg.u64_or("logging.max_file_bytes", 8 * 1024 * 1024)
        .clamp(64 * 1024, 64 * 1024 * 1024)
}
fn adjacent(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
pub fn append(path: &Path, line: &str, max_bytes: u64) -> std::io::Result<()> {
    if line.len() as u64 + 1 > max_bytes {
        return Err(std::io::Error::other("log record exceeds retention bound"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Nonblocking acquisition: an unavailable sink must not hang a request.
    let _lease = FileLease::acquire(&adjacent(path, ".lock"), true)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("log is not a regular file"));
    }
    if file.metadata()?.len().saturating_add(line.len() as u64 + 1) > max_bytes {
        drop(file);
        let previous = adjacent(path, ".1");
        // Windows rename does not replace. Only this documented retention file
        // is removed; the current log remains intact if the rename fails.
        match std::fs::remove_file(&previous) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        std::fs::rename(path, &previous)?;
        file = options.open(path)?;
    }
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_retains_recent_generation_and_rejects_oversized_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        for n in 0..20 {
            append(&path, &format!("{{\"n\":{n}}}"), 32).unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= 32);
        assert!(std::fs::metadata(adjacent(&path, ".1")).unwrap().len() <= 32);
        assert!(std::fs::read_to_string(&path).unwrap().contains("19"));
        assert!(append(&path, &"x".repeat(33), 32).is_err());
    }
}
