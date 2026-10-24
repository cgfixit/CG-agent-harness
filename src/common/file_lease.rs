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
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            use windows_sys::Win32::System::IO::OVERLAPPED;
            // SAFETY: zero initializes the byte-zero offset and unused event handle.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            // SAFETY: file is live, and this synchronous call borrows valid OVERLAPPED storage.
            if unsafe {
                LockFileEx(
                    file.as_raw_handle(),
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    1,
                    0,
                    &mut overlapped,
                )
            } == 0
            {
                let error = std::io::Error::last_os_error();
                return Err(if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                    std::io::Error::new(std::io::ErrorKind::WouldBlock, error)
                } else {
                    error
                });
            }
        }
        Ok(Self(file))
    }
}
impl Drop for FileLease {
    fn drop(&mut self) {
        #[cfg(not(any(unix, windows)))]
        let _ = &self.0;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: our descriptor remains live until Drop returns.
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
            use windows_sys::Win32::System::IO::OVERLAPPED;
            // SAFETY: zero initializes the byte-zero offset and unused event handle.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            // SAFETY: our handle remains live until Drop returns; release the acquired range.
            unsafe {
                UnlockFileEx(self.0.as_raw_handle(), 0, 1, 0, &mut overlapped);
            }
        }
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    #[test]
    fn busy_then_reacquire_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        let held = FileLease::acquire(&path, true).unwrap();
        assert_eq!(
            FileLease::acquire(&path, false).err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let probe = |busy: bool| {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "common::file_lease::tests::child_lease_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env("CGAH_LEASE_PROBE_PATH", &path)
                .env("CGAH_LEASE_PROBE_BUSY", if busy { "1" } else { "0" })
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            assert!(String::from_utf8_lossy(&result.stdout).contains("lease probe completed"));
        };
        probe(true);
        drop(held);
        probe(false);
        // The child exited; its lease must no longer exclude this process.
        let _reacquired = FileLease::acquire(&path, false).unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture for busy_then_reacquire_across_processes"]
    fn child_lease_probe() {
        let path = std::env::var_os("CGAH_LEASE_PROBE_PATH").unwrap();
        let result = FileLease::acquire(Path::new(&path), false);
        if std::env::var("CGAH_LEASE_PROBE_BUSY").unwrap() == "1" {
            assert_eq!(result.err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
        } else {
            // Leave release to process exit, independent of the Rust Drop path.
            std::mem::forget(result.unwrap());
        }
        println!("lease probe completed");
    }
}
