//! GitHub WRITE gate, port of `agentic/writer.py`. Exactly one executable op
//! (`pr_create`, always `--draft`) behind the master switch plus four gates:
//! `agentic.enabled` -> `mode == write` -> `writes_enabled` -> non-empty reason
//! -> `confirm == true`. `EXECUTION_ENABLED` ships true; the env kill switch is
//! AND-ed in (disable-only), read once per process.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};

use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};
use crate::common::identity;
use crate::common::process::{self, RunSpec};

use super::config::AgenticConfig;
use super::gh_client::{gh_env, resolve_gh};

pub const EXECUTION_ENABLED: bool = true;
pub const WRITE_DISABLE_ENV: &str = "CGAGENTHARNESS_AGENTIC_WRITE_DISABLE";
pub const DEFAULT_WRITE_TIMEOUT_SEC: u64 = 60;
const WRITE_OPS: [&str; 3] = ["pr_comment", "issue_comment", "pr_create"];
const EXECUTABLE_WRITE_OPS: [&str; 1] = ["pr_create"];

pub fn disabled_by_env() -> bool {
    static CELL: OnceLock<bool> = OnceLock::new();
    *CELL.get_or_init(|| {
        matches!(
            std::env::var(WRITE_DISABLE_ENV)
                .unwrap_or_default()
                .trim()
                .to_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Parse table for the kill switch's accepted values (exposed for tests).
pub fn env_value_disables(raw: &str) -> bool {
    matches!(raw.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

pub fn execution_enabled() -> bool {
    EXECUTION_ENABLED && !disabled_by_env()
}

fn require_int_number(op: &str, params: &Value) -> Result<String> {
    let n = params
        .get("number")
        .ok_or_else(|| HarnessError::agentic(format!("op '{op}' requires 'number' in params")).detail("op", op))?;
    n.as_i64()
        .map(|v| v.to_string())
        .ok_or_else(|| HarnessError::agentic("'number' must be an integer").detail("op", op))
}

fn require_head_branch(op: &str, params: &Value) -> Result<String> {
    let head = params.get("head").and_then(|h| h.as_str()).unwrap_or("");
    let id = identity::identity()?;
    if !id.branch_is_valid(head) {
        return Err(HarnessError::agentic(format!(
            "op '{op}' requires 'head' to be <vendor>/<topic> (allowed prefixes: {})",
            id.allowed_prefixes_help()
        ))
        .detail("op", op)
        .detail("head", crate::common::clip_chars(head, 120)));
    }
    Ok(head.to_string())
}

fn str_param(params: &Value, key: &str, default: &str) -> String {
    params.get(key).and_then(|v| v.as_str()).unwrap_or(default).to_string()
}

/// The argv a write WOULD use (display and drift comparison).
pub fn build_write_argv(op: &str, repo: &str, params: &Value, gh_bin: &str) -> Result<Vec<String>> {
    let s = |v: Vec<String>| v;
    match op {
        "pr_comment" => Ok(s(vec![
            gh_bin.into(),
            "pr".into(),
            "comment".into(),
            require_int_number(op, params)?,
            "--repo".into(),
            repo.into(),
            "--body".into(),
            str_param(params, "body", ""),
        ])),
        "issue_comment" => Ok(s(vec![
            gh_bin.into(),
            "issue".into(),
            "comment".into(),
            require_int_number(op, params)?,
            "--repo".into(),
            repo.into(),
            "--body".into(),
            str_param(params, "body", ""),
        ])),
        "pr_create" => {
            let head = require_head_branch(op, params)?;
            Ok(vec![
                gh_bin.into(),
                "pr".into(),
                "create".into(),
                "--repo".into(),
                repo.into(),
                "--head".into(),
                head,
                "--base".into(),
                str_param(params, "base", "main"),
                "--title".into(),
                str_param(params, "title", ""),
                "--body".into(),
                str_param(params, "body", ""),
                "--draft".into(),
            ])
        }
        _ => Err(HarnessError::agentic(format!("Unknown write op: '{op}'"))
            .detail("op", op)
            .detail("allowed", json!(WRITE_OPS))),
    }
}

fn refuse(audit: &Audit, msg: &str, op: &str, gate: &str, reason: &str) -> HarnessError {
    audit.log(json!({"event": "agentic_write_refused", "op": op, "gate": gate, "reason": reason}));
    HarnessError::write_refused(msg)
        .detail("op", op)
        .detail("failed_gate", gate)
}

/// The master switch plus the four gates, in order.
pub fn require_gates(cfg: &AgenticConfig, audit: &Audit, op: &str, reason: &str, confirm: bool) -> Result<()> {
    if !cfg.enabled {
        return Err(refuse(audit, "agentic.enabled is False", op, "enabled", reason));
    }
    if !WRITE_OPS.contains(&op) {
        return Err(HarnessError::agentic(format!("Unknown write op: '{op}'"))
            .detail("op", op)
            .detail("allowed", json!(WRITE_OPS)));
    }
    if !cfg.is_write_mode() {
        return Err(refuse(audit, "agentic.mode is not 'write'", op, "mode", reason));
    }
    if !cfg.writes_enabled {
        return Err(refuse(
            audit,
            "agentic.writes_enabled is False",
            op,
            "writes_enabled",
            reason,
        ));
    }
    if reason.trim().is_empty() {
        return Err(refuse(
            audit,
            "a non-empty human reason is required",
            op,
            "reason",
            reason,
        ));
    }
    if !confirm {
        return Err(refuse(
            audit,
            "explicit confirm=True is required",
            op,
            "confirm",
            reason,
        ));
    }
    Ok(())
}

/// Validate the gate and return a DRY-RUN plan. Never executes.
pub fn plan_write(
    cfg: &AgenticConfig,
    audit: &Audit,
    op: &str,
    reason: &str,
    confirm: bool,
    params: Value,
) -> Result<Value> {
    require_gates(cfg, audit, op, reason, confirm)?;
    let argv = build_write_argv(op, &cfg.repo, &params, "gh")?;
    audit.log(json!({"event": "agentic_write_dryrun", "op": op, "repo": cfg.repo, "reason": reason}));
    Ok(json!({
        "status": "dry_run_plan", "op": op, "repo": cfg.repo, "reason": reason, "params": params,
        "would_run": argv, "executed": false,
        "note": "plan only; execute_write() re-runs the full gate before performing this.",
    }))
}

/// Perform one gate-satisfied write (`pr_create` only). Re-runs every gate with a
/// FRESH `confirm`; rebuilds the argv from the plan's params and refuses drift.
pub fn execute_write(
    cfg: &AgenticConfig,
    audit: &Audit,
    plan: &Value,
    confirm: bool,
    timeout_sec: u64,
) -> Result<Value> {
    let op = plan.get("op").and_then(|o| o.as_str()).unwrap_or("").to_string();
    if !execution_enabled() {
        let why = if disabled_by_env() {
            format!("{WRITE_DISABLE_ENV} is set")
        } else {
            "EXECUTION_ENABLED is False".into()
        };
        audit.log(json!({"event": "agentic_write_execution_blocked", "op": op, "gate": "execution_enabled"}));
        return Err(
            HarnessError::write_refused(format!("Agentic write execution is disabled ({why})"))
                .detail("failed_gate", "execution_enabled")
                .detail("op", op),
        );
    }
    if op.is_empty() {
        return Err(HarnessError::agentic(
            "execute_write requires a plan dict carrying an 'op'",
        ));
    }
    let reason = plan.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
    require_gates(cfg, audit, &op, &reason, confirm)?;
    if !EXECUTABLE_WRITE_OPS.contains(&op.as_str()) {
        audit.log(json!({"event": "agentic_write_execution_blocked", "op": op, "gate": "executable_op"}));
        return Err(HarnessError::write_refused(format!(
            "write op '{op}' can be planned but not executed; executable ops are {EXECUTABLE_WRITE_OPS:?}"
        ))
        .detail("failed_gate", "executable_op")
        .detail("op", op));
    }
    if plan.get("repo").and_then(|r| r.as_str()) != Some(cfg.repo.as_str()) {
        return Err(
            HarnessError::write_refused("plan repo does not match the configured repo")
                .detail("failed_gate", "repo_match")
                .detail("op", op),
        );
    }
    let params = plan.get("params").cloned().unwrap_or(json!({}));
    let mut argv = build_write_argv(&op, &cfg.repo, &params, "gh")?;
    if let Some(declared) = plan.get("would_run").and_then(|w| w.as_array()) {
        let declared_tail: Vec<&str> = declared.iter().skip(1).filter_map(|v| v.as_str()).collect();
        let rebuilt_tail: Vec<&str> = argv.iter().skip(1).map(|s| s.as_str()).collect();
        if declared_tail != rebuilt_tail {
            return Err(HarnessError::write_refused(
                "plan would_run does not match the argv rebuilt from its own params",
            )
            .detail("failed_gate", "plan_integrity")
            .detail("op", op));
        }
    }
    let binary =
        resolve_gh().map_err(|_| HarnessError::agentic("gh binary not found on PATH").detail("op", op.clone()))?;
    argv[0] = binary.display().to_string();
    audit.log(json!({"event": "agentic_write_execute_start", "op": op, "repo": cfg.repo, "reason": reason}));
    let env = gh_env();
    let out = match process::run(RunSpec {
        argv: &argv,
        cwd: None,
        env: Some(&env),
        timeout: Duration::from_secs(timeout_sec),
        stdin: None,
    }) {
        Ok(o) => o,
        Err(process::ProcessError::Timeout { .. }) => {
            audit.log(json!({"event": "agentic_write_execute_timeout", "op": op, "repo": cfg.repo}));
            return Err(HarnessError::agentic(format!(
                "gh {op} timed out after {timeout_sec}s; the outcome is INDETERMINATE and was deliberately not retried (a retry can duplicate an accepted mutation)"
            ))
            .detail("op", op)
            .detail("indeterminate", true));
        }
        Err(process::ProcessError::Spawn(e)) => return Err(HarnessError::agentic(format!("gh could not start: {e}"))),
    };
    audit.log(json!({"event": "agentic_write_executed", "op": op, "repo": cfg.repo, "exit_code": out.status}));
    if out.status != Some(0) {
        return Err(
            HarnessError::agentic(format!("gh {op} failed with exit code {}", out.status.unwrap_or(-1)))
                .detail("op", op)
                .detail(
                    "stderr",
                    out.stderr
                        .chars()
                        .rev()
                        .take(2000)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<String>(),
                ),
        );
    }
    Ok(
        json!({"status": "executed", "op": op, "repo": cfg.repo, "reason": reason, "executed": true, "exit_code": 0, "stdout": out.stdout.trim()}),
    )
}
