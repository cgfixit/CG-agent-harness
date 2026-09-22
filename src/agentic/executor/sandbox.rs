//! Hard sandbox backends, port of `agentic/executor/hard_sandbox.py`.
//! A missing binary or failed capability probe raises `HARD_SANDBOX_UNAVAILABLE`
//! (exit 3): fail closed, no software fallback. `ArgvListSandbox` is the
//! unconstrained path and is never selected by `production_sandbox`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::errors::{HarnessError, Result};
use crate::common::process::{self, RunSpec};
use crate::common::sandbox_wrap::seatbelt_profile_with_inputs;

pub use crate::common::sandbox_wrap::{argv_binds_host_root, bwrap_argv, exclusive_scratch_probe, seatbelt_profile};

pub const MAX_OUTPUT_CHARS: usize = 20_000;

#[derive(Debug, Clone)]
pub struct SandboxOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

pub trait HardSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome;
    fn run_prepared(
        &self,
        argv: &[String],
        cwd: &Path,
        env: &BTreeMap<String, String>,
        timeout_sec: u64,
        _scratch: &Path,
        _read_roots: &[PathBuf],
    ) -> SandboxOutcome {
        self.run(argv, cwd, env, timeout_sec)
    }
    fn name(&self) -> &'static str;
}

pub fn truncate_output(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.to_string();
    }
    format!(
        "{}\n... [output truncated at {MAX_OUTPUT_CHARS} chars]",
        crate::common::clip_chars(text, MAX_OUTPUT_CHARS)
    )
}

/// Unconstrained argv-list runner (tests only; process-group kill on timeout).
pub struct ArgvListSandbox;

impl HardSandbox for ArgvListSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        match process::run(RunSpec {
            argv,
            cwd: Some(cwd),
            env: Some(env),
            timeout: Duration::from_secs(timeout_sec),
            stdin: None,
        }) {
            Ok(out) => SandboxOutcome {
                exit_code: out.status.unwrap_or(-1),
                stdout: truncate_output(&out.stdout),
                stderr: truncate_output(&out.stderr),
                timed_out: false,
            },
            Err(process::ProcessError::Timeout { .. }) => SandboxOutcome {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("timed out after {timeout_sec}s"),
                timed_out: true,
            },
            Err(process::ProcessError::Capture(e)) => SandboxOutcome {
                exit_code: -3,
                stdout: String::new(),
                stderr: e,
                timed_out: false,
            },
            Err(process::ProcessError::Spawn(e)) => SandboxOutcome {
                exit_code: -2,
                stdout: String::new(),
                stderr: format!("could not execute '{}': {e}", argv[0]),
                timed_out: false,
            },
        }
    }

    fn name(&self) -> &'static str {
        "argv-list"
    }
}

pub struct DarwinSeatbeltSandbox {
    sandbox_exec: PathBuf,
}

impl DarwinSeatbeltSandbox {
    pub fn new() -> Result<Self> {
        let path = process::which("sandbox-exec")
            .ok_or_else(|| HarnessError::sandbox_unavailable("sandbox-exec not found; Darwin Seatbelt fails closed"))?;
        Ok(Self { sandbox_exec: path })
    }
}

impl HardSandbox for DarwinSeatbeltSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        let tmp = match tempfile::Builder::new().prefix("cgah-seatbelt-").tempdir() {
            Ok(t) => t,
            Err(e) => {
                return SandboxOutcome {
                    exit_code: -2,
                    stdout: String::new(),
                    stderr: format!("could not create sandbox temp dir: {e}"),
                    timed_out: false,
                }
            }
        };
        let mut env_with_tmp = env.clone();
        for k in ["HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP"] {
            env_with_tmp.insert(k.into(), tmp.path().display().to_string());
        }
        self.run_prepared(argv, cwd, &env_with_tmp, timeout_sec, tmp.path(), &[])
    }

    fn run_prepared(
        &self,
        argv: &[String],
        cwd: &Path,
        env: &BTreeMap<String, String>,
        timeout_sec: u64,
        scratch: &Path,
        read_roots: &[PathBuf],
    ) -> SandboxOutcome {
        let mut wrapped = vec![
            self.sandbox_exec.display().to_string(),
            "-p".into(),
            seatbelt_profile_with_inputs(cwd, Some(scratch), read_roots),
            "--".into(),
        ];
        wrapped.extend(argv.iter().cloned());
        ArgvListSandbox.run(&wrapped, cwd, env, timeout_sec)
    }

    fn name(&self) -> &'static str {
        "darwin-seatbelt"
    }
}

pub struct LinuxNetnsSandbox {
    unshare: PathBuf,
    user_ns: bool,
}

