//! Append-only audit JSONL with recursive PII/secret redaction.
//!
//! Port of `utils/logger.py::audit_log` + `redact_sensitive`. The audit sink
//! never raises: a disk-full or serialization failure degrades to a tracing
//! warning so an already-computed response is never turned into a 500.
//!
//! The server moves appends to one writer thread (`with_writer_thread`, sized by
//! `logging.audit_queue_lines`), so a request never waits on the filesystem or
//! on the agentic child's lease. Other callers, including the short-lived
//! agentic child, append inline; the server flushes before it starts a child,
//! so the child's lines follow the server lines that authorized it.

use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use super::config::AppConfig;

const AUDIT_SKIP_KEYS: [&str; 3] = ["query_hash", "timestamp", "event"];
const DEFAULT_AUDIT_FILE: &str = "logs/audit.jsonl";
/// Default `logging.audit_queue_lines`: lines waiting for the writer thread. A
/// full queue drops the new line with a warning, the same outcome as a lease
/// that stays busy past its wait.
const QUEUE_LINES: u64 = 4096;
/// Longest a flush waits, queueing its barrier included, and longest dropping
/// the last handle waits for the writer to drain. Every queued line's own wait
/// already ends at its submission deadline, so a drain normally takes
/// milliseconds; only a stalled filesystem call runs into this bound.
const FLUSH_WAIT: Duration = Duration::from_secs(10);
/// How often a flush retries a full queue.
const FLUSH_RETRY: Duration = Duration::from_millis(5);

/// Secret shapes redacted whatever `policy.privacy.redact_secrets_like` says.
/// That list only adds patterns: a home seeded before a shape was known, or
/// one with no `policy.privacy` section, still redacts these.
const BUILTIN_SECRET_PATTERNS: [&str; 9] = [
    r"Bearer\s+[A-Za-z0-9\-_.]+",
    r#"[Aa][Pp][Ii][_-]?[Kk][Ee][Yy]["'\s]*[:=]["'\s]*[\w\-.]{4,}"#,
    "AKIA[0-9A-Z]{16}",
    "xox[baprs]-[0-9a-zA-Z-]+",
    // Classic (ghp_), OAuth (gho_), user-to-server (ghu_), server (ghs_)
    // and refresh (ghr_) tokens, then fine-grained personal access tokens.
    "gh[pousr]_[A-Za-z0-9]{36,}",
    "github_pat_[A-Za-z0-9_]{22,}",
    "sk-[a-zA-Z0-9]{20,}",
    "sk-ant-[a-zA-Z0-9_-]{20,}",
    "xai-[a-zA-Z0-9]{20,}",
];

/// Shortest loaded credential redacted by exact value. Shorter strings are
/// too likely to occur in ordinary text to replace safely.
const MIN_LITERAL_SECRET_CHARS: usize = 12;

#[derive(Clone, Default)]
pub struct Redactors {
    rules: Vec<(Regex, &'static str)>,
}

// Literal rules embed credential values; Debug must never print a pattern.
impl std::fmt::Debug for Redactors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactors").field("rules", &self.rules.len()).finish()
    }
}

impl Redactors {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut rules: Vec<(Regex, &'static str)> = BUILTIN_SECRET_PATTERNS
            .iter()
            .map(|pattern| (Regex::new(pattern).expect("static regex"), "[REDACTED_SECRET]"))
            .collect();
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
            if BUILTIN_SECRET_PATTERNS.contains(&pattern.as_str()) {
                continue;
            }
            match Regex::new(pattern) {
                Ok(re) => rules.push((re, "[REDACTED_SECRET]")),
                // Log the index only, never the pattern text (it is config content).
                Err(_) => tracing::warn!("privacy redaction pattern #{idx} failed to compile; skipped"),
            }
        }
        Self { rules }
    }

    /// Also redact these exact credential values, ahead of every pattern so
    /// no partial pattern match can split one first. Values shorter than
    /// `MIN_LITERAL_SECRET_CHARS` are skipped.
    pub fn with_literal_secrets<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut literals: Vec<String> = values
            .into_iter()
            .map(|value| value.as_ref().trim().to_string())
            .filter(|value| value.chars().count() >= MIN_LITERAL_SECRET_CHARS)
            .collect();
        // Longest first, so a value that contains another is replaced whole.
        literals.sort_by_key(|value| std::cmp::Reverse(value.len()));
        literals.dedup();
        let literal_rules = literals.iter().filter_map(|value| {
            Regex::new(&regex::escape(value))
                .ok()
                .map(|re| (re, "[REDACTED_SECRET]"))
        });
        self.rules = literal_rules.chain(self.rules).collect();
        self
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
    queue_lines: usize,
    flush_wait: Duration,
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
    /// Barriers: answered once every earlier line is written or refused.
    Flush(mpsc::Sender<()>),
    FlushAsync(tokio::sync::oneshot::Sender<()>),
}

