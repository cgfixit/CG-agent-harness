//! Append-only inference spend ledger. Dollars are derived at read time.
//!
//! Port of CyClaw `utils/spend.py` *behavior*, not the file. Never persist
//! prompt/query/content/messages or credentials. `TokenTally` / `estimate_tokens`
//! (UTF-8 bytes/4) are not USD.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Map, Value};
use time::Date;

use crate::common::audit::Audit;
use crate::common::bounded_log;
use crate::common::config::AppConfig;

pub const TICKS_PER_USD: f64 = 10_000_000_000.0;
pub const STALE_AFTER_DAYS: i64 = 30;
pub const DEFAULT_SPEND_FILE: &str = "logs/spend.jsonl";
const MAX_SUMMARY_BYTES: u64 = 16 * 1024 * 1024;

/// USD per 1M tokens. Re-verified 2026-09-19 against
/// https://docs.x.ai/developers/pricing and
/// https://platform.claude.com/docs/en/about-claude/pricing
struct RateRow {
    input: f64,
    output: f64,
    cached_input: Option<f64>,
    long_input: Option<f64>,
    long_cached_input: Option<f64>,
    long_output: Option<f64>,
    long_prompt_threshold: Option<u64>,
    cache_creation: Option<f64>,
    cache_creation_1h: Option<f64>,
    cache_read: Option<f64>,
}

const GROK_46: RateRow = RateRow {
    input: 2.00,
    output: 6.00,
    cached_input: Some(0.50),
    long_input: Some(4.00),
    long_cached_input: Some(1.00),
    long_output: Some(12.00),
    long_prompt_threshold: Some(200_000),
    cache_creation: None,
    cache_creation_1h: None,
    cache_read: None,
};
const GROK_45: RateRow = RateRow {
    input: 2.00,
    output: 6.00,
    cached_input: Some(0.30),
    long_input: Some(4.00),
    long_cached_input: Some(0.60),
    long_output: Some(12.00),
    long_prompt_threshold: Some(200_000),
    cache_creation: None,
    cache_creation_1h: None,
    cache_read: None,
};
const GROK_43: RateRow = RateRow {
    input: 1.25,
    output: 2.50,
    cached_input: Some(0.20),
    long_input: Some(2.50),
    long_cached_input: Some(0.40),
    long_output: Some(5.00),
    long_prompt_threshold: Some(200_000),
    cache_creation: None,
    cache_creation_1h: None,
    cache_read: None,
};
const CLAUDE_SONNET_5: RateRow = RateRow {
    input: 2.00,
    output: 10.00,
    cached_input: None,
    long_input: None,
    long_cached_input: None,
    long_output: None,
    long_prompt_threshold: None,
    cache_creation: Some(2.50),
    cache_creation_1h: Some(4.00),
    cache_read: Some(0.20),
};

const RATE_VERIFIED: &[(&str, &str)] = &[
    ("grok-4.6", "2026-09-19"),
    ("grok-4.5", "2026-09-19"),
    ("grok-4.3", "2026-09-19"),
    ("claude-sonnet-5", "2026-09-19"),
];

fn rate_for(model: &str) -> Option<&'static RateRow> {
    match model {
        "grok-4.6" => Some(&GROK_46),
        "grok-4.5" => Some(&GROK_45),
        "grok-4.3" => Some(&GROK_43),
        "claude-sonnet-5" => Some(&CLAUDE_SONNET_5),
        _ => None,
    }
}

/// Oldest `_RATE_VERIFIED` date. Staleness tracks the stalest row.
pub fn priced_as_of() -> &'static str {
    RATE_VERIFIED
        .iter()
        .map(|(_, d)| *d)
        .min()
        .expect("rate table is not empty")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTokens {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_5m_tokens: Option<u64>,
    pub cache_creation_1h_tokens: Option<u64>,
    pub vendor_cost_ticks: Option<u64>,
}

impl UsageTokens {
    pub fn usage_reported(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some()
    }

    pub fn usage_missing(&self) -> bool {
        !self.usage_reported()
    }

    fn all_counts_none(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cached_input_tokens.is_none()
            && self.reasoning_tokens.is_none()
            && self.cache_creation_input_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
            && self.cache_creation_5m_tokens.is_none()
            && self.cache_creation_1h_tokens.is_none()
    }

