//! Git's execution boundary for retained clones. Repository metadata is not a
//! source of executable configuration; authentication uses the installed gh.
use crate::common::{
    child_env::{self, Child},
    errors::{HarnessError, Result},
    process::{self, Output, RunSpec},
};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// Every `run`/`checked` call first reads the local config through `validate`,
/// bounded by this; the operation's own bound comes on top. The shim budgets in
/// `tests/shim_and_agent_routes.rs` sum both.
pub const VALIDATE_TIMEOUT_SEC: u64 = 30;
/// Bound for one metadata-sized operation (rev-parse, check-attr, index, tree
/// and ref writes). Network operations carry their own, larger bounds.
pub const DEFAULT_OP_TIMEOUT_SEC: u64 = 30;

pub(super) fn environment(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = [
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "GH_HOST",
        "SSH_AUTH_SOCK",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
    ]
    .iter()
    .filter_map(|key| std::env::var(key).ok().map(|v| (key.to_string(), v)))
    .collect();
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    for (key, value) in [
        ("GIT_CONFIG_GLOBAL", null),
        ("GIT_CONFIG_SYSTEM", null),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_ATTR_NOSYSTEM", "1"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
    ] {
        env.insert(key.into(), value.into());
    }
    for (key, value) in extra {
        env.insert((*key).into(), (*value).into());
    }
    // Last: git's credential.helper is `!gh auth git-credential`, so this env
    // is gh's env too, and `extra` must not be able to switch telemetry back on.
    child_env::apply(&mut env, Child::Gh);
    env
}

fn raw(
    worktree: &Path,
    args: &[&str],
    timeout: Duration,
    extra: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<Output> {
    let binary = process::which("git").ok_or_else(|| HarnessError::agentic("git executable not found on PATH"))?;
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut argv = vec![binary.display().to_string(), "--literal-pathspecs".into()];
    for setting in [
        format!("core.hooksPath={null}"),
        format!("core.attributesFile={null}"),
        "core.fsmonitor=false".into(),
        "diff.external=".into(),
        "commit.gpgSign=false".into(),
        "tag.gpgSign=false".into(),
        "maintenance.auto=false".into(),
        "gc.auto=0".into(),
        "credential.helper=".into(),
        "credential.helper=!gh auth git-credential".into(),
    ] {
        argv.extend(["-c".into(), setting]);
    }
    argv.extend(args.iter().map(|s| (*s).into()));
    process::run(RunSpec {
        argv: &argv,
        cwd: Some(worktree),
        env: Some(&environment(extra)),
        timeout,
        stdin,
    })
    .map_err(|_| {
        HarnessError::agentic("Git operation did not complete reliably; inspect state before retrying")
            .detail("indeterminate", true)
    })
}

/// Git `-c` arguments, placed before the subcommand, that make one network
/// operation authenticate through the identity-checked `gh` binary instead of
/// whichever `gh` is first on PATH (`raw`'s default helper). The empty value
/// resets Git's multi-valued helper list before the pinned helper is added.
pub fn pinned_credential_helper(gh: &Path) -> Result<[String; 4]> {
    let path = gh.display().to_string();
    // Git runs `!` helpers through the shell; single quotes keep the path literal.
    if !gh.is_absolute() || path.contains('\'') {
        return Err(HarnessError::agentic(
            "GitHub CLI path cannot be pinned as a Git credential helper",
        ));
    }
    Ok([
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        format!("credential.helper=!'{path}' auth git-credential"),
    ])
}

/// Fail closed on metadata capable of redirecting Git or executing programs.
/// Values are deliberately never included in errors or audit records.
fn validate(worktree: &Path) -> Result<()> {
    let git = worktree.join(".git");
    let metadata = std::fs::symlink_metadata(&git)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(HarnessError::write_refused(
            "Git metadata must be an ordinary clone directory",
        ));
    }
    for file in [
        "config",
        "commondir",
        "info/attributes",
        "config.worktree",
        "objects/info/alternates",
        "info/grafts",
    ] {
        if let Ok(meta) = std::fs::symlink_metadata(git.join(file)) {
            if !meta.is_file() || meta.file_type().is_symlink() || (file != "config" && meta.len() != 0) {
                return Err(HarnessError::write_refused(
                    "unsupported Git metadata; create a fresh isolated run",
                ));
            }
        }
    }
    let output = raw(
        worktree,
        &["config", "--local", "--no-includes", "--null", "--list"],
        Duration::from_secs(VALIDATE_TIMEOUT_SEC),
        &[],
        None,
    )?;
    if output.status != Some(0) {
        return Err(HarnessError::write_refused("cannot validate local Git configuration"));
    }
    for entry in output.stdout.split('\0').filter(|s| !s.is_empty()) {
        let (key, _) = entry.split_once('\n').unwrap_or((entry, ""));
        let key = key.to_ascii_lowercase();
        let ordinary = matches!(
            key.as_str(),
            "diff.external"
                | "core.fsmonitor"
                | "core.repositoryformatversion"
                | "core.filemode"
                | "core.bare"
                | "core.logallrefupdates"
                | "core.ignorecase"
                | "core.precomposeunicode"
                | "core.symlinks"
                | "extensions.objectformat"
                | "extensions.refstorage"
                | "remote.origin.url"
                | "remote.origin.fetch"
                | "user.name"
                | "user.email"
                | "init.defaultbranch"
        ) || (key.starts_with("branch.") && (key.ends_with(".remote") || key.ends_with(".merge")));
        if !ordinary {
            return Err(HarnessError::write_refused("unsupported local Git configuration; hooks, filters, includes, redirects and custom programs are not accepted"));
        }
    }
    Ok(())
}