fn write_jobs(sink: Arc<Sink>, jobs: Receiver<Job>) {
    for job in jobs {
        match job {
            Job::Line { path, line, deadline } => sink.append_logged(&path, &line, deadline),
            Job::Flush(done) => {
                let _ = done.send(());
            }
            Job::FlushAsync(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// `logging.audit_queue_lines`; 0 keeps appends inline.
fn queue_lines(cfg: &AppConfig) -> usize {
    match cfg.u64_or("logging.audit_queue_lines", QUEUE_LINES) {
        0 => 0,
        lines => lines.clamp(64, 65_536) as usize,
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
            queue_lines: queue_lines(cfg),
            flush_wait: FLUSH_WAIT,
            writer: None,
        }
    }

    /// See [`Redactors::with_literal_secrets`].
    pub fn with_literal_secrets<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.redactors = std::mem::take(&mut self.redactors).with_literal_secrets(values);
        self
    }

    /// Append from a dedicated thread instead of the caller's, unless
    /// `logging.audit_queue_lines` is 0. `log` then only redacts, serializes
    /// and queues. One thread keeps every line in submission order, and each
    /// line keeps the lease deadline it was submitted with.
    pub fn with_writer_thread(self) -> Self {
        match self.queue_lines {
            0 => self,
            lines => self.with_queue(lines),
        }
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

    fn queue(&self) -> Option<&SyncSender<Job>> {
        self.writer.as_ref().and_then(|w| w.queue.as_ref())
    }

    /// Wait until every line submitted before this call has been written or
    /// refused, for at most `FLUSH_WAIT` in all. An inline audit has nothing
    /// pending and returns at once.
    pub fn flush(&self) {
        let Some(queue) = self.queue() else {
            return;
        };
        let deadline = Instant::now() + self.flush_wait;
        let (done, drained) = mpsc::channel();
        let mut barrier = Job::Flush(done);
        // `send` would wait on a full queue with no deadline; retry instead.
        loop {
            match queue.try_send(barrier) {
                Ok(()) => break,
                Err(TrySendError::Full(job)) if Instant::now() < deadline => {
                    barrier = job;
                    std::thread::sleep(FLUSH_RETRY);
                }
                // Still full at the deadline, or the writer is gone.
                Err(_) => return,
            }
        }
        let _ = drained.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    }

    /// `flush` for async callers: the same barrier and bound, without blocking
    /// a runtime worker.
    pub async fn flush_async(&self) {
        let Some(queue) = self.queue() else {
            return;
        };
        let deadline = Instant::now() + self.flush_wait;
        let (done, drained) = tokio::sync::oneshot::channel();
        let mut barrier = Job::FlushAsync(done);
        loop {
            match queue.try_send(barrier) {
                Ok(()) => break,
                Err(TrySendError::Full(job)) if Instant::now() < deadline => {
                    barrier = job;
                    tokio::time::sleep(FLUSH_RETRY).await;
                }
                Err(_) => return,
            }
        }
        let _ = tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), drained).await;
    }

    /// Resolve `logging.audit_file` against the home directory.
    pub fn from_home(home: &Path, cfg: &AppConfig) -> Self {
        let configured = cfg.str_or("logging.audit_file", DEFAULT_AUDIT_FILE);
        let path = PathBuf::from(&configured);
        let path = if configured.is_empty()
            || path.is_absolute()
            // Windows `\x` and `C:x` are not absolute, yet `join` lets their
            // root or prefix replace the home; only plain names may remain.
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            tracing::warn!("logging.audit_file must stay home-relative; using default");
            home.join(DEFAULT_AUDIT_FILE)
        } else {
            home.join(path)
        };
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
            _ => {
                tracing::warn!("audit_log called with a non-object event");
                return;
            }
        };
        if let Some(query) = record.remove("query") {
            if self.include_query_hash {
                if let Some(q) = query.as_str() {
                    record.insert("query_hash".to_string(), Value::String(super::sha256_hex(q)));
                }
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
        let Some(queue) = self.queue() else {
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
            Err(TrySendError::Disconnected(_)) => {}
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
    /// ends at its own submission deadline, but a stalled filesystem call has
    /// none: after `FLUSH_WAIT` the writer is left behind with what it holds.
    fn drop(&mut self) {
        let Some(thread) = self.writer.take().and_then(|mut writer| {
            drop(writer.queue.take());
            writer.thread.take()
        }) else {
            return;
        };
        let deadline = Instant::now() + self.flush_wait;
        while !thread.is_finished() {
            if Instant::now() >= deadline {
                tracing::warn!("audit writer still busy at shutdown; lines it holds may be lost");
                return;
            }
            std::thread::sleep(FLUSH_RETRY);
        }
        let _ = thread.join();
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
    fn builtin_secret_shapes_are_redacted_without_a_privacy_section() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", &dir.path().join("config.yaml")).unwrap();
        let redactors = Redactors::from_config(&cfg);
        for secret in [
            format!("ghp_{}", "a1".repeat(18)),
            format!("gho_{}", "b2".repeat(18)),
            format!("ghs_{}", "c3".repeat(18)),
            format!("ghu_{}", "d4".repeat(18)),
            format!("ghr_{}", "e5".repeat(18)),
            format!("github_pat_{}_{}", "11ABCDEFG0123456789xyz", "Z".repeat(59)),
            format!("sk-ant-api03-{}", "f".repeat(40)),
            format!("xai-{}", "g".repeat(40)),
            "Authorization: Bearer abc.def-ghi_jkl".to_string(),
            "AKIAABCDEFGHIJKLMNOP".to_string(),
        ] {
            let line = format!("before {secret} after");
            let out = redactors.redact(&line);
            assert!(out.contains("[REDACTED_SECRET]"), "{out}");
            let token = secret.rsplit(' ').next().unwrap();
            assert!(!out.contains(token), "{out}");
        }
        // Configured patterns still add to the floor.
        let cfg = AppConfig::from_str(
            "policy:\n  privacy:\n    redact_secrets_like: ['corp-[0-9]{6}']\n",
            &dir.path().join("config.yaml"),
        )
        .unwrap();
        let out = Redactors::from_config(&cfg)
            .redact("corp-123456 and ghp_0123456789abcdef0123456789abcdef0123")
            .into_owned();
        assert_eq!(out, "[REDACTED_SECRET] and [REDACTED_SECRET]");
    }

    #[test]
    fn loaded_credentials_are_redacted_by_exact_value_and_never_debug_printed() {
        let dir = tempfile::tempdir().unwrap();
        let serpapi = "0123456789abcdef".repeat(4);
        let custom = "local-llm token with spaces";
        let audit = audit(dir.path(), "{}").with_literal_secrets([serpapi.as_str(), custom, "short-one", ""]);
        let out = audit.redact(&format!("q={serpapi}&x=1 {custom}! short-one"));
        assert_eq!(out, "q=[REDACTED_SECRET]&x=1 [REDACTED_SECRET]! short-one");
        audit.log(json!({"event":"probe","detail":format!("key {serpapi}")}));
        let lines = written(&dir.path().join("logs/audit.jsonl"));
        assert_eq!(lines[0]["detail"], "key [REDACTED_SECRET]");
        // Never echo the formatted value: on failure it would hold a secret.
        let shown = format!("{audit:?}");
        assert!(
            !shown.contains(&serpapi) && !shown.contains(custom),
            "Redactors Debug output contained a literal secret"
        );
    }

    #[test]
    fn disabling_query_hash_does_not_store_the_raw_query() {
        let dir = tempfile::tempdir().unwrap();
        let without_hash = audit(dir.path(), "logging:\n  audit_fields:\n    include_query_hash: false\n");
        without_hash.log(json!({"event":"search","query":"PRIVATE QUERY TEXT"}));
        let record = &written(without_hash.path())[0];
        assert!(record.get("query").is_none(), "{record}");
        assert!(record.get("query_hash").is_none(), "{record}");

        let default = audit(dir.path(), "logging: {}\n");
        default.log(json!({"event":"search","query":"PRIVATE QUERY TEXT"}));
        let hashed = &written(default.path())[1];
        assert!(hashed.get("query").is_none(), "{hashed}");
        assert_eq!(
            hashed["query_hash"],
            json!(super::super::sha256_hex("PRIVATE QUERY TEXT"))
        );
    }

    #[test]
    fn audit_file_rejects_absolute_and_parent_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let outside = dir.path().join("outside.jsonl");
        for configured in [outside.display().to_string(), "../outside.jsonl".into()] {
            let yaml = format!("logging:\n  audit_file: {configured:?}\n");
            let cfg = AppConfig::from_str(&yaml, &home.join("config.yaml")).unwrap();
            let audit = Audit::from_home(&home, &cfg);
            assert_eq!(audit.path(), home.join("logs/audit.jsonl"));
            audit.log(json!({"event":"inside"}));
        }
        assert_eq!(written(&home.join("logs/audit.jsonl")).len(), 2);
        assert!(!outside.exists());

        #[cfg(windows)]
        for configured in [r"C:outside.jsonl", r"\outside.jsonl"] {
            let yaml = format!("logging:\n  audit_file: {configured:?}\n");
            let cfg = AppConfig::from_str(&yaml, &home.join("config.yaml")).unwrap();
            assert_eq!(Audit::from_home(&home, &cfg).path(), home.join("logs/audit.jsonl"));
        }

        let cfg = AppConfig::from_str(
            "logging:\n  audit_file: logs/audit..old.jsonl\n",
            &home.join("config.yaml"),
        )
        .unwrap();
        assert_eq!(Audit::from_home(&home, &cfg).path(), home.join("logs/audit..old.jsonl"));
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

    #[test]
    fn flush_and_drop_stop_waiting_for_a_stalled_writer_at_their_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let mut audit = audit(dir.path(), "logging: {}\n").with_queue(1);
        audit.flush_wait = Duration::from_millis(200);
        let audit = Arc::new(audit);
        // The writer stalls inside its first append, as on a hung filesystem,
        // and one more line fills the queue behind it.
        let sink = audit.sink.clone();
        let stalled = sink.lock.lock().unwrap();
        let queue = audit.queue().unwrap();
        let mut queued = 0;
        let started = Instant::now();
        while queued < 2 && started.elapsed() < Duration::from_secs(5) {
            let line = Job::Line {
                path: audit.path().to_path_buf(),
                line: format!("{{\"n\":{queued}}}"),
                deadline: Instant::now() + Duration::from_secs(5),
            };
            match queue.try_send(line) {
                Ok(()) => queued += 1,
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        assert_eq!(queued, 2);
        let (done, finished) = mpsc::channel();
        let flushing = audit.clone();
        std::thread::spawn(move || {
            flushing.flush();
            drop(flushing);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(3)).is_ok(),
            "flush waited on the full queue past its deadline"
        );
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            drop(Arc::try_unwrap(audit).unwrap());
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(3)).is_ok(),
            "drop waited on the stalled writer past its deadline"
        );
        // Released, the writer still lands what it held.
        drop(stalled);
        let path = dir.path().join("logs/audit.jsonl");
        let started = Instant::now();
        while written(&path).len() < 2 && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(written(&path).len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn flush_async_returns_once_every_earlier_line_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let audit = audit(dir.path(), "logging:\n  audit_lease_wait_ms: 5000\n").with_writer_thread();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let held =
            super::super::file_lease::FileLease::acquire(&dir.path().join("logs/audit.jsonl.lock"), true).unwrap();
        audit.log(json!({"event":"before"}));
        let flushed = audit.flush_async();
        tokio::pin!(flushed);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut flushed)
                .await
                .is_err(),
            "the barrier passed a line the writer still holds"
        );
        drop(held);
        flushed.await;
        assert_eq!(written(audit.path()).last().unwrap()["event"], "before");
    }

    #[test]
    fn the_queue_size_comes_from_logging_audit_queue_lines() {
        let dir = tempfile::tempdir().unwrap();
        for (yaml, lines) in [
            ("logging: {}\n", 4096),
            ("logging:\n  audit_queue_lines: 1\n", 64),
            ("logging:\n  audit_queue_lines: 10000000\n", 65_536),
            ("logging:\n  audit_queue_lines: \"8\"\n", 4096),
            ("logging:\n  audit_queue_lines: 0\n", 0),
        ] {
            assert_eq!(audit(dir.path(), yaml).queue_lines, lines, "{yaml}");
        }
        // 0 keeps appends inline: the line is on disk when `log` returns.
        let inline = audit(dir.path(), "logging:\n  audit_queue_lines: 0\n").with_writer_thread();
        assert!(inline.writer.is_none());
        inline.log(json!({"event":"inline"}));
        assert_eq!(written(inline.path())[0]["event"], "inline");
    }
}
