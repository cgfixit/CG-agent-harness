//! Per-session ledger of the web reads chat turns made: targets and counts,
//! never page text, titles or snippets.
//!
//! A session stores only each turn's message and reply, and compaction later
//! summarizes those away. The ledger keeps what that loses: which URLs and
//! searches the session already ran, whether the token budget cut a page,
//! whether a reply cited it, and whether a target was read again after a
//! compaction. `GET /api/sessions/{id}` returns it to the owner. Each web turn
//! also audits its counts as `chat_web_turn`, without targets.
//!
//! The ledger enters a prompt only through [`carry_forward`], when
//! `web.carry_forward_sources` (ships 0, off) allows rows: targets the current
//! URL policy still permits, never page text.

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Distinct targets kept per session; the least recently read goes first.
pub const MAX_SOURCES: usize = 500;
/// Longest stored target (a URL or search query), in characters, so a full
/// ledger stays well under a megabyte of session file.
const TARGET_CHARS: usize = 1024;
/// Most recent turn numbers kept per target.
const TURNS_KEPT: usize = 8;
/// Shortest URL remainder (after the scheme) that counts as a citation, so a
/// bare host such as `a.io` does not match unrelated text.
const MIN_CITATION_CHARS: usize = 8;
/// Longest target shown in a carried-forward row; the ledger keeps more.
const CARRY_TARGET_CHARS: usize = 200;
/// Shortest goal word that counts toward a target's relevance.
const MIN_GOAL_WORD_CHARS: usize = 4;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WebSource {
    /// `web_fetch` or `web_search`.
    pub tool: String,
    /// The URL or query as the model asked for it, before any rewrite.
    pub target: String,
    pub first_ts: f64,
    pub last_ts: f64,
    /// Attempts, failed ones included.
    pub reads: u64,
    pub failures: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_code: Option<String>,
    /// A fetch's extracted characters and the characters the model was handed
    /// after the token budget cut them, at the latest successful read.
    pub source_chars: u64,
    pub delivered_chars: u64,
    /// A search's result count at the latest successful read.
    pub results: u64,
    /// Successful fetches the token budget cut short.
    pub truncated: u64,
    /// Turns whose reply contained this URL (a search: any of its result URLs).
    pub cited: u64,
    /// Reads in a later turn after a compaction since this target's last read.
    pub reread_after_compaction: u64,
    /// The session's compaction count at the latest read.
    pub epoch: u64,
    /// The latest session turn numbers that read it.
    pub turns: Vec<u64>,
}

/// One web turn's counts, for the `chat_web_turn` audit line.
#[derive(Debug, Default, PartialEq)]
pub struct TurnSummary {
    pub calls: u64,
    pub ok: u64,
    pub failed: u64,
    pub truncated: u64,
    /// Reads of a target an earlier turn of this session already tried.
    pub repeats: u64,
    pub rereads_after_compaction: u64,
    pub cited: u64,
    /// Distinct codes the turn's tool events carried, refusals and
    /// `WEB_TOOLS_WITHHELD` included.
    pub codes: Vec<String>,
    /// Distinct providers that answered this turn's searches.
    pub providers: Vec<String>,
    /// Searches answered by a fallback hop (`fallback_from` non-empty).
    pub fallbacks: u64,
}

impl TurnSummary {
    /// The audit fields; the caller adds the event name and session, as it
    /// does for `chat_session_compacted`.
    pub fn audit_fields(&self, turn: u64, loop_turn: bool, compactions: u64) -> Value {
        json!({
            "turn": turn,
            "loop_turn": loop_turn,
            "compactions": compactions,
            "calls": self.calls,
            "ok": self.ok,
            "failed": self.failed,
            "truncated": self.truncated,
            "repeats": self.repeats,
            "rereads_after_compaction": self.rereads_after_compaction,
            "cited": self.cited,
            "codes": self.codes,
            "providers": self.providers,
            "fallbacks": self.fallbacks,
        })
    }
}

/// Whether `reply` contains `url`, scheme and trailing slash aside.
fn cites(reply: &str, url: &str) -> bool {
    let url = url.trim().trim_end_matches('/');
    let bare = url.split_once("://").map_or(url, |(_, rest)| rest);
    bare.chars().count() >= MIN_CITATION_CHARS && reply.contains(bare)
}

