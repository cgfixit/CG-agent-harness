//! Suggest-don't-guess slash parser.
//!
//! Exact commands still dispatch. Fuzzy input that matches a mutation family
//! never executes; it only prints suggestions. This module does not call web
//! search, write memory, or spawn agentic work.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::edit_distance;
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
    "net",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<super::web_command::WebCommand>,
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

/// Invisible or direction-changing characters a slash line may not carry.
/// U+FEFF is whitespace to the console's JS `\s` but not to Rust, so the two
/// sides would split one line into different commands; the rest render as
/// nothing or reorder the text shown. ZWJ/ZWNJ (emoji, Persian, Indic text)
/// and LRM/RLM stay allowed: neither side splits on them.
pub(crate) fn is_hidden_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{FEFF}' | '\u{200B}' | '\u{180E}' | '\u{2060}'..='\u{2064}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

pub fn parse_line(input: &str) -> SlashParse {
    let trimmed = input.trim();
    // A leading U+FEFF is trimmed by the console but not by `str::trim`; such a
    // line is meant as a slash command and is refused below, not passed to chat.
    let bom_slash = trimmed.trim_start_matches(['\u{FEFF}', ' ']).starts_with('/');
    if trimmed.is_empty() || !(trimmed.starts_with('/') || bom_slash) {
        return SlashParse {
            web: None,
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

    // Slash commands are single-line operator intent, never pasted scripts.
    // Reject separators before tokenization can normalize them into spaces.
    if input
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return suggest_only(
            "slash commands require one line without control characters; not dispatched",
            &[],
        );
    }
    if input.chars().any(is_hidden_format_char) {
        return suggest_only(
            "slash commands cannot contain invisible or direction-changing characters (such as U+FEFF, U+200B or U+202E); retype the line; not dispatched",
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
    if !preexisting_slash_names().contains(cmd_l.as_str()) {
        if let Some(intent) = crate::netconnect::slash::interpret(&cmd_l, &tokens[1..], &preexisting_slash_names()) {
            return net_intent(intent);
        }
    }
    let (cmd, aliased, cmd_score) = resolve_command(&cmd_l);
    let Some(cmd) = cmd else {
        let near = nearby_commands(&cmd_l);
        return suggest_only(&format!("unknown command: /{cmd_l} — try /help"), &near);
    };

    let after_cmd = &tokens[1..];
    if cmd != "help" && after_cmd.len() == 1 && matches!(after_cmd[0], "help" | "--help" | "-h") {
        return parse_line(&format!("/help {cmd}"));
    }
    let (sub, args, sub_fuzzy) = resolve_sub(cmd, after_cmd);
    // Goals and loop counts have a free-form/numeric fallback. Other families
    // can suggest a mistyped subcommand without granting it execution authority.
    if sub.is_none() && !matches!(cmd, "goal" | "loop") {
        if let Some(raw_sub) = after_cmd.first() {
            let near = nearby_tokens(&raw_sub.to_ascii_lowercase(), known_subs(cmd));
            if !near.is_empty() {
                let mut parsed = suggest_only("unknown subcommand; suggestions were not dispatched", &[]);
                let tail = if after_cmd.len() > 1 {
                    format!(" {}", echoable_args(cmd, &after_cmd[1..]).join(" "))
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
    if matches!(cmd, "memory" | "web") && sub.is_none() && !after_cmd.is_empty() {
        if cmd == "web" && after_cmd == ["--help"] {
            return parse_line("/help web");
        }
        if cmd == "web" {
            // `/web tokio select macro` most often means a search. Suggest it;
            // a suggestion is printed, never dispatched.
            let mut parsed = suggest_only("unknown /web subcommand; not dispatched — did you mean a search?", &[]);
            parsed.suggestions = vec![SlashSuggestion {
                line: format!("/web search {}", after_cmd.join(" ")),
                score: 60,
            }];
            return parsed;
        }
        return suggest_only("unknown subcommand; not dispatched — use /help", &[]);
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
        _ => echoable_args(cmd, &args).join(" "),
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
    let web = if cmd == "web" {
        match super::web_command::parse(sub.as_deref(), &rest) {
            Ok(command) if command.action == "help" => return parse_line("/help web"),
            Ok(command) => Some(command),
            Err(message) => return suggest_only(&message, &[("help web", 100)]),
        }
    } else {
        None
    };
    let mut notice = None;
    if cmd == "memory" && sub.as_deref() == Some("consolidate") && rest.is_empty() {
        notice = Some(
            "consolidate needs selected episode ids or the Memory panel; no ids were taken from filler text".into(),
        );
    }

    let dispatch = !((mutation || cmd == "memory") && fuzzy) && confidence >= DISPATCH_THRESHOLD;

    if dispatch {
        SlashParse {
            web,
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
            web: None,
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

/// Slash roots that existed before `/net`. Netconnect aliases are checked
/// against this set and dropped on a hit. `net` itself is excluded because
/// this registry owns that root.
pub fn preexisting_slash_names() -> std::collections::BTreeSet<&'static str> {
    const EARLIER_ALIASES: &[&str] = &["cmds", "commands", "mem", "persona"];
    let mut names = std::collections::BTreeSet::new();
    for name in COMMANDS {
        if *name != "net" {
            names.insert(*name);
        }
    }
    names.extend(EARLIER_ALIASES.iter().copied());
    names
}

fn net_intent(intent: crate::netconnect::slash::Intent) -> SlashParse {
    match intent {
        crate::netconnect::slash::Intent::Dispatch { canonical, sub } => SlashParse {
            web: None,
            kind: SlashKind::Dispatch,
            dispatch: true,
            canonical: Some(canonical),
            command: Some("net".to_string()),
            sub: Some(sub.to_string()),
            rest: String::new(),
            confidence: 100,
            suggestions: Vec::new(),
            notice: None,
        },
        crate::netconnect::slash::Intent::Suggest { notice, suggestions } => SlashParse {
            web: None,
            kind: SlashKind::Suggest,
            dispatch: false,
            canonical: None,
            command: Some("net".to_string()),
            sub: None,
            rest: String::new(),
            confidence: 0,
            suggestions: suggestions
                .into_iter()
                .map(|line| SlashSuggestion { line, score: 70 })
                .collect(),
            notice: Some(notice),
        },
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
            "facts",
            "status",
        ],
        "api" => &["set", "clear"],
        "model" => &["list", "use", "profile"],
        "skill" => &["use", "clear", "status"],
        "web" => &[
            "help", "status", "check", "on", "off", "allow", "deny", "fetch", "search", "pages", "research", "cancel",
            "inject", "forget",
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
    // Web has a complete grammar, including inert help and validated flags.
    if cmd == "web" {
        return None;
    }
    // Validate fixed operands before console dispatch so ignored extra text
    // cannot authorize an action. Keep free-form command payloads intact.
    let max_args = match (cmd, sub) {
        ("memory", Some("on" | "off" | "clear" | "proposals" | "facts" | "status"))
        | ("soul", Some("on" | "off" | "edit" | "propose"))
        | ("skill", Some("clear"))
        | ("web", Some("on" | "off" | "inject" | "forget" | "cancel"))
        | ("agent", Some("cancel"))
        | ("loop", Some("auto" | "stop"))
        | ("model", Some("list" | "profile"))
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

/// Arguments the parser may return in `canonical`, `rest` or suggestions.
/// `/api set KEY value` carries a credential after the key name: the parse
/// response and the console's `normalized:` line must never repeat it, so
/// only the key name is echoed. The console dispatches `/api set` from the
/// operator's own line instead (see `runSlashMaybeFuzzy`).
fn echoable_args<'a>(cmd: &str, args: &'a [&'a str]) -> &'a [&'a str] {
    if cmd == "api" {
        &args[..args.len().min(1)]
    } else {
        args
    }
}

fn suggest_only(notice: &str, near: &[(&str, u8)]) -> SlashParse {
    SlashParse {
        web: None,
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

    const HIDDEN_NOTICE: &str = "slash commands cannot contain invisible or direction-changing characters (such as U+FEFF, U+200B or U+202E); retype the line; not dispatched";

    /// The console re-splits a dispatched canonical with JS `/\s+/`
    /// (assets/static/harness.html, `runSlash`). ECMAScript `\s` is Rust's
    /// whitespace minus U+0085, plus U+FEFF.
    fn js_split(line: &str) -> Vec<&str> {
        line.split(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}')
            .filter(|t| !t.is_empty())
            .collect()
    }

    #[test]
    fn hidden_format_characters_never_dispatch_or_echo() {
        for line in [
            // U+FEFF splits for the console but not for the parser, so these
            // used to dispatch as a bare root and run as push/confirm/apply.
            "/agent push\u{FEFF}abc --dry-run",
            "/agent confirm\u{FEFF}--dry-run",
            "/soul apply\u{FEFF}p1 --help",
            "/goal \u{FEFF}clear",
            "/api set KEY\u{FEFF}secret-value",
            "\u{FEFF}/model list",
            "/memory remember fact\u{202E}txet :: because",
            "/session rename a\u{200B}b",
            "/agent run fix\u{2066}bug\u{2069}",
            "/web search word\u{2060}joined",
        ] {
            let parsed = parse_line(line);
            assert!(!parsed.dispatch, "{line:?}: {parsed:?}");
            assert_eq!(parsed.kind, SlashKind::Suggest, "{line:?}");
            assert_eq!(parsed.notice.as_deref(), Some(HIDDEN_NOTICE), "{line:?}");
            let json = parsed.to_json().to_string();
            assert!(!json.contains("secret-value"), "{line:?} echoed: {json}");
        }
        // Joiners and marks that neither side splits on stay usable in text.
        for line in [
            "/memory save family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} :: operator note",
            "/memory save heart \u{2764}\u{FE0F} :: operator note",
            "/memory save \u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0645} :: operator note",
        ] {
            assert!(parse_line(line).dispatch, "{line:?}");
        }
    }

    use proptest::prelude::*;

    proptest::proptest! {
        /// Whatever the parser dispatches, the console must split into the
        /// same words the parser saw: a mismatch lets one side refuse what the
        /// other runs.
        #[test]
        fn dispatched_canonical_splits_the_same_in_rust_and_js(
            words in proptest::collection::vec(
                proptest::sample::select(vec![
                    "agent", "push", "confirm", "status", "memory", "save", "facts", "api", "set", "clear",
                    "goal", "stage", "loop", "stop", "model", "list", "use", "soul", "apply", "web", "search",
                    "session", "rename", "abc", "KEY", "--dry-run", "--help", "::", "reason", "please",
                ]),
                1..6,
            ),
            seps in proptest::collection::vec(
                proptest::sample::select(vec![" ", "  ", "\u{A0}", "\u{FEFF}", "\u{200B}", "\u{3000}", "\u{2009}", "\u{202E}", "\u{200D}"]),
                6,
            ),
        ) {
            let mut line = String::from("/");
            for (i, word) in words.iter().enumerate() {
                if i > 0 {
                    line.push_str(seps[i]);
                }
                line.push_str(word);
            }
            let parsed = parse_line(&line);
            if parsed.dispatch {
                let canonical = parsed.canonical.clone().unwrap_or_default();
                prop_assert_eq!(js_split(&canonical), canonical.split_whitespace().collect::<Vec<_>>(), "{:?}", line);
                prop_assert!(!line.chars().any(is_hidden_format_char), "{:?}", line);
            }
        }
    }

    #[test]
    fn slash_control_characters_and_ignored_memory_operands_never_dispatch() {
        for line in [
            "/memory\nclear",
            "/memory\tclear",
            "/memory clear\n",
            "/memory\r\noff",
            "/memory\u{2028}clear",
            "/memory\u{2029}clear",
            "/memory add first line\n/memory clear",
            "/memory save first line\nsecond line :: reason",
            "/agent\nconfirm operator approved",
            "/api\tclear GROK_API_KEY",
            "/session\r\nnew",
            "/soul\u{2028}apply proposal operator-approved",
            "/model\u{2029}use grok",
            "/goal\nclear",
            "/skill\tclear",
            "/style\noff",
        ] {
            let parsed = parse_line(line);
            assert!(!parsed.dispatch, "{line:?}");
            assert_eq!(
                parsed.notice.as_deref(),
                Some("slash commands require one line without control characters; not dispatched"),
                "{line:?}"
            );
        }
        for line in [
            "/memory proposals and then clear",
            "/memory retrieve --help",
            "/memory retrieve --dry-run private preference",
            "/memory facts clear",
            "/memory facts --help",
            "/memory status off",
            "/memory please facts",
            "/memory my facts",
            "/mem facts",
        ] {
            assert!(!parse_line(line).dispatch, "{line:?}");
        }
        for (line, sub) in [
            ("/memory facts", "facts"),
            ("/MEMORY FACTS", "facts"),
            ("  /memory   status ", "status"),
        ] {
            let parsed = parse_line(line);
            assert!(parsed.dispatch, "{line:?}");
            assert_eq!(parsed.confidence, 100, "{line:?}");
            assert_eq!(parsed.sub.as_deref(), Some(sub), "{line:?}");
            assert_eq!(parsed.canonical.as_deref(), Some(format!("/memory {sub}").as_str()));
            assert!(!is_mutation("memory", Some(sub)), "{line:?}");
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
    fn api_set_never_echoes_the_credential() {
        // Failure messages name a case index, never the line or the parse:
        // either would print the credential this test guards.
        let secret = "sk-slash-parse-secret-0123456789";
        let exact = parse_line(&format!("/api set EXAMPLE {secret}"));
        assert!(exact.dispatch, "exact /api set must dispatch");
        assert!(
            exact.command.as_deref() == Some("api")
                && exact.sub.as_deref() == Some("set")
                && exact.canonical.as_deref() == Some("/api set EXAMPLE")
                && exact.rest == "EXAMPLE",
            "exact /api set must echo only the key name"
        );
        for (case, line) in [
            format!("/api set EXAMPLE {secret}"),
            format!("/API SET EXAMPLE {secret}"),
            format!("/api   set  EXAMPLE   {secret} with spaces"),
            format!("/api sett EXAMPLE {secret}"),
            format!("/api please set EXAMPLE {secret}"),
            format!("/api EXAMPLE {secret}"),
            format!("/apii set EXAMPLE {secret}"),
            format!("/api set EXAMPLE {secret} and search docs"),
        ]
        .iter()
        .enumerate()
        {
            let echoed = parse_line(line).to_json().to_string();
            assert!(!echoed.contains(secret), "case {case} echoed the credential");
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
            if p.command.as_deref() == Some("help") {
                assert_eq!(
                    p.canonical.as_deref(),
                    Some(
                        format!(
                            "/help {}",
                            line.split_whitespace().next().unwrap().trim_start_matches('/')
                        )
                        .as_str()
                    )
                );
                assert!(p.web.is_none(), "help must not carry a web mutation");
                continue;
            }
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
            "/web allow https://example.com/* help https://example.com/start?flag=--help",
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
            "/web search -- --help for The Who",
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
            ("/memory fact", vec!["/memory facts"]),
            ("/memory stats", vec!["/memory status"]),
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

    #[test]
    fn net_exact_alias_dispatches_and_fuzzy_does_not_execute() {
        for line in [
            "/net status",
            "/netconnect devices",
            "/lan devices",
            "/scan status",
            "/ports devices",
            "/speed status",
        ] {
            let parsed = parse_line(line);
            assert!(parsed.dispatch, "{line}: {parsed:?}");
            assert_eq!(parsed.command.as_deref(), Some("net"));
            assert!(parsed.canonical.as_deref().unwrap().starts_with("/net "));
        }
        for line in ["/nett status", "/scann devices", "/net devic", "/net devicee"] {
            let parsed = parse_line(line);
            assert!(!parsed.dispatch, "{line}: {parsed:?}");
            assert_eq!(parsed.kind, SlashKind::Suggest);
            let notice = parsed.notice.unwrap_or_default();
            assert!(notice.contains("did you mean"), "{line}: {notice}");
            assert!(
                parsed.suggestions.iter().all(|item| item.line != "/net device"),
                "{line}"
            );
        }
        let device = parse_line("/net device");
        assert!(device.dispatch);
        assert_eq!(device.sub.as_deref(), Some("device"));
        let (kept, dropped) = crate::netconnect::slash::classify_aliases(&preexisting_slash_names());
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(kept, crate::netconnect::slash::CANDIDATE_ALIASES);
    }

    #[test]
    fn a_bare_web_query_suggests_a_search_without_dispatching() {
        let parsed = parse_line("/web tokio select macro");
        assert!(!parsed.dispatch);
        assert_eq!(parsed.kind, SlashKind::Suggest);
        assert_eq!(parsed.suggestions[0].line, "/web search tokio select macro");
    }
}
