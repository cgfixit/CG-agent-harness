//! Background real-repo runs.
//!
//! `POST /api/agent/run` remains a synchronous API. The console submits through
//! `POST /api/agent/jobs`, which runs the same validated request in a detached
//! tokio task so a closed tab, a proxy timeout, or a slow planner cannot orphan
//! the result: the child still crosses the shim, the run and chat gates are
//! held by the task (not the request), and the outcome is kept here until it
//! is read or evicted.
//!
//! A job id is the SERVER's handle; the agentic run id (minted by the child)
//! appears inside the result once the child finishes. Cancelling aborts the
//! task, which drops the child with `kill_on_drop`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::server::errors::ApiError;

pub const RUNNING: &str = "running";
pub const FINISHED: &str = "finished";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";

/// Terminal jobs retained per process (oldest evicted first).
pub const MAX_RETAINED_JOBS: usize = 32;

pub struct Job {
    pub job_id: String,
    pub action: String,
    pub created_at: f64,
    pub finished_at: Option<f64>,
    pub status: &'static str,
    /// The body the synchronous route would have returned (200).
    pub result: Option<Value>,
    /// The error envelope the synchronous route would have returned, plus its HTTP status.
    pub error: Option<(u16, Value)>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "job_id": self.job_id,
            "action": self.action,
            "status": self.status,
            "created_at": self.created_at,
            "finished_at": self.finished_at,
        });
        if let Some(r) = &self.result {
            v["result"] = r.clone();
        }
        if let Some((code, body)) = &self.error {
            v["error"] = json!({"http_status": code, "detail": body["detail"].clone()});
        }
        v
    }
}

#[derive(Default)]
pub struct JobStore {
    inner: Mutex<BTreeMap<String, Job>>,
}

impl JobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a running job. Evicts the oldest terminal jobs beyond the cap.
    pub fn insert_running(&self, job_id: &str, action: &str, handle: JoinHandle<()>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let terminal: Vec<(f64, String)> = g
            .values()
            .filter(|j| j.status != RUNNING)
            .map(|j| (j.created_at, j.job_id.clone()))
            .collect();
        if terminal.len() >= MAX_RETAINED_JOBS {
            let mut sorted = terminal;
            sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (_, id) in sorted.iter().take(sorted.len() + 1 - MAX_RETAINED_JOBS) {
                g.remove(id);
            }
        }
        g.insert(
            job_id.to_string(),
            Job {
                job_id: job_id.to_string(),
                action: action.to_string(),
                created_at: crate::common::now_ts(),
                finished_at: None,
                status: RUNNING,
                result: None,
                error: None,
                handle: Some(handle),
            },
        );
    }

    pub fn finish(&self, job_id: &str, outcome: Result<Value, ApiError>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(j) = g.get_mut(job_id) {
            if j.status != RUNNING {
                return; // cancelled while the task was finishing: cancellation wins
            }
            j.finished_at = Some(crate::common::now_ts());
            j.handle = None;
            match outcome {
                Ok(v) => {
                    j.status = if v.get("ok").and_then(Value::as_bool) == Some(false) {
                        FAILED
                    } else {
                        FINISHED
                    };
                    j.result = Some(v);
                }
                Err(e) => {
                    j.status = FAILED;
                    j.error = Some((e.status.as_u16(), e.body()));
                }
            }
        }
    }

    pub fn get(&self, job_id: &str) -> Option<Value> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.get(job_id).map(Job::to_json)
    }

    pub fn list(&self) -> Vec<Value> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<&Job> = g.values().collect();
        v.sort_by(|a, b| {
            b.created_at
                .partial_cmp(&a.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v.into_iter()
            .map(|j| json!({"job_id": j.job_id, "action": j.action, "status": j.status, "created_at": j.created_at, "finished_at": j.finished_at}))
            .collect()
    }

    /// Abort a running job. Returns the job's new state, or None if unknown.
    pub fn cancel(&self, job_id: &str) -> Option<Value> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let j = g.get_mut(job_id)?;
        if j.status == RUNNING {
            if let Some(h) = j.handle.take() {
                h.abort();
            }
            j.status = CANCELLED;
            j.finished_at = Some(crate::common::now_ts());
        }
        Some(j.to_json())
    }

    pub fn running_count(&self) -> usize {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.values().filter(|j| j.status == RUNNING).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_handle() -> JoinHandle<()> {
        tokio::spawn(async {})
    }

    #[tokio::test]
    async fn cli_failure_is_a_failed_job_with_its_result_preserved() {
        let store = JobStore::new();
        store.insert_running("failed-child", "real-repo-run", idle_handle());
        store.finish(
            "failed-child",
            Ok(json!({"ok": false, "exit_code": 4, "stderr": "write refused"})),
        );
        let value = store.get("failed-child").unwrap();
        assert_eq!(value["status"], FAILED);
        assert_eq!(value["result"]["exit_code"], 4);
        assert_eq!(value["result"]["stderr"], "write refused");
    }

    #[tokio::test]
    async fn lifecycle_running_finished_failed_cancelled() {
        let store = JobStore::new();
        store.insert_running("a", "real-repo-run", idle_handle());
        assert_eq!(store.get("a").unwrap()["status"], RUNNING);
        assert_eq!(store.running_count(), 1);
        store.finish("a", Ok(json!({"ok": true})));
        let a = store.get("a").unwrap();
        assert_eq!(a["status"], FINISHED);
        assert_eq!(a["result"]["ok"], true);
        assert!(a["finished_at"].is_number());

        store.insert_running("b", "real-repo-run", idle_handle());
        store.finish(
            "b",
            Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "AGENTIC_DISABLED",
                "off",
            )),
        );
        let b = store.get("b").unwrap();
        assert_eq!(b["status"], FAILED);
        assert_eq!(b["error"]["http_status"], 409);
        assert_eq!(b["error"]["detail"]["code"], "AGENTIC_DISABLED");

        store.insert_running("c", "real-repo-run", tokio::spawn(std::future::pending()));
        let c = store.cancel("c").unwrap();
        assert_eq!(c["status"], CANCELLED);
        // A late finish after cancel does not resurrect the job.
        store.finish("c", Ok(json!({})));
        assert_eq!(store.get("c").unwrap()["status"], CANCELLED);
        assert!(store.cancel("nope").is_none());
        assert_eq!(store.list().len(), 3);
    }

    #[tokio::test]
    async fn terminal_jobs_are_evicted_beyond_the_cap_but_running_ones_never() {
        let store = JobStore::new();
        store.insert_running("keep-running", "real-repo-run", tokio::spawn(std::future::pending()));
        for i in 0..(MAX_RETAINED_JOBS + 5) {
            let id = format!("j{i:03}");
            store.insert_running(&id, "real-repo-run", idle_handle());
            store.finish(&id, Ok(json!({})));
        }
        let ids: Vec<String> = store
            .list()
            .iter()
            .map(|j| j["job_id"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&"keep-running".to_string()));
        assert!(ids.len() <= MAX_RETAINED_JOBS + 1);
        assert!(!ids.contains(&"j000".to_string()), "oldest terminal job evicted");
    }
}