    fn count(&self, field: fn(&Self) -> Option<u64>) -> u64 {
        field(self).unwrap_or(0)
    }
}

fn json_u64(v: Option<&Value>) -> Option<u64> {
    match v {
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    }
}

/// xAI Responses (`input_tokens`) and Chat Completions (`prompt_tokens`).
pub fn parse_grok_usage(usage: Option<&Value>) -> UsageTokens {
    let Some(usage) = usage else {
        return UsageTokens::default();
    };
    UsageTokens {
        input_tokens: json_u64(usage.get("input_tokens")).or_else(|| json_u64(usage.get("prompt_tokens"))),
        output_tokens: json_u64(usage.get("output_tokens")).or_else(|| json_u64(usage.get("completion_tokens"))),
        cached_input_tokens: json_u64(usage.pointer("/input_tokens_details/cached_tokens"))
            .or_else(|| json_u64(usage.pointer("/prompt_tokens_details/cached_tokens"))),
        reasoning_tokens: json_u64(usage.pointer("/output_tokens_details/reasoning_tokens"))
            .or_else(|| json_u64(usage.pointer("/completion_tokens_details/reasoning_tokens"))),
        vendor_cost_ticks: json_u64(usage.get("cost_in_usd_ticks")),
        ..UsageTokens::default()
    }
}

pub fn parse_claude_usage(usage: Option<&Value>) -> UsageTokens {
    let Some(usage) = usage else {
        return UsageTokens::default();
    };
    UsageTokens {
        input_tokens: json_u64(usage.get("input_tokens")),
        output_tokens: json_u64(usage.get("output_tokens")),
        cache_creation_input_tokens: json_u64(usage.get("cache_creation_input_tokens")),
        cache_read_input_tokens: json_u64(usage.get("cache_read_input_tokens")),
        cache_creation_5m_tokens: json_u64(usage.pointer("/cache_creation/ephemeral_5m_input_tokens")),
        cache_creation_1h_tokens: json_u64(usage.pointer("/cache_creation/ephemeral_1h_input_tokens")),
        ..UsageTokens::default()
    }
}

pub fn parse_local_usage(usage: Option<&Value>) -> UsageTokens {
    let Some(usage) = usage else {
        return UsageTokens::default();
    };
    UsageTokens {
        input_tokens: json_u64(usage.get("prompt_tokens")),
        output_tokens: json_u64(usage.get("completion_tokens")),
        ..UsageTokens::default()
    }
}

