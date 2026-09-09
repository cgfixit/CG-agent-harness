//! Synchronous argv-list subprocess runner with a hard timeout.
//!
//! Every subprocess in this crate is an argv list (never a shell string).
//! Unix capture uses nonblocking pipes under one deadline and a fixed byte ceiling.
//! The shim shares that runner with cooperative cancellation. macOS additionally
//! stops observed descendants across process groups; ancestry cleanup is best-effort.
//! Other platforms retain their existing runner and do not inherit these guarantees.

use std::collections::BTreeMap;
#[cfg(not(unix))]
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
#[cfg(not(unix))]
use std::time::Instant;

#[cfg(unix)]
mod unix;
/// Internal safety ceiling shared by machine-readable subprocess consumers.
pub const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("command timed out after {timeout_sec}s")]
    Timeout { timeout_sec: f64 },
    #[error("cannot spawn command: {0}")]
    Spawn(std::io::Error),
    #[error("subprocess capture failed: {0}")]
    Capture(String),
}

pub struct RunSpec<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a Path>,
    /// When `Some`, the child's environment is exactly this map (nothing inherited).
    pub env: Option<&'a BTreeMap<String, String>>,
    pub timeout: Duration,
    pub stdin: Option<&'a [u8]>,
}

fn build_command(spec: &RunSpec<'_>) -> Command {
    let mut cmd = Command::new(&spec.argv[0]);
    cmd.args(&spec.argv[1..]);
    if let Some(cwd) = spec.cwd {
        cmd.current_dir(cwd);
    }
    if let Some(env) = spec.env {
        cmd.env_clear();
        cmd.envs(env.iter());
    }
    cmd.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Run to completion or timeout. A timed-out child is SIGKILLed (whole
/// process group on unix) and reported as `Err(Timeout)`.
#[cfg(unix)]
pub fn run(spec: RunSpec<'_>) -> Result<Output, ProcessError> {
    run_cancellable(spec, &std::sync::atomic::AtomicBool::new(false))
}

#[cfg(unix)]
pub fn run_cancellable(spec: RunSpec<'_>, cancelled: &std::sync::atomic::AtomicBool) -> Result<Output, ProcessError> {
    assert!(!spec.argv.is_empty(), "argv must not be empty");
    unix::run(spec, cancelled)
}

#[cfg(not(unix))]
pub fn run(spec: RunSpec<'_>) -> Result<Output, ProcessError> {
    assert!(!spec.argv.is_empty(), "argv must not be empty");
    let mut cmd = build_command(&spec);
    let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;
    if let (Some(data), Some(mut stdin)) = (spec.stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = stdin.write_all(data);
        drop(stdin);
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || read_all(stdout));
    let err_thread = std::thread::spawn(move || read_all(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= spec.timeout {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => {
                kill_tree(&mut child);
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    match status {
        Some(s) => Ok(Output {
            status: s.code(),
            stdout,
            stderr,
            timed_out: false,
        }),
        None => Err(ProcessError::Timeout {
            timeout_sec: spec.timeout.as_secs_f64(),
        }),
    }
}

#[cfg(not(unix))]
fn read_all<R: Read>(reader: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut r) = reader {
        let _ = r.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// SIGKILL the child's whole process group (unix) or the child (elsewhere).
pub fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        // SAFETY: killpg on a pid we spawned into its own group.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// True when a process with this pid is alive (used for stale-lock reclaim).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 performs error checking only.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        err.raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: querying a handle we immediately close.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            CloseHandle(h);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

/// Locate an executable on PATH (like `shutil.which`).
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let candidates: Vec<String> = if cfg!(windows) {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
        let mut v = vec![name.to_string()];
        for ext in exts.split(';') {
            if !ext.is_empty() {
                v.push(format!("{name}{}", ext.to_lowercase()));
                v.push(format!("{name}{ext}"));
            }
        }
        v
    } else {
        vec![name.to_string()]
    };
    for dir in std::env::split_paths(&path_var) {
        for cand in &candidates {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