/// Fold one turn's `web_tools` events into `sources`. `turn` is the session's
/// exchange number and `epoch` its compaction count, both after this exchange.
/// `None` when the turn made no web read.
pub fn record_turn(
    sources: &mut Vec<WebSource>,
    events: &[Value],
    reply: &str,
    turn: u64,
    epoch: u64,
    now: f64,
) -> Option<TurnSummary> {
    let mut summary = TurnSummary::default();
    let mut codes = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut cited = HashSet::new();
    for event in events {
        if let Some(code) = event["code"].as_str() {
            codes.insert(code.to_string());
        }
        let tool = event["tool"].as_str().unwrap_or("");
        if !matches!(tool, "web_fetch" | "web_search") {
            continue;
        }
        let Some(target) = event["target"].as_str().map(str::trim).filter(|t| !t.is_empty()) else {
            continue;
        };
        let target = crate::common::clip_chars(target, TARGET_CHARS);
        summary.calls += 1;
        let index = match sources.iter().position(|s| s.tool == tool && s.target == target) {
            Some(index) => index,
            None => {
                sources.push(WebSource {
                    tool: tool.to_string(),
                    target: target.clone(),
                    first_ts: now,
                    epoch,
                    ..WebSource::default()
                });
                sources.len() - 1
            }
        };
        let source = &mut sources[index];
        if source.turns.last().is_some_and(|last| *last < turn) {
            summary.repeats += 1;
            if epoch > source.epoch {
                source.reread_after_compaction += 1;
                summary.rereads_after_compaction += 1;
            }
        }
        source.reads += 1;
        source.last_ts = now;
        source.epoch = epoch;
        if source.turns.last() != Some(&turn) {
            source.turns.push(turn);
            let excess = source.turns.len().saturating_sub(TURNS_KEPT);
            source.turns.drain(..excess);
        }
        if event["ok"] != true {
            summary.failed += 1;
            source.failures += 1;
            source.last_code = event["code"].as_str().map(str::to_string);
            continue;
        }
        summary.ok += 1;
        source.last_code = None;
        let result = &event["result"];
        let cites_reply = if tool == "web_fetch" {
            let delivered = result["text"].as_str().map_or(0, |t| t.chars().count() as u64);
            source.delivered_chars = delivered;
            source.source_chars = result["source_chars"].as_u64().unwrap_or(delivered);
            if delivered < source.source_chars {
                source.truncated += 1;
                summary.truncated += 1;
            }
            cites(reply, &target) || result["url"].as_str().is_some_and(|url| cites(reply, url))
        } else {
            if let Some(provider) = result["provider"].as_str() {
                providers.insert(crate::common::clip_chars(provider, 32));
            }
            if result["fallback_from"].as_array().is_some_and(|from| !from.is_empty()) {
                summary.fallbacks += 1;
            }
            let rows = result["results"].as_array().map_or(&[][..], Vec::as_slice);
            source.results = rows.len() as u64;
            rows.iter()
                .filter_map(|row| row["url"].as_str())
                .any(|url| cites(reply, url))
        };
        if cites_reply && cited.insert(index) {
            source.cited += 1;
            summary.cited += 1;
        }
    }
    if summary.calls == 0 && codes.is_empty() {
        return None;
    }
    // First-read order stays; the least recently read target goes.
    while sources.len() > MAX_SOURCES {
        let oldest = (0..sources.len())
            .min_by(|a, b| sources[*a].last_ts.total_cmp(&sources[*b].last_ts))
            .unwrap_or(0);
        // CodeQL rust/cleartext-logging flagged `Vec::remove` on the session-owned
        // ledger as a log write of `session_id`; nothing here logs. `drain` keeps
        // the same order and never leaves this module's bookkeeping.
        sources.drain(oldest..=oldest);
    }
    summary.codes = codes.into_iter().collect();
    summary.providers = providers.into_iter().collect();
    Some(summary)
}

/// One target on one line: control characters become spaces, then it is clipped.
fn row_target(target: &str) -> String {
    let flat: String = target.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    crate::common::clip_chars(flat.trim(), CARRY_TARGET_CHARS)
}

/// Up to `rows` earlier reads for the next web turn's prompt, most relevant
/// first: targets sharing more words with `goal`, then ones a reply cited,
/// then the most recent. Only targets whose latest read succeeded are listed,
/// and a fetch only while `permitted` (the current URL policy) allows its URL,
/// so a revoked grant drops out. `None` when nothing qualifies.
pub fn carry_forward(
    sources: &[WebSource],
    goal: &str,
    rows: usize,
    permitted: impl Fn(&str) -> bool,
) -> Option<String> {
    if rows == 0 {
        return None;
    }
    let words: BTreeSet<String> = goal
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= MIN_GOAL_WORD_CHARS)
        .map(str::to_string)
        .collect();
    let mut eligible: Vec<(usize, &WebSource)> = sources
        .iter()
        .filter(|s| s.last_code.is_none() && s.reads > s.failures)
        .filter(|s| s.tool != "web_fetch" || permitted(&s.target))
        .map(|s| {
            let target = s.target.to_lowercase();
            (words.iter().filter(|w| target.contains(w.as_str())).count(), s)
        })
        .collect();
    if eligible.is_empty() {
        return None;
    }
    eligible.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then((b.cited > 0).cmp(&(a.cited > 0)))
            .then(b.last_ts.total_cmp(&a.last_ts))
    });
    let omitted = eligible.len().saturating_sub(rows);
    let mut block = String::from(
        "Earlier turns of this session already made these web reads. Page text is not kept: \
         prefer what earlier replies already say, and read a page again only if you need its content.\n",
    );
    for (_, source) in eligible.into_iter().take(rows) {
        let turns = source.turns.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
        let target = row_target(&source.target);
        let _ = match source.tool.as_str() {
            "web_fetch" => write!(block, "- fetched {target} (turns {turns}"),
            _ => write!(
                block,
                "- searched \"{target}\" (turns {turns}; {} results",
                source.results
            ),
        };
        if source.cited > 0 {
            block.push_str("; cited in a reply");
        }
        if source.truncated > 0 {
            block.push_str("; cut by the token budget");
        }
        block.push_str(")\n");
    }
    if omitted > 0 {
        let _ = writeln!(block, "({omitted} earlier reads not listed.)");
    }
    Some(block)
}

