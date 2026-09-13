//! Append-only audit JSONL with recursive PII/secret redaction.
//!
//! Port of `utils/logger.py::audit_log` + `redact_sensitive`. The audit sink
//! never raises: a disk-full or serialization failure degrades to a tracing
//! warning so an already-computed response is never turned into a 500.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use regex::Regex;
use serde_json::Value;

use super::config::AppConfig;

const AUDIT_SKIP_KEYS: [&str; 3] = ["query_hash", "timestamp", "event"];

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

    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (re, replacement) in &self.rules {
            out = re.replace_all(&out, *replacement).into_owned();
        }
        out
    }

    pub fn redact_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.redact(s)),
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
    retention: super::bounded_log::Retention,
    lock: Mutex<()>,
}

impl Audit {
    pub fn new(path: PathBuf, cfg: &AppConfig) -> Self {
        let include_query_hash = cfg.bool_opt("logging.audit_fields.include_query_hash").unwrap_or(true);
        Self {
            path,
            redactors: Redactors::from_config(cfg),
            include_query_hash,
            retention: super::bounded_log::retention(cfg),
            lock: Mutex::new(()),
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
        self.redactors.redact(text)
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
        if self.append_line(&self.path, &line).is_err() {
            tracing::warn!("audit sink unavailable, busy, or record exceeds retention bound");
        }
    }

    pub fn append_spend(&self, path: &Path, record: &Value) {
        if self.append_line(path, &record.to_string()).is_err() {
            tracing::warn!("spend sink unavailable, busy, or record exceeds retention bound");
        }
    }

    /// Retry a busy lease *outside* `self.lock`. Sleeping under that mutex
    /// serialized every `/api/*` `portal.request` behind the waiter, so the
    /// nth caller both stalled and then dropped the line when the shared
    /// deadline expired. One non-blocking attempt runs under the mutex
    /// (Windows has no flock); the wait itself does not.
    fn append_line(&self, path: &Path, line: &str) -> std::io::Result<()> {
        let deadline = Instant::now() + self.retention.lease_wait;
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