pub fn parse_provider_usage(provider: &str, usage: Option<&Value>) -> UsageTokens {
    match provider {
        "claude" => parse_claude_usage(usage),
        "local" => parse_local_usage(usage),
        _ => parse_grok_usage(usage),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsdEstimate {
    pub usd: Option<f64>,
    pub rate_unknown: bool,
    pub priced_as_of: &'static str,
    pub usd_source: &'static str,
}

pub fn billed_output_tokens(tokens: &UsageTokens) -> u64 {
    tokens.count(|t| t.output_tokens) + tokens.count(|t| t.reasoning_tokens)
}

pub fn rates_are_stale(now: Option<Date>) -> bool {
    let Ok(priced) = Date::parse(priced_as_of(), &time::format_description::well_known::Iso8601::DATE) else {
        return true;
    };
    let today = now.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    (today - priced).whole_days() > STALE_AFTER_DAYS
}

static STALE_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once_if_stale() {
    if !rates_are_stale(None) {
        return;
    }
    if STALE_WARNED.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::warn!(
        "spend rate table priced_as_of {} is older than {} days",
        priced_as_of(),
        STALE_AFTER_DAYS
    );
}

#[cfg(test)]
pub fn reset_stale_warning_for_tests() {
    STALE_WARNED.store(false, Ordering::SeqCst);
}

pub fn estimate_usd(model: &str, tokens: &UsageTokens, provider: &str) -> UsdEstimate {
    let priced = priced_as_of();
    if provider == "local" {
        return UsdEstimate {
            usd: None,
            rate_unknown: false,
            priced_as_of: priced,
            usd_source: "local_unpriced",
        };
    }
    if let Some(ticks) = tokens.vendor_cost_ticks {
        return UsdEstimate {
            usd: Some(ticks as f64 / TICKS_PER_USD),
            rate_unknown: false,
            priced_as_of: priced,
            usd_source: "vendor_ticks",
        };
    }
    if tokens.all_counts_none() {
        return UsdEstimate {
            usd: None,
            rate_unknown: false,
            priced_as_of: priced,
            usd_source: "incomplete",
        };
    }
    let Some(rates) = rate_for(model) else {
        return UsdEstimate {
            usd: None,
            rate_unknown: true,
            priced_as_of: priced,
            usd_source: "unknown_model",
        };
    };
    let usd = if rates.cached_input.is_some() {
        grok_table_usd(tokens, rates)
    } else {
        claude_table_usd(tokens, rates)
    };
    UsdEstimate {
        usd: Some(usd),
        rate_unknown: false,
        priced_as_of: priced,
        usd_source: "rate_table",
    }
}

fn grok_table_usd(tokens: &UsageTokens, rates: &RateRow) -> f64 {
    let prompt = tokens.count(|t| t.input_tokens);
    let use_long = rates.long_prompt_threshold.is_some_and(|threshold| prompt >= threshold);
    let input_rate = if use_long {
        rates.long_input.unwrap_or(rates.input)
    } else {
        rates.input
    };
    let cached_rate = if use_long {
        rates.long_cached_input.or(rates.cached_input).unwrap_or(0.0)
    } else {
        rates.cached_input.unwrap_or(0.0)
    };
    let output_rate = if use_long {
        rates.long_output.unwrap_or(rates.output)
    } else {
        rates.output
    };
    let cached = tokens.count(|t| t.cached_input_tokens);
    let uncached = prompt.saturating_sub(cached);
    uncached as f64 * input_rate / 1_000_000.0
        + cached as f64 * cached_rate / 1_000_000.0
        + billed_output_tokens(tokens) as f64 * output_rate / 1_000_000.0
}

fn claude_table_usd(tokens: &UsageTokens, rates: &RateRow) -> f64 {
    let write_5m = rates.cache_creation.unwrap_or(0.0);
    let write_1h = rates.cache_creation_1h.unwrap_or(write_5m);
    let has_ttl = tokens.cache_creation_5m_tokens.is_some() || tokens.cache_creation_1h_tokens.is_some();
    let cache_write_usd = if has_ttl {
        let split_5m = tokens.count(|t| t.cache_creation_5m_tokens);
        let split_1h = tokens.count(|t| t.cache_creation_1h_tokens);
        let total_write = tokens.count(|t| t.cache_creation_input_tokens);
        let residual = total_write.saturating_sub(split_5m.saturating_add(split_1h));
        split_5m as f64 * write_5m / 1_000_000.0
            + split_1h as f64 * write_1h / 1_000_000.0
            + residual as f64 * write_5m / 1_000_000.0
    } else {
        tokens.count(|t| t.cache_creation_input_tokens) as f64 * write_5m / 1_000_000.0
    };
    tokens.count(|t| t.input_tokens) as f64 * rates.input / 1_000_000.0
        + cache_write_usd
        + tokens.count(|t| t.cache_read_input_tokens) as f64 * rates.cache_read.unwrap_or(0.0) / 1_000_000.0
        + billed_output_tokens(tokens) as f64 * rates.output / 1_000_000.0
}

const ALLOWED_SOURCES: &[&str] = &["chat", "loop", "agentic", "eval"];

fn normalize_source(source: &str) -> &'static str {
    ALLOWED_SOURCES
        .iter()
        .copied()
        .find(|allowed| source.eq_ignore_ascii_case(allowed))
        .unwrap_or("unknown")
}

fn normalize_provider(provider: &str) -> String {
    let trimmed = provider.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        "unknown".into()
    } else {
        trimmed
    }
}

pub struct SpendEvent<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub served_model: Option<&'a str>,
    pub source: &'a str,
    pub tokens: UsageTokens,
    pub outcome: Option<&'a str>,
}

pub fn spend_path(home: &Path, cfg: &AppConfig) -> PathBuf {
    let raw = cfg.str_or("logging.spend_file", DEFAULT_SPEND_FILE);
    // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
    // Reconstruct from the checked string so the sanitized value reaches FS APIs.
    if raw.contains("..") {
        tracing::warn!("logging.spend_file refused parent-directory components; using default");
        return home.join(DEFAULT_SPEND_FILE);
    }
    if Path::new(&raw).is_absolute() {
        tracing::warn!("logging.spend_file refused an absolute path; using default");
        return home.join(DEFAULT_SPEND_FILE);
    }
    home.join(PathBuf::from(raw))
}

