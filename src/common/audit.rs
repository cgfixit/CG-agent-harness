//! Append-only audit JSONL with recursive PII/secret redaction.
//!
//! Port of `utils/logger.py::audit_log` + `redact_sensitive`. A record that
//! cannot be queued or written is never only a tracing warning. [`Audit::log`]
//! counts it in a gap file beside the audit log (`<path>.gaps`) that
//! [`Audit::gap_status`] reports and the next process loads. [`Audit::log_required`]
//! returns [`AuditRefused`] so a security commit can stop before it lands.
//! Neither path waits on a full queue.
//!
//! The server moves appends to one writer thread (`with_writer_thread`, sized by
//! `logging.audit_queue_lines`), so a request never waits on the filesystem or
//! on the agentic child's lease. Other callers, including the short-lived
//! agentic child, append inline; the server flushes before it starts a child,
//! so the child's lines follow the server lines that authorized it. A refused
//! inline append is counted the same way.

use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use super::config::AppConfig;

const AUDIT_SKIP_KEYS: [&str; 3] = ["query_hash", "timestamp", "event"];
const DEFAULT_AUDIT_FILE: &str = "logs/audit.jsonl";
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
const PII_LABEL: &str = "[REDACTED_PII]";

/// `policy.privacy.redact_financial`: personal and financial numbers, each
/// confirmed by its own check so ordinary numbers stay. Best effort, not a
/// complete PII detector. A rule with a capture group redacts only the group,
/// so a label such as "routing number" stays readable. These rules never make
/// [`Redactors::contains_secret`] true: they redact, they do not refuse input.
/// A match check for one checked redaction rule.
type Check = fn(&str) -> bool;
/// A checked rule: pattern, label, check.
type CheckedRule = (Regex, &'static str, Check);

const FINANCIAL_PATTERNS: [(&str, Check); 5] = [
    // Payment card numbers: 13–19 digits, optional single spaces or dashes, Luhn-valid.
    (r"\b(?:\d[ -]?){12,18}\d\b", card_number_valid),
    // IBANs: country code, check digits, 11–30 more characters, mod-97 valid.
    (
        r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,3})?\b",
        iban_valid,
    ),
    // US ABA routing numbers only beside their label, checksum-valid.
    (
        r"(?i)\b(?:routing|aba|rtn)\b[^0-9\n]{0,24}(\d{9})\b",
        routing_number_valid,
    ),
    // Bank account numbers only beside their label.
    (
        r"(?i)\b(?:account|acct)\b(?:\s*(?:number|num|no\.?|#))?\s*[:#]?\s*(\d{6,17})\b",
        any_match,
    ),
    // US Social Security numbers in their dashed form, valid area/group/serial.
    (r"\b\d{3}-\d{2}-\d{4}\b", ssn_valid),
];

fn any_match(_: &str) -> bool {
    true
}

