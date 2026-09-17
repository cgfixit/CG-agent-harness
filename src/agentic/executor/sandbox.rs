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
    prefer_linux_sandbox_from(LinuxBubblewrapSandbox::new(), LinuxNetnsSandbox::new())
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

#[cfg(windows)]
impl HardSandbox for WindowsJobObjectSandbox {
    fn run(&self, argv: &[String], cwd: &Path, env: &BTreeMap<String, String>, timeout_sec: u64) -> SandboxOutcome {
        use std::io::Read;
        use std::os::windows::io::AsRawHandle;
        use std::process::{Command, Stdio};
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 job-object calls with a zeroed struct.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return SandboxOutcome {
                exit_code: -2,
                stdout: String::new(),
                stderr: "CreateJobObjectW failed".into(),
                timed_out: false,
            };
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = 32;
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            unsafe { CloseHandle(job) };
            return SandboxOutcome {
                exit_code: -2,
                stdout: String::new(),
                stderr: "SetInformationJobObject failed".into(),
                timed_out: false,
            };
        }
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                unsafe { CloseHandle(job) };
                return SandboxOutcome {
                    exit_code: -2,
                    stdout: String::new(),
                    stderr: format!("could not execute '{}': {e}", argv[0]),
                    timed_out: false,
                };
            }
        };
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) };
        if assigned == 0 {
            let _ = child.kill();
            let _ = child.wait();
            unsafe { CloseHandle(job) };
            return SandboxOutcome {
                exit_code: -2,
                stdout: String::new(),
                stderr: "AssignProcessToJobObject failed; refusing unconstrained run".into(),
                timed_out: false,
            };
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut s) = stdout {
                let _ = s.read_to_end(&mut b);
            }
            String::from_utf8_lossy(&b).into_owned()
        });
        let err_t = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_end(&mut b);
            }
            String::from_utf8_lossy(&b).into_owned()
        });
        let started = std::time::Instant::now();
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {
                    if started.elapsed() >= Duration::from_secs(timeout_sec) {
                        unsafe { TerminateJobObject(job, 1) };
                        let _ = child.wait();
                        timed_out = true;
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(_) => {
                    unsafe { TerminateJobObject(job, 1) };
                    let _ = child.wait();
                    break None;
                }
            }
        };
        unsafe { CloseHandle(job) };
        let so = out_t.join().unwrap_or_default();
        let se = err_t.join().unwrap_or_default();
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
