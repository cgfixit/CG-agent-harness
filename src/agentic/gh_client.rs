//! Read-only `gh` subprocess wrapper, port of `agentic/gh_client.py`.
//! argv is always a list, the binary is resolved to an absolute path, only an
//! allow-listed set of read ops can be built, and no token ever enters argv.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{json, Value};

use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};
use crate::common::process::{self, RunSpec};

use super::config::repo_re;

pub const DEFAULT_MIN_GH: (u32, u32, u32) = (2, 40, 0);
pub const MAX_LIST_LIMIT: u64 = 1000;
pub const MAX_DIFF_CHARS: usize = 200_000;
pub const CLONE_DEPTH: u32 = 1;
pub const DEFAULT_CLONE_TIMEOUT_SEC: u64 = 120;
const MAX_BACKOFF_SEC: f64 = 30.0;

#[derive(Debug)]
struct CheckedGh {
    binary: PathBuf,
    version: (u32, u32, u32),
    identity: ExecutableIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableIdentity {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

static CHECKED_GH: OnceLock<CheckedGh> = OnceLock::new();

pub const READ_OPS: [&str; 7] = [
    "pr_view",
    "pr_list",
    "pr_diff",
    "issue_view",
    "issue_list",
    "repo_view",
    "repo_clone",
];

const PR_FIELDS: &str = "number,title,state,author,headRefName,baseRefName,isDraft,url,body,labels,assignees,createdAt,updatedAt,mergeable,reviewDecision,additions,deletions,changedFiles";
const PR_LIST_FIELDS: &str = "number,title,state,author,isDraft,url,labels,createdAt,updatedAt";
const ISSUE_FIELDS: &str = "number,title,state,author,labels,url,body,assignees,milestone,createdAt,updatedAt,comments";
const ISSUE_LIST_FIELDS: &str = "number,title,state,author,url,labels,createdAt,updatedAt";
const REPO_FIELDS: &str = "name,owner,description,defaultBranchRef,isPrivate,url,isArchived,pushedAt,primaryLanguage,repositoryTopics,stargazerCount,licenseInfo";

fn version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)gh version\s+(\d+)\.(\d+)\.(\d+)").expect("static regex"))
}

fn transient_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:timeout|timed out|temporary failure|connection (?:reset|refused|timed out)|could not resolve host|network is unreachable|i/o timeout|\bEOF\b|HTTP 5\d\d|HTTP 429|rate limit|server error|service unavailable|bad gateway)",
        )
        .expect("static regex")
    })
}

pub fn is_transient_gh_error(stderr: &str) -> bool {
    transient_re().is_match(stderr)
}

/// Full inherited env with telemetry opt-outs forced on top.
pub fn gh_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    env.insert("GH_TELEMETRY".into(), "false".into());
    env.insert("GH_NO_UPDATE_NOTIFIER".into(), "1".into());
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    env.insert("DO_NOT_TRACK".into(), "1".into());
    env
}

pub fn resolve_gh() -> Result<PathBuf> {
    process::which("gh").ok_or_else(|| {
        HarnessError::gh_not_installed("GitHub CLI (gh) not found on PATH")
            .detail("looked_for", "gh")
            .detail("install_hint", "see https://github.com/cli/cli#installation")
    })
}

/// Confirm `gh` is installed and at/above `min_version`.
///
/// The canonical `gh` path and detected version are cached together after the
/// first successful spawn, so a later PATH change cannot substitute a different
/// unchecked executable. Callers like
/// `fetch_pr_context` and `fetch_repo_context` can invoke `run_read` several
/// times per `agentic` action, and each call previously re-spawned
/// `gh --version` (with its own retry/timeout) before doing the real read.
/// Failures are never cached, so a transient spawn/timeout error still
/// retries on the next call.
pub fn check_gh_version(min_version: (u32, u32, u32)) -> Result<(u32, u32, u32)> {
    Ok(checked_gh(min_version)?.version)
}