fn digits(text: &str) -> Vec<u32> {
    text.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn card_number_valid(text: &str) -> bool {
    let digits = digits(text);
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                if d * 2 > 9 {
                    d * 2 - 9
                } else {
                    d * 2
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

fn iban_valid(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&compact.len()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut remainder: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let value = match c.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        for digit in value.to_string().chars().filter_map(|d| d.to_digit(10)) {
            remainder = (remainder * 10 + digit) % 97;
        }
    }
    remainder == 1
}

fn routing_number_valid(text: &str) -> bool {
    let d = digits(text);
    d.len() == 9 && (3 * (d[0] + d[3] + d[6]) + 7 * (d[1] + d[4] + d[7]) + d[2] + d[5] + d[8]) % 10 == 0
}

fn ssn_valid(text: &str) -> bool {
    let d = digits(text);
    if d.len() != 9 {
        return false;
    }
    let area = d[0] * 100 + d[1] * 10 + d[2];
    area != 0 && area != 666 && area < 900 && (d[3], d[4]) != (0, 0) && d[5..] != [0, 0, 0, 0]
}

/// IPv6 candidates for `redact_ips` (group 1), set apart from identifiers so
/// a path such as `std::net` never yields one, and kept only when they parse
/// as an address, so clock times such as `12:30:45` stay.
const IPV6_CANDIDATE: &str = r"(?:^|[^0-9A-Za-z_:.])([0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7})(?:$|[^0-9A-Za-z_:.])";

fn ipv6_valid(text: &str) -> bool {
    (text.contains("::") || text.matches(':').count() == 7)
        && text.chars().any(|c| c.is_ascii_hexdigit())
        && text.parse::<std::net::Ipv6Addr>().is_ok()
}

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

/// HMAC-SHA256 (RFC 2104) over the `sha2` dependency already in the tree.
pub(crate) fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0u8; 64];
    block[..32].copy_from_slice(key);
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

#[derive(Clone, Default)]
pub struct Redactors {
    rules: Vec<(Regex, &'static str)>,
    /// Rules whose match must also pass a check (`FINANCIAL_PATTERNS`, IPv6);
    /// a capture group, when present, is the redacted span.
    checked: Vec<CheckedRule>,
}

// Literal rules embed credential values; Debug must never print a pattern.
impl std::fmt::Debug for Redactors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactors")
            .field("rules", &self.rules.len())
            .field("checked", &self.checked.len())
            .finish()
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
        let mut checked: Vec<CheckedRule> = Vec::new();
        if cfg.flag_is_true("policy.privacy.redact_ips") {
            rules.push((
                Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").expect("static regex"),
                "[REDACTED_IP]",
            ));
            checked.push((
                Regex::new(IPV6_CANDIDATE).expect("static regex"),
                "[REDACTED_IP]",
                ipv6_valid,
            ));
        }
        if cfg.flag_is_true("policy.privacy.redact_financial") {
            checked.extend(
                FINANCIAL_PATTERNS
                    .iter()
                    .map(|(pattern, check)| (Regex::new(pattern).expect("static regex"), PII_LABEL, *check)),
            );
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
        Self { rules, checked }
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
        self.redact_spans(text, None).0
    }

    /// `redact`, but each label carries a short HMAC-SHA256 tag of the span it
    /// replaced (`[REDACTED_IP#1a2b3c4d]`): equal values share a tag and
    /// different values almost never do, so a reviewer can see that a
    /// redacted value changed without seeing it. The tag cannot be reversed
    /// without `key`, which callers keep in memory only. Also returns how many
    /// spans were redacted.
    pub fn redact_tagged(&self, text: &str, key: &[u8; 32]) -> (String, usize) {
        let (out, count) = self.redact_spans(text, Some(key));
        (out.into_owned(), count)
    }

    fn redact_spans<'a>(&self, text: &'a str, key: Option<&[u8; 32]>) -> (Cow<'a, str>, usize) {
        let label_for = |label: &'static str, span: &str| -> Cow<'static, str> {
            match key {
                None => Cow::Borrowed(label),
                Some(key) => {
                    let tag = hmac_sha256(key, span.as_bytes());
                    let hex: String = tag[..4].iter().map(|b| format!("{b:02x}")).collect();
                    Cow::Owned(format!("{}#{hex}]", label.trim_end_matches(']')))
                }
            }
        };
        let mut count = 0;
        let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
        for (re, replacement) in &self.rules {
            spans.extend(
                re.find_iter(text)
                    .filter(|m| !m.is_empty())
                    .map(|m| (m.start(), m.end(), *replacement)),
            );
        }
        for (re, replacement, check) in &self.checked {
            for caps in re.captures_iter(text) {
                let span = caps.get(1).or_else(|| caps.get(0));
                if let Some(m) = span.filter(|m| !m.is_empty() && check(m.as_str())) {
                    // A whole-match rule (cards, IBANs) checks the whole match;
                    // a labeled rule checks the number it captured.
                    spans.push((m.start(), m.end(), *replacement));
                }
            }
        }
        if spans.is_empty() {
            return (Cow::Borrowed(text), 0);
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
                        out.push_str(&label_for(l, &text[s..e]));
                        count += 1;
                        cursor = e;
                    }
                }
            }
        }
        if let Some((s, e, l)) = merged {
            out.push_str(&text[cursor..s]);
            out.push_str(&label_for(l, &text[s..e]));
            count += 1;
            cursor = e;
        }
        out.push_str(&text[cursor..]);
        (Cow::Owned(out), count)
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
    ledger: Arc<GapLedger>,
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
    /// Test-only: the writer thread dies while the sender stays up, so the
    /// next `try_send` is `Disconnected`.
    #[cfg(test)]
    Panic,
}

