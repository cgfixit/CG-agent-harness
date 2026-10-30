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
use sha2::Digest;

use super::config::AppConfig;

const AUDIT_SKIP_KEYS: [&str; 5] = ["query_hash", "timestamp", "event", "audit_prev", "audit_mac"];
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

const SECRET_LABEL: &str = "[REDACTED_SECRET]";

/// Credential values this process knows: every [`super::mcp::SECRET_ENV`]
/// variable set in its environment, plus the configured local-model keys
/// (`models.local_llm.api_key` and its fallback's), for exact-value
/// redaction. The server and the agentic child each call this on their own
/// environment, so no credential crosses the process boundary for it.
pub fn known_secret_values(cfg: &AppConfig) -> Vec<String> {
    let mut values: Vec<String> = super::mcp::SECRET_ENV
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .chain(
            ["models.local_llm.api_key", "models.local_llm.fallback.api_key"]
                .iter()
                .filter_map(|key| cfg.secret(key).ok()),
        )
        .filter(|value| !value.trim().is_empty())
        .collect();
    if let Some(serde_yaml_ng::Value::Sequence(servers)) = cfg.get("mcp.servers") {
        for server in servers {
            if let Some(value) = server.get("url").and_then(|v| v.as_str()) {
                if let Ok(secret) = super::config::resolve_secret(value) {
                    if let Ok(url) = url::Url::parse(&secret) {
                        values.extend(url.query_pairs().map(|(_, value)| value.into_owned()));
                        values.extend(url.password().map(str::to_owned));
                    }
                    values.push(secret);
                }
            }
            if let Some(env) = server.get("env").and_then(|v| v.as_mapping()) {
                for value in env.values().filter_map(|v| v.as_str()) {
                    if let Ok(secret) = super::config::resolve_secret(value) {
                        values.push(secret);
                    }
                }
            }
        }
    }
    values
}

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
            .map(|pattern| (Regex::new(pattern).expect("static regex"), SECRET_LABEL))
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
                Ok(re) => rules.push((re, SECRET_LABEL)),
                // Log the index only, never the pattern text (it is config content).
                Err(_) => tracing::warn!("privacy redaction pattern #{idx} failed to compile; skipped"),
            }
        }
        Self { rules }
    }

    /// Also redact these exact credential values. Values shorter than
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
        let literal_rules = literals
            .iter()
            .filter_map(|value| Regex::new(&regex::escape(value)).ok().map(|re| (re, SECRET_LABEL)));
        self.rules = literal_rules.chain(self.rules).collect();
        self
    }

    pub fn contains_secret(&self, text: &str) -> bool {
        self.rules
            .iter()
            .any(|(pattern, label)| *label == SECRET_LABEL && pattern.is_match(text))
    }

    pub fn contains_secret_json(&self, value: &Value) -> bool {
        match value {
            Value::String(text) => self.contains_secret(text),
            Value::Array(values) => values.iter().any(|v| self.contains_secret_json(v)),
            Value::Object(values) => values
                .iter()
                .any(|(key, v)| self.contains_secret(key) || self.contains_secret_json(v)),
            _ => false,
        }
    }

    /// Apply every rule to the original text and replace the union of their
    /// matches. Matching sequentially would let a narrow rule rewrite part of
    /// a value before a broader rule sees it (a built-in `Bearer` shape would
    /// cut a configured `Bearer\s+\S+` short and expose the suffix).
    /// Overlapping matches merge into one replacement; a secret label wins
    /// over email and IP labels. The input is borrowed back untouched when no
    /// rule matched, so the audit path allocates only for real hits.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
        for (re, replacement) in &self.rules {
            spans.extend(
                re.find_iter(text)
                    .filter(|m| !m.is_empty())
                    .map(|m| (m.start(), m.end(), *replacement)),
            );
        }
        if spans.is_empty() {
            return Cow::Borrowed(text);
        }
        spans.sort_by_key(|&(start, end, _)| (start, std::cmp::Reverse(end)));
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        let mut merged: Option<(usize, usize, &'static str)> = None;
        for (start, end, label) in spans {
            match merged.as_mut() {
                Some((_, merged_end, merged_label)) if start < *merged_end => {
                    *merged_end = (*merged_end).max(end);
                    if label == SECRET_LABEL {
                        *merged_label = SECRET_LABEL;
                    }
                }
                _ => {
                    if let Some((s, e, l)) = merged.replace((start, end, label)) {
                        out.push_str(&text[cursor..s]);
                        out.push_str(l);
                        cursor = e;
                    }
                }
            }
        }
        if let Some((s, e, l)) = merged {
            out.push_str(&text[cursor..s]);
            out.push_str(l);
            cursor = e;
        }
        out.push_str(&text[cursor..]);
        Cow::Owned(out)
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
    /// Opt-in HMAC chain (`logging.audit_hmac`). Stamped in `log`, before
    /// queueing, so chain order is submission order.
    chain: Option<Arc<Mutex<MacState>>>,
}