impl LinuxNetnsSandbox {
    pub fn new() -> Result<Self> {
        let path = process::which("unshare")
            .ok_or_else(|| HarnessError::sandbox_unavailable("unshare not found; Linux netns fails closed"))?;
        let probe = |extra: &[&str]| -> bool {
            let mut argv = vec![path.display().to_string()];
            argv.extend(extra.iter().map(|s| s.to_string()));
            argv.push("/bin/true".into());
            matches!(
                process::run(RunSpec { argv: &argv, cwd: None, env: None, timeout: Duration::from_secs(5), stdin: None }),
                Ok(out) if out.status == Some(0)
            )
        };
        if probe(&["--net"]) {
            return Ok(Self {
                unshare: path,
                user_ns: false,
            });
        }
        if probe(&["--user", "--map-root-user", "--net"]) {
            return Ok(Self {
                unshare: path,
                user_ns: true,
            });
        }
        Err(HarnessError::sandbox_unavailable(
            "unshare --net probe failed (EPERM or unsupported); Linux netns fails closed",
        ))
    }
}

impl HardSandbox for LinuxNetnsSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        let mut wrapped = vec![self.unshare.display().to_string()];
        if self.user_ns {
            wrapped.push("--user".into());
            wrapped.push("--map-root-user".into());
        }
        wrapped.push("--net".into());
        wrapped.push("--".into());
        wrapped.extend(argv.iter().cloned());
        ArgvListSandbox.run(&wrapped, cwd, env, timeout_sec)
    }

    fn run_prepared(
        &self,
        argv: &[String],
        cwd: &Path,
        env: &BTreeMap<String, String>,
        timeout_sec: u64,
        scratch: &Path,
        read_roots: &[PathBuf],
    ) -> SandboxOutcome {
        // netns cannot honor FS mounts; the gap is the linux-netns name.
        let _ = (scratch, read_roots);
        self.run(argv, cwd, env, timeout_sec)
    }

    fn name(&self) -> &'static str {
        "linux-netns"
    }
}

pub struct LinuxBubblewrapSandbox {
    bwrap: PathBuf,
}

impl LinuxBubblewrapSandbox {
    pub fn new() -> Result<Self> {
        let path = crate::common::sandbox_wrap::probe_linux_bwrap().map_err(HarnessError::sandbox_unavailable)?;
        Ok(Self { bwrap: path })
    }
}

impl HardSandbox for LinuxBubblewrapSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        let tmp = match tempfile::Builder::new().prefix("cgah-bwrap-").tempdir() {
            Ok(t) => t,
            Err(e) => {
                return SandboxOutcome {
                    exit_code: -2,
                    stdout: String::new(),
                    stderr: format!("could not create sandbox temp dir: {e}"),
                    timed_out: false,
                }
            }
        };
        let mut env_with_tmp = env.clone();
        for k in ["HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP"] {
            env_with_tmp.insert(k.into(), tmp.path().display().to_string());
        }
        self.run_prepared(argv, cwd, &env_with_tmp, timeout_sec, tmp.path(), &[])
    }

    fn run_prepared(
        &self,
        argv: &[String],
        cwd: &Path,
        env: &BTreeMap<String, String>,
        timeout_sec: u64,
        scratch: &Path,
        read_roots: &[PathBuf],
    ) -> SandboxOutcome {
        let wrapped = match bwrap_argv(&self.bwrap, argv, cwd, scratch, read_roots) {
            Ok(v) => v,
            Err(e) => {
                return SandboxOutcome {
                    exit_code: -2,
                    stdout: String::new(),
                    stderr: e,
                    timed_out: false,
                }
            }
        };
        ArgvListSandbox.run(&wrapped, cwd, env, timeout_sec)
    }

    fn name(&self) -> &'static str {
        "linux-bwrap"
    }
}

fn prefer_linux_sandbox() -> Result<Box<dyn HardSandbox>> {
    match LinuxBubblewrapSandbox::new() {
        Ok(sb) => prefer_linux_sandbox_from(Ok(sb), Err(HarnessError::sandbox_unavailable("unused"))),
        Err(bwrap_err) => prefer_linux_sandbox_from(Err(bwrap_err), LinuxNetnsSandbox::new()),
    }
}