fn gap_path(audit: &Path) -> PathBuf {
    let mut name = audit.as_os_str().to_os_string();
    name.push(".gaps");
    PathBuf::from(name)
}

fn gap_line(since: u64, total: u64) -> String {
    let stamp = serde_json::to_string(&super::iso_now()).unwrap_or_else(|_| "\"\"".to_string());
    format!("{{\"event\":\"audit_gap\",\"dropped\":{since},\"total\":{total},\"timestamp\":{stamp}}}")
}

/// Durable count of records that were not queued or not written.
/// The writer thread and `Audit`'s drop both persist it; `log` only bumps
/// the atomic, so a full queue does not wait on this file.
#[derive(Debug)]
struct GapLedger {
    audit_path: PathBuf,
    gap_path: PathBuf,
    dropped: AtomicU64,
    durable: AtomicU64,
    /// How many drops an `audit_gap` line has already described.
    reported: AtomicU64,
    persist_error: AtomicBool,
    unreadable: AtomicBool,
    persist_lock: Mutex<()>,
}

impl GapLedger {
    fn open(audit_path: &Path) -> Self {
        let gap_path = gap_path(audit_path);
        let (count, unreadable) = match std::fs::read_to_string(&gap_path) {
            Err(error) if error.kind() == ErrorKind::NotFound => (0, false),
            Err(_) => (0, true),
            Ok(text) => match serde_json::from_str::<Value>(text.trim()) {
                Ok(value) => match value.get("dropped").and_then(Value::as_u64) {
                    Some(count) => (count, false),
                    None => (0, true),
                },
                Err(_) => (0, true),
            },
        };
        Self {
            audit_path: audit_path.to_path_buf(),
            gap_path,
            dropped: AtomicU64::new(count),
            durable: AtomicU64::new(if unreadable { 0 } else { count }),
            reported: AtomicU64::new(count),
            persist_error: AtomicBool::new(false),
            unreadable: AtomicBool::new(unreadable),
            persist_lock: Mutex::new(()),
        }
    }

    fn note_drop(&self) -> u64 {
        let count = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(dropped = count, "audit record dropped; drop counted");
        count
    }

