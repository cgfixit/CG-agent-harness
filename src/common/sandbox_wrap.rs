//! Argv wrapping for Darwin Seatbelt and Linux bubblewrap.
//!
//! Shared by agentic verification and MCP stdio so `src/server` never imports
//! `crate::agentic`. Never binds host root.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use super::errors::{HarnessError, Result};
use super::mcp_policy::{Containment, NetworkPolicy, StdioCapabilities};
use super::process::{self, RunSpec};

fn wrap_err(message: impl Into<String>) -> HarnessError {
    HarnessError::new("MCP_STDIO", message)
}

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
const MACOS_OS_RO_DIRS: &[&str] = &[
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
];

pub struct WrappedStdio {
    pub argv: Vec<String>,
    pub backend: &'static str,
    pub probe_reason: String,
    pub child_cwd: PathBuf,
    pub scratch: PathBuf,
    _scratch: tempfile::TempDir,
    _candidate: Option<tempfile::TempDir>,
}

pub fn classify_probe_err(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("rtm_newaddr") {
        "rtm_newaddr"
    } else if lower.contains("not found") || lower.contains("no such file") {
        "missing_binary"
    } else if lower.contains("eperm") || lower.contains("operation not permitted") {
        "eperm"
    } else {
        "probe_failed"
    }
}

pub fn refuse_home_overlap(path: &Path, home: &Path) -> Result<()> {
    let resolved_path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved_home = dunce::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if resolved_path == resolved_home
        || resolved_path.starts_with(&resolved_home)
        || resolved_home.starts_with(&resolved_path)
    {
        return Err(HarnessError::new(
            "MCP_HOME_REFUSED",
            format!(
                "mcp stdio path {} overlaps harness home {}",
                resolved_path.display(),
                resolved_home.display()
            ),
        ));
    }
    Ok(())
}

fn canonical_bin(name: &str) -> Result<PathBuf> {
    let found = process::which(name)
        .ok_or_else(|| HarnessError::sandbox_unavailable(format!("{name} not found; MCP stdio fails closed")))?;
    dunce::canonicalize(&found)
        .map_err(|e| HarnessError::sandbox_unavailable(format!("{name} path {}: {e}", found.display())))
}

/// Candidate and prepared inputs are read-only; only owned scratch is writable.
pub fn seatbelt_profile(cwd: &Path, tmpdir: Option<&Path>) -> String {
    seatbelt_profile_with_inputs(cwd, tmpdir, &[])
}

pub fn seatbelt_profile_with_inputs(cwd: &Path, tmpdir: Option<&Path>, read_roots: &[PathBuf]) -> String {
    seatbelt_profile_with_grants(cwd, tmpdir, read_roots, &[], NetworkPolicy::Deny)
}

fn seatbelt_profile_with_grants(
    cwd: &Path,
    tmpdir: Option<&Path>,
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    network: NetworkPolicy,
) -> String {
    let esc = |p: &Path| {
        dunce::canonicalize(p)
            .unwrap_or(p.to_path_buf())
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    };
    let mut roots = vec![cwd.to_path_buf()];
    for root in MACOS_OS_RO_DIRS {
        roots.push(PathBuf::from(root));
    }
    if let Some(tmp) = tmpdir {
        roots.push(tmp.to_path_buf());
    }
    roots.extend_from_slice(read_roots);
    roots.extend_from_slice(write_roots);
    let reads = roots
        .iter()
        .map(|p| format!("(subpath \"{}\")", esc(p)))
        .collect::<Vec<_>>()
        .join(" ");
    let writable = tmpdir
        .into_iter()
        .chain(write_roots.iter().map(PathBuf::as_path))
        .map(|p| format!("(subpath \"{}\")", esc(p)))
        .collect::<Vec<_>>();
    let writes = if writable.is_empty() {
        "(deny file-write*)".into()
    } else if writable.len() == 1 {
        format!("(deny file-write* (require-not {}))", writable[0])
    } else {
        format!("(deny file-write* (require-not (require-any {})))", writable.join(" "))
    };
    let network = if network == NetworkPolicy::Deny {
        "(deny network*)\n"
    } else {
        ""
    };
    format!("(version 1)\n(allow default)\n{network}(deny file-read-data (require-not (require-any (literal \"/\") (literal \"/private/etc/ssl/openssl.cnf\") {reads})))\n{writes}\n")
}

pub fn canonical_existing(path: &Path) -> std::result::Result<PathBuf, String> {
    let resolved = dunce::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if resolved == Path::new("/") {
        return Err("refusing to mount host root".into());
    }
    Ok(resolved)
}

pub fn argv_binds_host_root(argv: &[String]) -> bool {
    argv.windows(3)
        .any(|w| matches!(w[0].as_str(), "--ro-bind" | "--bind" | "--ro-bind-try") && w[1] == "/" && w[2] == "/")
}

pub fn bwrap_argv(
    bwrap: &Path,
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
) -> std::result::Result<Vec<String>, String> {
    bwrap_argv_inner(bwrap, argv, cwd, scratch, read_roots, &[], true)
}

