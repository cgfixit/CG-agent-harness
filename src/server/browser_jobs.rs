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
/// A held request is paused in the task's Chrome, waiting for its owner's verdict.
pub const JOB_WAITING: &str = "waiting";
/// Cancel was asked for; the child has not exited yet.
pub const JOB_CANCELLING: &str = "cancelling";
pub const JOB_FINISHED: &str = "finished";
pub const JOB_FAILED: &str = "failed";
pub const JOB_CANCELLED: &str = "cancelled";
/// Retained browser jobs; the oldest ended ones are evicted first.
pub const MAX_BROWSER_JOBS: usize = 8;
/// The longest stdout line read as an event. A `waiting` line carries a URL of
/// up to 8,192 characters and 2,000 characters of body, JSON-escaped.
const MAX_EVENT_LINE_BYTES: usize = 96 * 1024;
/// The longest `turn` or `resumed` line.
const MAX_SHORT_EVENT_BYTES: usize = 512;
/// What of a paused request is kept and shown: the same cuts the sidecar makes.
const MAX_SHOWN_URL_CHARS: usize = 8_192;
const MAX_SHOWN_PREVIEW_CHARS: usize = 2_000;

/// Why a verdict was not passed on.
#[derive(Debug, PartialEq)]
pub enum VerdictRefusal {
    /// No such job for this account.
    NoJob,
    /// The job has no request waiting (it was decided, expired, or never asked).
    NothingWaiting,
    /// The verdict names a different request than the one waiting.
    Stale,
}

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
    /// The paused request, as its owner is shown it.
    pending: Option<Value>,
    cancel: Option<CancelHandle>,
}