    fn persist(&self) {
        let _guard = self.persist_lock.lock().unwrap_or_else(|poison| poison.into_inner());
        let count = self.dropped.load(Ordering::Relaxed);
        if count == 0 && self.unreadable.load(Ordering::Relaxed) {
            return;
        }
        if count == 0 && !self.gap_path.exists() {
            return;
        }
        if let Some(parent) = self.gap_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                self.persist_error.store(true, Ordering::Relaxed);
                return;
            }
        }
        for _ in 0..8 {
            let count = self.dropped.load(Ordering::Relaxed);
            let body = format!("{{\"dropped\":{count}}}\n");
            match super::atomic::write_atomic(&self.gap_path, body.as_bytes(), Some(0o600)) {
                Ok(()) => {
                    self.durable.store(count, Ordering::Relaxed);
                    self.unreadable.store(false, Ordering::Relaxed);
                    if self.dropped.load(Ordering::Relaxed) == count {
                        self.persist_error.store(false, Ordering::Relaxed);
                        return;
                    }
                }
                Err(_) => {
                    self.persist_error.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
        self.persist_error.store(true, Ordering::Relaxed);
    }

    /// One audit line for the drops not yet described, then the counter file.
    /// A failed gap line is not itself counted: the counter already names the hole.
    fn seal(&self, sink: &Sink) {
        let dropped = self.dropped.load(Ordering::Relaxed);
        let reported = self.reported.load(Ordering::Relaxed);
        if dropped > reported {
            let line = gap_line(dropped - reported, dropped);
            let deadline = Instant::now() + sink.retention.lease_wait;
            if sink.append(&self.audit_path, &line, deadline).is_ok() {
                self.reported.fetch_max(dropped, Ordering::Relaxed);
            }
        }
        if self.dropped.load(Ordering::Relaxed) != self.durable.load(Ordering::Relaxed)
            || self.persist_error.load(Ordering::Relaxed)
        {
            self.persist();
        }
    }

    fn status(&self) -> Value {
        serde_json::json!({
            "dropped": self.dropped.load(Ordering::Relaxed),
            "durable": self.durable.load(Ordering::Relaxed),
            "persist_failed": self.persist_error.load(Ordering::Relaxed),
            "record_unreadable": self.unreadable.load(Ordering::Relaxed),
        })
    }
}

fn write_jobs(sink: Arc<Sink>, ledger: Arc<GapLedger>, jobs: Receiver<Job>) {
    for job in jobs {
        match job {
            Job::Line { path, line, deadline } => {
                if sink.append(&path, &line, deadline).is_err() {
                    ledger.note_drop();
                }
            }
            Job::Flush(done) => {
                ledger.seal(&sink);
                let _ = done.send(());
            }
            Job::FlushAsync(done) => {
                ledger.seal(&sink);
                let _ = done.send(());
            }
            #[cfg(test)]
            Job::Panic => panic!("audit writer test panic"),
        }
        ledger.seal(&sink);
    }
    ledger.seal(&sink);
}

/// A required audit record was not accepted. The caller must not commit the
/// action. Counted callers ignore this and leave the gap report instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditRefused;

/// `logging.audit_queue_lines`: lines waiting for the writer thread; 0 keeps
/// appends inline. A full queue refuses the new line without waiting, counts
/// it, and leaves the action to the caller (`log` continues, `log_required` does not).
fn queue_lines(cfg: &AppConfig) -> usize {
    cfg.limit("logging.audit_queue_lines") as usize
}

impl Audit {
    pub fn new(path: PathBuf, cfg: &AppConfig) -> Self {
        let include_query_hash = cfg.bool_opt("logging.audit_fields.include_query_hash").unwrap_or(true);
        let ledger = Arc::new(GapLedger::open(&path));
        Self {
            path,
            redactors: Redactors::from_config(cfg),
            include_query_hash,
            sink: Arc::new(Sink {
                retention: super::bounded_log::retention(cfg),
                lock: Mutex::new(()),
            }),
            ledger,
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
        let ledger = Arc::clone(&self.ledger);
        match std::thread::Builder::new()
            .name("audit-writer".into())
            .spawn(move || write_jobs(sink, ledger, jobs))
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

    /// Append one redacted, timestamped record.
    ///
    /// `true` when the record was accepted (queued, or written inline).
    /// `false` when it was not. A full queue, a dead writer, or a failed
    /// inline append is counted in [`Audit::gap_status`]; a non-object event
    /// is refused and not counted, because there was no record. The caller
    /// still proceeds. Security commits use [`Audit::log_required`].
    ///
    /// Counted, because failing the caller would brick a path that has already
    /// happened or must stay up: `portal.request` (every request), chat and
    /// web telemetry, browser results (the child already ran), config reload
    /// (a tighter limit must still apply), job cancel, notifications, boot
    /// sweeps, and the agentic child's inline lines (the write gates already
    /// ran; this process has no HTTP request to fail). Password changes stay
    /// counted too: a wrong current password must not be stored as a success,
    /// and the route does not hold the previous hash to undo one.
    /// Web policy stays counted: a refused change must not add a policy line,
    /// and this handler has no one-step rollback of the policy file.
    pub fn log(&self, event: Value) -> bool {
        self.enqueue(event).is_ok()
    }

    /// [`Audit::log`] for a commit that must not land without its record.
    /// `Err` means the record was not accepted. The drop is still counted.
    ///
    /// Fail closed at sign-in (the new session is revoked), account create
    /// (the new account is deleted), role, disable and delete (the store
    /// change runs only after this returns `Ok`, and a last-admin refusal
    /// returns before it), managed key writes, legacy session adoption
    /// (adoption is released again), and machine-key mint or revoke.
    pub fn log_required(&self, event: Value) -> std::result::Result<(), AuditRefused> {
        self.enqueue(event)
    }

    /// Records dropped since the gap file was created, including ones loaded
    /// at startup.
    pub fn dropped(&self) -> u64 {
        self.ledger.dropped.load(Ordering::Relaxed)
    }

    /// Operator report embedded in `/api/status` and `/api/diag` as `audit_gap`.
    pub fn gap_status(&self) -> Value {
        self.ledger.status()
    }

    fn enqueue(&self, event: Value) -> std::result::Result<(), AuditRefused> {
        let Some(line) = self.line(event) else {
            return Err(AuditRefused);
        };
        // Deadline is stamped at submission, before any in-process wait, so
        // callers that pile up behind a busy sink share one bound.
        let deadline = Instant::now() + self.sink.retention.lease_wait;
        let Some(queue) = self.queue() else {
            return self.write_inline(&self.path, &line, deadline);
        };
        match queue.try_send(Job::Line {
            path: self.path.clone(),
            line,
            deadline,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.ledger.note_drop();
                Err(AuditRefused)
            }
            // The writer thread is gone: append inline, and count it if that fails.
            Err(TrySendError::Disconnected(Job::Line { path, line, deadline })) => {
                self.write_inline(&path, &line, deadline)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.ledger.note_drop();
                Err(AuditRefused)
            }
        }
    }

    fn write_inline(&self, path: &Path, line: &str, deadline: Instant) -> std::result::Result<(), AuditRefused> {
        match self.sink.append(path, line, deadline) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.ledger.note_drop();
                Err(AuditRefused)
            }
        }
    }

    /// [`Audit::log`] for a record whose loss must be detectable: written
    /// inline after every line queued before it, and `true` only once it is
    /// in the file. A caller that needs the record keeps its own copy until
    /// then (the #965 role-migration outbox does).
    pub fn log_confirmed(&self, event: Value) -> bool {
        let Some(line) = self.line(event) else {
            return false;
        };
        self.flush();
        let deadline = Instant::now() + self.sink.retention.lease_wait;
        match self.sink.append(&self.path, &line, deadline) {
            Ok(()) => true,
            Err(_) => {
                self.ledger.note_drop();
                false
            }
        }
    }

    /// The redacted, timestamped JSON line for one event; `None` when the
    /// event is not an object or cannot be serialized.
    fn line(&self, event: Value) -> Option<String> {
        let mut record = match event {
            Value::Object(map) => map,
            _ => {
                tracing::warn!("audit_log called with a non-object event");
                return None;
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
        match serde_json::to_string(&Value::Object(record)) {
            Ok(line) => Some(line),
            Err(e) => {
                tracing::warn!("audit_log failed to serialize event: {e}");
                None
            }
        }
    }

    #[cfg(test)]
    fn close_writer(&mut self) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        writer.queue.take();
        if let Some(thread) = writer.thread.take() {
            let _ = thread.join();
        }
    }

    #[cfg(test)]
    fn panic_writer(&mut self) {
        let Some(queue) = self.queue().cloned() else {
            return;
        };
        let _ = queue.try_send(Job::Panic);
        if let Some(writer) = self.writer.as_mut() {
            if let Some(thread) = writer.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub fn append_spend(&self, path: &Path, record: &Value) {
        let deadline = Instant::now() + self.sink.retention.lease_wait;
        if self.sink.append(path, &record.to_string(), deadline).is_err() {
            tracing::warn!("spend sink unavailable, busy, or record exceeds retention bound");
        }
    }
}

/// Save the gap counter, but do not let a hung filesystem hold shutdown.
/// The save runs on its own thread. After `wait` it is left running, the same
/// way a stalled writer is abandoned: `write_atomic` has no deadline of its own.
fn persist_bounded(ledger: &Arc<GapLedger>, wait: Duration) {
    let ledger = Arc::clone(ledger);
    let thread = match std::thread::Builder::new()
        .name("audit-gap".into())
        .spawn(move || ledger.persist())
    {
        Ok(thread) => thread,
        Err(error) => {
            tracing::warn!("audit gap counter could not be saved ({error})");
            return;
        }
    };
    let deadline = Instant::now() + wait;
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            tracing::warn!("audit gap counter still writing at shutdown; the saved count may be short");
            return;
        }
        std::thread::sleep(FLUSH_RETRY);
    }
    let _ = thread.join();
}

impl Drop for Audit {
    /// Close the queue and let the writer drain it. Each queued line's wait
    /// ends at its own submission deadline, but a stalled filesystem call has
    /// none: after `FLUSH_WAIT` the writer is left behind with what it holds.
    /// The gap counter is saved under that same bound, before and after the
    /// writer, so a stuck writer cannot take the count and a stuck counter
    /// file cannot hold the process.
    fn drop(&mut self) {
        persist_bounded(&self.ledger, self.flush_wait);
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
        persist_bounded(&self.ledger, self.flush_wait);
    }
}

impl Sink {
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
    fn hmac_sha256_matches_rfc_4231_case_one() {
        // A 20-byte key zero-padded to 32 bytes is the same HMAC key.
        let mut key = [0u8; 32];
        key[..20].fill(0x0b);
        let mac: String = hmac_sha256(&key, b"Hi There")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(mac, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[test]
    fn tagged_redaction_shows_that_a_hidden_value_changed_without_showing_it() {
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), Path::new("config.yaml")).unwrap();
        let r = Redactors::from_config(&cfg);
        let key = [7u8; 32];
        let diff = "-listen = \"127.0.0.1:8080\"\n+listen = \"0.0.0.0:8080\"\n same 127.0.0.1\n";
        let (tagged, count) = r.redact_tagged(diff, &key);
        assert_eq!(count, 3, "{tagged}");
        assert!(!tagged.contains("127.0.0.1") && !tagged.contains("0.0.0.0"), "{tagged}");
        let tags: Vec<&str> = tagged
            .match_indices("[REDACTED_IP#")
            .map(|(i, _)| &tagged[i..i + 22])
            .collect();
        assert_eq!(tags.len(), 3, "{tagged}");
        assert_ne!(tags[0], tags[1], "a changed value must look changed: {tagged}");
        assert_eq!(tags[0], tags[2], "an equal value keeps its tag: {tagged}");
        // Another key gives other tags; the untagged path is unchanged.
        assert_ne!(r.redact_tagged(diff, &[8u8; 32]).0, tagged);
        assert_eq!(
            r.redact(diff),
            diff.replace("127.0.0.1", "[REDACTED_IP]")
                .replace("0.0.0.0", "[REDACTED_IP]")
        );
        // Tagged text survives a second pass (the response envelope redacts again).
        let secret = "api_key = \"sk-live-abcdef123456\" mail ops@example.com";
        let (tagged, count) = r.redact_tagged(secret, &key);
        assert_eq!(count, 2, "{tagged}");
        assert!(
            !tagged.contains("abcdef123456") && !tagged.contains("example.com"),
            "{tagged}"
        );
        assert_eq!(r.redact(&tagged), tagged);
        assert_eq!(r.redact_tagged("nothing here", &key), ("nothing here".to_string(), 0));
    }

    #[test]
    fn financial_numbers_and_ipv6_redact_only_when_their_check_passes() {
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), Path::new("config.yaml")).unwrap();
        let r = Redactors::from_config(&cfg);
        for (text, kept, gone) in [
            // Test card numbers (Luhn-valid), spaced and dashed.
            ("card 4111 1111 1111 1111 ok", "card ", "4111"),
            ("card 5500-0000-0000-0004.", "card ", "5500"),
            ("iban GB82 WEST 1234 5698 7654 32 end", "iban ", "WEST"),
            ("routing number: 021000021 here", "routing number: ", "021000021"),
            ("acct # 12345678901 thanks", "acct # ", "12345678901"),
            ("ssn 123-45-6789", "ssn ", "6789"),
            ("peer 2001:db8::1 and fe80::1ff:fe23:4567:890a", "peer ", "2001:db8"),
        ] {
            let out = r.redact(text);
            assert!(out.contains(kept) && !out.contains(gone), "{text} -> {out}");
            assert!(!r.contains_secret(text), "PII redacts, it never refuses input: {text}");
        }
        for text in [
            "order 4111 1111 1111 1112",       // Luhn fails
            "GB82 WEST 1234 5698 7654 33",     // mod-97 fails
            "routing 123456789",               // ABA checksum fails
            "id 021000021",                    // valid routing digits, no label
            "ssn 000-12-3456 and 666-12-3456", // invalid areas
            "use std::net::Ipv6Addr at 12:30:45",
            "ratio 3:2:1 and a :: b",
        ] {
            assert_eq!(r.redact(text), text, "kept as is");
        }
        let off = Redactors::from_config(
            &AppConfig::from_str("policy: {privacy: {redact_financial: false}}", Path::new("c.yaml")).unwrap(),
        );
        assert_eq!(off.redact("card 4111 1111 1111 1111"), "card 4111 1111 1111 1111");
    }

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
        // dropped, and the first submitted line always lands. The gap line is
        // the record of that hole.
        let lines = written(audit.path());
        let busy: Vec<_> = lines.iter().filter(|line| line["event"] == "busy").collect();
        assert!((1..=2).contains(&busy.len()), "{lines:?}");
        assert_eq!(busy[0]["n"], 0);
        assert!(lines.iter().any(|line| line["event"] == "audit_gap"), "{lines:?}");
        assert!(audit.dropped() + busy.len() as u64 <= 50);
        assert!(audit.dropped() >= 1, "{lines:?}");
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

    #[test]
    fn a_confirmed_line_lands_after_queued_lines_or_reports_that_it_did_not() {
        let dir = tempfile::tempdir().unwrap();
        let queued = audit(dir.path(), "logging: {}\n").with_writer_thread();
        queued.log(json!({"event":"queued"}));
        assert!(queued.log_confirmed(json!({"event":"confirmed"})));
        let events: Vec<Value> = written(queued.path()).iter().map(|row| row["event"].clone()).collect();
        assert_eq!(events, [json!("queued"), json!("confirmed")]);
        // A log directory that is a file refuses the write: `false`, so the
        // caller keeps what it has not yet recorded.
        let blocked = tempfile::tempdir().unwrap();
        std::fs::write(blocked.path().join("logs"), b"not a directory").unwrap();
        assert!(!audit(blocked.path(), "logging: {}\n").log_confirmed(json!({"event":"refused"})));
        assert!(!queued.log_confirmed(json!("not an object")));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_full_queue_refuses_a_required_record_and_counts_the_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut audit = audit(dir.path(), "logging:\n  audit_lease_wait_ms: 2000\n").with_queue(1);
        audit.flush_wait = Duration::from_millis(200);
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let held =
            super::super::file_lease::FileLease::acquire(&dir.path().join("logs/audit.jsonl.lock"), true).unwrap();
        let started = Instant::now();
        let mut refused = 0;
        for n in 0..40 {
            if audit.log_required(json!({"event":"queued","n":n})).is_err() {
                refused += 1;
            }
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "recording waited on the queue or the disk"
        );
        assert!(refused >= 1, "a full queue accepted every required record");
        assert!(audit.dropped() >= refused as u64, "a refusal was not counted");
        assert_eq!(audit.gap_status()["dropped"], json!(audit.dropped()));
        drop(held);
    }

    #[test]
    fn a_stopped_writer_that_cannot_append_refuses_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("logs"), b"not a directory").unwrap();
        let mut audit = audit(dir.path(), "logging: {}\n").with_queue(2);
        audit.close_writer();
        assert!(
            audit.log_required(json!({"event":"closed"})).is_err(),
            "a stopped writer that cannot append still accepted the record"
        );
        assert!(audit.dropped() >= 1, "the refused record was not counted");
        assert!(audit.gap_status()["dropped"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn a_dead_writer_that_cannot_append_refuses_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("logs"), b"not a directory").unwrap();
        let mut audit = audit(dir.path(), "logging: {}\n").with_queue(2);
        audit.panic_writer();
        assert!(
            audit.log_required(json!({"event":"after_writer"})).is_err(),
            "a dead writer that cannot append still accepted the record"
        );
        assert!(audit.dropped() >= 1, "the refused record was not counted");
        assert!(audit.gap_status()["dropped"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn a_dead_writer_appends_inline_when_the_file_can_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut audit = audit(dir.path(), "logging: {}\n").with_queue(2);
        audit.panic_writer();
        assert!(audit.log_required(json!({"event":"inline_after"})).is_ok());
        assert_eq!(written(audit.path())[0]["event"], "inline_after");
        assert_eq!(audit.dropped(), 0);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_drop_count_survives_restart_and_stays_in_the_operator_report() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = "logging:\n  audit_lease_wait_ms: 2000\n";
        let dropped = {
            let mut audit = audit(dir.path(), yaml).with_queue(1);
            audit.flush_wait = Duration::from_millis(200);
            std::fs::create_dir_all(dir.path().join("logs")).unwrap();
            let held =
                super::super::file_lease::FileLease::acquire(&dir.path().join("logs/audit.jsonl.lock"), true).unwrap();
            for n in 0..20 {
                let _ = audit.log_required(json!({"event":"spill","n":n}));
            }
            let n = audit.dropped();
            assert!(n >= 1, "the queue did not drop");
            drop(held);
            audit.flush();
            let holes: Vec<Value> = written(audit.path())
                .into_iter()
                .filter(|row| row["event"] == "audit_gap")
                .collect();
            assert!(!holes.is_empty(), "the audit log has no record of the hole");
            assert_eq!(audit.gap_status()["dropped"], json!(n));
            n
        };
        let again = audit(dir.path(), yaml);
        assert_eq!(again.dropped(), dropped, "restart forgot the drops");
        let report = again.gap_status();
        assert_eq!(report["dropped"], json!(dropped));
        assert_eq!(report["durable"], json!(dropped));
        assert_eq!(report["persist_failed"], json!(false));
        assert_eq!(report["record_unreadable"], json!(false));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_gap_file_that_cannot_be_replaced_stays_in_the_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        // A directory where the counter file belongs: the save cannot replace it.
        std::fs::create_dir(dir.path().join("logs/audit.jsonl.gaps")).unwrap();
        let mut first = audit(dir.path(), "logging:\n  audit_lease_wait_ms: 2000\n").with_queue(1);
        first.flush_wait = Duration::from_millis(200);
        let held =
            super::super::file_lease::FileLease::acquire(&dir.path().join("logs/audit.jsonl.lock"), true).unwrap();
        for n in 0..20 {
            let _ = first.log(json!({"event":"disk","n":n}));
        }
        drop(held);
        first.flush();
        let report = first.gap_status();
        assert!(report["dropped"].as_u64().unwrap() >= 1, "{report}");
        assert_eq!(report["persist_failed"], json!(true), "{report}");
        drop(first);
        let again = audit(dir.path(), "logging: {}\n");
        let reloaded = again.gap_status();
        assert_eq!(reloaded["record_unreadable"], json!(true), "{reloaded}");
        assert_eq!(reloaded["dropped"], json!(0));
    }
}
