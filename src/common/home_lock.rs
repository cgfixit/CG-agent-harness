//! Advisory ownership for a server home, never PID-file authority.
use super::file_lease::FileLease;
use std::path::Path;

pub struct HomeLock {
    _lease: FileLease,
}
impl HomeLock {
    pub fn acquire(home: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(home)?;
        Ok(Self {
            _lease: FileLease::acquire(&home.join(".server.lock"), true)?,
        })
    }
}