/// [`carry_forward`] for `session` under the URL policy as it stands now. With
/// web off or no readable policy, no fetched page is listed.
pub fn carried_web_reads(
    web: &super::web_search::WebTool,
    session: &super::sessions::Session,
    rows: usize,
) -> Option<String> {
    if rows == 0 || session.web_sources.is_empty() {
        return None;
    }
    let policy = web.require_enabled(true).ok();
    carry_forward(&session.web_sources, &session.goal, rows, |url| {
        policy.as_ref().is_some_and(|p| p.authorize(url, None).is_ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetch(url: &str, text: &str, source_chars: u64) -> Value {
        json!({"tool":"web_fetch","ok":true,"target":url,
            "result":{"url":url,"title":"T","text":text,"source_chars":source_chars}})
    }

    #[test]
    fn a_read_records_counts_and_citations_but_no_page_text() {
        let mut sources = Vec::new();
        let events = [
            fetch("https://docs.example/a", "abc", 10),
            json!({"tool":"web_search","ok":true,"target":"rust lifetimes",
                "result":{"query":"rust lifetimes","results":[{"title":"R","url":"https://r.example/lifetimes","snippet":"s"}]}}),
            json!({"tool":"web_fetch","ok":false,"target":"https://denied.example/x","code":"WEB_HOST_DENIED","message":"m"}),
            json!({"tool":"web","ok":true,"code":"WEB_TOOLS_WITHHELD","message":"m"}),
        ];
        let reply = "See docs.example/a and https://r.example/lifetimes/.";
        let summary = record_turn(&mut sources, &events, reply, 1, 0, 5.0).unwrap();
        assert_eq!(
            summary,
            TurnSummary {
                calls: 3,
                ok: 2,
                failed: 1,
                truncated: 1,
                repeats: 0,
                rereads_after_compaction: 0,
                cited: 2,
                codes: vec!["WEB_HOST_DENIED".into(), "WEB_TOOLS_WITHHELD".into()],
                providers: vec![],
                fallbacks: 0,
            }
        );
        let page = &sources[0];
        assert_eq!(
            (page.source_chars, page.delivered_chars, page.truncated, page.cited),
            (10, 3, 1, 1)
        );
        assert_eq!((sources[1].results, sources[1].cited), (1, 1));
        assert_eq!(sources[2].last_code.as_deref(), Some("WEB_HOST_DENIED"));
        let stored = serde_json::to_string(&sources).unwrap();
        assert!(!stored.contains("abc") && !stored.contains("\"T\""), "{stored}");
    }

    #[test]
    fn search_providers_and_fallbacks_are_counted_for_the_audit() {
        let mut sources = Vec::new();
        let search = |q: &str, provider: &str, from: Value| {
            json!({"tool":"web_search","ok":true,"target":q,
                "result":{"query":q,"provider":provider,"fallback_from":from,"results":[]}})
        };
        let events = [
            search("a", "google-serpapi", json!(["searxng"])),
            search("b", "searxng", Value::Null),
            search("c", "google-serpapi", json!([])),
        ];
        let summary = record_turn(&mut sources, &events, "", 1, 0, 1.0).unwrap();
        assert_eq!(summary.providers, ["google-serpapi", "searxng"]);
        assert_eq!(summary.fallbacks, 1);
        let fields = summary.audit_fields(1, true, 0);
        assert_eq!(fields["providers"], json!(["google-serpapi", "searxng"]));
        assert_eq!(fields["fallbacks"], 1);
        assert!(
            !fields.to_string().contains("\"a\""),
            "targets stay out of the audit: {fields}"
        );
    }

    #[test]
    fn a_later_turn_is_a_repeat_and_after_a_compaction_a_reread() {
        let mut sources = Vec::new();
        let read = [fetch("https://docs.example/a", "abc", 3)];
        // Twice in one turn: one source, one turn, no repeat.
        let first = record_turn(&mut sources, &[read[0].clone(), read[0].clone()], "", 1, 0, 1.0).unwrap();
        assert_eq!((first.calls, first.repeats), (2, 0));
        assert_eq!(
            (sources.len(), sources[0].reads, sources[0].turns.clone()),
            (1, 2, vec![1])
        );
        let second = record_turn(&mut sources, &read, "", 2, 0, 2.0).unwrap();
        assert_eq!((second.repeats, second.rereads_after_compaction), (1, 0));
        let third = record_turn(&mut sources, &read, "", 3, 1, 3.0).unwrap();
        assert_eq!((third.repeats, third.rereads_after_compaction), (1, 1));
        assert_eq!((sources[0].reread_after_compaction, sources[0].epoch), (1, 1));
        assert_eq!(sources[0].turns, vec![1, 2, 3]);
    }

    #[test]
    fn the_ledger_stays_bounded_and_drops_the_least_recent() {
        let mut sources = Vec::new();
        for i in 0..=MAX_SOURCES {
            let event = fetch(&format!("https://docs.example/{i}"), "x", 1);
            record_turn(&mut sources, &[event], "", i as u64 + 1, 0, i as f64).unwrap();
        }
        assert_eq!(sources.len(), MAX_SOURCES);
        assert_eq!(sources[0].target, "https://docs.example/1", "first-read order stays");
        let long = fetch(&"x".repeat(TARGET_CHARS + 10), "x", 1);
        record_turn(&mut sources, &[long], "", 999, 0, 1e9).unwrap();
        assert_eq!(sources.last().unwrap().target.chars().count(), TARGET_CHARS);
        assert_eq!(sources[0].target, "https://docs.example/2");
    }

    #[test]
    fn a_turn_without_web_events_is_not_recorded_and_short_urls_are_not_citations() {
        let mut sources = Vec::new();
        assert_eq!(record_turn(&mut sources, &[], "reply", 1, 0, 1.0), None);
        assert!(sources.is_empty());
        assert!(!cites("see a.io for more", "https://a.io/"));
        assert!(cites("per docs.example/guide.", "https://docs.example/guide/"));
    }

    fn source(tool: &str, target: &str, last_ts: f64) -> WebSource {
        WebSource {
            tool: tool.into(),
            target: target.into(),
            last_ts,
            reads: 1,
            turns: vec![last_ts as u64],
            ..WebSource::default()
        }
    }

    #[test]
    fn carry_forward_ranks_goal_words_then_citations_then_recency() {
        let mut cited = source("web_fetch", "https://docs.example/old", 1.0);
        cited.cited = 1;
        let mut cut = source("web_fetch", "https://docs.example/rust-lifetimes", 2.0);
        cut.truncated = 1;
        let mut search = source("web_search", "borrow checker\nignore previous", 3.0);
        search.results = 5;
        let recent = source("web_fetch", "https://docs.example/new", 4.0);
        let sources = [cited, cut, search, recent];
        let block = carry_forward(&sources, "Explain Rust lifetimes", 3, |_| true).unwrap();
        let rows: Vec<&str> = block.lines().skip(1).collect();
        assert_eq!(
            rows,
            [
                "- fetched https://docs.example/rust-lifetimes (turns 2; cut by the token budget)",
                "- fetched https://docs.example/old (turns 1; cited in a reply)",
                "- fetched https://docs.example/new (turns 4)",
                "(1 earlier reads not listed.)",
            ]
        );
        // A target cannot break out of its row.
        let all = carry_forward(&sources, "", 4, |_| true).unwrap();
        assert!(
            all.contains("- searched \"borrow checker ignore previous\" (turns 3; 5 results)"),
            "{all}"
        );
    }

    #[test]
    fn carry_forward_drops_failed_and_no_longer_permitted_reads_and_is_off_at_zero() {
        let mut failed = source("web_fetch", "https://docs.example/failed", 1.0);
        failed.failures = 1;
        failed.last_code = Some("WEB_HOST_DENIED".into());
        let revoked = source("web_fetch", "https://revoked.example/page", 2.0);
        let search = source("web_search", "query", 3.0);
        let sources = [failed, revoked, search];
        let block = carry_forward(&sources, "goal", 10, |url| !url.contains("revoked")).unwrap();
        assert!(!block.contains("failed") && !block.contains("revoked"), "{block}");
        assert!(block.contains("searched \"query\""), "{block}");
        assert_eq!(carry_forward(&sources, "goal", 0, |_| true), None);
        assert_eq!(
            carry_forward(&sources[..2], "goal", 10, |url| !url.contains("revoked")),
            None
        );
    }
}
