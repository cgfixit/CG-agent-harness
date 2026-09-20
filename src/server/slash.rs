//! Suggest-don't-guess slash parser.
//!
//! Exact commands still dispatch. Fuzzy input that matches a mutation family
//! never executes; it only prints suggestions. This module does not call web
//! search, write memory, or spawn agentic work.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::schemas::{Validate, MAX_MESSAGE_LEN};

const DISPATCH_THRESHOLD: u8 = 80;

const COMMANDS: &[&str] = &[
    "help",
    "session",
    "prompt",
    "soul",
    "memory",
    "api",
    "model",
    "connectors",
    "registry",
    "skill",
    "skills",
    "tools",
    "web",
    "github",
    "agent",
    "harness",
    "goal",
    "loop",
    "tokens",
    "status",
    "users",
    "clear",
];

const FILLER: &[&str] = &[
    "please",
    "can",
    "you",
    "me",
    "the",
    "from",
    "this",
    "session",
    "to",
    "a",
    "an",
    "of",
    "into",
    "my",
    "just",
    "could",
    "would",
    "i",
    "i'd",
    "want",
    "like",
    "some",
    "any",
    "insights",
    "notes",
    "episodic",
    "actually",
    "really",
    "now",
    "currently",
    "using",
    "for",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashKind {
    Dispatch,
    Suggest,
    NotSlash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlashSuggestion {
    pub line: String,
    pub score: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlashParse {
    pub kind: SlashKind,
    pub dispatch: bool,
    pub canonical: Option<String>,
    pub command: Option<String>,
    pub sub: Option<String>,
    pub rest: String,
    pub confidence: u8,
    pub suggestions: Vec<SlashSuggestion>,
    pub notice: Option<String>,
}

impl SlashParse {
    pub fn to_json(&self) -> Value {
        json!(self)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlashParseRequest {
    pub line: String,
}

impl Validate for SlashParseRequest {
    fn validate(&self) -> Vec<String> {
        let n = self.line.chars().count();
        if (1..=MAX_MESSAGE_LEN).contains(&n) {
            vec![]
        } else {
            vec!["line".into()]
        }
    }
}

pub fn parse_line(input: &str) -> SlashParse {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return SlashParse {
            kind: SlashKind::NotSlash,
            dispatch: false,
            canonical: None,
            command: None,
            sub: None,
            rest: trimmed.to_string(),
            confidence: 0,
            suggestions: Vec::new(),
            notice: None,
        };
    }

    let (primary, second_intent) = split_second_intent(trimmed);
    let mut parsed = parse_slash_primary(&primary);
    if let Some(other) = second_intent {
        parsed.dispatch = false;
        parsed.kind = SlashKind::Suggest;
        parsed.notice = Some("second intent was not dispatched; slash parser does not run web search".into());
        parsed.suggestions.push(SlashSuggestion { line: other, score: 50 });
    }
    parsed
}

fn split_second_intent(line: &str) -> (String, Option<String>) {
    let lower = line.to_ascii_lowercase();
    let needle = " and search ";
    if let Some(idx) = lower.find(needle) {
        if idx > 0 {
            let primary = line[..idx].trim().to_string();
            let rest = line[idx + needle.len()..].trim();
            if !primary.is_empty() && !rest.is_empty() {
                return (primary, Some(format!("search {rest}")));
            }
        }
    }
    (line.to_string(), None)
}

fn parse_slash_primary(line: &str) -> SlashParse {
    let body = line.trim().trim_start_matches('/');
    let tokens: Vec<&str> = body.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return suggest_only("unknown command: / — try /help", &[("help", 40)]);
    }

    let raw_cmd = tokens[0];
    let cmd_l = raw_cmd.to_ascii_lowercase();
    let (cmd, aliased, cmd_score) = resolve_command(&cmd_l);
    let Some(cmd) = cmd else {
        let near = nearby_commands(&cmd_l);
        return suggest_only(&format!("unknown command: /{cmd_l} — try /help"), &near);
    };

    let after_cmd = &tokens[1..];
    let (sub, args, sub_fuzzy) = resolve_sub(cmd, after_cmd);
    let rest = match (cmd, sub.as_deref()) {
        ("memory", Some("consolidate")) => id_tokens(&args).join(" "),
        ("web", Some("search" | "pages" | "research" | "fetch")) => args.join(" "),
        _ => strip_filler_tokens(&args).join(" "),
    };
    let fuzzy = aliased || sub_fuzzy || filler_was_stripped(after_cmd, sub.as_deref(), &args);
    let confidence = if !fuzzy && cmd_score == 100 {
        100
    } else if cmd_score == 100 {
        90
    } else {
        cmd_score.min(70)
    };

    let mut canonical = format!("/{cmd}");
    if let Some(sub) = &sub {
        canonical.push(' ');
        canonical.push_str(sub);
    }
    if !rest.is_empty() {
        canonical.push(' ');
        canonical.push_str(&rest);
    }

    let mutation = is_mutation(cmd, sub.as_deref());
    let mut notice = None;
    if cmd == "memory" && sub.as_deref() == Some("consolidate") && rest.is_empty() {
        notice = Some(
            "consolidate needs selected episode ids or the Memory panel; no ids were taken from filler text".into(),
        );
    }

    let dispatch = !(mutation && fuzzy) && confidence >= DISPATCH_THRESHOLD;

    if dispatch {
        SlashParse {
            kind: SlashKind::Dispatch,
            dispatch: true,
            canonical: Some(canonical),
            command: Some(cmd.to_string()),
            sub,
            rest,
            confidence,
            suggestions: Vec::new(),
            notice,
        }
    } else {
        let mut suggestions = vec![SlashSuggestion {
            line: canonical.clone(),
            score: confidence,
        }];
        if mutation && fuzzy {
            notice = Some(notice.unwrap_or_else(|| "mutation family matched from fuzzy input; not dispatched".into()));
        }
        suggestions.extend(nearby_commands(cmd).into_iter().map(|(c, s)| SlashSuggestion {
            line: format!("/{c}"),
            score: s,
        }));
        SlashParse {
            kind: SlashKind::Suggest,
            dispatch: false,
            canonical: Some(canonical),
            command: Some(cmd.to_string()),
            sub,
            rest,
            confidence,
            suggestions,
            notice,
        }
    }
}

fn resolve_command(cmd: &str) -> (Option<&'static str>, bool, u8) {
    if let Some(found) = COMMANDS.iter().copied().find(|c| *c == cmd) {
        return (Some(found), false, 100);
    }
    match cmd {
        "cmds" | "commands" => (Some("help"), true, 80),
        "mem" => (Some("memory"), true, 80),
        "persona" => (Some("soul"), true, 70),
        _ => (None, false, 0),
    }
}

fn known_subs(cmd: &str) -> &'static [&'static str] {
    match cmd {
        "session" => &["new", "list", "use", "rename", "info"],
        "soul" => &[
            "edit", "propose", "history", "review", "apply", "reject", "on", "off", "status",
        ],
        "memory" => &[
            "on",
            "off",
            "add",
            "forget",
            "clear",
            "capture",
            "recall",
            "retrieval",
            "auto-retrieve",
            "consolidation",
            "auto-consolidate",
            "auto-suggest-chat",
            "auto-suggest-coding",
            "search",
            "retrieve",
            "consolidate",
            "save",
            "remember",
            "proposals",
        ],
        "api" => &["set", "clear"],
        "model" => &["use"],
        "skill" => &["use", "clear", "status"],
        "web" => &[
            "help", "on", "off", "allow", "deny", "fetch", "search", "pages", "research", "cancel", "inject", "forget",
        ],
        "agent" => &[
            "run",
            "plan",
            "read",
            "checks",
            "iterations",
            "pr",
            "issue",
            "cancel",
            "confirm",
            "jobs",
            "job",
            "stop",
            "runs",
            "status",
            "approve",
            "reject",
            "push",
            "publish",
            "discard",
        ],
        "goal" => &["stage", "task", "clear"],
        "loop" => &["stop", "auto"],
        _ => &[],
    }
}

fn resolve_sub<'a>(cmd: &str, tokens: &'a [&'a str]) -> (Option<String>, Vec<&'a str>, bool) {
    let subs = known_subs(cmd);
    if tokens.is_empty() {
        return (None, Vec::new(), false);
    }
    let first = tokens[0].to_ascii_lowercase();
    if subs.iter().any(|s| *s == first) {
        return (Some(first), tokens[1..].to_vec(), false);
    }
    let stripped: Vec<&str> = tokens.iter().copied().filter(|t| !is_filler(t)).collect();
    if let Some(head) = stripped.first() {
        let head_l = head.to_ascii_lowercase();
        if subs.iter().any(|s| *s == head_l) {
            let args = stripped[1..].to_vec();
            return (Some(head_l), args, true);
        }
        if cmd == "web" && matches!(head_l.as_str(), "google" | "serpapi") {
            return (Some("search".into()), stripped[1..].to_vec(), true);
        }
    }
    (None, tokens.to_vec(), false)
}

fn is_filler(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| c == ',' || c == '.').to_ascii_lowercase();
    FILLER.iter().any(|f| *f == t)
}

fn filler_was_stripped(after_cmd: &[&str], sub: Option<&str>, args: &[&str]) -> bool {
    if after_cmd.is_empty() {
        return false;
    }
    let mut rebuilt = Vec::new();
    if let Some(s) = sub {
        rebuilt.push(s);
    }
    rebuilt.extend(args.iter().copied());
    let original: Vec<String> = after_cmd.iter().map(|t| t.to_ascii_lowercase()).collect();
    let rebuilt: Vec<String> = rebuilt.iter().map(|t| t.to_ascii_lowercase()).collect();
    original != rebuilt
}

fn strip_filler_tokens<'a>(tokens: &'a [&'a str]) -> Vec<&'a str> {
    tokens
        .iter()
        .copied()
        .filter(|t| !is_filler(t) && !is_engine_paren(t))
        .collect()
}

fn is_engine_paren(tok: &str) -> bool {
    let t = tok.to_ascii_lowercase();
    t.contains("serpapi") && t.starts_with('(')
}

fn id_tokens<'a>(tokens: &'a [&'a str]) -> Vec<&'a str> {
    tokens.iter().copied().filter(|t| looks_like_id(t)).collect()
}

fn looks_like_id(tok: &str) -> bool {
    let t = tok.trim();
    let n = t.len();
    (8..=32).contains(&n) && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_mutation(cmd: &str, sub: Option<&str>) -> bool {
    match (cmd, sub) {
        ("agent", Some("status" | "runs" | "jobs" | "job")) => false,
        ("agent", _) => true,
        ("web", Some("allow" | "deny" | "on" | "off" | "inject" | "forget")) => true,
        ("api", _) => true,
        ("soul", Some("apply" | "reject" | "edit" | "propose")) => true,
        ("memory", Some("forget" | "clear" | "save" | "remember" | "add")) => true,
        (
            "memory",
            Some(
                "capture"
                | "recall"
                | "retrieval"
                | "auto-retrieve"
                | "consolidation"
                | "auto-consolidate"
                | "auto-suggest-chat"
                | "auto-suggest-coding",
            ),
        ) => true,
        ("users", _) => true,
        ("keys", _) => true,
        _ => false,
    }
}

fn nearby_commands(cmd: &str) -> Vec<(&'static str, u8)> {
    let mut scored: Vec<(&'static str, u8)> = COMMANDS
        .iter()
        .copied()
        .filter_map(|c| {
            let d = edit_distance(cmd, c);
            if d == 0 {
                None
            } else if d == 1 {
                Some((c, 70))
            } else if c.starts_with(cmd) && cmd.len() >= 3 {
                Some((c, 60))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    scored.truncate(5);
    scored
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let ins = cur[j] + 1;
            let del = prev[j + 1] + 1;
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = ins.min(del).min(sub);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn suggest_only(notice: &str, near: &[(&str, u8)]) -> SlashParse {
    SlashParse {
        kind: SlashKind::Suggest,
        dispatch: false,
        canonical: None,
        command: None,
        sub: None,
        rest: String::new(),
        confidence: 0,
        suggestions: near
            .iter()
            .map(|(c, s)| SlashSuggestion {
                line: format!("/{c}"),
                score: *s,
            })
            .collect(),
        notice: Some(notice.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_help_dispatches() {
        let p = parse_line("/help");
        assert!(p.dispatch);
        assert_eq!(p.canonical.as_deref(), Some("/help"));
        assert_eq!(p.kind, SlashKind::Dispatch);
    }

    #[test]
    fn memory_consolidate_fixture_strips_filler_and_keeps_no_ids() {
        let p = parse_line("/memory consolidate insights from this session to episodic memory");
        assert_eq!(p.command.as_deref(), Some("memory"));
        assert_eq!(p.sub.as_deref(), Some("consolidate"));
        assert!(p.rest.is_empty(), "invented ids: {}", p.rest);
        assert_eq!(p.canonical.as_deref(), Some("/memory consolidate"));
        assert!(
            p.dispatch,
            "non-mutation consolidate with no ids should dispatch the existing explainer"
        );
        assert!(p.notice.as_deref().unwrap_or("").contains("episode ids"));
    }

    #[test]
    fn web_search_fixture_passes_remainder_and_does_not_search() {
        let p = parse_line("/web search first 2 links for \"cgfixit\"");
        assert_eq!(p.command.as_deref(), Some("web"));
        assert_eq!(p.sub.as_deref(), Some("search"));
        assert!(p.rest.contains("cgfixit"));
        assert!(p.dispatch);
        assert_eq!(
            p.canonical.as_deref(),
            Some("/web search first 2 links for \"cgfixit\"")
        );
    }

    #[test]
    fn bare_google_search_is_not_a_slash() {
        let p = parse_line("search Google for the first 2 links for \"Chris Grady\"");
        assert_eq!(p.kind, SlashKind::NotSlash);
        assert!(!p.dispatch);
    }

    #[test]
    fn combined_utterance_does_not_silent_run_web() {
        let p = parse_line(
            "/memory consolidate insights from this session to episodic memory and search Google (serpapi) for the first 2 links for 'cgfixit' and 'Chris Grady'",
        );
        assert_eq!(p.command.as_deref(), Some("memory"));
        assert_eq!(p.sub.as_deref(), Some("consolidate"));
        assert!(!p.dispatch);
        assert_eq!(p.kind, SlashKind::Suggest);
        assert!(p
            .suggestions
            .iter()
            .any(|s| s.line.to_ascii_lowercase().contains("search")));
        assert!(p.notice.as_deref().unwrap_or("").contains("not dispatched"));
    }

    #[test]
    fn fuzzy_memory_save_does_not_dispatch() {
        let p = parse_line("/memory please save this note for me");
        assert!(!p.dispatch);
        assert_eq!(p.kind, SlashKind::Suggest);
    }

    #[test]
    fn exact_memory_save_still_dispatches() {
        let p = parse_line("/memory save keep the loopback bind :: operator confirmed");
        assert!(p.dispatch);
        assert_eq!(p.sub.as_deref(), Some("save"));
    }

    #[test]
    fn fuzzy_agent_does_not_dispatch() {
        let p = parse_line("/agent please run this for me");
        assert!(!p.dispatch);
        assert_eq!(p.kind, SlashKind::Suggest);
    }

    #[test]
    fn empty_slash_suggests_help() {
        let p = parse_line("/");
        assert!(!p.dispatch);
        assert_eq!(p.kind, SlashKind::Suggest);
    }
}