/// CodeQL rust/path-injection treats `contains("..") == false` as a sink barrier.
/// Reconstruct the path from the checked string so the sanitized value reaches FS APIs.
fn refuse_parent_components(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    if raw.contains("..") {
        return None;
    }
    Some(PathBuf::from(raw.as_ref()))
}

fn insert_token(map: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    map.insert(
        key.to_string(),
        match value {
            Some(n) => json!(n),
            None => Value::Null,
        },
    );
}

pub fn row_from_event(event: &SpendEvent<'_>) -> Value {
    let mut map = Map::new();
    map.insert("timestamp".into(), json!(crate::common::iso_now()));
    map.insert("provider".into(), json!(normalize_provider(event.provider)));
    map.insert("model".into(), json!(event.model));
    if let Some(served) = event.served_model.map(str::trim).filter(|s| !s.is_empty()) {
        map.insert("served_model".into(), json!(served));
    }
    map.insert("source".into(), json!(normalize_source(event.source)));
    insert_token(&mut map, "input_tokens", event.tokens.input_tokens);
    insert_token(&mut map, "output_tokens", event.tokens.output_tokens);
    insert_token(&mut map, "cached_input_tokens", event.tokens.cached_input_tokens);
    insert_token(&mut map, "reasoning_tokens", event.tokens.reasoning_tokens);
    insert_token(
        &mut map,
        "cache_creation_input_tokens",
        event.tokens.cache_creation_input_tokens,
    );
    insert_token(
        &mut map,
        "cache_read_input_tokens",
        event.tokens.cache_read_input_tokens,
    );
    insert_token(
        &mut map,
        "cache_creation_5m_tokens",
        event.tokens.cache_creation_5m_tokens,
    );
    insert_token(
        &mut map,
        "cache_creation_1h_tokens",
        event.tokens.cache_creation_1h_tokens,
    );
    insert_token(&mut map, "vendor_cost_ticks", event.tokens.vendor_cost_ticks);
    map.insert("usage_missing".into(), json!(event.tokens.usage_missing()));
    if let Some(outcome) = event.outcome.map(str::trim).filter(|s| !s.is_empty()) {
        map.insert("outcome".into(), json!(outcome));
    }
    Value::Object(map)
}

static APPEND_LOCK: Mutex<()> = Mutex::new(());

/// Append one JSONL row. Never raises. Does not write `usd`.
pub fn append_row(path: &Path, record: &Value, max_bytes: u64) {
    warn_once_if_stale();
    let Some(path) = refuse_parent_components(path) else {
        tracing::warn!("spend ledger path refused parent-directory components");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("spend ledger parent dir unavailable: {e}");
            return;
        }
    }
    let line = match serde_json::to_string(record) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("spend ledger failed to serialize: {e}");
            return;
        }
    };
    let _guard = APPEND_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if bounded_log::append(&path, &line, max_bytes).is_err() {
        tracing::warn!("spend sink unavailable, busy, or record exceeds retention bound");
    }
}

pub fn record(audit: Option<&Audit>, path: &Path, max_bytes: u64, event: SpendEvent<'_>) {
    let row = row_from_event(&event);
    match audit {
        Some(audit) => audit.append_spend(path, &row),
        None => append_row(path, &row, max_bytes),
    }
}

fn tokens_from_row(row: &Value) -> UsageTokens {
    UsageTokens {
        input_tokens: json_u64(row.get("input_tokens")),
        output_tokens: json_u64(row.get("output_tokens")),
        cached_input_tokens: json_u64(row.get("cached_input_tokens")),
        reasoning_tokens: json_u64(row.get("reasoning_tokens")),
        cache_creation_input_tokens: json_u64(row.get("cache_creation_input_tokens")),
        cache_read_input_tokens: json_u64(row.get("cache_read_input_tokens")),
        cache_creation_5m_tokens: json_u64(row.get("cache_creation_5m_tokens")),
        cache_creation_1h_tokens: json_u64(row.get("cache_creation_1h_tokens")),
        vendor_cost_ticks: json_u64(row.get("vendor_cost_ticks")),
    }
}

