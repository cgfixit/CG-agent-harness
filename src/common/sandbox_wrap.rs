//! Argv wrapping for Darwin Seatbelt and Linux bubblewrap.
//!
//! Shared by agentic verification and MCP stdio so `src/server` never imports
//! `crate::agentic`. Never binds host root.

use std::path::{Path, PathBuf};
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
    pub child_cwd: PathBuf,
    pub scratch: PathBuf,
    _scratch: tempfile::TempDir,
    _candidate: Option<tempfile::TempDir>,
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

pub fn wrap_mcp_stdio(argv: &[String], cwd: Option<&Path>) -> Result<WrappedStdio> {
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
    let read_roots = command_read_roots(argv, Some(&child_cwd));
    let (wrapped, backend) = wrap_argv(argv, &child_cwd, scratch.path(), &read_roots)?;
    Ok(WrappedStdio {
        argv: wrapped,
        backend,
        child_cwd,
        scratch: scratch.path().to_path_buf(),
        _scratch: scratch,
        _candidate: candidate,
    })
}

/// Same probe `LinuxBubblewrapSandbox::new` uses. Presence of `bwrap` is not enough:
/// GitHub Actions often fails `bwrap` with `Failed RTM_NEWADDR`.
pub fn probe_linux_bwrap() -> std::result::Result<PathBuf, String> {
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
    let wrapped = bwrap_argv(&path, &["/bin/true".into()], &candidate, &scratch, &[])?;
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

fn linux_unshare_prefix() -> Option<Vec<String>> {
    let path = process::which("unshare")?;
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
) -> Result<(Vec<String>, &'static str)> {
    if cfg!(target_os = "macos") {
        let sandbox_exec = process::which("sandbox-exec").ok_or_else(|| {
            HarnessError::sandbox_unavailable("sandbox-exec not found; Darwin MCP stdio fails closed")
        })?;
        let mut out = vec![
            sandbox_exec.display().to_string(),
            "-p".into(),
            seatbelt_profile_with_inputs(cwd, Some(scratch), read_roots),
            "--".into(),
        ];
        out.extend(argv.iter().cloned());
        return Ok((out, "darwin-seatbelt"));
    }
    if cfg!(target_os = "linux") {
        if let Ok(bwrap) = probe_linux_bwrap() {
            let wrapped =
                bwrap_argv(&bwrap, argv, cwd, scratch, read_roots).map_err(|e| wrap_err(format!("bwrap argv: {e}")))?;
            return Ok((wrapped, "linux-bwrap"));
        }
        if let Some(mut prefix) = linux_unshare_prefix() {
            prefix.extend(argv.iter().cloned());
            return Ok((prefix, "linux-netns"));
        }
        return Err(HarnessError::sandbox_unavailable(
            "linux MCP stdio sandbox unavailable (bwrap probe failed and unshare --net probe failed)",
        ));
    }
    Ok((argv.to_vec(), "windows-stdio"))
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
        let wrapped = wrap_mcp_stdio(&["/bin/echo".into(), "ok".into()], None).expect("wrap");
        assert_eq!(wrapped.backend, "darwin-seatbelt");
        assert!(wrapped.argv[0].contains("sandbox-exec"), "{:?}", wrapped.argv);
        assert_eq!(wrapped.argv[1], "-p");
    }
}
