//! Detached `/browse` tasks (`POST /api/browser/jobs`).
//!
//! A job runs the same checked request as `POST /api/browser/run`, but its
//! task holds the model gate and the child, not the HTTP request, so a closed
//! tab does not end the run. Records are owner-bound, bounded and kept in this
//! process's memory only; a restart ends the job and loses the record. The
//! task text is never stored here.

use std::sync::Mutex;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const JOB_RUNNING: &str = "running";
/// Cancel was asked for; the child has not exited yet.
pub const JOB_CANCELLING: &str = "cancelling";
pub const JOB_FINISHED: &str = "finished";
pub const JOB_FAILED: &str = "failed";
pub const JOB_CANCELLED: &str = "cancelled";
/// Retained browser jobs; the oldest ended ones are evicted first.
pub const MAX_BROWSER_JOBS: usize = 8;
/// The longest stdout line read as a progress event.
const MAX_EVENT_LINE_BYTES: usize = 512;

/// How a job's task ended, as its routes report it.
pub enum Outcome {
    /// The child exited and its report passed the delivery checks.
    Report(Value),
    /// The child could not run, timed out, or delivery was refused.
    Failed { code: String, message: String },
    /// The child did not stop in time after a cancel and was killed.
    Killed,
}

/// The way to stop a running job: a `cancel` line for the child, then a
/// token its task watches so a child that does not stop is killed.
pub struct CancelHandle {
    pub input: std::sync::mpsc::Sender<Vec<u8>>,
    pub token: CancellationToken,
}

struct BrowserJob {
    job_id: String,
    owner: String,
    origins: Vec<String>,
    status: &'static str,
    created_at: f64,
    finished_at: Option<f64>,
    turns: u64,
    result: Option<Value>,
    error: Option<Value>,
    cancel: Option<CancelHandle>,
}

impl BrowserJob {
    fn ended(&self) -> bool {
        !matches!(self.status, JOB_RUNNING | JOB_CANCELLING)
    }

    fn to_json(&self) -> Value {
        let mut job = json!({
            "job_id": self.job_id,
            "status": self.status,
            "origins": self.origins,
            "created_at": self.created_at,
            "finished_at": self.finished_at,
            "turns": self.turns,
        });
        if let Some(result) = &self.result {
            job["result"] = result.clone();
        }
        if let Some(error) = &self.error {
            job["error"] = error.clone();
        }
        job
    }
}

#[derive(Default)]
pub struct BrowserJobs(Mutex<Vec<BrowserJob>>);