fn day_key(timestamp: &str) -> String {
    timestamp.chars().take(10).collect()
}

/// Read-time rollup by provider/model/UTC-day. Re-reads the append-only file.
pub fn summarize_file(path: &Path) -> Value {
    warn_once_if_stale();
    let mut rows: Vec<Value> = Vec::new();
    if let Some(path) = refuse_parent_components(path) {
        if path.exists() {
            let mut text = String::new();
            match std::fs::File::open(&path).and_then(|f| f.take(MAX_SUMMARY_BYTES + 1).read_to_string(&mut text)) {
                Ok(_) if (text.len() as u64) <= MAX_SUMMARY_BYTES => {
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(line) {
                            rows.push(v);
                        }
                    }
                }
                Ok(_) => tracing::warn!("spend ledger exceeds the summary bound; refusing to roll up"),
                Err(e) => tracing::warn!("spend ledger unreadable: {e}"),
            }
        }
    }
    type GroupKey = (String, String, String);
    type GroupAgg = (UsageTokens, usize, Option<f64>);
    let mut groups: BTreeMap<GroupKey, GroupAgg> = BTreeMap::new();
    for row in &rows {
        let provider = row
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let model = row.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let day = day_key(row.get("timestamp").and_then(Value::as_str).unwrap_or(""));
        let tokens = tokens_from_row(row);
        let est = estimate_usd(&model, &tokens, &provider);
        let entry = groups
            .entry((provider, model, day))
            .or_insert((UsageTokens::default(), 0, Some(0.0)));
        entry.1 += 1;
        add_opt(&mut entry.0.input_tokens, tokens.input_tokens);
        add_opt(&mut entry.0.output_tokens, tokens.output_tokens);
        add_opt(&mut entry.0.cached_input_tokens, tokens.cached_input_tokens);
        add_opt(&mut entry.0.reasoning_tokens, tokens.reasoning_tokens);
        add_opt(
            &mut entry.0.cache_creation_input_tokens,
            tokens.cache_creation_input_tokens,
        );
        add_opt(&mut entry.0.cache_read_input_tokens, tokens.cache_read_input_tokens);
        add_opt(&mut entry.0.cache_creation_5m_tokens, tokens.cache_creation_5m_tokens);
        add_opt(&mut entry.0.cache_creation_1h_tokens, tokens.cache_creation_1h_tokens);
        add_opt(&mut entry.0.vendor_cost_ticks, tokens.vendor_cost_ticks);
        match (entry.2, est.usd) {
            (Some(acc), Some(usd)) => entry.2 = Some(acc + usd),
            (Some(_), None) | (None, _) => entry.2 = None,
        }
    }
    let days: Vec<Value> = groups
        .into_iter()
        .map(|((provider, model, day), (tokens, calls, usd))| {
            let est = estimate_usd(&model, &tokens, &provider);
            json!({
                "day": day,
                "provider": provider,
                "model": model,
                "calls": calls,
                "input_tokens": tokens.input_tokens,
                "output_tokens": tokens.output_tokens,
                "estimate": {
                    "usd": usd,
                    "rate_unknown": est.rate_unknown,
                    "priced_as_of": est.priced_as_of,
                    "usd_source": if usd.is_some() { "sum_of_rows" } else { est.usd_source },
                    "rates_stale": rates_are_stale(None),
                }
            })
        })
        .collect();
    json!({
        "priced_as_of": priced_as_of(),
        "rates_stale": rates_are_stale(None),
        "stale_after_days": STALE_AFTER_DAYS,
        "rows": rows.len(),
        "days": days,
        "note": "Local Ollama/OpenAI-compat rows are unpriced. Warmup, pull, and MCP broker dispatch are excluded from this ledger by definition (not billed inference). Dollars are read-time only; the JSONL never stores usd. TokenTally and compaction estimate_tokens are not USD.",
    })
}