/// Keyed state for the per-line tamper-evidence chain. `prev` starts at the
/// genesis MAC, so even the first chained line anchors to the key.
#[derive(Debug)]
struct MacState {
    key: [u8; 32],
    prev: [u8; 32],
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
            chain: None,
        }
    }

    /// Enable the per-line HMAC chain with a 32-byte key (see
    /// [`resolve_hmac_key`] for key provisioning). The chain is stamped over
    /// the redacted record before queueing; `Audit::log` remains infallible.
    pub fn with_hmac_chain(mut self, key: [u8; 32]) -> Self {
        self.chain = Some(Arc::new(Mutex::new(MacState {
            prev: hmac_sha256(&key, AUDIT_CHAIN_GENESIS),
            key,
        })));
        self
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
        let line = if let Some(chain) = &self.chain {
            // The MAC covers the redacted, query-hashed record exactly as it
            // will sit in the file, minus the two chain fields themselves.
            // serde_json's Map is a BTreeMap here (no preserve_order), so key
            // order is sorted and this serialization is canonical.
            let canonical = match serde_json::to_string(&Value::Object(record.clone())) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("audit_log failed to serialize event: {e}");
                    return;
                }
            };
            let mut state = chain.lock().unwrap_or_else(|p| p.into_inner());
            let mut msg = Vec::with_capacity(32 + canonical.len());
            msg.extend_from_slice(&state.prev);
            msg.extend_from_slice(canonical.as_bytes());
            let mac = hmac_sha256(&state.key, &msg);
            record.insert("audit_prev".to_string(), Value::String(super::hex_lower(&state.prev)));
            record.insert("audit_mac".to_string(), Value::String(super::hex_lower(&mac)));
            state.prev = mac;
            match serde_json::to_string(&Value::Object(record)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("audit_log failed to serialize chained event: {e}");
                    return;
                }
            }
        } else {
            match serde_json::to_string(&Value::Object(record)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("audit_log failed to serialize event: {e}");
                    return;
                }
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

/// Config flag and credential-store name for the opt-in audit chain.
pub const AUDIT_HMAC_FLAG: &str = "logging.audit_hmac";
pub const AUDIT_CHAIN_KEY_NAME: &str = "AUDIT_CHAIN_KEY";
/// Domain separation for the first chained line's `audit_prev`.
const AUDIT_CHAIN_GENESIS: &[u8] = b"cgagentharness-audit-genesis";

/// RFC 2104 HMAC-SHA256 over the repo's `sha2` (the `hmac` crate on crates.io
/// pairs with sha2 0.10, this tree pins sha2 0.11, so the two passes are
/// spelled out here; validated against RFC 4231 vectors in the tests).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = sha2::Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let ih = sha2::Sha256::digest(&inner);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&ih);
    let oh = sha2::Sha256::digest(&outer);
    let mut out = [0u8; 32];
    out.copy_from_slice(&oh);
    out
}