pub fn run(
    worktree: &Path,
    args: &[&str],
    timeout: Duration,
    extra: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<Output> {
    validate(worktree)?;
    raw(worktree, args, timeout, extra, stdin)
}

pub fn checked(
    worktree: &Path,
    args: &[&str],
    timeout: Duration,
    extra: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<String> {
    let output = run(worktree, args, timeout, extra, stdin)?;
    if output.status != Some(0) {
        return Err(HarnessError::agentic("Git operation failed; no automatic retry")
            .detail("exit_code", output.status.unwrap_or(-1)));
    }
    Ok(output.stdout)
}

/// Attributes that can transform bytes are outside the raw-content review
/// contract. Refuse them rather than silently committing the wrong representation.
pub fn require_raw_paths(worktree: &Path, paths: &[String]) -> Result<()> {
    let mut args = vec![
        "check-attr",
        "-z",
        "filter",
        "text",
        "eol",
        "working-tree-encoding",
        "ident",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let text = checked(worktree, &args, Duration::from_secs(DEFAULT_OP_TIMEOUT_SEC), &[], None)?;
    let parts: Vec<_> = text.split_terminator('\0').collect();
    if parts.len() % 3 != 0 {
        return Err(HarnessError::agentic("invalid Git attribute response"));
    }
    if parts
        .as_chunks::<3>()
        .0
        .iter()
        .any(|part| !matches!(part[2], "unspecified" | "unset"))
    {
        return Err(HarnessError::write_refused(
            "selected path uses Git content transformations; this review contract requires raw bytes",
        ));
    }
    Ok(())
}

/// gh's authenticated clone still invokes Git. Drop ambient Git control variables
/// and use an empty template with hooks disabled before any checkout occurs.
pub fn isolate_clone_environment(env: &mut BTreeMap<String, String>, template: &Path) {
    env.retain(|key, _| !key.starts_with("GIT_"));
    for (key, value) in environment(&[]) {
        if key.starts_with("GIT_") {
            env.insert(key, value);
        }
    }
    env.insert("GIT_TEMPLATE_DIR".into(), template.display().to_string());
    env.insert("GIT_CONFIG_COUNT".into(), "1".into());
    env.insert("GIT_CONFIG_KEY_0".into(), "core.hooksPath".into());
    env.insert(
        "GIT_CONFIG_VALUE_0".into(),
        if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_helper_requires_an_absolute_quotable_path() {
        // temp_dir() is absolute on every platform (a drive path on Windows).
        let gh = std::env::temp_dir().join("gh dir").join("gh");
        let args = super::pinned_credential_helper(&gh).unwrap();
        assert_eq!(args[..3], ["-c", "credential.helper=", "-c"]);
        assert_eq!(
            args[3],
            format!("credential.helper=!'{}' auth git-credential", gh.display())
        );
        let quoted = std::env::temp_dir().join("it's").join("gh");
        for refused in [
            std::path::PathBuf::from("gh"),
            std::path::PathBuf::from("relative/gh"),
            quoted,
        ] {
            assert!(
                super::pinned_credential_helper(&refused).is_err(),
                "{}",
                refused.display()
            );
        }
    }

    /// Real `git credential fill`: without the pinned arguments Git asks the
    /// `gh` on PATH; with them it asks only the pinned binary.
    #[cfg(unix)]
    #[test]
    fn pinned_helper_replaces_the_path_gh_for_one_operation() {
        use std::os::unix::fs::PermissionsExt;
        if super::process::which("git").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fake = |folder: &str, name: &str| {
            let folder = dir.path().join(folder);
            std::fs::create_dir_all(&folder).unwrap();
            let gh = folder.join("gh");
            let marker = dir.path().join(name);
            let script = format!(
                "#!/bin/sh\ntouch '{}'\nprintf 'username=x-access-token\\npassword={name}\\n'\n",
                marker.display()
            );
            std::fs::write(&gh, script).unwrap();
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
            (gh, marker)
        };
        let (_, decoy) = fake("on path", "decoy");
        let (pinned, pinned_marker) = fake("pinned gh", "pinned");
        let path = format!("{}:/usr/bin:/bin", dir.path().join("on path").display());
        let fill = |args: &[&str]| {
            let mut argv: Vec<&str> = args.to_vec();
            argv.extend(["credential", "fill"]);
            super::raw(
                dir.path(),
                &argv,
                std::time::Duration::from_secs(30),
                &[("PATH", &path)],
                Some(b"protocol=https\nhost=github.com\n\n"),
            )
            .unwrap()
        };
        let default = fill(&[]);
        assert!(default.stdout.contains("password=decoy"), "{}", default.stderr);
        std::fs::remove_file(&decoy).unwrap();
        let helper = super::pinned_credential_helper(&pinned).unwrap();
        let args: Vec<&str> = helper.iter().map(String::as_str).collect();
        let pinned_output = fill(&args);
        assert!(
            pinned_output.stdout.contains("password=pinned"),
            "{}",
            pinned_output.stderr
        );
        assert!(pinned_marker.exists());
        assert!(
            !decoy.exists(),
            "the PATH gh must not be asked once the helper is pinned"
        );
    }

    #[test]
    fn credential_helper_receives_only_supported_tokens_and_no_git_overrides() {
        let env = super::environment(&[]);
        for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
            assert_eq!(env.get(key), std::env::var(key).ok().as_ref());
        }
        assert!(!env.contains_key("SERPAPI_API_KEY"));
        assert!(!env.contains_key("GIT_CONFIG_COUNT"));
        assert_eq!(env.get("GIT_CONFIG_NOSYSTEM").map(String::as_str), Some("1"));
        assert_eq!(env.get("GH_TELEMETRY").map(String::as_str), Some("false"));
        assert_eq!(env.get("GH_NO_UPDATE_NOTIFIER").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("GH_NO_EXTENSION_UPDATE_NOTIFIER").map(String::as_str),
            Some("1")
        );
        assert_eq!(env.get("DO_NOT_TRACK").map(String::as_str), Some("1"));
        for (key, value) in super::child_env::telemetry_opt_outs(super::Child::Gh) {
            assert_eq!(env.get(*key).map(String::as_str), Some(*value), "{key}");
        }
        assert_eq!(
            super::environment(&[("GH_TELEMETRY", "true")])
                .get("GH_TELEMETRY")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            super::environment(&[("GH_NO_EXTENSION_UPDATE_NOTIFIER", "0"), ("DO_NOT_TRACK", "0")])
                .get("DO_NOT_TRACK")
                .map(String::as_str),
            Some("1")
        );
    }
}
