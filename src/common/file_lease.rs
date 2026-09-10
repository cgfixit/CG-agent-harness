//! OS-held ownership. Lock files are never deleted and PIDs are never authority.
use std::fs::{File, OpenOptions};
use std::path::Path;

pub struct FileLease(File);
impl FileLease {
    pub fn acquire(path: &Path, create: bool) -> std::io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("lease is not a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            // SAFETY: getuid has no arguments; file is our live descriptor.
            if file.metadata()?.uid() != unsafe { libc::getuid() } {
                return Err(std::io::Error::other("lease is not owned by this user"));
            }
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Self(file))
    }
}
impl Drop for FileLease {
    fn drop(&mut self) {
        let _ = &self.0;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: our descriptor remains live until Drop returns.
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