impl BrowserJobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<BrowserJob>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record a started job. Jobs still running are never evicted.
    pub fn insert(&self, job_id: &str, owner: &str, origins: &[String], cancel: CancelHandle) {
        let mut jobs = self.lock();
        jobs.push(BrowserJob {
            job_id: job_id.to_string(),
            owner: owner.to_string(),
            origins: origins.to_vec(),
            status: JOB_RUNNING,
            created_at: crate::common::now_ts(),
            finished_at: None,
            turns: 0,
            result: None,
            error: None,
            cancel: Some(cancel),
        });
        while jobs.len() > MAX_BROWSER_JOBS {
            match jobs.iter().position(BrowserJob::ended) {
                Some(oldest) => drop(jobs.remove(oldest)),
                None => break,
            }
        }
    }

    /// One stdout line from the job's child. Only a short
    /// `{"event":"turn","turn":N}` line changes anything; the count only grows.
    pub fn note_line(&self, job_id: &str, line: &str) {
        if line.len() > MAX_EVENT_LINE_BYTES {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let (Some("turn"), Some(turn)) = (event["event"].as_str(), event["turn"].as_u64()) else {
            return;
        };
        if let Some(job) = self.lock().iter_mut().find(|job| job.job_id == job_id) {
            if !job.ended() {
                job.turns = job.turns.max(turn);
            }
        }
    }

    /// Ask `owner`'s job to stop. `None` for another account's job or an
    /// unknown id; otherwise the job as it now reads. A job that already ended
    /// is returned unchanged.
    pub fn cancel(&self, owner: &str, job_id: &str) -> Option<Value> {
        let mut jobs = self.lock();
        let job = jobs.iter_mut().find(|job| job.owner == owner && job.job_id == job_id)?;
        if job.status == JOB_RUNNING {
            job.status = JOB_CANCELLING;
            if let Some(cancel) = &job.cancel {
                // A child that already exited has no reader; the token still ends the task.
                let _ = cancel.input.send(b"cancel\n".to_vec());
                cancel.token.cancel();
            }
        }
        Some(job.to_json())
    }

    /// Set the terminal state, once. A second call changes nothing. A job
    /// that was asked to stop ends `cancelled` and keeps what its child reported.
    pub fn finish(&self, job_id: &str, outcome: Outcome) {
        let mut jobs = self.lock();
        let Some(job) = jobs.iter_mut().find(|job| job.job_id == job_id) else {
            return;
        };
        if job.ended() {
            return;
        }
        let cancelling = job.status == JOB_CANCELLING;
        job.finished_at = Some(crate::common::now_ts());
        // Dropping the handle closes the child's stdin.
        job.cancel = None;
        match outcome {
            Outcome::Report(report) => {
                job.status = if cancelling { JOB_CANCELLED } else { JOB_FINISHED };
                job.result = Some(report);
            }
            Outcome::Failed { code, message } => {
                job.status = if cancelling { JOB_CANCELLED } else { JOB_FAILED };
                job.error = Some(json!({"code": code, "message": message}));
            }
            Outcome::Killed => {
                job.status = JOB_CANCELLED;
                job.error = Some(json!({
                    "code": "BROWSER_CANCELLED",
                    "message": "The task did not stop in time after the cancel; its processes were killed.",
                }));
            }
        }
    }

    /// One of `owner`'s jobs, or `None` for any other account's.
    pub fn get(&self, owner: &str, job_id: &str) -> Option<Value> {
        self.lock()
            .iter()
            .find(|job| job.owner == owner && job.job_id == job_id)
            .map(BrowserJob::to_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> (CancelHandle, std::sync::mpsc::Receiver<Vec<u8>>, CancellationToken) {
        let (input, sent) = std::sync::mpsc::channel();
        let token = CancellationToken::new();
        (
            CancelHandle {
                input,
                token: token.clone(),
            },
            sent,
            token,
        )
    }

    fn start(jobs: &BrowserJobs, id: &str, owner: &str) -> (std::sync::mpsc::Receiver<Vec<u8>>, CancellationToken) {
        let (cancel, sent, token) = handle();
        jobs.insert(id, owner, &["https://example.com:443".to_string()], cancel);
        (sent, token)
    }

    #[test]
    fn a_job_is_read_only_by_its_owner_and_counts_turn_events() {
        let jobs = BrowserJobs::default();
        let _held = start(&jobs, "a", "admin-1");
        assert!(jobs.get("admin-2", "a").is_none());
        assert!(jobs.cancel("admin-2", "a").is_none());
        for line in [
            r#"{"event":"turn","turn":2}"#,
            r#"{"event":"turn","turn":1}"#,
            r#"{"event":"other","turn":9}"#,
            r#"{"finished":true,"turn":9}"#,
            "not json",
            &format!(r#"{{"event":"turn","turn":7,"pad":"{}"}}"#, "x".repeat(600)),
        ] {
            jobs.note_line("a", line);
        }
        let job = jobs.get("admin-1", "a").unwrap();
        assert_eq!(job["status"], JOB_RUNNING);
        assert_eq!(job["turns"], 2);
        assert!(job.get("task").is_none());
    }

    #[test]
    fn cancel_tells_the_child_once_and_the_end_state_is_set_once() {
        let jobs = BrowserJobs::default();
        let (sent, token) = start(&jobs, "a", "admin-1");
        assert_eq!(jobs.cancel("admin-1", "a").unwrap()["status"], JOB_CANCELLING);
        assert_eq!(jobs.cancel("admin-1", "a").unwrap()["status"], JOB_CANCELLING);
        assert_eq!(sent.try_iter().collect::<Vec<_>>(), vec![b"cancel\n".to_vec()]);
        assert!(token.is_cancelled());
        // The child's own report after a cancel is kept, under `cancelled`.
        jobs.finish("a", Outcome::Report(json!({"finished": false})));
        jobs.finish("a", Outcome::Killed);
        let job = jobs.get("admin-1", "a").unwrap();
        assert_eq!(job["status"], JOB_CANCELLED);
        assert_eq!(job["result"]["finished"], false);
        assert!(job.get("error").is_none());
        // Finishing closed the child's stdin.
        assert!(matches!(
            sent.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        // A cancel after the end changes nothing.
        assert_eq!(jobs.cancel("admin-1", "a").unwrap()["status"], JOB_CANCELLED);
    }

    #[test]
    fn outcomes_map_to_states_and_running_jobs_are_never_evicted() {
        let jobs = BrowserJobs::default();
        let _running = start(&jobs, "running", "admin-1");
        let _killed = start(&jobs, "killed", "admin-1");
        jobs.cancel("admin-1", "killed");
        jobs.finish("killed", Outcome::Killed);
        assert_eq!(
            jobs.get("admin-1", "killed").unwrap()["error"]["code"],
            "BROWSER_CANCELLED"
        );
        let mut held = Vec::new();
        for n in 0..MAX_BROWSER_JOBS + 2 {
            let id = format!("job-{n}");
            held.push(start(&jobs, &id, "admin-1"));
            let outcome = if n % 2 == 0 {
                Outcome::Report(json!({"finished": true}))
            } else {
                Outcome::Failed {
                    code: "BROWSER_TIMEOUT".into(),
                    message: "late".into(),
                }
            };
            jobs.finish(&id, outcome);
        }
        assert_eq!(jobs.lock().len(), MAX_BROWSER_JOBS);
        assert_eq!(jobs.get("admin-1", "running").unwrap()["status"], JOB_RUNNING);
        assert!(
            jobs.get("admin-1", "killed").is_none(),
            "the oldest ended job goes first"
        );
        let last = MAX_BROWSER_JOBS + 1;
        assert_eq!(
            jobs.get("admin-1", &format!("job-{last}")).unwrap()["status"],
            JOB_FAILED
        );
        assert_eq!(
            jobs.get("admin-1", &format!("job-{}", last - 1)).unwrap()["status"],
            JOB_FINISHED
        );
    }
}