impl BrowserJob {
    fn ended(&self) -> bool {
        !matches!(self.status, JOB_RUNNING | JOB_WAITING | JOB_CANCELLING)
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
        if let Some(pending) = &self.pending {
            job["pending"] = pending.clone();
        }
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
            pending: None,
            cancel: Some(cancel),
        });
        while jobs.len() > MAX_BROWSER_JOBS {
            match jobs.iter().position(BrowserJob::ended) {
                Some(oldest) => drop(jobs.remove(oldest)),
                None => break,
            }
        }
    }

    /// One stdout line from the job's child. A short `{"event":"turn","turn":N}`
    /// line moves the turn count, which only grows. A `waiting` line records the
    /// paused request and a `resumed` line clears it. Anything else, and any
    /// line for a job that ended or is being cancelled, changes nothing.
    pub fn note_line(&self, job_id: &str, line: &str) {
        if line.len() > MAX_EVENT_LINE_BYTES {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let mut jobs = self.lock();
        let Some(job) = jobs.iter_mut().find(|job| job.job_id == job_id) else {
            return;
        };
        if !matches!(job.status, JOB_RUNNING | JOB_WAITING) {
            return;
        }
        // Only a `waiting` line has a reason to be long.
        let short = line.len() <= MAX_SHORT_EVENT_BYTES;
        match event["event"].as_str().filter(|name| short || *name == "waiting") {
            Some("turn") => {
                if let Some(turn) = event["turn"].as_u64() {
                    job.turns = job.turns.max(turn);
                }
            }
            Some("waiting") => {
                if let Some(pending) = shown_request(&event) {
                    job.pending = Some(pending);
                    job.status = JOB_WAITING;
                }
            }
            Some("resumed") => {
                job.pending = None;
                job.status = JOB_RUNNING;
            }
            _ => {}
        }
    }

    /// Pass the owner's verdict on the waiting request to the child. `request`
    /// and `sha256` must name the request that is waiting now, so an answer to
    /// an earlier question, or to a body the owner did not see, sends nothing.
    /// On success the request is no longer waiting and is returned as it was
    /// shown, for the audit line. A verdict is passed on at most once.
    pub fn verdict(
        &self,
        owner: &str,
        job_id: &str,
        (request, sha256): (u64, Option<&str>),
        allow: bool,
    ) -> Result<Value, VerdictRefusal> {
        let mut jobs = self.lock();
        let job = jobs
            .iter_mut()
            .find(|job| job.owner == owner && job.job_id == job_id)
            .ok_or(VerdictRefusal::NoJob)?;
        let (JOB_WAITING, Some(pending)) = (job.status, &job.pending) else {
            return Err(VerdictRefusal::NothingWaiting);
        };
        if pending["request"].as_u64() != Some(request)
            || sha256.is_some_and(|sha256| pending["sha256"].as_str() != Some(sha256))
        {
            return Err(VerdictRefusal::Stale);
        }
        let line = json!({"verdict": if allow { "approve" } else { "deny" }, "id": request});
        if let Some(cancel) = &job.cancel {
            // A child that already exited has no reader; its job ends by itself.
            let _ = cancel.input.send(format!("{line}\n").into_bytes());
        }
        job.status = JOB_RUNNING;
        job.pending.take().ok_or(VerdictRefusal::NothingWaiting)
    }

    /// The request waiting on `owner`'s job, if one is.
    pub fn pending(&self, owner: &str, job_id: &str) -> Result<Value, VerdictRefusal> {
        let jobs = self.lock();
        let job = jobs
            .iter()
            .find(|job| job.owner == owner && job.job_id == job_id)
            .ok_or(VerdictRefusal::NoJob)?;
        match (job.status, &job.pending) {
            (JOB_WAITING, Some(pending)) => Ok(pending.clone()),
            _ => Err(VerdictRefusal::NothingWaiting),
        }
    }

    /// Ask `owner`'s job to stop. `None` for another account's job or an
    /// unknown id; otherwise the job as it now reads. A job that already ended
    /// is returned unchanged.
    pub fn cancel(&self, owner: &str, job_id: &str) -> Option<Value> {
        let mut jobs = self.lock();
        let job = jobs.iter_mut().find(|job| job.owner == owner && job.job_id == job_id)?;
        if matches!(job.status, JOB_RUNNING | JOB_WAITING) {
            job.status = JOB_CANCELLING;
            // The child answers its own waiting request with a no.
            job.pending = None;
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
        job.pending = None;
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

/// The paused request from a child's `waiting` line, as it is stored and
/// shown: fixed fields, each of the expected type and cut to the sidecar's
/// limits. Everything but the id, size and hash is page-written text. `None`
/// when the line is not a complete question.
fn shown_request(event: &Value) -> Option<Value> {
    let question = &event["question"];
    let sha256 = question["sha256"].as_str()?;
    if !crate::common::is_lower_hex(sha256, 64) {
        return None;
    }
    let cut = |key: &str, max: usize| -> Option<String> { Some(question[key].as_str()?.chars().take(max).collect()) };
    let now = crate::common::now_ts();
    Some(json!({
        "request": question["approve"].as_u64()?,
        "method": cut("method", 16)?,
        "url": cut("url", MAX_SHOWN_URL_CHARS)?,
        "content_type": cut("content_type", 200)?,
        "size": question["size"].as_u64()?,
        "sha256": sha256,
        "preview": cut("preview", MAX_SHOWN_PREVIEW_CHARS)?,
        "asked_at": now,
        "expires_at": now + event["wait_sec"].as_u64()? as f64,
    }))
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

    fn waiting(id: u64, body_sha: &str) -> String {
        json!({"event": "waiting", "wait_sec": 120, "question": {
            "approve": id, "method": "POST", "url": "https://example.com/save", "content_type": "text/plain",
            "size": 5, "sha256": body_sha, "preview": "qty=2",
        }})
        .to_string()
    }

    /// #995: a verdict reaches the child once, only for the request that is
    /// waiting now, only from the job's owner.
    #[test]
    fn a_verdict_is_passed_on_once_and_only_for_the_waiting_request() {
        let jobs = BrowserJobs::default();
        let (sent, _token) = start(&jobs, "a", "admin-1");
        let sha = "ab".repeat(32);
        let other = "cd".repeat(32);
        assert_eq!(
            jobs.verdict("admin-1", "a", (7, Some(&sha)), true),
            Err(VerdictRefusal::NothingWaiting)
        );
        jobs.note_line("a", &waiting(7, &sha));
        let job = jobs.get("admin-1", "a").unwrap();
        assert_eq!(job["status"], JOB_WAITING);
        assert_eq!(job["pending"]["preview"], "qty=2");
        assert_eq!(job["pending"]["request"], 7);
        assert_eq!(
            jobs.verdict("admin-2", "a", (7, Some(&sha)), true),
            Err(VerdictRefusal::NoJob)
        );
        assert_eq!(
            jobs.verdict("admin-1", "a", (6, Some(&sha)), true),
            Err(VerdictRefusal::Stale)
        );
        assert_eq!(
            jobs.verdict("admin-1", "a", (7, Some(&other)), true),
            Err(VerdictRefusal::Stale)
        );
        assert!(sent.try_recv().is_err(), "a refused verdict writes nothing");
        let shown = jobs.verdict("admin-1", "a", (7, Some(&sha)), true).unwrap();
        assert_eq!(shown["url"], "https://example.com/save");
        assert_eq!(
            String::from_utf8(sent.try_recv().unwrap()).unwrap(),
            "{\"id\":7,\"verdict\":\"approve\"}\n"
        );
        assert_eq!(
            jobs.verdict("admin-1", "a", (7, Some(&sha)), true),
            Err(VerdictRefusal::NothingWaiting)
        );
        assert_eq!(jobs.get("admin-1", "a").unwrap()["status"], JOB_RUNNING);
        // A deny needs no hash; a resumed line clears a question nobody answered.
        jobs.note_line("a", &waiting(8, &sha));
        jobs.verdict("admin-1", "a", (8, None), false).unwrap();
        assert!(String::from_utf8(sent.try_recv().unwrap())
            .unwrap()
            .contains("\"deny\""));
        jobs.note_line("a", &waiting(9, &sha));
        jobs.note_line("a", r#"{"event":"resumed","id":9,"decision":"expired"}"#);
        let job = jobs.get("admin-1", "a").unwrap();
        assert!(job.get("pending").is_none() && job["status"] == JOB_RUNNING);
    }

    #[test]
    fn a_waiting_line_is_stored_only_when_complete_and_is_cut_to_the_shown_limits() {
        let jobs = BrowserJobs::default();
        let _held = start(&jobs, "a", "admin-1");
        let sha = "ab".repeat(32);
        for bad in [
            waiting(1, "not-a-hash"),
            json!({"event": "waiting", "wait_sec": 120, "question": {"approve": 1, "sha256": sha}}).to_string(),
            json!({"event": "waiting", "question": {"approve": 1}}).to_string(),
        ] {
            jobs.note_line("a", &bad);
            assert_eq!(jobs.get("admin-1", "a").unwrap()["status"], JOB_RUNNING, "{bad}");
        }
        let long = json!({"event": "waiting", "wait_sec": 30, "question": {
            "approve": 2, "method": "POST", "url": format!("https://example.com/{}", "u".repeat(9_000)),
            "content_type": "text/plain", "size": 9_000, "sha256": sha, "preview": "p".repeat(3_000), "extra": "dropped",
        }})
        .to_string();
        jobs.note_line("a", &long);
        let pending = jobs.pending("admin-1", "a").unwrap();
        assert_eq!(pending["url"].as_str().unwrap().chars().count(), MAX_SHOWN_URL_CHARS);
        assert_eq!(
            pending["preview"].as_str().unwrap().chars().count(),
            MAX_SHOWN_PREVIEW_CHARS
        );
        assert!(pending.get("extra").is_none());
        assert_eq!(
            pending["expires_at"].as_f64().unwrap() - pending["asked_at"].as_f64().unwrap(),
            30.0
        );
        // Cancel while waiting: nothing is left to approve.
        jobs.cancel("admin-1", "a");
        assert_eq!(jobs.pending("admin-1", "a"), Err(VerdictRefusal::NothingWaiting));
        assert_eq!(
            jobs.verdict("admin-1", "a", (2, Some(&sha)), true),
            Err(VerdictRefusal::NothingWaiting)
        );
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
