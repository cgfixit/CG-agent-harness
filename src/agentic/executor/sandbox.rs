//! Hard sandbox backends, port of `agentic/executor/hard_sandbox.py`.
//! A missing binary or failed capability probe raises `HARD_SANDBOX_UNAVAILABLE`
//! (exit 3): fail closed, no software fallback. `ArgvListSandbox` is the
//! unconstrained path and is never selected by `production_sandbox`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::errors::{HarnessError, Result};
use crate::common::process::{self, RunSpec};

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

/// Candidate and prepared inputs are read-only; only owned scratch is writable.
/// Metadata reads remain available for tool discovery; file data is allowlisted.
pub fn seatbelt_profile(cwd: &Path, tmpdir: Option<&Path>) -> String {
    seatbelt_profile_with_inputs(cwd, tmpdir, &[])
}

fn seatbelt_profile_with_inputs(cwd: &Path, tmpdir: Option<&Path>, read_roots: &[PathBuf]) -> String {
    let esc = |p: &Path| {
        dunce::canonicalize(p)
            .unwrap_or(p.to_path_buf())
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    };
    let mut roots = vec![cwd.to_path_buf()];
    // Operating-system executables, libraries, SDKs and device streams. No
    // general home, /private, /Library, or Homebrew prefix is exposed.
    for root in [
        "/System",
        "/usr/bin",
        "/usr/lib",
        "/usr/libexec",
        "/usr/share",
        "/bin",
        "/sbin",
        "/dev",
        "/Library/Developer/CommandLineTools",
        "/Applications/Xcode.app/Contents/Developer",
        "/private/var/db/dyld",
    ] {
        roots.push(PathBuf::from(root));
    }
    if let Some(tmp) = tmpdir {
        roots.push(tmp.to_path_buf());
    }
    roots.extend_from_slice(read_roots);
    let reads = roots
        .iter()
        .map(|p| format!("(subpath \"{}\")", esc(p)))
        .collect::<Vec<_>>()
        .join(" ");
    let writes = tmpdir
        .map(|p| format!("(deny file-write* (require-not (subpath \"{}\")))", esc(p)))
        .unwrap_or_else(|| "(deny file-write*)".into());
    format!("(version 1)\n(allow default)\n(deny network*)\n(deny file-read-data (require-not (require-any (literal \"/\") (literal \"/private/etc/ssl/openssl.cnf\") {reads})))\n{writes}\n")
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

/// Allowlisted Linux FS + net jail via bubblewrap. Never binds host root.
const LINUX_OS_RO_DIRS: &[&str] = &[
    "/usr", "/bin", "/lib", "/lib64", "/lib32", "/sbin", "/etc/ssl", "/etc/pki",
];
const LINUX_OS_RO_FILES: &[&str] = &[
    "/etc/passwd",
    "/etc/group",
    "/etc/nsswitch.conf",
    "/etc/ld.so.cache",
    "/etc/os-release",
    "/etc/localtime",
    "/etc/hosts",
];

fn canonical_existing(path: &Path) -> std::result::Result<PathBuf, String> {
    let resolved = dunce::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if resolved == Path::new("/") {
        return Err("refusing to mount host root".into());
    }
    Ok(resolved)
}

fn argv_binds_host_root(argv: &[String]) -> bool {
    argv.windows(3)
        .any(|w| matches!(w[0].as_str(), "--ro-bind" | "--bind" | "--ro-bind-try") && w[1] == "/" && w[2] == "/")
}

/// Single source of bwrap mount flags. Testable without executing bwrap.
pub fn bwrap_argv(
    bwrap: &Path,
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
) -> std::result::Result<Vec<String>, String> {
    let candidate = canonical_existing(cwd)?;
    if !candidate.is_dir() {
        return Err(format!("{} is not a directory", candidate.display()));
    }
    let scratch_dir = canonical_existing(scratch)?;
    if !scratch_dir.is_dir() {
        return Err(format!("{} is not a directory", scratch_dir.display()));
    }
    if scratch_dir == candidate {
        return Err("scratch must differ from candidate".into());
    }
    // Scratch is shared across a request's checks and is writable by sandboxed
    // code, so an earlier check may have planted this name. create_new (O_EXCL)
    // refuses any existing entry instead of following a symlink and truncating
    // its target on the host.
    let probe = scratch_dir.join(".cgah-bwrap-write");
    let mut probe_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("scratch probe {}: {e}", probe.display()))?;
    use std::io::Write;
    probe_file
        .write_all(b"ok")
        .map_err(|e| format!("scratch is not writable: {e}"))?;
    drop(probe_file);
    let _ = std::fs::remove_file(&probe);

    let mut out = vec![
        bwrap.display().to_string(),
        "--die-with-parent".into(),
        "--unshare-net".into(),
        "--unshare-pid".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ];
    if Path::new("/proc").exists() {
        out.extend(["--proc".into(), "/proc".into()]);
    }
    if Path::new("/dev").exists() {
        out.extend(["--dev".into(), "/dev".into()]);
    }
    for dir in LINUX_OS_RO_DIRS {
        if Path::new(dir).exists() {
            out.extend(["--ro-bind".into(), (*dir).into(), (*dir).into()]);
        }
    }
    for file in LINUX_OS_RO_FILES {
        if Path::new(file).exists() {
            out.extend(["--ro-bind".into(), (*file).into(), (*file).into()]);
        }
    }
    out.extend([
        "--ro-bind".into(),
        candidate.display().to_string(),
        candidate.display().to_string(),
    ]);
    for root in read_roots {
        let resolved = canonical_existing(root)?;
        out.extend([
            "--ro-bind".into(),
            resolved.display().to_string(),
            resolved.display().to_string(),
        ]);
    }
    out.extend([
        "--bind".into(),
        scratch_dir.display().to_string(),
        scratch_dir.display().to_string(),
        "--chdir".into(),
        candidate.display().to_string(),
        "--".into(),
    ]);
    out.extend(argv.iter().cloned());
    if argv_binds_host_root(&out) {
        return Err("bwrap argv would bind host root".into());
    }
    Ok(out)
}

