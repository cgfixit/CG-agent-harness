//! On-disk persistence for real-repo runs, port of `agentic/real_repo_run_store.py`.
//! One JSON record per run under `<workspace_root>/runs/<run_id>.json`.
//!
//! Lifecycle: `running -> pending_decision -> approved | rejected -> discarded`,
//! `-> exhausted -> discarded`, `-> failed -> discarded`. `discarded` only via
//! the explicit discard action.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::common::atomic::write_json_atomic;
use crate::common::errors::{HarnessError, Result};

/// Deliberately identical to `server::agent_policy::RUN_ID_PATTERN` (I6 forbids the import).
pub const RUN_ID_PATTERN: &str = r"\A[0-9a-f]{32}\z";
pub const PENDING_DECISION: &str = "pending_decision";
pub const APPROVED: &str = "approved";
const TERMINAL: [&str; 4] = ["approved", "rejected", "exhausted", "failed"];

pub fn run_id_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(RUN_ID_PATTERN).expect("static regex"))
}

pub fn new_run_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealRepoRunRecord {
    pub run_id: String,
    pub repo: String,
    pub dest: String,
    pub status: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub commit_message: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub iterations: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub pushed: bool,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub plan_sha256: Option<String>,
    #[serde(default)]
    pub acceptance_digest: Option<String>,
    #[serde(default)]
    pub acceptance_base_head: Option<String>,
    #[serde(default)]
    pub approved_commit: Option<String>,
    #[serde(default)]
    pub origin_url: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl RealRepoRunRecord {
    pub fn new(run_id: &str, repo: &str, dest: &str, status: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            repo: repo.to_string(),
            dest: dest.to_string(),
            status: status.to_string(),
            provider: None,
            branch_name: None,
            commit_message: None,
            changed_files: Vec::new(),
            iterations: 0,
            error: None,
            pushed: false,
            pr_url: None,
            plan_sha256: None,
            acceptance_digest: None,
            acceptance_base_head: None,
            approved_commit: None,
            origin_url: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

fn run_path(runs_dir: &Path, run_id: &str) -> Result<PathBuf> {
    if !run_id_re().is_match(run_id) {
        return Err(HarnessError::agentic("invalid run_id").detail("run_id", crate::common::clip_chars(run_id, 64)));
    }
    Ok(runs_dir.join(format!("{run_id}.json")))
}

pub fn save_run(runs_dir: &Path, record: &mut RealRepoRunRecord) -> Result<()> {
    record.updated_at = crate::common::iso_now();
    if record.created_at.is_empty() {
        record.created_at = record.updated_at.clone();
    }
    let path = run_path(runs_dir, &record.run_id)?;
    std::fs::create_dir_all(runs_dir)?;
    write_json_atomic(&path, &record.to_json())
        .map_err(|_| HarnessError::agentic("failed to persist run record").detail("run_id", record.run_id.clone()))
}

pub fn load_run(runs_dir: &Path, run_id: &str) -> Result<RealRepoRunRecord> {
    let path = run_path(runs_dir, run_id)?;
    if !path.is_file() {
        return Err(HarnessError::agentic("run not found").detail("run_id", run_id));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|_| HarnessError::agentic("run record is unreadable or corrupt").detail("run_id", run_id))?;
    serde_json::from_str(&text)
        .map_err(|_| HarnessError::agentic("run record is unreadable or corrupt").detail("run_id", run_id))
}

pub fn require_pending_decision(record: &RealRepoRunRecord) -> Result<()> {
    if TERMINAL.contains(&record.status.as_str()) {
        return Err(
            HarnessError::agentic(format!("run {} was already decided ({})", record.run_id, record.status))
                .detail("run_id", record.run_id.clone())
                .detail("status", record.status.clone()),
        );
    }
    if record.status != PENDING_DECISION {
        return Err(HarnessError::agentic(format!(
            "run {} is not awaiting a decision (status: {})",
            record.run_id, record.status
        ))
        .detail("run_id", record.run_id.clone())
        .detail("status", record.status.clone()));
    }
    Ok(())
}

pub fn require_approved_for_push(record: &RealRepoRunRecord) -> Result<()> {
    if record.status != APPROVED {
        return Err(HarnessError::agentic(format!(
            "run {} is not approved (status: {}); nothing to push",
            record.run_id, record.status
        )));
    }
    if record.pushed {
        return Err(HarnessError::agentic(format!(
            "run {} was already pushed",
            record.run_id
        )));
    }
    Ok(())
}

pub fn require_pushed_for_publish(record: &RealRepoRunRecord) -> Result<()> {
    if record.status != APPROVED {
        return Err(HarnessError::agentic(format!(
            "run {} is not approved (status: {}); nothing to publish",
            record.run_id, record.status
        )));
    }
    if !record.pushed {
        return Err(HarnessError::agentic(format!(
            "run {} has not been pushed; a draft PR needs its head branch on origin first",
            record.run_id
        )));
    }
    if record.pr_url.is_some() {
        return Err(
            HarnessError::agentic(format!("run {} already has a pull request", record.run_id))
                .detail("pr_url", record.pr_url.clone().unwrap_or_default()),
        );
    }
    Ok(())
}

/// Kept for the complete worker lifetime, before publishing its running record.
/// Unix can prove the owner exited without consulting an untrusted PID file.
pub fn acquire_run_lease(runs_dir: &Path, run_id: &str, create: bool) -> Result<crate::common::file_lease::FileLease> {
    let path = run_path(runs_dir, run_id)?.with_extension("lease");
    std::fs::create_dir_all(runs_dir)?;
    crate::common::file_lease::FileLease::acquire(&path, create).map_err(|_| {
        HarnessError::agentic("run ownership is live, unavailable, or predates recovery support; do not discard it")
    })
}

/// Reconcile only records with ownership evidence from this implementation.
/// Older records without a lease remain unknown; never guess they are dead.
pub fn reconcile_run(runs_dir: &Path, record: &mut RealRepoRunRecord) -> Result<()> {
    #[cfg(unix)]
    if record.status == "running" {
        if let Ok(_lease) = acquire_run_lease(runs_dir, &record.run_id, false) {
            // Completion may race the initial read. Re-read under the lease.
            *record = load_run(runs_dir, &record.run_id)?;
            if record.status == "running" {
                record.status = "interrupted".into();
                record.error = Some(
                    "Worker ownership ended before completion. Inspect retained evidence; no operation was resumed."
                        .into(),
                );
                save_run(runs_dir, record)?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (runs_dir, record);
    Ok(())
}

#[cfg(all(test, unix))]
mod recovery_tests {
    use super::*;
    #[test]
    fn recovery_requires_released_ownership_and_preserves_pending() {
        let dir = tempfile::tempdir().unwrap();
        let id = new_run_id();
        let mut record = RealRepoRunRecord::new(&id, "fixture", "fixture", "running");
        save_run(dir.path(), &mut record).unwrap();
        reconcile_run(dir.path(), &mut record).unwrap();
        assert_eq!(record.status, "running", "legacy state is unknown");
        let owner = acquire_run_lease(dir.path(), &id, true).unwrap();
        reconcile_run(dir.path(), &mut record).unwrap();
        assert_eq!(record.status, "running", "live worker must remain live");
        drop(owner);
        reconcile_run(dir.path(), &mut record).unwrap();
        assert_eq!(record.status, "interrupted");
        assert!(require_pending_decision(&record).is_err());
        assert!(require_approved_for_push(&record).is_err());
        let mut stale = record.clone();
        stale.status = "running".into();
        record.status = PENDING_DECISION.into();
        save_run(dir.path(), &mut record).unwrap();
        reconcile_run(dir.path(), &mut stale).unwrap();
        assert_eq!(stale.status, PENDING_DECISION, "racing completion must survive");
    }
}
