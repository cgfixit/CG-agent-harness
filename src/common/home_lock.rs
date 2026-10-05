//! Advisory ownership for a server home, never PID-file authority.
use super::file_lease::FileLease;
use std::path::Path;

pub struct HomeLock {
    _lease: FileLease,
}
impl HomeLock {
    pub fn acquire(home: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(home)?;
        match FileLease::acquire(&home.join(".server.lock"), true) {
            Ok(lease) => Ok(Self { _lease: lease }),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Err(anyhow::anyhow!(
                "the server is running and holds this home; stop it before renewing the certificate or starting another server"
            )),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn busy_home_lock_names_the_server_and_does_not_wait() {
        let dir = tempfile::tempdir().expect("temp home");
        let held = match HomeLock::acquire(dir.path()) {
            Ok(lock) => lock,
            Err(err) => panic!("first lock should succeed: {err}"),
        };
        let started = std::time::Instant::now();
        let err = match HomeLock::acquire(dir.path()) {
            Err(err) => err,
            Ok(_) => panic!("second lock must fail closed"),
        };
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        let text = err.to_string();
        assert!(text.contains("the server is running and holds this home"), "{text}");
        assert!(text.contains("stop it before renewing the certificate"), "{text}");
        assert!(!text.contains("os error"), "{text}");
        drop(held);
        if let Err(err) = HomeLock::acquire(dir.path()) {
            panic!("lock should succeed after release: {err}");
        }
    }
}