fn prefer_linux_sandbox_from(
    bwrap: Result<LinuxBubblewrapSandbox>,
    netns: Result<LinuxNetnsSandbox>,
) -> Result<Box<dyn HardSandbox>> {
    match bwrap {
        Ok(sb) => {
            tracing::info!(sandbox = "linux-bwrap", "linux hard sandbox backend selected");
            Ok(Box::new(sb))
        }
        Err(bwrap_err) => match netns {
            Ok(sb) => {
                tracing::warn!(
                    bwrap = bwrap_err.message.as_str(),
                    "linux-bwrap unavailable; falling back to linux-netns (network isolation only, host filesystem visible)"
                );
                tracing::info!(sandbox = "linux-netns", "linux hard sandbox backend selected");
                Ok(Box::new(sb))
            }
            Err(netns_err) => Err(HarnessError::sandbox_unavailable(format!(
                "linux hard sandbox unavailable: bwrap: {}; netns: {}",
                bwrap_err.message, netns_err.message
            ))),
        },
    }
}

/// Windows Job Object with KILL_ON_JOB_CLOSE: a process-tree kill boundary.
pub struct WindowsJobObjectSandbox;

/// `JobChild` requires an absolute native executable. A bare name resolves
/// through the check's own `PATH` (else the process `PATH`), as `Command` did.
#[cfg(windows)]
fn resolve_windows_exe(program: &str, env: &BTreeMap<String, String>) -> Option<std::path::PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if path.components().count() != 1 {
        return None;
    }
    let name = if path.extension().is_some() {
        program.to_string()
    } else {
        format!("{program}.exe")
    };
    let search = env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var("PATH").ok())?;
    std::env::split_paths(&search)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

#[cfg(windows)]
impl HardSandbox for WindowsJobObjectSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        use std::io::Read;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        use crate::common::process::MAX_CAPTURE_BYTES;
        use crate::common::windows_job::JobChild;

        let refused = |stderr: String| SandboxOutcome {
            exit_code: -2,
            stdout: String::new(),
            stderr,
            timed_out: false,
        };
        let Some(program) = resolve_windows_exe(&argv[0], env) else {
            return refused(format!("could not execute '{}': no native .exe found", argv[0]));
        };
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&argv[1..]).current_dir(cwd).env_clear().envs(env.iter());
        // The kernel places the child in the kill-on-close job during
        // CreateProcess (JOB_LIST), before any of its code runs. Assigning after
        // spawn left a window in which its descendants escaped the job.
        let mut child = match JobChild::spawn(&cmd, 32, None) {
            Ok(child) => child,
            Err(e) => {
                return refused(format!(
                    "could not execute '{}' inside a Job Object; refusing unconstrained run: {e}",
                    argv[0]
                ))
            }
        };
        drop(child.stdin.take());
        // One combined capture bound, as on unix: overflow is refused, not truncated.
        let total = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicBool::new(false));
        let reader = |pipe: Option<std::fs::File>| {
            let (total, overflow) = (total.clone(), overflow.clone());
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                let Some(mut pipe) = pipe else { return kept };
                let mut buffer = [0u8; 8192];
                loop {
                    let n = match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    if total.fetch_add(n, Ordering::SeqCst) + n > MAX_CAPTURE_BYTES {
                        overflow.store(true, Ordering::SeqCst);
                        break;
                    }
                    kept.extend_from_slice(&buffer[..n]);
                }
                kept
            })
        };
        let out_t = reader(child.stdout.take());
        let err_t = reader(child.stderr.take());
        let started = std::time::Instant::now();
        let mut timed_out = false;
        let status = loop {
            if overflow.load(Ordering::SeqCst) {
                let _ = child.start_kill();
                break None;
            }
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() >= Duration::from_secs(timeout_sec) => {
                    let _ = child.start_kill();
                    timed_out = true;
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
                Err(_) => {
                    let _ = child.start_kill();
                    break None;
                }
            }
        };
        // Dropping the child terminates and closes the job: any descendant still
        // running is killed, which also closes the pipes the readers wait on.
        drop(child);
        let so = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
        let se = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
        if overflow.load(Ordering::SeqCst) {
            return SandboxOutcome {
                exit_code: -3,
                stdout: String::new(),
                stderr: format!("output exceeded {MAX_CAPTURE_BYTES} bytes; incomplete output refused"),
                timed_out: false,
            };
        }
        if timed_out {
            return SandboxOutcome {
                exit_code: -1,
                stdout: truncate_output(&so),
                stderr: format!("timed out after {timeout_sec}s"),
                timed_out: true,
            };
        }
        SandboxOutcome {
            exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
            stdout: truncate_output(&so),
            stderr: truncate_output(&se),
            timed_out: false,
        }
    }

    fn name(&self) -> &'static str {
        "windows-job-object"
    }
}

#[cfg(not(windows))]
impl HardSandbox for WindowsJobObjectSandbox {
    fn run(&self, _argv: &[String], _cwd: &Path, _env: &BTreeMap<String, String>, _timeout_sec: u64) -> SandboxOutcome {
        SandboxOutcome {
            exit_code: -2,
            stdout: String::new(),
            stderr: "job objects are Windows-only".into(),
            timed_out: false,
        }
    }

