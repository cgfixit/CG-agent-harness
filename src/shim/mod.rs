//! The ONLY server -> agentic edge. Port of `utils/ops_runner.py`.
//!
//! Builds an argv list beginning `[current_exe, "agentic", "--config", <cfg>, <action>]`
//! from an 11-action whitelist and spawns it as a CHILD PROCESS with a hard
//! timeout. Nothing in this module (or anywhere under `server/`) links the
//! `agentic` module; the process boundary IS the isolation (Invariant 6).
//!
//! Argv discipline: free-text values travel as `--opt=value` single elements so
//! a value beginning with `-` binds to its option instead of being reparsed as
//! a flag; `body`/`plan`/`checks` travel through temp files that are unlinked on
//! every exit path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempPath;

use crate::common::audit::Redactors;
use crate::common::repo_paths::canonical_repo_relative_path;

pub const DEFAULT_TIMEOUT_SEC: u64 = 120;
pub const REAL_REPO_RUN_FALLBACK_PLANNER_SEC: u64 = 720;
pub const REAL_REPO_RUN_DEFAULT_ITERATIONS: u64 = 3;
pub const REAL_REPO_RUN_CHECK_SEC: u64 = 120;
pub const REAL_REPO_RUN_OVERHEAD_SEC: u64 = 300;
/// A ceiling on the ceiling: a synchronous HTTP request held open longer than
/// this is its own failure mode.
pub const REAL_REPO_RUN_MAX_TIMEOUT_SEC: u64 = 3600;

pub const ACTIONS: [&str; 11] = [
    "status",
    "test",
    "context",
    "propose-skill",
    "apply-skill",
    "real-repo-run",
    "real-repo-run-status",
    "real-repo-run-decide",
    "real-repo-run-push",
    "real-repo-run-publish",
    "real-repo-run-discard",
];
pub const JSON_ACTIONS: [&str; 9] = [
    "context",
    "propose-skill",
    "apply-skill",
    "real-repo-run",
    "real-repo-run-status",
    "real-repo-run-decide",
    "real-repo-run-push",
    "real-repo-run-publish",
    "real-repo-run-discard",
];

#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    /// A disallowed action or malformed request (HTTP 400).
    #[error("{0}")]
    Ops(String),
    /// The child outlived its budget and was killed (HTTP 504).
    #[error("agentic {action} exceeded its {timeout_sec}s budget")]
    Timeout { action: String, timeout_sec: u64 },
    /// Temp-file or spawn failure before/while launching (HTTP 502).
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, Default)]
pub struct OpsRequest {
    pub action: String,
    pub pr: Option<i64>,
    pub issue: Option<i64>,
    pub no_diff: bool,
    pub name: Option<String>,
    pub desc: Option<String>,
    pub body: Option<String>,
    pub reason: Option<String>,
    pub confirm: bool,
    pub instruction: Option<String>,
    pub checks: Option<Vec<Value>>,
    pub branch: Option<String>,
    pub commit_message: Option<String>,
    pub plan: Option<String>,
    pub read_files: Option<Vec<String>>,
    pub max_iterations: Option<i64>,
    pub run_id: Option<String>,
    pub decision: Option<String>,
}