pub struct LinuxBubblewrapSandbox {
    bwrap: PathBuf,
}

impl LinuxBubblewrapSandbox {
    pub fn new() -> Result<Self> {
        let path = process::which("bwrap")
            .ok_or_else(|| HarnessError::sandbox_unavailable("bwrap not found; Linux bubblewrap fails closed"))?;
        // `which` can return a relative path when PATH holds a relative entry,
        // and the sandboxed run later sets cwd to the untrusted worktree; pin
        // an absolute executable so a repo-local `tools/bwrap` cannot replace it.
        let path = dunce::canonicalize(&path)
            .map_err(|e| HarnessError::sandbox_unavailable(format!("bwrap path {}: {e}", path.display())))?;
        let tmp = tempfile::Builder::new()
            .prefix("cgah-bwrap-probe-")
            .tempdir()
            .map_err(|e| HarnessError::sandbox_unavailable(format!("bwrap probe tempdir: {e}")))?;
        let candidate = tmp.path().join("candidate");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir(&candidate)
            .map_err(|e| HarnessError::sandbox_unavailable(format!("bwrap probe candidate: {e}")))?;
        std::fs::create_dir(&scratch)
            .map_err(|e| HarnessError::sandbox_unavailable(format!("bwrap probe scratch: {e}")))?;
        let wrapped = bwrap_argv(&path, &["/bin/true".into()], &candidate, &scratch, &[])
            .map_err(|e| HarnessError::sandbox_unavailable(format!("bwrap argv: {e}")))?;
        match process::run(RunSpec {
            argv: &wrapped,
            cwd: None,
            env: None,
            timeout: Duration::from_secs(5),
            stdin: None,
        }) {
            Ok(out) if out.status == Some(0) => Ok(Self { bwrap: path }),
            Ok(out) => Err(HarnessError::sandbox_unavailable(format!(
                "bwrap probe failed (status {:?}): {}",
                out.status, out.stderr
            ))),
            Err(e) => Err(HarnessError::sandbox_unavailable(format!("bwrap probe: {e}"))),
        }
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
        Ok(sb) => Ok(Box::new(sb)),
        Err(bwrap_err) => match LinuxNetnsSandbox::new() {
            Ok(sb) => {
                tracing::warn!(
                    bwrap = bwrap_err.message.as_str(),
                    "linux-bwrap unavailable; falling back to linux-netns (network isolation only, host filesystem visible)"
                );
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
        let candidate = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(target.path(), scratch.path().join(".cgah-bwrap-write")).unwrap();
        let err = bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            candidate.path(),
            scratch.path(),
            &[],
        )
        .unwrap_err();
        assert!(err.contains("scratch probe"), "{err}");
        assert_eq!(std::fs::read(target.path()).unwrap(), b"");
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
