//! Git's execution boundary for retained clones. Repository metadata is not a
//! source of executable configuration; authentication uses the installed gh.
use crate::common::{
    errors::{HarnessError, Result},
    process::{self, Output, RunSpec},
};
use std::{collections::BTreeMap, path::Path, time::Duration};

fn environment(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = ["PATH", "HOME", "LANG", "LC_ALL"]
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
        Duration::from_secs(30),
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
    let text = checked(worktree, &args, Duration::from_secs(30), &[], None)?;
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
