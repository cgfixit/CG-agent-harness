//! Append-only audit JSONL with recursive PII/secret redaction.
//!
//! Port of `utils/logger.py::audit_log` + `redact_sensitive`. The audit sink
//! never raises: a disk-full or serialization failure degrades to a tracing
//! warning so an already-computed response is never turned into a 500.
//!
//! The server moves appends to one writer thread (`with_writer_thread`), so a
//! request never waits on the filesystem or on the agentic child's lease. Other
//! callers, including the short-lived agentic child, append inline.

use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use super::config::AppConfig;

const AUDIT_SKIP_KEYS: [&str; 3] = ["query_hash", "timestamp", "event"];
/// Lines waiting for the writer thread. A full queue drops the new line with a
/// warning, the same outcome as a lease that stays busy past its wait.
const QUEUE_LINES: usize = 4096;
/// Longest `flush` waits. Every queued line's own wait already ends at its
/// submission deadline, so a drain normally takes milliseconds.
const FLUSH_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct Redactors {
    rules: Vec<(Regex, &'static str)>,
}

impl Redactors {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut rules: Vec<(Regex, &'static str)> = Vec::new();
        if cfg.flag_is_true("policy.privacy.redact_emails") {
            rules.push((
                Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").expect("static regex"),
                "[REDACTED_EMAIL]",
            ));
        }
        if cfg.flag_is_true("policy.privacy.redact_ips") {
            rules.push((
                Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").expect("static regex"),
                "[REDACTED_IP]",
            ));
        }
        for (idx, pattern) in cfg.str_list("policy.privacy.redact_secrets_like").iter().enumerate() {
            match Regex::new(pattern) {
                Ok(re) => rules.push((re, "[REDACTED_SECRET]")),
                // Log the index only, never the pattern text (it is config content).
                Err(e) => tracing::warn!("privacy redaction pattern #{idx} failed to compile ({e}); skipped"),
            }
        }
        Self { rules }
    }

    /// Apply every rule. The input is borrowed back untouched when no rule
    /// matched, so the per-request audit path allocates only for real hits
    /// instead of once per rule per string.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut owned: Option<String> = None;
        for (re, replacement) in &self.rules {
            let replaced = match re.replace_all(owned.as_deref().unwrap_or(text), *replacement) {
                Cow::Owned(s) => Some(s),
                Cow::Borrowed(_) => None,
            };
            if let Some(s) = replaced {
                owned = Some(s);
            }
        }
        match owned {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(text),
        }
    }

    pub fn redact_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.redact(s).into_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.redact_value(v)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), self.redact_value(v))).collect()),
            other => other.clone(),
        }
    }
}

/// The audit sink: one JSONL file, a lock, and the compiled redactors.
#[derive(Debug)]
pub struct Audit {
    path: PathBuf,
    redactors: Redactors,
    include_query_hash: bool,
    sink: Arc<Sink>,
    writer: Option<Writer>,
}

/// The appending half, shared with the writer thread.
#[derive(Debug)]
struct Sink {
    retention: super::bounded_log::Retention,
    lock: Mutex<()>,
}

/// A dedicated thread that performs every append in submission order.
#[derive(Debug)]
struct Writer {
    queue: Option<SyncSender<Job>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug)]
enum Job {
    Line {
        path: PathBuf,
        line: String,
        deadline: Instant,
    },
    Flush(mpsc::Sender<()>),
}

