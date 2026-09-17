//! Argv wrapping for Darwin Seatbelt and Linux bubblewrap.
//!
//! Shared by agentic verification and MCP stdio so `src/server` never imports
//! `crate::agentic`. Never binds host root.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use super::errors::{HarnessError, Result};
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
    let esc = |p: &Path| {
        dunce::canonicalize(p)
            .unwrap_or(p.to_path_buf())
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    };
    let mut roots = vec![cwd.to_path_buf()];
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
    bwrap_argv_inner(bwrap, argv, cwd, scratch, read_roots, true)
}

fn bwrap_argv_inner(
    bwrap: &Path,
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
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

pub fn command_read_roots(argv: &[String], cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(cwd) = cwd {
        roots.push(cwd.to_path_buf());
    }
    for item in argv {
        let path = Path::new(item);
        if !path.is_absolute() {
            continue;
        }
        let Ok(canon) = dunce::canonicalize(path) else {
            continue;
        };
        if let Some(parent) = canon.parent() {
            roots.push(parent.to_path_buf());
        }
        for prefix in ["/opt/homebrew", "/usr/local"] {
            if canon.starts_with(prefix) {
                roots.push(PathBuf::from(prefix));
            }
        }
        roots.push(canon);
    }
    roots
}

pub fn wrap_mcp_stdio(argv: &[String], cwd: Option<&Path>, home: Option<&Path>) -> Result<WrappedStdio> {
    let scratch = tempfile::Builder::new()
        .prefix("cgah-mcp-scratch-")
        .tempdir()
        .map_err(|e| wrap_err(format!("sandbox scratch: {e}")))?;
    let (child_cwd, candidate) = match cwd {
        Some(path) => (path.to_path_buf(), None),
        None => {
            let dir = tempfile::Builder::new()
                .prefix("cgah-mcp-cwd-")
                .tempdir()
                .map_err(|e| wrap_err(format!("sandbox cwd: {e}")))?;
            let path = dir.path().to_path_buf();
            (path, Some(dir))
        }
    };
    if let Some(home) = home {
        refuse_home_overlap(&child_cwd, home)?;
        for item in argv {
            let path = Path::new(item);
            if path.is_absolute() {
                refuse_home_overlap(path, home)?;
            }
        }
    }
    let read_roots = command_read_roots(argv, Some(&child_cwd));
    if let Some(home) = home {
        for root in &read_roots {
            refuse_home_overlap(root, home)?;
        }
    }
    let (wrapped, backend, probe_reason) = wrap_argv(argv, &child_cwd, scratch.path(), &read_roots)?;
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
#[derive(Clone)]
pub struct LinuxBwrap {
    pub path: PathBuf,
    pub unshare_net: bool,
    pub net_err: Option<String>,
}

/// Agentic verification requires network isolation. MCP stdio may fall back to
/// FS-only bwrap when `--unshare-net` is EPERM (GitHub Actions).
pub fn probe_linux_bwrap() -> std::result::Result<PathBuf, String> {
    static CACHED: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
    CACHED
        .get_or_init(|| probe_linux_bwrap_kind(true).map(|found| found.path))
        .clone()
}

fn probed_linux_bwrap() -> std::result::Result<LinuxBwrap, String> {
    static CACHED: OnceLock<std::result::Result<LinuxBwrap, String>> = OnceLock::new();
    CACHED.get_or_init(probe_linux_bwrap_uncached).clone()
}

fn probe_linux_bwrap_uncached() -> std::result::Result<LinuxBwrap, String> {
    match probe_linux_bwrap_kind(true) {
        Ok(found) => Ok(found),
        Err(net_err) => {
            let mut found = probe_linux_bwrap_kind(false).map_err(|fs_err| format!("{net_err}; fs-only: {fs_err}"))?;
            found.net_err = Some(net_err);
            Ok(found)
        }
    }
}

fn probe_linux_bwrap_kind(unshare_net: bool) -> std::result::Result<LinuxBwrap, String> {
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
    let wrapped = bwrap_argv_inner(&path, &["/bin/true".into()], &candidate, &scratch, &[], unshare_net)?;
    match process::run(RunSpec {
        argv: &wrapped,
        cwd: None,
        env: None,
        timeout: Duration::from_secs(5),
        stdin: None,
    }) {
        Ok(out) if out.status == Some(0) => Ok(LinuxBwrap {
            path,
            unshare_net,
            net_err: None,
        }),
        Ok(out) => Err(format!("bwrap probe failed (status {:?}): {}", out.status, out.stderr)),
        Err(e) => Err(format!("bwrap probe: {e}")),
    }
}

fn linux_unshare_prefix() -> Option<Vec<String>> {
    static CACHED: OnceLock<Option<Vec<String>>> = OnceLock::new();
    CACHED.get_or_init(linux_unshare_prefix_uncached).clone()
}

fn linux_unshare_prefix_uncached() -> Option<Vec<String>> {
    let path = process::which("unshare")?;
    let path = dunce::canonicalize(&path).ok()?;
    let probe = |extra: &[&str]| -> bool {
        let mut argv = vec![path.display().to_string()];
        argv.extend(extra.iter().map(|s| (*s).to_string()));
        argv.push("/bin/true".into());
        matches!(
            process::run(RunSpec {
                argv: &argv,
                cwd: None,
                env: None,
                timeout: Duration::from_secs(5),
                stdin: None,
            }),
            Ok(out) if out.status == Some(0)
        )
    };
    if probe(&["--net"]) {
        return Some(vec![path.display().to_string(), "--net".into(), "--".into()]);
    }
    if probe(&["--user", "--map-root-user", "--net"]) {
        return Some(vec![
            path.display().to_string(),
            "--user".into(),
            "--map-root-user".into(),
            "--net".into(),
            "--".into(),
        ]);
    }
    None
}

fn wrap_argv(
    argv: &[String],
    cwd: &Path,
    scratch: &Path,
    read_roots: &[PathBuf],
) -> Result<(Vec<String>, &'static str, String)> {
    if cfg!(target_os = "macos") {
        let sandbox_exec = canonical_bin("sandbox-exec")?;
        let mut out = vec![
            sandbox_exec.display().to_string(),
            "-p".into(),
            seatbelt_profile_with_inputs(cwd, Some(scratch), read_roots),
            "--".into(),
        ];
        out.extend(argv.iter().cloned());
        return Ok((out, "darwin-seatbelt", "sandbox-exec".into()));
    }
    if cfg!(target_os = "linux") {
        match probed_linux_bwrap() {
            Ok(found) => {
                let wrapped = bwrap_argv_inner(&found.path, argv, cwd, scratch, read_roots, found.unshare_net)
                    .map_err(|e| wrap_err(format!("bwrap argv: {e}")))?;
                let backend = if found.unshare_net {
                    "linux-bwrap"
                } else {
                    "linux-bwrap-fs"
                };
                let reason = if found.unshare_net {
                    "bwrap --unshare-net".into()
                } else {
                    let detail = found.net_err.as_deref().unwrap_or("bwrap --unshare-net failed");
                    format!("{}: {detail}; using fs-only", classify_probe_err(detail))
                };
                return Ok((wrapped, backend, reason));
            }
            Err(err) => {
                if let Some(mut prefix) = linux_unshare_prefix() {
                    prefix.extend(argv.iter().cloned());
                    return Ok((
                        prefix,
                        "linux-netns",
                        format!("{}: {err}; unshare --net", classify_probe_err(&err)),
                    ));
                }
                return Ok((
                    argv.to_vec(),
                    "linux-unconfined",
                    format!("{}: {err}; unshare unavailable", classify_probe_err(&err)),
                ));
            }
        }
    }
    Ok((argv.to_vec(), "windows-stdio", "windows".into()))
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
        let wrapped = wrap_mcp_stdio(&["/bin/echo".into(), "ok".into()], None, None).expect("wrap");
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
}
