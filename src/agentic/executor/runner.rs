//! Run a fixed set of verification commands over a worktree inside a hard
//! sandbox. Port of `agentic/executor/runner.py`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};

use super::sandbox::{production_sandbox, HardSandbox, SandboxOutcome};

pub const DEFAULT_CHECK_TIMEOUT_SEC: u64 = 120;
const ALLOWED_ENV_VARS: [&str; 6] = [
    "PATH",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "PYTHONIOENCODING",
];

/// Allowlisted subset + telemetry opt-outs + explicit proxy hostility.
pub fn scrubbed_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = ALLOWED_ENV_VARS
        .iter()
        .filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v)))
        .collect();
    for (k, v) in [
        ("NO_PROXY", "*"),
        ("no_proxy", "*"),
        ("PIP_NO_INDEX", "1"),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
        ("CARGO_NET_OFFLINE", "true"),
        ("DO_NOT_TRACK", "1"),
        ("GH_TELEMETRY", "false"),
        ("HF_HUB_DISABLE_TELEMETRY", "1"),
        ("ANONYMIZED_TELEMETRY", "false"),
    ] {
        env.insert(k.into(), v.into());
    }
    env
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    pub argv: Vec<String>,
    pub timeout_sec: u64,
}

impl Check {
    pub fn new(name: &str, argv: Vec<String>) -> Result<Self> {
        Self::with_timeout(name, argv, DEFAULT_CHECK_TIMEOUT_SEC)
    }

    pub fn with_timeout(name: &str, argv: Vec<String>, timeout_sec: u64) -> Result<Self> {
        if name.is_empty() {
            return Err(HarnessError::agentic("Check.name must be non-empty"));
        }
        if argv.is_empty() {
            return Err(HarnessError::agentic(format!("Check '{name}': argv must be non-empty")));
        }
        Ok(Self {
            name: name.to_string(),
            argv,
            timeout_sec,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub exit_code: i32,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VerificationReport {
    pub ok: bool,
    pub results: Vec<CheckResult>,
}

impl VerificationReport {
    pub fn failed_names(&self) -> Vec<String> {
        self.results.iter().filter(|r| !r.ok).map(|r| r.name.clone()).collect()
    }
}

/// Run every check sequentially inside the sandbox. `sandbox` is for tests;
/// production callers pass `None` and get `production_sandbox()`.
pub fn run_verification(
    worktree: &Path,
    checks: &[Check],
    audit: &Audit,
    sandbox: Option<&dyn HardSandbox>,
) -> Result<VerificationReport> {
    if checks.is_empty() {
        return Ok(VerificationReport {
            ok: true,
            results: Vec::new(),
        });
    }
    let owned: Option<Box<dyn HardSandbox>> = if sandbox.is_none() {
        Some(production_sandbox()?)
    } else {
        None
    };
    let backend: &dyn HardSandbox = match sandbox {
        Some(s) => s,
        None => owned.as_deref().expect("production sandbox"),
    };
    let mut env = scrubbed_env();
    let home = tempfile::Builder::new().prefix("cgah-exec-home-").tempdir()?;
    env.insert("HOME".into(), home.path().display().to_string());
    env.insert("USERPROFILE".into(), home.path().display().to_string());
    let mut results = Vec::new();
    for check in checks {
        let outcome: SandboxOutcome = backend.run(&check.argv, worktree, &env, check.timeout_sec);
        let ok = outcome.exit_code == 0 && !outcome.timed_out;
        audit.log(json!({
            "event": "agentic_executor_check_result", "check": check.name, "exit_code": outcome.exit_code,
            "ok": ok, "timed_out": outcome.timed_out, "sandbox": backend.name(),
        }));
        results.push(CheckResult {
            name: check.name.clone(),
            exit_code: outcome.exit_code,
            ok,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            timed_out: outcome.timed_out,
        });
    }
    Ok(VerificationReport {
        ok: results.iter().all(|r| r.ok),
        results,
    })
}