fn executable_identity(binary: &Path) -> Result<ExecutableIdentity> {
    let metadata = std::fs::metadata(binary).map_err(|e| {
        HarnessError::gh_not_installed(format!("Could not inspect GitHub CLI (gh): {e}"))
            .detail("path", binary.display().to_string())
    })?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(ExecutableIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

fn changed_gh(binary: &Path) -> HarnessError {
    HarnessError::gh_version("GitHub CLI (gh) changed after version validation; restart and retry")
        .detail("path", binary.display().to_string())
}

fn checked_gh(min_version: (u32, u32, u32)) -> Result<&'static CheckedGh> {
    if CHECKED_GH.get().is_none() {
        let checked = check_gh_version_uncached(min_version)?;
        let _ = CHECKED_GH.set(checked);
    }
    let checked = CHECKED_GH.get().expect("checked gh set after successful validation");
    if executable_identity(&checked.binary)? != checked.identity {
        return Err(changed_gh(&checked.binary));
    }
    if checked.version < min_version {
        return Err(HarnessError::gh_version(format!(
            "gh {}.{}.{} is too old; need >= {}.{}.{}",
            checked.version.0, checked.version.1, checked.version.2, min_version.0, min_version.1, min_version.2
        )));
    }
    Ok(checked)
}

fn check_gh_version_uncached(min_version: (u32, u32, u32)) -> Result<CheckedGh> {
    let resolved = resolve_gh()?;
    let binary = dunce::canonicalize(&resolved).map_err(|e| {
        HarnessError::gh_not_installed(format!("Could not resolve GitHub CLI (gh): {e}"))
            .detail("path", resolved.display().to_string())
    })?;
    let identity = executable_identity(&binary)?;
    check_gh_binary(
        binary,
        identity,
        min_version,
        Duration::from_secs(10),
        Duration::from_secs(1),
    )
}

fn check_gh_binary(
    binary: PathBuf,
    identity: ExecutableIdentity,
    min_version: (u32, u32, u32),
    timeout: Duration,
    retry_delay: Duration,
) -> Result<CheckedGh> {
    let argv = vec![binary.display().to_string(), "--version".into()];
    let env = gh_env();
    let mut last_timeout = None;
    let mut output = None;
    for attempt in 1..=2u32 {
        if executable_identity(&binary)? != identity {
            return Err(changed_gh(&binary));
        }
        match process::run(RunSpec {
            argv: &argv,
            cwd: None,
            env: Some(&env),
            timeout,
            stdin: None,
        }) {
            Ok(out) => {
                output = Some(out);
                last_timeout = None;
                break;
            }
            Err(process::ProcessError::Timeout { .. }) => {
                last_timeout = Some(attempt);
                if attempt < 2 {
                    std::thread::sleep(retry_delay);
                }
            }
            Err(process::ProcessError::Capture(e)) => return Err(HarnessError::agentic(e)),
            Err(process::ProcessError::Spawn(e)) => {
                return Err(HarnessError::gh_not_installed(format!(
                    "Could not execute GitHub CLI (gh): {e}"
                )));
            }
        }
    }
    if last_timeout.is_some() {
        return Err(HarnessError::gh_version("gh version check timed out"));
    }
    let out = output.expect("output present when no timeout");
    let text = format!("{}{}", out.stdout, out.stderr);
    let caps = version_re()
        .captures(&text)
        .ok_or_else(|| HarnessError::gh_version("Could not parse gh version output"))?;
    let found = (
        caps[1].parse::<u32>().unwrap_or(0),
        caps[2].parse::<u32>().unwrap_or(0),
        caps[3].parse::<u32>().unwrap_or(0),
    );
    if found < min_version {
        return Err(HarnessError::gh_version(format!(
            "gh {}.{}.{} is too old; need >= {}.{}.{}",
            found.0, found.1, found.2, min_version.0, min_version.1, min_version.2
        )));
    }
    if executable_identity(&binary)? != identity {
        return Err(changed_gh(&binary));
    }
    Ok(CheckedGh {
        binary,
        version: found,
        identity,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn version_retry_refuses_a_replaced_executable() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let marker = dir.path().join("version-started");
        let replacement_ran = dir.path().join("replacement-ran");
        let replacement = dir.path().join("replacement-gh");
        write_executable(
            &replacement,
            &format!(
                "#!/bin/sh\nprintf ran > '{}'\necho 'gh version 2.60.0'\n",
                replacement_ran.display()
            ),
        );
        write_executable(
            &binary,
            &format!(
                "#!/bin/sh\nmv '{}' \"$0\"\n: > '{}'\nsleep 30\n",
                replacement.display(),
                marker.display()
            ),
        );
        let identity = executable_identity(&binary).unwrap();
        let error = check_gh_binary(
            binary,
            identity,
            DEFAULT_MIN_GH,
            Duration::from_secs(2),
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(marker.exists(), "fake gh version check did not start");
        assert!(
            error.message.contains("changed after version validation"),
            "unexpected error: {}",
            error.message
        );
        assert!(!replacement_ran.exists(), "replacement gh must not execute");
    }
}

/// argv for a read-only op. `dest` (repo_clone) is ALWAYS computed internally by callers.
pub fn build_read_argv(
    op: &str,
    repo: &str,
    number: Option<i64>,
    limit: u64,
    gh_bin: &str,
    dest: Option<&str>,
) -> Result<Vec<String>> {
    if !READ_OPS.contains(&op) {
        return Err(HarnessError::agentic(format!("Unknown or non-read-only gh op: '{op}'")).detail("op", op));
    }
    if !repo_re().is_match(repo) {
        return Err(
            HarnessError::agentic("invalid repo slug (must be 'owner/name', alphanumeric-leading)")
                .detail("repo", repo),
        );
    }
    let num = number.map(|n| n.to_string());
    if matches!(op, "pr_view" | "pr_diff" | "issue_view") && num.is_none() {
        return Err(HarnessError::agentic(format!("op '{op}' requires a 'number'")).detail("op", op));
    }
    let safe_limit = limit.min(MAX_LIST_LIMIT).to_string();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    Ok(match op {
        "pr_view" => s(&[
            gh_bin,
            "pr",
            "view",
            num.as_deref().unwrap_or(""),
            "--repo",
            repo,
            "--json",
            PR_FIELDS,
        ]),
        "pr_diff" => s(&[gh_bin, "pr", "diff", num.as_deref().unwrap_or(""), "--repo", repo]),
        "pr_list" => s(&[
            gh_bin,
            "pr",
            "list",
            "--repo",
            repo,
            "--json",
            PR_LIST_FIELDS,
            "--limit",
            &safe_limit,
        ]),
        "issue_view" => s(&[
            gh_bin,
            "issue",
            "view",
            num.as_deref().unwrap_or(""),
            "--repo",
            repo,
            "--json",
            ISSUE_FIELDS,
        ]),
        "issue_list" => s(&[
            gh_bin,
            "issue",
            "list",
            "--repo",
            repo,
            "--json",
            ISSUE_LIST_FIELDS,
            "--limit",
            &safe_limit,
        ]),
        "repo_clone" => {
            let d = dest.ok_or_else(|| HarnessError::agentic("repo_clone requires a 'dest' path"))?;
            s(&[
                gh_bin,
                "repo",
                "clone",
                repo,
                d,
                "--",
                "--depth",
                &CLONE_DEPTH.to_string(),
            ])
        }
        _ => s(&[gh_bin, "repo", "view", repo, "--json", REPO_FIELDS]),
    })
}

pub struct ReadRequest<'a> {
    pub op: &'a str,
    pub repo: &'a str,
    pub number: Option<i64>,
    pub limit: u64,
    pub min_version: (u32, u32, u32),
    pub timeout_sec: u64,
    pub retries: u32,
    pub dest: Option<&'a Path>,
}

/// Run a read-only op. `pr_diff` -> `{"diff"}`, `repo_clone` -> `{"dest"}`, else `{"data"}`.
pub fn run_read(audit: &Audit, req: &ReadRequest<'_>) -> Result<Value> {
    let checked = checked_gh(req.min_version)?;
    let found = checked.version;
    let dest_str = req.dest.map(|d| d.display().to_string());
    let argv = build_read_argv(
        req.op,
        req.repo,
        req.number,
        req.limit,
        &checked.binary.display().to_string(),
        dest_str.as_deref(),
    )?;
    let mut env = gh_env();
    let clone_template = if req.op == "repo_clone" {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    if let Some(template) = &clone_template {
        super::git::isolate_clone_environment(&mut env, template.path());
    }
    let attempts = req.retries + 1;
    let mut completed: Option<process::Output> = None;
    for attempt in 1..=attempts {
        if executable_identity(&checked.binary)? != checked.identity {
            return Err(changed_gh(&checked.binary));
        }
        match process::run(RunSpec {
            argv: &argv,
            cwd: None,
            env: Some(&env),
            timeout: Duration::from_secs(req.timeout_sec),
            stdin: None,
        }) {
            Ok(out) => {
                if out.status != Some(0) && attempt < attempts && is_transient_gh_error(&out.stderr) {
                    audit.log(json!({"event": "agentic_read_retry", "op": req.op, "repo": req.repo, "attempt": attempt, "exit_code": out.status}));
                    std::thread::sleep(Duration::from_secs_f64(
                        (2.0f64 * 2f64.powi(attempt as i32 - 1)).min(MAX_BACKOFF_SEC),
                    ));
                    continue;
                }
                completed = Some(out);
                break;
            }
            Err(process::ProcessError::Timeout { .. }) => {
                audit.log(json!({"event": "agentic_read_timeout", "op": req.op, "repo": req.repo, "attempt": attempt}));
                if attempt < attempts {
                    if req.op == "repo_clone" {
                        if let Some(d) = req.dest {
                            let _ = std::fs::remove_dir_all(d);
                        }
                    }
                    std::thread::sleep(Duration::from_secs_f64(
                        (2.0f64 * 2f64.powi(attempt as i32 - 1)).min(MAX_BACKOFF_SEC),
                    ));
                    continue;
                }
                return Err(
                    HarnessError::agentic(format!("gh {} timed out after {}s", req.op, req.timeout_sec))
                        .detail("op", req.op)
                        .detail("repo", req.repo),
                );
            }
            Err(process::ProcessError::Capture(e)) => return Err(HarnessError::agentic(e)),
            Err(process::ProcessError::Spawn(e)) => {
                return Err(HarnessError::gh_not_installed(format!("Could not execute gh: {e}")));
            }
        }
    }
    let out = completed.ok_or_else(|| HarnessError::agentic(format!("gh {} never executed", req.op)))?;
    audit.log(json!({
        "event": "agentic_read", "op": req.op, "repo": req.repo, "number": req.number,
        "dest": dest_str, "gh_version": format!("{}.{}.{}", found.0, found.1, found.2), "exit_code": out.status,
    }));
    if out.status != Some(0) {
        return Err(HarnessError::agentic(format!(
            "gh {} failed with exit code {}",
            req.op,
            out.status.unwrap_or(-1)
        ))
        .detail("op", req.op)
        .detail("repo", req.repo)
        .detail("stderr", crate::common::clip_chars(&out.stderr, 500)));
    }
    if req.op == "pr_diff" {
        let mut diff = out.stdout;
        if diff.chars().count() > MAX_DIFF_CHARS {
            diff = format!(
                "{}\n... [diff truncated at {MAX_DIFF_CHARS} chars]",
                crate::common::clip_chars(&diff, MAX_DIFF_CHARS)
            );
        }
        return Ok(json!({"op": req.op, "repo": req.repo, "diff": diff}));
    }
    if req.op == "repo_clone" {
        return Ok(json!({"op": req.op, "repo": req.repo, "dest": dest_str}));
    }
    let text = if out.stdout.trim().is_empty() {
        "null".to_string()
    } else {
        out.stdout.clone()
    };
    let data: Value = serde_json::from_str(&text).map_err(|_| {
        HarnessError::agentic(format!("gh {} returned non-JSON output", req.op))
            .detail("op", req.op)
            .detail("output", crate::common::clip_chars(&out.stdout, 500))
    })?;
    Ok(json!({"op": req.op, "repo": req.repo, "data": data}))
}
