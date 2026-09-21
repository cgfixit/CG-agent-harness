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
    "analytics",
    "help",
    "session",
    "prompt",
    "soul",
    "style",
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

    // Memory commands are single-line operator intent, never pasted scripts.
    let root = trimmed.split_whitespace().next().unwrap_or("");
    if matches!(root.to_ascii_lowercase().as_str(), "/memory" | "/mem")
        && input
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return suggest_only(
            "memory commands require one line without control characters; not dispatched",
            &[],
        );
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
    // Only the conversational consolidation form has a second-intent grammar.
    // In an exact save/search/rename command, these words belong to its data.
    if !lower.starts_with("/memory consolidate ") && !lower.starts_with("/mem consolidate ") {
        return (line.to_string(), None);
    }
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
    let body = line.trim().strip_prefix('/').unwrap_or(line);
    if body.starts_with('/') || body.starts_with(char::is_whitespace) {
        return suggest_only(
            "use one slash immediately before the command — try /help",
            &[("help", 40)],
        );
    }
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
    // Goals and loop counts have a free-form/numeric fallback. Other families
    // can suggest a mistyped subcommand without granting it execution authority.
    if sub.is_none() && !matches!(cmd, "goal" | "loop") {
        if let Some(raw_sub) = after_cmd.first() {
            let near = nearby_tokens(&raw_sub.to_ascii_lowercase(), known_subs(cmd));
            if !near.is_empty() {
                let mut parsed = suggest_only("unknown subcommand; suggestions were not dispatched", &[]);
                let tail = if after_cmd.len() > 1 {
                    format!(" {}", after_cmd[1..].join(" "))
                } else {
                    String::new()
                };
                parsed.suggestions = near
                    .into_iter()
                    .map(|(sub, score)| SlashSuggestion {
                        line: format!("/{cmd} {sub}{tail}"),
                        score,
                    })
                    .collect();
                return parsed;
            }
        }
    }
    if cmd == "memory" && sub.is_none() && !after_cmd.is_empty() {
        return suggest_only("unknown memory subcommand; not dispatched — use /help", &[]);
    }
    if let Some(notice) = mutation_argument_refusal(cmd, sub.as_deref(), &args) {
        return suggest_only(notice, &[("help", 100)]);
    }
    let rest = match (cmd, sub.as_deref()) {
        ("memory", Some("consolidate")) => id_tokens(&args).join(" "),
        ("web", Some("search" | "pages" | "research" | "fetch")) => args.join(" "),
        // A style id is opaque: an overlay may be called `notes` or
        // `session`, which the filler list would otherwise swallow.
        ("style", _) => args.join(" "),
        _ => args.join(" "),
    };
    let fuzzy = aliased
        || sub_fuzzy
        || filler_was_stripped(after_cmd, sub.as_deref(), &args)
        || (cmd == "memory" && sub.as_deref() == Some("consolidate") && rest != args.join(" "));
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

    let mutation = is_mutation(cmd, sub.as_deref())
        || (cmd == "memory" && sub.as_deref() == Some("consolidate") && !rest.is_empty());
    let mut notice = None;
    if cmd == "memory" && sub.as_deref() == Some("consolidate") && rest.is_empty() {
        notice = Some(
            "consolidate needs selected episode ids or the Memory panel; no ids were taken from filler text".into(),
        );
    }

    let dispatch = !((mutation || cmd == "memory") && fuzzy) && confidence >= DISPATCH_THRESHOLD;

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
        if cmd == "memory" && fuzzy {
            notice = Some("memory requires an exact command; suggestions were not dispatched".into());
        } else if mutation && fuzzy {
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
            "pr-body",
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
    // Once a subcommand is found, all remaining words are its payload.
    let stripped: Vec<&str> = tokens.iter().copied().skip_while(|t| is_filler(t)).collect();
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
        ("web", Some("allow" | "deny" | "on" | "off" | "inject" | "forget" | "cancel")) => true,
        ("api", _) => true,
        ("soul", Some("apply" | "reject" | "edit" | "propose")) => true,
        ("soul", Some("on" | "off")) => true,
        ("memory", Some("on" | "off" | "forget" | "clear" | "save" | "remember" | "add")) => true,
        ("session", Some("new" | "rename" | "use")) => true,
        ("goal" | "style" | "model" | "skill" | "loop" | "clear", _) => true,
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

fn mutation_argument_refusal(cmd: &str, sub: Option<&str>, args: &[&str]) -> Option<&'static str> {
    // Validate fixed operands before console dispatch so ignored extra text
    // cannot authorize an action. Keep free-form command payloads intact.
    let max_args = match (cmd, sub) {
        ("memory", Some("on" | "off" | "clear" | "proposals"))
        | ("soul", Some("on" | "off" | "edit" | "propose"))
        | ("skill", Some("clear"))
        | ("web", Some("on" | "off" | "inject" | "forget" | "cancel"))
        | ("agent", Some("cancel"))
        | ("loop", Some("auto" | "stop"))
        | ("clear", None) => Some(0),
        (
            "memory",
            Some(
                "forget"
                | "capture"
                | "recall"
                | "retrieval"
                | "auto-retrieve"
                | "consolidation"
                | "auto-consolidate"
                | "auto-suggest-chat"
                | "auto-suggest-coding",
            ),
        )
        | ("session", Some("use"))
        | ("api", Some("clear"))
        | ("agent", Some("plan" | "iterations" | "pr" | "issue" | "stop" | "discard" | "pr-body"))
        | ("goal", Some("stage"))
        | ("loop", None) => Some(1),
        ("skill", None) if args.first().is_some_and(|arg| arg.starts_with("check:")) => Some(1),
        ("web", Some("allow")) => Some(3),
        _ => None,
    };
    if max_args.is_some_and(|max| args.len() > max || args.iter().any(|arg| is_control_flag(arg))) {
        return Some("unsupported arguments; command not dispatched — use /help for syntax");
    }

    if cmd == "memory" && sub == Some("retrieve") && args.first().is_some_and(|arg| is_control_flag(arg)) {
        return Some("retrieve starts a chat; help or dry-run flags are not supported — use /help");
    }

    let reason_start = match (cmd, sub) {
        ("agent", Some("confirm")) => Some(0),
        ("agent", Some("approve" | "reject" | "push" | "publish")) | ("soul", Some("apply" | "reject")) => Some(1),
        _ => None,
    };
    let reason_is_help = reason_start.is_some_and(|start| is_help_reason(args.get(start..).unwrap_or_default()))
        || (matches!((cmd, sub), ("memory", Some("save" | "remember")))
            && args
                .join(" ")
                .rsplit_once("::")
                .is_some_and(|(_, reason)| is_help_reason(&reason.split_whitespace().collect::<Vec<_>>())));
    if reason_is_help {
        return Some("help or dry-run text is not an approval reason; command not dispatched — use /help");
    }
    None
}