fn add_opt(dst: &mut Option<u64>, src: Option<u64>) {
    if let Some(n) = src {
        *dst = Some(dst.unwrap_or(0).saturating_add(n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grok_responses_fixture_prefers_ticks_and_table_path_matches_hand_arithmetic() {
        let body = json!({
            "model": "grok-4.20-0309-reasoning",
            "usage": {
                "input_tokens": 131,
                "output_tokens": 624,
                "input_tokens_details": {"cached_tokens": 128},
                "output_tokens_details": {"reasoning_tokens": 246},
                "cost_in_usd_ticks": 37_756_000
            }
        });
        let tokens = parse_grok_usage(body.get("usage"));
        assert_eq!(tokens.input_tokens, Some(131));
        assert_eq!(tokens.output_tokens, Some(624));
        assert_eq!(tokens.cached_input_tokens, Some(128));
        assert_eq!(tokens.reasoning_tokens, Some(246));
        assert_eq!(tokens.vendor_cost_ticks, Some(37_756_000));
        assert!(tokens.usage_reported());
        let with_ticks = estimate_usd("grok-4.6", &tokens, "grok");
        assert_eq!(with_ticks.usd_source, "vendor_ticks");
        assert!((with_ticks.usd.unwrap() - 37_756_000.0 / TICKS_PER_USD).abs() < 1e-18);
        let mut table_tokens = tokens.clone();
        table_tokens.vendor_cost_ticks = None;
        let table = estimate_usd("grok-4.6", &table_tokens, "grok");
        assert_eq!(table.usd_source, "rate_table");
        // uncached 3 * $2 + cached 128 * $0.50 + billed_out 870 * $6 per 1M
        let expected = 3.0 * 2.0 / 1e6 + 128.0 * 0.50 / 1e6 + 870.0 * 6.0 / 1e6;
        assert!((table.usd.unwrap() - expected).abs() < 1e-15, "{}", table.usd.unwrap());
        let row = row_from_event(&SpendEvent {
            provider: "grok",
            model: "grok-4.6",
            served_model: body.get("model").and_then(Value::as_str),
            source: "chat",
            tokens,
            outcome: None,
        });
        assert!(row.get("usd").is_none());
        assert_eq!(row["served_model"], "grok-4.20-0309-reasoning");
        assert_eq!(row["usage_missing"], false);
    }

    #[test]
    fn claude_cache_ttl_split_is_priced_at_5m_and_1h_rates() {
        let usage = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 30,
            "cache_read_input_tokens": 5,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 10,
                "ephemeral_1h_input_tokens": 20
            }
        });
        let tokens = parse_claude_usage(Some(&usage));
        assert_eq!(tokens.cache_creation_5m_tokens, Some(10));
        assert_eq!(tokens.cache_creation_1h_tokens, Some(20));
        let est = estimate_usd("claude-sonnet-5", &tokens, "claude");
        let expected = 100.0 * 2.0 / 1e6 + 10.0 * 2.50 / 1e6 + 20.0 * 4.00 / 1e6 + 5.0 * 0.20 / 1e6 + 50.0 * 10.0 / 1e6;
        assert_eq!(est.usd_source, "rate_table");
        assert!((est.usd.unwrap() - expected).abs() < 1e-15, "{}", est.usd.unwrap());
    }

    #[test]
    fn missing_usage_is_incomplete_and_not_reported() {
        let tokens = parse_grok_usage(None);
        assert!(tokens.usage_missing());
        assert!(!tokens.usage_reported());
        let est = estimate_usd("grok-4.6", &tokens, "grok");
        assert_eq!(est.usd_source, "incomplete");
        assert!(est.usd.is_none());
        assert!(!est.rate_unknown);
    }

    #[test]
    fn unknown_model_is_rate_unknown() {
        let tokens = UsageTokens {
            input_tokens: Some(10),
            output_tokens: Some(4),
            ..UsageTokens::default()
        };
        let est = estimate_usd("not-a-real-model", &tokens, "grok");
        assert!(est.rate_unknown);
        assert!(est.usd.is_none());
    }

    #[test]
    fn local_usage_is_unpriced() {
        let tokens = parse_local_usage(Some(&json!({"prompt_tokens": 11, "completion_tokens": 3})));
        assert_eq!(tokens.input_tokens, Some(11));
        let est = estimate_usd("qwen3.8:27b-mlx", &tokens, "local");
        assert_eq!(est.usd_source, "local_unpriced");
        assert!(est.usd.is_none());
    }

    #[test]
    fn jsonl_omits_prompt_and_credentials_and_survives_reread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("spend.jsonl");
        let grok = parse_grok_usage(Some(&json!({
            "input_tokens": 131,
            "output_tokens": 624,
            "input_tokens_details": {"cached_tokens": 128},
            "output_tokens_details": {"reasoning_tokens": 246},
            "cost_in_usd_ticks": 37_756_000
        })));
        record(
            None,
            &path,
            8 * 1024 * 1024,
            SpendEvent {
                provider: "grok",
                model: "grok-4.6",
                served_model: Some("grok-4.20-0309-reasoning"),
                source: "chat",
                tokens: grok,
                outcome: None,
            },
        );
        record(
            None,
            &path,
            8 * 1024 * 1024,
            SpendEvent {
                provider: "claude",
                model: "claude-sonnet-5",
                served_model: None,
                source: "chat",
                tokens: parse_claude_usage(Some(&json!({
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cache_creation_input_tokens": 30,
                    "cache_read_input_tokens": 5,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 10,
                        "ephemeral_1h_input_tokens": 20
                    }
                }))),
                outcome: None,
            },
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("usd"));
        assert!(!text.contains("SECRET"));
        assert!(!text.contains("sk-"));
        assert!(!text.contains("xai-"));
        assert!(!text.contains("failed_after_billing"));
        let first = summarize_file(&path);
        assert_eq!(first["rows"], 2);
        assert_eq!(first["days"].as_array().unwrap().len(), 2);
        record(
            None,
            &path,
            8 * 1024 * 1024,
            SpendEvent {
                provider: "local",
                model: "qwen3.8:27b-mlx",
                served_model: None,
                source: "chat",
                tokens: parse_local_usage(Some(&json!({"prompt_tokens": 11, "completion_tokens": 3}))),
                outcome: None,
            },
        );
        let again = summarize_file(&path);
        assert_eq!(again["rows"], 3);
        let local = again["days"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["provider"] == "local")
            .unwrap();
        assert!(local["estimate"]["usd"].is_null());
        assert_eq!(local["estimate"]["usd_source"], "local_unpriced");
    }

    #[test]
    fn empty_text_row_carries_failed_after_billing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend.jsonl");
        record(
            None,
            &path,
            8 * 1024 * 1024,
            SpendEvent {
                provider: "grok",
                model: "grok-4.6",
                served_model: None,
                source: "chat",
                tokens: parse_grok_usage(Some(&json!({"input_tokens": 2, "output_tokens": 0}))),
                outcome: Some("failed_after_billing"),
            },
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("failed_after_billing"));
        let row: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(row["outcome"], "failed_after_billing");
        assert_eq!(row["usage_missing"], false);
    }

    #[test]
    fn priced_as_of_is_the_oldest_verified_date() {
        assert_eq!(priced_as_of(), "2026-09-19");
        assert!(!rates_are_stale(
            Date::from_calendar_date(2026, time::Month::September, 19).ok()
        ));
        assert!(rates_are_stale(
            Date::from_calendar_date(2026, time::Month::November, 1).ok()
        ));
    }

    #[test]
    fn spend_path_refuses_absolute_and_parent_components() {
        let home = Path::new("/tmp/cgagent-home");
        let default = home.join(DEFAULT_SPEND_FILE);
        let ok = AppConfig::from_str(
            "logging:\n  spend_file: \"logs/custom.jsonl\"",
            Path::new("config.yaml"),
        )
        .unwrap();
        assert_eq!(spend_path(home, &ok), home.join("logs/custom.jsonl"));
        let traversal =
            AppConfig::from_str("logging:\n  spend_file: \"../escape.jsonl\"", Path::new("config.yaml")).unwrap();
        assert_eq!(spend_path(home, &traversal), default);
        let abs_value = if cfg!(windows) {
            r"C:\Windows\win.ini"
        } else {
            "/etc/passwd"
        };
        let abs_yaml = format!("logging:\n  spend_file: \"{abs_value}\"");
        let absolute = AppConfig::from_str(&abs_yaml, Path::new("config.yaml")).unwrap();
        assert_eq!(spend_path(home, &absolute), default);
    }

    #[test]
    fn append_and_summarize_refuse_parent_components() {
        let dir = tempfile::tempdir().unwrap();
        let evil = dir.path().join("logs").join("..").join("outside.jsonl");
        append_row(&evil, &json!({"provider": "local"}), 1024);
        assert!(!dir.path().join("outside.jsonl").exists());
        let summary = summarize_file(&evil);
        assert_eq!(summary["rows"], 0);
    }
}