    fn name(&self) -> &'static str {
        "windows-job-object"
    }
}

/// The host backend, or `HARD_SANDBOX_UNAVAILABLE`. Never `ArgvListSandbox`.
pub fn production_sandbox() -> Result<Box<dyn HardSandbox>> {
    if cfg!(windows) {
        return Ok(Box::new(WindowsJobObjectSandbox));
    }
    if cfg!(target_os = "macos") {
        return Ok(Box::new(DarwinSeatbeltSandbox::new()?));
    }
    if cfg!(target_os = "linux") {
        return prefer_linux_sandbox();
    }
    Err(HarnessError::sandbox_unavailable(format!(
        "no hard-sandbox backend for platform {}; agentic verification fails closed",
        std::env::consts::OS
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_argv(cwd: &Path, scratch: &Path, read_roots: &[PathBuf]) -> Vec<String> {
        bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            cwd,
            scratch,
            read_roots,
        )
        .expect("bwrap argv")
    }

    #[test]
    fn bwrap_argv_never_binds_host_root() {
        let candidate = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let argv = sample_argv(candidate.path(), scratch.path(), &[]);
        assert!(!argv_binds_host_root(&argv));
        assert!(argv.contains(&"--unshare-net".into()));
        assert!(argv.contains(&"--tmpfs".into()));
        assert!(argv.contains(&"--die-with-parent".into()));
        let cand = dunce::canonicalize(candidate.path()).unwrap();
        let scratch_p = dunce::canonicalize(scratch.path()).unwrap();
        let ro = argv.windows(3).any(|w| w[0] == "--ro-bind" && Path::new(&w[1]) == cand);
        let rw = argv
            .windows(3)
            .any(|w| w[0] == "--bind" && Path::new(&w[1]) == scratch_p);
        assert!(ro, "{argv:?}");
        assert!(rw, "{argv:?}");
    }

    #[cfg(unix)]
    #[test]
    fn bwrap_probe_refuses_planted_symlink() {
        let target = tempfile::NamedTempFile::new().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let planted = scratch.path().join(".cgah-bwrap-write-planted");
        std::os::unix::fs::symlink(target.path(), &planted).unwrap();
        let err = exclusive_scratch_probe(&planted).unwrap_err();
        assert!(err.contains("scratch probe"), "{err}");
        assert_eq!(std::fs::read(target.path()).unwrap(), b"");
    }

    #[test]
    fn bwrap_probe_unique_names_allow_parallel_checks_on_one_scratch() {
        let candidate = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let first = bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            candidate.path(),
            scratch.path(),
            &[],
        );
        let second = bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            candidate.path(),
            scratch.path(),
            &[],
        );
        first.expect("first probe");
        second.expect("second probe on the same scratch");
    }

    #[test]
    fn prefer_linux_sandbox_falls_back_then_fails_closed() {
        let bwrap = LinuxBubblewrapSandbox {
            bwrap: PathBuf::from("/bin/true"),
        };
        let netns = LinuxNetnsSandbox {
            unshare: PathBuf::from("/bin/true"),
            user_ns: false,
        };
        match prefer_linux_sandbox_from(Ok(bwrap), Err(HarnessError::sandbox_unavailable("netns unused"))) {
            Ok(sb) => assert_eq!(sb.name(), "linux-bwrap"),
            Err(e) => panic!("bwrap should win: {e}"),
        }

        match prefer_linux_sandbox_from(Err(HarnessError::sandbox_unavailable("bwrap missing")), Ok(netns)) {
            Ok(sb) => assert_eq!(sb.name(), "linux-netns"),
            Err(e) => panic!("netns should be the fallback: {e}"),
        }

        let both = match prefer_linux_sandbox_from(
            Err(HarnessError::sandbox_unavailable("bwrap missing")),
            Err(HarnessError::sandbox_unavailable("unshare missing")),
        ) {
            Ok(_) => panic!("both missing must fail closed"),
            Err(e) => e,
        };
        assert_eq!(both.code, "HARD_SANDBOX_UNAVAILABLE");
        assert_eq!(crate::agentic::cli::exit_code_for(&both), crate::agentic::cli::EXIT_ENV);
    }

    #[test]
    fn bwrap_argv_refuses_host_root_read_root() {
        let candidate = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let err = bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            candidate.path(),
            scratch.path(),
            &[PathBuf::from("/")],
        )
        .unwrap_err();
        assert!(err.contains("host root"), "{err}");
    }
}