impl OpsRequest {
    pub fn new(action: &str) -> Self {
        Self {
            action: action.to_string(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpsResult {
    pub subsystem: String,
    pub action: String,
    pub exit_code: i32,
    pub ok: bool,
    pub label: String,
    pub stdout: String,
    pub stderr: String,
    pub parsed: Option<Value>,
}

impl OpsResult {
    /// Redacted envelope for the console.
    pub fn to_json(&self, redactors: &Redactors) -> Value {
        json!({
            "subsystem": self.subsystem,
            "action": self.action,
            "exit_code": self.exit_code,
            "ok": self.ok,
            "label": self.label,
            "stdout": redactors.redact(&self.stdout),
            "stderr": redactors.redact(&self.stderr),
            "parsed": self.parsed.as_ref().map(|p| redactors.redact_value(p)).unwrap_or(Value::Null),
        })
    }

    /// The CLI's exit-0 "layer disabled" banner must not read as HTTP success.
    pub fn is_disabled_layer_success(&self) -> bool {
        self.ok
            && self.parsed.is_none()
            && (self.stdout.contains("Agentic layer disabled")
                || self.stdout.contains("real-repo coding subsystem disabled"))
    }
}

/// exit code -> (ok, label)
pub fn label_for(code: i32) -> (bool, &'static str) {
    match code {
        0 => (true, "ok"),
        2 => (false, "failed"),
        3 => (false, "env_config"),
        4 => (false, "write_refused"),
        _ => (false, "unknown"),
    }
}

/// UNCAPPED wall-clock budget for one `real-repo-run` request shape.
pub fn real_repo_run_budget_sec(planner_timeout_sec: u64, max_iterations: Option<i64>, check_count: usize) -> u64 {
    let planner = if planner_timeout_sec == 0 {
        REAL_REPO_RUN_FALLBACK_PLANNER_SEC
    } else {
        planner_timeout_sec
    };
    let iterations = match max_iterations {
        Some(n) if n > 0 => n as u64,
        _ => REAL_REPO_RUN_DEFAULT_ITERATIONS,
    };
    iterations * planner
        + iterations * (check_count.max(1) as u64) * REAL_REPO_RUN_CHECK_SEC
        + REAL_REPO_RUN_OVERHEAD_SEC
}

pub fn real_repo_run_timeout_sec(planner_timeout_sec: u64, max_iterations: Option<i64>, check_count: usize) -> u64 {
    real_repo_run_budget_sec(planner_timeout_sec, max_iterations, check_count).min(REAL_REPO_RUN_MAX_TIMEOUT_SEC)
}

/// Where the child lives and what it needs.
#[derive(Debug, Clone)]
pub struct ShimContext {
    pub exe: PathBuf,
    pub config_path: PathBuf,
    pub cwd: PathBuf,
    pub tmp_dir: PathBuf,
    pub planner_timeout_sec: u64,
}

impl ShimContext {
    pub fn new(config_path: &Path, cwd: &Path, tmp_dir: &Path, planner_timeout_sec: u64) -> std::io::Result<Self> {
        Ok(Self {
            exe: std::env::current_exe()?,
            config_path: config_path.to_path_buf(),
            cwd: cwd.to_path_buf(),
            tmp_dir: tmp_dir.to_path_buf(),
            planner_timeout_sec,
        })
    }
}

fn write_temp(tmp_dir: &Path, prefix: &str, text: &str) -> Result<TempPath, ShimError> {
    use std::io::Write;
    std::fs::create_dir_all(tmp_dir).map_err(|e| ShimError::Io(format!("cannot create temp dir: {e}")))?;
    let mut f = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".txt")
        .tempfile_in(tmp_dir)
        .map_err(|e| ShimError::Io(format!("cannot stage temp file: {e}")))?;
    f.write_all(text.as_bytes())
        .map_err(|e| ShimError::Io(format!("cannot write temp file: {e}")))?;
    f.flush()
        .map_err(|e| ShimError::Io(format!("cannot flush temp file: {e}")))?;
    // Close the handle so the child can open it on Windows.
    Ok(f.into_temp_path())
}

fn non_empty(v: &Option<String>) -> bool {
    v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Validate a request before any process is launched (mirrors ops_runner's checks).
pub fn validate(req: &OpsRequest) -> Result<(), ShimError> {
    if !ACTIONS.contains(&req.action.as_str()) {
        return Err(ShimError::Ops(format!("action not allowed: {}", req.action)));
    }
    let a = req.action.as_str();
    if a == "propose-skill" || a == "apply-skill" {
        if !non_empty(&req.name) || !non_empty(&req.desc) {
            return Err(ShimError::Ops(format!("{a} requires name and desc")));
        }
        if a == "apply-skill" && !non_empty(&req.reason) {
            return Err(ShimError::Ops("apply-skill requires a non-empty reason".into()));
        }
    }
    if a == "real-repo-run" {
        if !non_empty(&req.instruction) {
            return Err(ShimError::Ops("real-repo-run requires a non-empty instruction".into()));
        }
        if req.checks.as_ref().map(|c| c.is_empty()).unwrap_or(true) {
            return Err(ShimError::Ops("real-repo-run requires a non-empty checks list".into()));
        }
        if !non_empty(&req.branch) || !non_empty(&req.commit_message) {
            return Err(ShimError::Ops(
                "real-repo-run requires both branch and commit_message".into(),
            ));
        }
        if !non_empty(&req.reason) {
            return Err(ShimError::Ops("real-repo-run requires a non-empty reason".into()));
        }
        if let Some(plan) = &req.plan {
            if plan.trim().is_empty() {
                return Err(ShimError::Ops("real-repo-run plan must not be blank".into()));
            }
        }
        if let Some(files) = &req.read_files {
            for p in files {
                if canonical_repo_relative_path(p).is_none() {
                    return Err(ShimError::Ops(
                        "real-repo-run read_files must contain non-empty repo-relative paths without NUL bytes, traversal, or absolute/drive forms".into(),
                    ));
                }
            }
        }
    }
    if a.starts_with("real-repo-run-") && !non_empty(&req.run_id) {
        return Err(ShimError::Ops(format!("{a} requires run_id")));
    }
    if a == "real-repo-run-decide" && !matches!(req.decision.as_deref(), Some("approve") | Some("reject")) {
        return Err(ShimError::Ops(
            "real-repo-run-decide requires decision to be 'approve' or 'reject'".into(),
        ));
    }
    if a == "real-repo-run-publish" && !non_empty(&req.reason) {
        return Err(ShimError::Ops(
            "real-repo-run-publish requires a non-empty reason".into(),
        ));
    }
    Ok(())
}

/// The full argv plus the temp files that must outlive the child.
pub fn build_argv(ctx: &ShimContext, req: &OpsRequest) -> Result<(Vec<String>, Vec<TempPath>), ShimError> {
    validate(req)?;
    let mut argv: Vec<String> = vec![
        ctx.exe.display().to_string(),
        "agentic".into(),
        "--config".into(),
        ctx.config_path.display().to_string(),
        req.action.clone(),
    ];
    let mut temps: Vec<TempPath> = Vec::new();
    match req.action.as_str() {
        "context" => {
            if let Some(pr) = req.pr {
                argv.push("--pr".into());
                argv.push(pr.to_string());
            } else if let Some(issue) = req.issue {
                argv.push("--issue".into());
                argv.push(issue.to_string());
            } else {
                argv.push("--repo".into());
            }
            if req.no_diff {
                argv.push("--no-diff".into());
            }
        }
        "propose-skill" | "apply-skill" => {
            argv.push(format!("--name={}", req.name.clone().unwrap_or_default()));
            argv.push(format!("--desc={}", req.desc.clone().unwrap_or_default()));
            if let Some(body) = &req.body {
                if !body.is_empty() {
                    let t = write_temp(&ctx.tmp_dir, "cgah_skill_", body)?;
                    argv.push("--body-file".into());
                    argv.push(t.display().to_string());
                    temps.push(t);
                }
            }
            if let Some(reason) = &req.reason {
                if !reason.is_empty() {
                    argv.push(format!("--reason={reason}"));
                }
            }
            if req.action == "apply-skill" && req.confirm {
                argv.push("--confirm".into());
            }
        }
        "real-repo-run" => {
            if let Some(pr) = req.pr {
                argv.push("--pr".into());
                argv.push(pr.to_string());
            } else if let Some(issue) = req.issue {
                argv.push("--issue".into());
                argv.push(issue.to_string());
            } else {
                argv.push("--repo".into());
            }
            let checks = serde_json::to_string(&json!({"checks": req.checks.clone().unwrap_or_default()}))
                .map_err(|e| ShimError::Io(e.to_string()))?;
            let checks_file = write_temp(&ctx.tmp_dir, "cgah_checks_", &checks)?;
            argv.push(format!("--instruction={}", req.instruction.clone().unwrap_or_default()));
            argv.push("--checks-file".into());
            argv.push(checks_file.display().to_string());
            temps.push(checks_file);
            argv.push(format!("--branch={}", req.branch.clone().unwrap_or_default()));
            argv.push(format!(
                "--commit-message={}",
                req.commit_message.clone().unwrap_or_default()
            ));
            argv.push(format!("--reason={}", req.reason.clone().unwrap_or_default()));
            if let Some(plan) = &req.plan {
                if !plan.is_empty() {
                    let t = write_temp(&ctx.tmp_dir, "cgah_plan_", plan)?;
                    argv.push("--plan-file".into());
                    argv.push(t.display().to_string());
                    temps.push(t);
                }
            }
            for f in req.read_files.clone().unwrap_or_default() {
                argv.push(format!("--read-file={f}"));
            }
            if let Some(n) = req.max_iterations {
                if n > 0 {
                    argv.push("--max-iterations".into());
                    argv.push(n.to_string());
                }
            }
            if req.confirm {
                argv.push("--confirm".into());
            }
        }
        "real-repo-run-status" | "real-repo-run-discard" => {
            argv.push(format!("--run-id={}", req.run_id.clone().unwrap_or_default()));
        }
        "real-repo-run-decide" => {
            argv.push(format!("--run-id={}", req.run_id.clone().unwrap_or_default()));
            argv.push("--decision".into());
            argv.push(req.decision.clone().unwrap_or_default());
            argv.push(format!("--reason={}", req.reason.clone().unwrap_or_default()));
            if req.confirm {
                argv.push("--confirm".into());
            }
        }
        "real-repo-run-push" | "real-repo-run-publish" => {
            argv.push(format!("--run-id={}", req.run_id.clone().unwrap_or_default()));
            argv.push(format!("--reason={}", req.reason.clone().unwrap_or_default()));
            if req.confirm {
                argv.push("--confirm".into());
            }
        }
        _ => {}
    }
    Ok((argv, temps))
}

/// Budget for a request: only `real-repo-run` gets a non-default timeout.
pub fn timeout_for(ctx: &ShimContext, req: &OpsRequest) -> Duration {
    let secs = if req.action == "real-repo-run" {
        real_repo_run_timeout_sec(
            ctx.planner_timeout_sec,
            req.max_iterations,
            req.checks.as_ref().map(|c| c.len()).unwrap_or(0),
        )
    } else {
        DEFAULT_TIMEOUT_SEC
    };
    Duration::from_secs(secs)
}

/// Spawn `argv` as a child and wait for it, killing the whole process group on timeout.
pub async fn run_argv(argv: &[String], cwd: &Path, timeout: Duration) -> Result<(i32, String, String), ShimError> {
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| ShimError::Io(format!("cannot spawn agentic child: {e}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).into_owned()
    });
    let err_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        if let Some(mut s) = stderr {
            let _ = s.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).into_owned()
    });
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(ShimError::Io(format!("agentic child wait failed: {e}"))),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // SAFETY: killpg on a group we created with process_group(0).
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            out_task.abort();
            err_task.abort();
            return Err(ShimError::Timeout {
                action: String::new(),
                timeout_sec: timeout.as_secs(),
            });
        }
    };
    let stdout = out_task.await.unwrap_or_default();
    let stderr = err_task.await.unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    Ok((code, stdout, stderr))
}

/// The shim entry point: validate, build argv, spawn, label. Temp files are
/// removed on every path (the `TempPath` values drop at the end of this scope).
pub async fn run_agentic_op(ctx: &ShimContext, req: &OpsRequest) -> Result<OpsResult, ShimError> {
    let (argv, _temps) = build_argv(ctx, req)?;
    let timeout = timeout_for(ctx, req);
    let (code, stdout, stderr) = run_argv(&argv, &ctx.cwd, timeout).await.map_err(|e| match e {
        ShimError::Timeout { timeout_sec, .. } => ShimError::Timeout {
            action: req.action.clone(),
            timeout_sec,
        },
        other => other,
    })?;
    let (ok, label) = label_for(code);
    let parsed = if ok && JSON_ACTIONS.contains(&req.action.as_str()) {
        serde_json::from_str::<Value>(&stdout).ok()
    } else {
        None
    };
    Ok(OpsResult {
        subsystem: "agentic".into(),
        action: req.action.clone(),
        exit_code: code,
        ok,
        label: label.into(),
        stdout,
        stderr,
        parsed,
    })
}