fn bwrap_argv_inner(
    bwrap: &Path,
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    unshare_net: bool,
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
    exclusive_scratch_probe(&unique_scratch_probe(&scratch_dir))?;

    let mut out = vec![bwrap.display().to_string(), "--die-with-parent".into()];
    if unshare_net {
        out.push("--unshare-net".into());
    }
    out.extend(["--unshare-pid".into(), "--tmpfs".into(), "/tmp".into()]);
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
    for root in write_roots {
        let resolved = canonical_existing(root)?;
        out.extend([
            "--bind".into(),
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

fn unique_scratch_probe(scratch_dir: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    scratch_dir.join(format!(".cgah-bwrap-write-{}-{n}", std::process::id()))
}

pub fn exclusive_scratch_probe(probe: &Path) -> std::result::Result<(), String> {
    let mut probe_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(probe)
        .map_err(|e| format!("scratch probe {}: {e}", probe.display()))?;
    use std::io::Write;
    probe_file
        .write_all(b"ok")
        .map_err(|e| format!("scratch is not writable: {e}"))?;
    drop(probe_file);
    let _ = std::fs::remove_file(probe);
    Ok(())
}

pub fn wrap_mcp_stdio(
    argv: &[String],
    cwd: Option<&Path>,
    home: Option<&Path>,
    capabilities: &StdioCapabilities,
) -> Result<WrappedStdio> {
    let (mut read_roots, write_roots) = capabilities.resolve_roots(home)?;
    if capabilities.containment == Containment::JobObject && !cfg!(windows) {
        return Err(HarnessError::new(
            "MCP_CONTAINMENT_UNAVAILABLE",
            "job_object is a Windows-only trusted-server exception",
        ));
    }
    if let Some(home) = home {
        let runtime_roots: Vec<&str> = if cfg!(target_os = "macos") {
            MACOS_OS_RO_DIRS.to_vec()
        } else if cfg!(target_os = "linux") {
            LINUX_OS_RO_DIRS.iter().chain(LINUX_OS_RO_FILES).copied().collect()
        } else {
            vec![]
        };
        for root in runtime_roots {
            refuse_home_overlap(Path::new(root), home)?;
        }
    }
    if capabilities.containment == Containment::Strict && !cfg!(target_os = "linux") {
        return Err(HarnessError::new(
            "MCP_CONTAINMENT_UNAVAILABLE",
            "Strict MCP containment requires the Linux service supervisor; this platform refuses strict stdio",
        ));
    }
    let scratch = tempfile::Builder::new()
        .prefix("cgah-mcp-scratch-")
        .tempdir()
        .map_err(|e| wrap_err(format!("sandbox scratch: {e}")))?;
    let (child_cwd, candidate) = match cwd {
        Some(path) => (
            canonical_existing(path).map_err(|_| wrap_err("MCP cwd is unavailable"))?,
            None,
        ),
        None => {
            let dir = tempfile::Builder::new()
                .prefix("cgah-mcp-cwd-")
                .tempdir()
                .map_err(|e| wrap_err(format!("sandbox cwd: {e}")))?;
            let path = dir.path().to_path_buf();
            (path, Some(dir))
        }
    };
    capabilities.validate_resolved_root(&child_cwd, home)?;
    if let Some(home) = home {
        for item in argv {
            let path = Path::new(item);
            if path.is_absolute() {
                refuse_home_overlap(path, home)?;
            }
        }
    }
    if !child_cwd.is_dir() {
        return Err(wrap_err("MCP cwd must be a directory"));
    }
    let program = argv.first().ok_or_else(|| wrap_err("MCP command is empty"))?;
    let program = canonical_existing(Path::new(program)).map_err(|_| wrap_err("MCP executable is unavailable"))?;
    capabilities.validate_resolved_root(&program, home)?;
    read_roots.push(program.clone());
    let mut command = argv.to_vec();
    command[0] = program.to_string_lossy().into_owned();
    let (wrapped, backend, probe_reason) = if capabilities.containment == Containment::JobObject {
        (
            command,
            "windows-job-object-unrestricted",
            "Explicit trusted-server exception: unrestricted filesystem and network; Job Object process and memory bounds only".into(),
        )
    } else {
        wrap_argv(
            &command,
            &child_cwd,
            scratch.path(),
            &read_roots,
            &write_roots,
            capabilities.network,
        )?
    };
    Ok(WrappedStdio {
        argv: wrapped,
        backend,
        probe_reason,
        child_cwd,
        scratch: scratch.path().to_path_buf(),
        _scratch: scratch,
        _candidate: candidate,
    })
}

/// Same probe `LinuxBubblewrapSandbox::new` uses. Presence of `bwrap` is not enough:
/// GitHub Actions often fails `bwrap` with `Failed RTM_NEWADDR`. Cached per process.
/// Agentic verification and network-denied MCP both require the full backend.
pub fn probe_linux_bwrap() -> std::result::Result<PathBuf, String> {
    static CACHED: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
    CACHED.get_or_init(|| probe_linux_bwrap_kind(true)).clone()
}

fn probe_linux_bwrap_kind(unshare_net: bool) -> std::result::Result<PathBuf, String> {
    let path = process::which("bwrap").ok_or_else(|| "bwrap not found".to_string())?;
    let path = dunce::canonicalize(&path).map_err(|e| format!("bwrap path {}: {e}", path.display()))?;
    let tmp = tempfile::Builder::new()
        .prefix("cgah-bwrap-probe-")
        .tempdir()
        .map_err(|e| format!("bwrap probe tempdir: {e}"))?;
    let candidate = tmp.path().join("candidate");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&candidate).map_err(|e| format!("bwrap probe candidate: {e}"))?;
    std::fs::create_dir(&scratch).map_err(|e| format!("bwrap probe scratch: {e}"))?;
    let wrapped = bwrap_argv_inner(
        &path,
        &["/bin/true".into()],
        &candidate,
        &scratch,
        &[],
        &[],
        unshare_net,
    )?;
    match process::run(RunSpec {
        argv: &wrapped,
        cwd: None,
        env: None,
        timeout: Duration::from_secs(5),
        stdin: None,
    }) {
        Ok(out) if out.status == Some(0) => Ok(path),
        Ok(out) => Err(format!("bwrap probe failed (status {:?}): {}", out.status, out.stderr)),
        Err(e) => Err(format!("bwrap probe: {e}")),
    }
}

fn wrap_argv(
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    network: NetworkPolicy,
) -> Result<(Vec<String>, &'static str, String)> {
    if cfg!(target_os = "macos") {
        let sandbox_exec = canonical_bin("sandbox-exec")?;
        let mut out = vec![
            sandbox_exec.display().to_string(),
            "-p".into(),
            seatbelt_profile_with_grants(cwd, Some(scratch), read_roots, write_roots, network),
            "--".into(),
        ];
        out.extend(argv.iter().cloned());
        return Ok((out, "darwin-seatbelt", "sandbox-exec".into()));
    }
    if cfg!(target_os = "linux") {
        let deny_network = network == NetworkPolicy::Deny;
        let path = if deny_network {
            probe_linux_bwrap()
        } else {
            probe_linux_bwrap_kind(false)
        }
        .map_err(|e| {
            HarnessError::sandbox_unavailable(format!("MCP capabilities unavailable: {}", classify_probe_err(&e)))
        })?;
        let wrapped = bwrap_argv_inner(&path, argv, cwd, scratch, read_roots, write_roots, deny_network)
            .map_err(|e| wrap_err(format!("bwrap argv: {e}")))?;
        return Ok((
            wrapped,
            if deny_network { "linux-bwrap" } else { "linux-bwrap-fs" },
            if deny_network {
                "network denied".into()
            } else {
                "operator granted unrestricted network".into()
            },
        ));
    }
    Err(HarnessError::sandbox_unavailable(
        "MCP filesystem confinement is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_never_binds_host_root() {
        let candidate = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let argv = bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            &["/bin/true".into()],
            candidate.path(),
            scratch.path(),
            &[],
        )
        .expect("bwrap argv");
        assert!(!argv_binds_host_root(&argv));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stdio_wrap_uses_seatbelt() {
        let capabilities = StdioCapabilities {
            version: 1,
            filesystem: Default::default(),
            read_roots: vec![],
            write_roots: vec![],
            network: NetworkPolicy::Deny,
            containment: Containment::ProcessGroup,
            limits: None,
        };
        let wrapped = wrap_mcp_stdio(&["/bin/echo".into(), "ok".into()], None, None, &capabilities).expect("wrap");
        assert_eq!(wrapped.backend, "darwin-seatbelt");
        let bin = Path::new(&wrapped.argv[0]);
        assert!(bin.is_absolute(), "{:?}", wrapped.argv);
        assert_eq!(bin, dunce::canonicalize(bin).unwrap().as_path(), "{:?}", wrapped.argv);
        assert_eq!(wrapped.argv[1], "-p");
    }

    #[test]
    fn home_overlap_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let inside = home.path().join("nested");
        std::fs::create_dir(&inside).unwrap();
        let err = refuse_home_overlap(&inside, home.path()).unwrap_err();
        assert_eq!(err.code, "MCP_HOME_REFUSED");
        let parent = home.path().parent().unwrap();
        let err = refuse_home_overlap(parent, home.path()).unwrap_err();
        assert_eq!(err.code, "MCP_HOME_REFUSED");
    }

    #[cfg(unix)]
    #[test]
    fn mcp_home_must_not_be_exposed_by_fixed_runtime_grants() {
        let policy: StdioCapabilities = serde_json::from_value(serde_json::json!({
            "version":1,"network":"deny","containment":"process_group"
        }))
        .unwrap();
        let result = wrap_mcp_stdio(
            &["/bin/true".into()],
            None,
            Some(Path::new("/usr/share/cgah-fixture-home")),
            &policy,
        );
        assert_eq!(
            result.err().expect("runtime grant must not expose home").code,
            "MCP_HOME_REFUSED"
        );
    }
}
