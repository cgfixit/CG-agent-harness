//! Read-only `gh` subprocess wrapper, port of `agentic/gh_client.py`.
//! argv is always a list, the binary is resolved to an absolute path, only an
//! allow-listed set of read ops can be built, and no token ever enters argv.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

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
pub fn check_gh_version(min_version: (u32, u32, u32)) -> Result<(u32, u32, u32)> {
    let binary = resolve_gh()?;
    let argv = vec![binary.display().to_string(), "--version".into()];
    let env = gh_env();
    let mut last_timeout = None;
    let mut output = None;
    for attempt in 1..=2u32 {
        match process::run(RunSpec {
            argv: &argv,
            cwd: None,
            env: Some(&env),
            timeout: Duration::from_secs(10),
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
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
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
    Ok(found)
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
    let found = check_gh_version(req.min_version)?;
    let binary = resolve_gh()?;
    let dest_str = req.dest.map(|d| d.display().to_string());
    let argv = build_read_argv(
        req.op,
        req.repo,
        req.number,
        req.limit,
        &binary.display().to_string(),
        dest_str.as_deref(),
    )?;
    let env = gh_env();
    let attempts = req.retries + 1;
    let mut completed: Option<process::Output> = None;
    for attempt in 1..=attempts {
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
