//! User/home-scoped instance ownership. This channel only requests window focus.
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub struct Instance {
    _lock: File,
    pub listener: UnixListener,
    socket: PathBuf,
}

impl Instance {
    pub fn acquire(home: &Path) -> Result<Option<Self>, String> {
        std::fs::create_dir_all(home).map_err(|_| "Cannot create the configured application home.")?;
        let home = home.canonicalize().map_err(|_| "Cannot resolve application home.")?;
        // SAFETY: getuid has no arguments.
        let uid = unsafe { libc::getuid() };
        let directory = PathBuf::from(format!("/private/tmp/cgah-desktop-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("Cannot create private desktop ownership directory.".into()),
        }
        let metadata =
            std::fs::symlink_metadata(&directory).map_err(|_| "Cannot inspect desktop ownership directory.")?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err("Desktop ownership directory must be private and owned by this user.".into());
        }
        use std::os::unix::ffi::OsStrExt;
        let digest = hex::encode(Sha256::digest(home.as_os_str().as_bytes()));
        let socket = directory.join(format!("{}.sock", &digest[..24]));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join(format!("{}.lock", &digest[..24])))
            .map_err(|_| "Cannot open desktop ownership lock.")?;
        let metadata = lock.metadata().map_err(|_| "Cannot inspect desktop lock.")?;
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err("Unsafe desktop ownership lock.".into());
        }
        // SAFETY: lock remains open for the entire instance lifetime.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let mut stream = UnixStream::connect(&socket)
                .map_err(|_| "This home's desktop is already starting. Reopen it shortly.")?;
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(1)))
                .map_err(|_| "Cannot notify existing desktop.")?;
            stream
                .write_all(b"focus\n")
                .map_err(|_| "Cannot notify existing desktop.")?;
            return Ok(None);
        }
        if socket.exists() {
            std::fs::remove_file(&socket).map_err(|_| "Cannot remove stale focus socket.")?;
        }
        let listener = UnixListener::bind(&socket).map_err(|_| "Cannot bind private focus socket.")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "Cannot configure focus socket.")?;
        Ok(Some(Self {
            _lock: lock,
            listener,
            socket,
        }))
    }

    pub fn requested_focus(&self) -> bool {
        if let Ok((mut connection, _)) = self.listener.accept() {
            let _ = connection.set_read_timeout(Some(std::time::Duration::from_millis(100)));
            let mut frame = [0u8; 6];
            return connection.read_exact(&mut frame).is_ok() && &frame == b"focus\n";
        }
        false
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_is_home_scoped_and_second_instance_only_requests_focus() {
        let dir = tempfile::tempdir().unwrap();
        let first = Instance::acquire(&dir.path().join("one")).unwrap().unwrap();
        assert!(Instance::acquire(&dir.path().join("one")).unwrap().is_none());
        assert!(first.requested_focus());
        let other = Instance::acquire(&dir.path().join("two")).unwrap().unwrap();
        assert!(!other.requested_focus());
        drop(first);
        assert!(Instance::acquire(&dir.path().join("one")).unwrap().is_some());
    }
}