fn hex_decode_32(value: &str) -> Option<[u8; 32]> {
    if !super::is_lower_hex(value, 64) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// Resolve (or create) the audit-chain key when `logging.audit_hmac` is the
/// literal boolean true. Never fails: a missing store, an unreadable entry,
/// or a value that is not 64 lower-hex chars disables the chain with a
/// warning — the audit sink's never-fails contract wins over tamper evidence.
pub fn resolve_hmac_key(
    cfg: &AppConfig,
    store: &dyn crate::common::credential_store::CredentialStore,
) -> Option<[u8; 32]> {
    if !cfg.flag_is_true(AUDIT_HMAC_FLAG) {
        return None;
    }
    match store.get(AUDIT_CHAIN_KEY_NAME) {
        Ok(Some(value)) => match hex_decode_32(&value) {
            Some(key) => Some(key),
            None => {
                tracing::warn!("{AUDIT_CHAIN_KEY_NAME} is not 64 lower-hex chars; audit chain disabled");
                None
            }
        },
        Ok(None) => {
            let value = super::random_hex(32);
            match store.set(AUDIT_CHAIN_KEY_NAME, &value) {
                Ok(()) => hex_decode_32(&value),
                Err(e) => {
                    tracing::warn!("cannot persist {AUDIT_CHAIN_KEY_NAME} ({e:?}); audit chain disabled");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("credential store unavailable ({e:?}); audit chain disabled");
            None
        }
    }
}

/// Outcome of [`verify_hmac_chain`]. `first_bad_line` is the 1-based JSONL
/// line where the chain broke (tampered content, swapped MAC, or a deleted
/// chained line detected via the successor's `audit_prev`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainReport {
    pub ok: bool,
    pub chained: u64,
    pub unchained: u64,
    pub first_bad_line: Option<u64>,
}

/// Verify the HMAC chain in an audit JSONL file. Only the subsequence of
/// lines carrying `audit_mac` forms the chain: lines written by processes
/// without the key (the agentic child, MCP key minting) are counted as
/// `unchained` and skipped. Deleting chained TAIL lines is not detectable —
/// each line anchors only to its predecessor; that is a deliberate scope
/// limit (cg-researcher's signed head file is not ported).
pub fn verify_hmac_chain(path: &Path, key: &[u8; 32]) -> std::io::Result<ChainReport> {
    let text = std::fs::read_to_string(path)?;
    let mut prev = hmac_sha256(key, AUDIT_CHAIN_GENESIS);
    let mut report = ChainReport {
        ok: true,
        chained: 0,
        unchained: 0,
        first_bad_line: None,
    };
    for (i, line) in text.lines().enumerate() {
        let n = (i + 1) as u64;
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            report.unchained += 1;
            continue;
        };
        let Some(obj) = value.as_object() else {
            report.unchained += 1;
            continue;
        };
        let (Some(mac_hex), Some(prev_hex)) = (
            obj.get("audit_mac").and_then(Value::as_str),
            obj.get("audit_prev").and_then(Value::as_str),
        ) else {
            report.unchained += 1;
            continue;
        };
        let Some(stored_mac) = hex_decode_32(mac_hex) else {
            report.ok = false;
            report.first_bad_line = Some(n);
            break;
        };
        let mut canonical_map = obj.clone();
        canonical_map.remove("audit_mac");
        canonical_map.remove("audit_prev");
        let canonical = serde_json::to_string(&Value::Object(canonical_map)).map_err(std::io::Error::other)?;
        let mut msg = Vec::with_capacity(32 + canonical.len());
        msg.extend_from_slice(&prev);
        msg.extend_from_slice(canonical.as_bytes());
        if stored_mac != hmac_sha256(key, &msg) || prev_hex != super::hex_lower(&prev) {
            report.ok = false;
            report.first_bad_line = Some(n);
            break;
        }
        prev = stored_mac;
        report.chained += 1;
    }
    Ok(report)
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
    /// deadline expired. One non-blocking attempt runs under the mutex;
    /// the wait itself does not.
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

    #[test]
    fn secret_presence_checks_decoded_json_without_treating_ip_addresses_as_credentials() {
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), Path::new("config.yaml")).unwrap();
        let secret = "synthetic-quoted-credential-\"-\\";
        let redactors = Redactors::from_config(&cfg).with_literal_secrets([secret]);
        assert!(redactors.contains_secret_json(&json!({"nested":[{"model":secret}]})));
        assert!(!redactors.contains_secret_json(&json!({"url":"127.0.0.1","count":5})));
    }

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
            assert!(out.contains(SECRET_LABEL), "{out}");
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
    fn a_builtin_shape_never_cuts_a_broader_configured_pattern_short() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str(
            "policy:\n  privacy:\n    redact_emails: true\n    redact_secrets_like: ['Bearer\\s+\\S+']\n",
            &dir.path().join("config.yaml"),
        )
        .unwrap();
        let redactors = Redactors::from_config(&cfg);
        assert_eq!(
            redactors.redact("Authorization: Bearer abc+DEF/ghi== next"),
            "Authorization: [REDACTED_SECRET] next"
        );
        // Overlapping secret and email matches merge; the secret label wins.
        assert_eq!(
            redactors.redact("Bearer ops@example.com tail"),
            "[REDACTED_SECRET] tail"
        );
        assert_eq!(redactors.redact("mail ops@example.com"), "mail [REDACTED_EMAIL]");
        assert!(matches!(redactors.redact("nothing here"), Cow::Borrowed(_)));
    }

    #[test]
    fn configured_environment_and_mcp_url_secrets_are_redacted() {
        let name = "CGAH_AUDIT_REFERENCE_FIXTURE";
        let secret = "synthetic-config-secret-304";
        std::env::set_var(name, secret);
        let cfg = AppConfig::from_str(
            &format!("models:\n  local_llm:\n    api_key: env:{name}\nmcp:\n  servers:\n    - env: {{TOKEN: env:{name}}}\n      url: https://example.invalid/sse?token=synthetic-query-secret-304\n"),
            Path::new("config.yaml"),
        ).unwrap();
        let redactors = Redactors::from_config(&cfg).with_literal_secrets(known_secret_values(&cfg));
        std::env::remove_var(name);
        assert_eq!(
            redactors.redact("synthetic-config-secret-304 synthetic-query-secret-304"),
            "[REDACTED_SECRET] [REDACTED_SECRET]"
        );
    }

    #[test]
    fn configured_local_model_keys_are_known_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str(
            "models:\n  local_llm:\n    api_key: primary-local-bearer-0001\n    fallback:\n      api_key: fallback-local-bearer-0002\n",
            &dir.path().join("config.yaml"),
        )
        .unwrap();
        let known = known_secret_values(&cfg);
        assert!(known.iter().any(|v| v == "primary-local-bearer-0001"));
        assert!(known.iter().any(|v| v == "fallback-local-bearer-0002"));
        let redactors = Redactors::from_config(&cfg).with_literal_secrets(known);
        assert_eq!(
            redactors.redact("a primary-local-bearer-0001 b fallback-local-bearer-0002"),
            "a [REDACTED_SECRET] b [REDACTED_SECRET]"
        );
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
        let dotted = Audit::from_home(&home, &cfg);
        assert_eq!(dotted.path(), home.join("logs/audit..old.jsonl"));
        // A doubled dot inside a name is not a parent component: the record lands.
        dotted.log(json!({"event":"dotted"}));
        dotted.flush();
        assert_eq!(written(&home.join("logs/audit..old.jsonl")).len(), 1);
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

    #[cfg(any(unix, windows))]
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

    #[cfg(any(unix, windows))]
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

    fn hex(bytes: &[u8]) -> String {
        super::super::hex_lower(bytes)
    }

    fn chained_audit(dir: &Path, key: [u8; 32]) -> (Audit, PathBuf) {
        let a = audit(dir, "{}").with_hmac_chain(key);
        let path = a.path().to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        (a, path)
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_vectors() {
        // Case 1: key 0x0b x20, data "Hi There"
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Case 2: key "Jefe", data "what do ya want for nothing?"
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Case 3: key 0xaa x20, data 0xdd x50
        let mac = hmac_sha256(&[0xaa; 20], &[0xdd; 50]);
        assert_eq!(
            hex(&mac),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        // Key longer than the block is hashed first (case 6)
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn chained_lines_verify_and_carry_the_genesis_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let key = [7u8; 32];
        let (chained, path) = chained_audit(dir.path(), key);
        let genesis = hmac_sha256(&key, AUDIT_CHAIN_GENESIS);
        chained.log(json!({"event":"one", "n":1}));
        chained.log(json!({"event":"two", "n":2}));
        chained.log(json!({"event":"three", "n":3}));
        let lines = written(&path);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["audit_prev"], hex(&genesis));
        for line in &lines {
            assert!(line["audit_mac"].as_str().unwrap().len() == 64);
        }
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(report.ok);
        assert_eq!(report.chained, 3);
        assert_eq!(report.unchained, 0);
        assert_eq!(report.first_bad_line, None);
        // Chain-off audits write no mac fields (backward compatibility). The
        // plain audit shares this test's file with the chained one, so its
        // line is the last, not the first.
        let plain = audit(dir.path(), "{}");
        plain.log(json!({"event":"plain"}));
        let plain_lines = written(plain.path());
        let last = plain_lines.last().unwrap();
        assert_eq!(last["event"], "plain");
        assert!(last.get("audit_mac").is_none());
        assert!(last.get("audit_prev").is_none());
    }

    #[test]
    fn tampering_with_a_line_breaks_the_chain_at_that_line() {
        let dir = tempfile::tempdir().unwrap();
        let key = [9u8; 32];
        let (audit, path) = chained_audit(dir.path(), key);
        audit.log(json!({"event":"one"}));
        audit.log(json!({"event":"two"}));
        audit.log(json!({"event":"three"}));
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        // Edit the middle line's payload but keep valid JSON.
        lines[1] = lines[1].replace("\"two\"", "\"tampered\"");
        std::fs::write(&path, lines.join("\n")).unwrap();
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(!report.ok);
        assert_eq!(report.first_bad_line, Some(2));
    }

    #[test]
    fn deleting_a_middle_chained_line_is_detected_by_the_successor() {
        let dir = tempfile::tempdir().unwrap();
        let key = [3u8; 32];
        let (audit, path) = chained_audit(dir.path(), key);
        audit.log(json!({"event":"one"}));
        audit.log(json!({"event":"two"}));
        audit.log(json!({"event":"three"}));
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines.remove(1);
        std::fs::write(&path, lines.join("\n")).unwrap();
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(!report.ok);
        assert_eq!(report.first_bad_line, Some(2));
    }

    #[test]
    fn unchained_interleaved_lines_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let key = [5u8; 32];
        let (audit, path) = chained_audit(dir.path(), key);
        // A second Audit without the chain (the agentic child's shape) shares
        // the same file between chained lines.
        let plain = Audit::new(
            path.clone(),
            &AppConfig::from_str("{}", &dir.path().join("c.yaml")).unwrap(),
        );
        audit.log(json!({"event":"one"}));
        plain.log(json!({"event":"child_inline"}));
        audit.log(json!({"event":"two"}));
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(report.ok);
        assert_eq!(report.chained, 2);
        assert_eq!(report.unchained, 1);
    }

    #[test]
    fn the_mac_covers_the_redacted_query_hashed_line() {
        let dir = tempfile::tempdir().unwrap();
        let key = [11u8; 32];
        let (audit, path) = chained_audit(dir.path(), key);
        let secret = format!("ghp_{}", "a1".repeat(18));
        audit.log(json!({"event":"search", "query":"my secret search words", "token": secret}));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("my secret search words"), "query must be hashed");
        assert!(!text.contains(&secret), "secret must be redacted");
        assert!(text.contains("query_hash"));
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(report.ok, "mac computed over the redacted, query-hashed record");
    }

    #[test]
    fn removing_the_chained_tail_is_not_detected_by_design() {
        let dir = tempfile::tempdir().unwrap();
        let key = [13u8; 32];
        let (audit, path) = chained_audit(dir.path(), key);
        audit.log(json!({"event":"one"}));
        audit.log(json!({"event":"two"}));
        audit.log(json!({"event":"three"}));
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines.pop();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let report = verify_hmac_chain(&path, &key).unwrap();
        assert!(report.ok, "each line anchors only to its predecessor");
        assert_eq!(report.chained, 2);
    }

    #[derive(Default)]
    struct MapStore(std::sync::Mutex<std::collections::BTreeMap<String, String>>);

    use super::super::credential_store::CredentialStore;

    impl super::super::credential_store::CredentialStore for MapStore {
        fn get(&self, name: &str) -> Result<Option<String>, super::super::credential_store::StoreError> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> Result<(), super::super::credential_store::StoreError> {
            self.0.lock().unwrap().insert(name.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), super::super::credential_store::StoreError> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    #[test]
    fn resolve_hmac_key_is_flag_gated_and_store_backed() {
        let dir = tempfile::tempdir().unwrap();
        let on = AppConfig::from_str("logging:\n  audit_hmac: true\n", &dir.path().join("c.yaml")).unwrap();
        let off = AppConfig::from_str("{}", &dir.path().join("c.yaml")).unwrap();
        // Flag off: no store access at all.
        assert!(resolve_hmac_key(&off, &MapStore::default()).is_none());
        // Flag on, empty store: a key is generated and persisted, and reads back.
        let store = MapStore::default();
        let key = resolve_hmac_key(&on, &store).expect("generated");
        let stored = store.get(AUDIT_CHAIN_KEY_NAME).unwrap().expect("stored");
        assert_eq!(stored.len(), 64);
        let again = resolve_hmac_key(&on, &store).expect("reused");
        assert_eq!(key, again, "an existing key is reused, not rotated");
        // A corrupted store value disables the chain instead of crashing.
        store.set(AUDIT_CHAIN_KEY_NAME, "not-hex").unwrap();
        assert!(resolve_hmac_key(&on, &store).is_none());
    }
}
