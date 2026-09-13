//! Two-generation JSONL retention, serialized between cooperating Unix writers.
use super::{config::AppConfig, file_lease::FileLease};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A busy lease is retried within this bound before the record is refused.
/// The agentic child and the server append to one audit file, so a momentary
/// overlap must not drop either writer's evidence; an unavailable sink still
/// cannot hold a request longer than this.
const LEASE_WAIT: Duration = Duration::from_millis(50);
const LEASE_RETRY: Duration = Duration::from_millis(2);

fn acquire_lease(path: &Path) -> std::io::Result<FileLease> {
    let deadline = Instant::now() + LEASE_WAIT;
    loop {
        match FileLease::acquire(path, true) {
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(LEASE_RETRY),
            other => return other,
        }
    }
}

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
    let _lease = acquire_lease(&adjacent(path, ".lock"))?;
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

    #[cfg(unix)]
    #[test]
    fn a_briefly_busy_lease_is_awaited_and_a_stuck_one_is_refused_in_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let lock = adjacent(&path, ".lock");
        // Another writer holds the lease from before this append starts and
        // releases it inside the retry window: the record must land, after
        // waiting, instead of being dropped on the first EWOULDBLOCK.
        let held = FileLease::acquire(&lock, true).unwrap();
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(held);
        });
        let start = Instant::now();
        append(&path, "{\"n\":1}", 1024).unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(15),
            "append did not wait for the lease"
        );
        holder.join().unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"n\":1"));
        // A lease held past the bound is refused, and the caller is not hung.
        let _stuck = FileLease::acquire(&lock, true).unwrap();
        let start = Instant::now();
        let err = append(&path, "{\"n\":2}", 1024).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::WouldBlock);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("\"n\":2"));
    }
}