fn is_control_flag(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "-h" | "--help" | "--dry-run" | "--dryrun"
    )
}

fn is_help_reason(tokens: &[&str]) -> bool {
    tokens.first().is_some_and(|token| is_control_flag(token))
        || (tokens.len() == 1 && matches!(tokens[0].to_ascii_lowercase().as_str(), "help" | "?"))
}

fn nearby_commands(cmd: &str) -> Vec<(&'static str, u8)> {
    nearby_tokens(cmd, COMMANDS)
}

fn nearby_tokens<'a>(cmd: &str, candidates: &[&'a str]) -> Vec<(&'a str, u8)> {
    let mut scored: Vec<(&str, u8)> = candidates
        .iter()
        .copied()
        .filter_map(|c| {
            let d = edit_distance(cmd, c);
            if d == 0 {
                None
            } else if d == 1 || adjacent_transposition(cmd, c) {
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

fn adjacent_transposition(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len() != b.len() {
        return false;
    }
    let differences: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    matches!(differences.as_slice(), [i, j] if *j == i + 1 && a[*i] == b[*j] && a[*j] == b[*i])
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
    fn all_fuzzy_memory_forms_are_suggestions_only() {
        for line in [
            "/memory please retrieve private preference",
            "/memory can you search private preference",
            "/memory please proposals",
            "/memory consolidate insights from this session",
            "/mem retrieve private preference",
            "/mem",
            "/memory something-unrecognized",
        ] {
            let parsed = parse_line(line);
            assert!(!parsed.dispatch, "{line}: {parsed:?}");
            assert_eq!(parsed.kind, SlashKind::Suggest);
        }
    }

    #[test]
    fn memory_control_characters_and_ignored_operands_never_dispatch() {
        for line in [
            "/memory\nclear",
            "/memory\tclear",
            "/memory clear\n",
            "/memory\r\noff",
            "/memory\u{2028}clear",
            "/memory\u{2029}clear",
            "/memory add first line\n/memory clear",
            "/memory save first line\nsecond line :: reason",
            "/memory proposals and then clear",
            "/memory retrieve --help",
            "/memory retrieve --dry-run private preference",
        ] {
            assert!(!parse_line(line).dispatch, "{line:?}");
        }
        for line in [
            "/memory",
            "/memory proposals",
            "/MEMORY CAPTURE OFF",
            "/memory retrieve private preference",
            "/memory search private preference",
            "/memory consolidate 123456789abcdef0123456789abcdef0",
            "/memory add literal --help text",
            "/memory save note :: operator requested",
        ] {
            assert!(parse_line(line).dispatch, "{line:?}");
        }
    }

    #[test]
    fn exact_help_dispatches() {
        let p = parse_line("/help");
        assert!(p.dispatch);
        assert_eq!(p.canonical.as_deref(), Some("/help"));
        assert_eq!(p.kind, SlashKind::Dispatch);
    }

    #[test]
    fn exact_analytics_dispatches_as_a_read() {
        let p = parse_line("/analytics");
        assert!(p.dispatch);
        assert_eq!(p.confidence, 100);
        assert_eq!(p.canonical.as_deref(), Some("/analytics"));
        assert_eq!(p.kind, SlashKind::Dispatch);
    }

    #[test]
    fn exact_style_dispatches_with_its_name() {
        for (line, canonical) in [
            ("/style concise", "/style concise"),
            ("/style off", "/style off"),
            ("/style", "/style"),
            // Overlay ids that collide with filler words stay intact.
            ("/style notes", "/style notes"),
            ("/style session", "/style session"),
            ("/style My-Notes", "/style My-Notes"),
        ] {
            let p = parse_line(line);
            assert!(p.dispatch, "{line}: {p:?}");
            assert_eq!(p.kind, SlashKind::Dispatch, "{line}");
            assert_eq!(p.command.as_deref(), Some("style"), "{line}");
            assert_eq!(p.canonical.as_deref(), Some(canonical), "{line}");
        }
    }

    #[test]
    fn memory_consolidate_fixture_suggests_without_inventing_ids() {
        let p = parse_line("/memory consolidate insights from this session to episodic memory");
        assert_eq!(p.command.as_deref(), Some("memory"));
        assert_eq!(p.sub.as_deref(), Some("consolidate"));
        assert!(p.rest.is_empty(), "invented ids: {}", p.rest);
        assert_eq!(p.canonical.as_deref(), Some("/memory consolidate"));
        assert!(!p.dispatch, "conversational memory commands only suggest");
        assert!(p.notice.as_deref().unwrap_or("").contains("not dispatched"));
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
        assert_eq!(
            p.canonical.as_deref(),
            Some("/memory save keep the loopback bind :: operator confirmed")
        );
    }

    #[test]
    fn exact_command_arguments_are_data_not_filler_or_second_intents() {
        for line in [
            "/memory save search the docs and search Google :: for my notes",
            "/memory remember this is the note :: please keep this for me",
            "/agent confirm this is the reason for my change",
            "/session rename notes from this session",
            "/goal the notes for this session",
            "/web search the phrase and search Google",
            "/api set EXAMPLE just-for-me",
        ] {
            let parsed = parse_line(line);
            assert!(parsed.dispatch, "{line}: {parsed:?}");
            assert_eq!(parsed.canonical.as_deref(), Some(line), "{line}");
        }
        for line in [
            "/mem please save this note :: my reason",
            "/agent please confirm for me",
            "/memory please clear",
            "/memory please on",
            "/soul please off",
            "/web please cancel",
            "/session please new",
            "/model please use local",
            "/memory consolidate please use 123456789abcdef0",
        ] {
            assert!(!parse_line(line).dispatch, "{line}");
        }
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

    #[test]
    fn malformed_slash_prefix_cannot_become_a_mutation() {
        for line in [
            "//memory clear",
            "///agent confirm approved",
            "/ memory clear",
            "/\nweb off",
        ] {
            assert!(!parse_line(line).dispatch, "{line}");
        }
        assert!(parse_line("  /memory clear  ").dispatch);
    }

    #[test]
    fn conversational_prefix_preserves_the_entire_payload() {
        for (line, canonical, dispatch) in [
            ("/web please search The Who", "/web search The Who", true),
            ("/web google to be or not to be", "/web search to be or not to be", true),
            (
                "/memory please search notes from my session",
                "/memory search notes from my session",
                false,
            ),
            (
                "/memory please save The Who notes :: for my notes",
                "/memory save The Who notes :: for my notes",
                false,
            ),
            (
                "/agent please confirm for my change",
                "/agent confirm for my change",
                false,
            ),
        ] {
            let p = parse_line(line);
            assert_eq!(p.canonical.as_deref(), Some(canonical), "{line}");
            assert_eq!(p.dispatch, dispatch, "{line}");
        }
    }

    #[test]
    fn extra_arguments_and_help_cannot_authorize_mutations() {
        for line in [
            "/memory clear --help",
            "/memory clear what does this do",
            "/memory on please",
            "/memory forget note-id --dry-run",
            "/memory capture off --dry-run",
            "/soul off --help",
            "/soul edit --help",
            "/session use session-id --help",
            "/api clear EXAMPLE --dry-run",
            "/skill clear --help",
            "/web off --help",
            "/web inject --help",
            "/web forget --dry-run",
            "/web allow https://example.com --dry-run",
            "/web allow https://example.com --help",
            "/agent cancel --help",
            "/agent discard run-id --dry-run",
            "/agent stop job-id --help",
            "/agent pr-body run-id --help",
            "/goal stage codex/test --help",
            "/loop auto --help",
            "/loop stop --help",
            "/loop 2 --help",
            "/clear --help",
            "/skill check:rust-test --help",
            "/agent confirm --help",
            "/agent confirm --dry-run",
            "/agent confirm help",
            "/agent confirm ?",
            "/agent approve run-id --help",
            "/agent push run-id --dry-run",
            "/agent publish run-id -h",
            "/agent reject run-id --help",
            "/soul apply proposal-id --help",
            "/soul reject proposal-id --dry-run",
            "/memory save note :: --help",
            "/memory remember note :: --dry-run",
        ] {
            let p = parse_line(line);
            assert!(!p.dispatch, "{line}: {p:?}");
            assert_eq!(p.kind, SlashKind::Suggest, "{line}");
        }
    }

    #[test]
    fn exact_mutations_and_free_form_flag_mentions_remain_available() {
        for line in [
            "/memory clear",
            "/memory on",
            "/memory forget note-id",
            "/memory capture off",
            "/soul off",
            "/soul edit",
            "/session use session-id",
            "/api clear EXAMPLE",
            "/skill clear",
            "/skill use one two",
            "/web off",
            "/web inject",
            "/web forget",
            "/web allow https://example.com",
            "/web allow https://example.com docs-notes_2",
            "/web allow https://example.com help https://example.com/start?flag=--help",
            "/agent cancel",
            "/agent discard run-id",
            "/agent stop job-id",
            "/agent pr-body run-id",
            "/goal stage codex/test",
            "/loop auto",
            "/loop 2",
            "/loop stop",
            "/clear",
            "/skill check:rust-test",
            "/agent confirm fix --help output",
            "/agent approve run-id repair dry-run mode",
            "/agent push run-id approved fix",
            "/agent publish run-id ready for review",
            "/agent reject run-id",
            "/soul apply proposal-id improve help",
            "/soul reject proposal-id keep current text",
            "/memory save --help explains the flags :: preserve useful notes",
            "/memory remember help with dry-run :: keep this for me",
            "/memory add --help",
            "/goal --help output should explain flags",
            "/goal clear the cache",
            "/session rename help with --dry-run",
            "/web search --help for The Who",
        ] {
            let p = parse_line(line);
            assert!(p.dispatch, "{line}: {p:?}");
            assert_eq!(p.canonical.as_deref(), Some(line), "{line}");
        }
    }

    #[test]
    fn typos_and_ambiguous_subcommands_only_suggest() {
        for (line, expected) in [
            ("/hlep", vec!["/help"]),
            ("/memroy", vec!["/memory"]),
            ("/skil", vec!["/skill", "/skills"]),
            ("/memory cler", vec!["/memory clear"]),
            (
                "/memory retr notes",
                vec!["/memory retrieval notes", "/memory retrieve notes"],
            ),
            (
                "/agent confrim keep --help text",
                vec!["/agent confirm keep --help text"],
            ),
        ] {
            let p = parse_line(line);
            assert!(!p.dispatch, "{line}: {p:?}");
            for suggestion in expected {
                assert!(p.suggestions.iter().any(|s| s.line == suggestion), "{line}: {p:?}");
            }
            assert!(p.suggestions.len() <= 5, "{line}");
        }
    }
}