fn write_jobs(sink: Arc<Sink>, jobs: Receiver<Job>) {
    for job in jobs {
        match job {
            Job::Line { path, line, deadline } => sink.append_logged(&path, &line, deadline),
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

impl Audit {
    pub fn new(path: PathBuf, cfg: &AppConfig) -> Self {
        let include_query_hash = cfg.bool_opt("logging.audit_fields.include_query_hash").unwrap_or(true);
        Self {
            path,
            redactors: Redactors::from_config(cfg),
            include_query_hash,
            sink: Arc::new(Sink {
                retention: super::bounded_log::retention(cfg),
                lock: Mutex::new(()),
            }),
            writer: None,
        }
    }

    /// Append from a dedicated thread instead of the caller's. `log` then only
    /// redacts, serializes and queues. One thread keeps every line in
    /// submission order, and each line keeps the lease deadline it was
    /// submitted with.
    pub fn with_writer_thread(self) -> Self {
        self.with_queue(QUEUE_LINES)
    }

    fn with_queue(mut self, lines: usize) -> Self {
        let (queue, jobs) = mpsc::sync_channel(lines);
        let sink = self.sink.clone();
        match std::thread::Builder::new()
            .name("audit-writer".into())
            .spawn(move || write_jobs(sink, jobs))
        {
            Ok(thread) => {
                self.writer = Some(Writer {
                    queue: Some(queue),
                    thread: Some(thread),
                })
            }
            Err(e) => tracing::warn!("audit writer thread unavailable ({e}); appending inline"),
        }
        self
    }

    /// Wait until every line submitted before this call has been written or
    /// refused. An inline audit has nothing pending and returns at once.
    pub fn flush(&self) {
        let Some(queue) = self.writer.as_ref().and_then(|w| w.queue.as_ref()) else {
            return;
        };
        let (done, drained) = mpsc::channel();
        if queue.send(Job::Flush(done)).is_ok() {
            let _ = drained.recv_timeout(FLUSH_WAIT);
        }
    }

    /// Resolve `logging.audit_file` against the home directory.
    pub fn from_home(home: &Path, cfg: &AppConfig) -> Self {
        let configured = cfg.str_or("logging.audit_file", "logs/audit.jsonl");
        let path = PathBuf::from(&configured);
        let path = if path.is_absolute() { path } else { home.join(path) };
        Self::new(path, cfg)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn redactors(&self) -> &Redactors {
        &self.redactors
    }

    pub fn redact(&self, text: &str) -> String {
        self.redactors.redact(text).into_owned()
    }

    /// Append one redacted, timestamped record. Never fails.
    pub fn log(&self, event: Value) {
        let mut record = match event {
            Value::Object(map) => map,
            other => {
                tracing::warn!("audit_log called with a non-object event: {other}");
                return;
            }
        };
        if let Some(query) = record.remove("query") {
            if self.include_query_hash {
                if let Some(q) = query.as_str() {
                    record.insert("query_hash".to_string(), Value::String(super::sha256_hex(q)));
                }
            } else {
                record.insert("query".to_string(), query);
            }
        }
        let keys: Vec<String> = record.keys().cloned().collect();
        for key in keys {
            if AUDIT_SKIP_KEYS.contains(&key.as_str()) {
                continue;
            }
            if let Some(v) = record.get(&key) {
                let redacted = self.redactors.redact_value(v);
                record.insert(key, redacted);
            }
        }
        record.insert("timestamp".to_string(), Value::String(super::iso_now()));
        let line = match serde_json::to_string(&Value::Object(record)) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("audit_log failed to serialize event: {e}");
                return;
            }
        };
        // Deadline is stamped at submission, before any in-process wait, so
        // callers that pile up behind a busy sink share one bound.
        let deadline = Instant::now() + self.sink.retention.lease_wait;
        let Some(queue) = self.writer.as_ref().and_then(|w| w.queue.as_ref()) else {
            return self.sink.append_logged(&self.path, &line, deadline);
        };
        match queue.try_send(Job::Line {
            path: self.path.clone(),
            line,
            deadline,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => tracing::warn!("audit queue full; record dropped"),
            // The writer thread is gone (it never panics on I/O): append inline.
            Err(TrySendError::Disconnected(Job::Line { path, line, deadline })) => {
                self.sink.append_logged(&path, &line, deadline)
            }
            Err(TrySendError::Disconnected(Job::Flush(_))) => {}
        }
    }

    pub fn append_spend(&self, path: &Path, record: &Value) {
        let deadline = Instant::now() + self.sink.retention.lease_wait;
        if self.sink.append(path, &record.to_string(), deadline).is_err() {
            tracing::warn!("spend sink unavailable, busy, or record exceeds retention bound");
        }
    }
}

impl Drop for Audit {
    /// Close the queue and let the writer drain it. Each queued line's wait
    /// ends at its own submission deadline, so the join is bounded.
    fn drop(&mut self) {
        if let Some(mut writer) = self.writer.take() {
            drop(writer.queue.take());
            if let Some(thread) = writer.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Sink {
    fn append_logged(&self, path: &Path, line: &str, deadline: Instant) {
        if self.append(path, line, deadline).is_err() {
            tracing::warn!("audit sink unavailable, busy, or record exceeds retention bound");
        }
    }

    /// Retry a busy lease *outside* `self.lock`. Sleeping under that mutex
    /// serialized every `/api/*` `portal.request` behind the waiter, so the
    /// nth caller both stalled and then dropped the line when the shared
    /// deadline expired. One non-blocking attempt runs under the mutex
    /// (Windows has no flock); the wait itself does not.
    fn append(&self, path: &Path, line: &str, deadline: Instant) -> std::io::Result<()> {
        let retry = self.retention.lease_retry;
        loop {
            {
                let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
                // `append` is the non-blocking path (busy lease → WouldBlock).
                match super::bounded_log::append(path, line, self.retention.max_bytes) {
                    Ok(()) => return Ok(()),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(std::io::Error::from(ErrorKind::WouldBlock));
            }
            std::thread::sleep(retry.min(deadline.saturating_duration_since(now)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn audit(dir: &Path, yaml: &str) -> Audit {
        let cfg = AppConfig::from_str(yaml, &dir.join("config.yaml")).unwrap();
        Audit::new(dir.join("logs/audit.jsonl"), &cfg)
    }

    fn written(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_writer_thread_keeps_each_senders_order_and_flush_and_drop_drain_it() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(audit(dir.path(), "logging: {}\n").with_writer_thread());
        let senders: Vec<_> = (0..4)
            .map(|thread| {
                let audit = audit.clone();
                std::thread::spawn(move || {
                    for n in 0..50 {
                        audit.log(json!({"event":"ordered","thread":thread,"n":n}));
                    }
                })
            })
            .collect();
        for sender in senders {
            sender.join().unwrap();
        }
        audit.flush();
        let lines = written(audit.path());
        assert_eq!(lines.len(), 200);
        for thread in 0..4 {
            let order: Vec<u64> = lines
                .iter()
                .filter(|line| line["thread"] == thread)
                .map(|line| line["n"].as_u64().unwrap())
                .collect();
            assert_eq!(order, (0..50).collect::<Vec<u64>>());
        }
        // Dropping the last handle drains what is still queued.
        audit.log(json!({"event":"last"}));
        let path = audit.path().to_path_buf();
        drop(Arc::try_unwrap(audit).unwrap());
        assert_eq!(written(&path).last().unwrap()["event"], "last");
    }

    #[cfg(unix)]
    #[test]
    fn a_busy_lease_never_blocks_log_and_a_full_queue_drops_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        // Long enough that an inline append would visibly wait for the lease.
        let audit = audit(dir.path(), "logging:\n  audit_lease_wait_ms: 2000\n").with_queue(1);
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let held =
            super::super::file_lease::FileLease::acquire(&dir.path().join("logs/audit.jsonl.lock"), true).unwrap();
        let started = Instant::now();
        for n in 0..50 {
            audit.log(json!({"event":"busy","n":n}));
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "log waited for the lease"
        );
        drop(held);
        audit.flush();
        // At most one line in the writer's hands and one queued; the rest were
        // dropped, and the first submitted line always lands.
        let lines = written(audit.path());
        assert!((1..=2).contains(&lines.len()), "{lines:?}");
        assert_eq!(lines[0]["n"], 0);
    }
}
