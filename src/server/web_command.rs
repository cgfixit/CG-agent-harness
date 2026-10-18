//! Pure slash grammar. Produces data, never URLs with inferred hosts or authority.
//! HTTP handlers independently validate every request and permission.
use serde::Serialize;
use serde_json::{json, Value};

use super::web_policy::{canonical_url, valid_group, Rule, MAX_BATCH_URLS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebCommand {
    pub action: String,
    pub body: Value,
    /// Operator-facing note printed before dispatch; never sent to the route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// Operators and models often omit the scheme (`*.example.org`,
/// `example.org/*`). Only `https://` is supplied, never HTTP, a host or a
/// path: `example.org` stays the exact root page and `*.example.org` still
/// excludes the apex, exactly as with the scheme typed out.
fn with_https(arg: &str) -> String {
    // A URL copied from rendered markdown or prose can arrive wrapped in
    // backticks or typographic quotes; no URL starts with one.
    let arg = [('`', '`'), ('“', '”'), ('‘', '’')]
        .iter()
        .find_map(|(open, close)| arg.strip_prefix(*open)?.strip_suffix(*close))
        .unwrap_or(arg);
    if arg.contains("://") {
        arg.to_string()
    } else {
        format!("https://{arg}")
    }
}

/// Punctuation copied with a host (`example.org,` or `example.org)`) parses as a
/// different, never-matching host; refuse it instead of saving a dead rule.
fn host_punctuation(url: &str) -> Option<char> {
    let authority = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    authority
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '*' | ':' | '[' | ']')))
}

fn checked(raw: &str) -> Result<String, String> {
    let url = with_https(raw);
    match host_punctuation(&url) {
        Some(c) => Err(pattern_refused(raw, &format!("the host contains '{c}'"))),
        None => Ok(url),
    }
}

/// A refused pattern names itself and the accepted shapes, so the operator
/// does not have to guess which of several operands failed or why.
fn pattern_refused(raw: &str, message: &str) -> String {
    let hint = if raw.starts_with('/') {
        " Remove the leading / (the command is /web; the pattern starts with the host)."
    } else {
        ""
    };
    format!(
        "{raw}: {message}.{hint} Use https://www.example.org/* (one host, all paths), https://*.example.org/* (subdomains, apex excluded) or https://example.org/page (one page)."
    )
}

#[derive(Debug)]
struct Word {
    text: String,
    quote: Option<char>,
}

/// Quotes delimit whole arguments (URLs, queries or patterns). There is no shell
/// expansion or backslash unescaping, so a pasted URL keeps its exact identity.
fn words(input: &str) -> Result<Vec<Word>, String> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut word = String::new();
        // An apostrophe opens a quoted argument only when a closing one ends a
        // later word (`'tokio select'`); otherwise it is text (`'til death`).
        // `'90s` and `'til` are elisions, not openers.
        let quoted = c == '"' || c == '\'' && !elision(chars.clone()) && closes_later(chars.clone());
        if quoted {
            let mut closed = false;
            while let Some(next) = chars.next() {
                // A single-quoted span closes only at an apostrophe that ends a
                // word, so `'women's health'` keeps its inner apostrophe.
                if next == c && (c == '"' || chars.peek().is_none_or(|n| n.is_whitespace())) {
                    closed = true;
                    break;
                }
                word.push(next);
            }
            if !closed || chars.peek().is_some_and(|c| !c.is_whitespace()) {
                return Err("Unclosed or embedded quote; quote a whole argument.".into());
            }
        } else {
            word.push(c);
            while chars.peek().is_some_and(|c| !c.is_whitespace()) {
                word.push(chars.next().unwrap());
            }
        }
        if word.is_empty() {
            return Err("Empty argument; nothing was dispatched.".into());
        }
        out.push(Word {
            text: word,
            quote: quoted.then_some(c),
        });
    }
    Ok(out)
}

/// Whether the text after an apostrophe is an elided word (`'90s`, `'til`,
/// `'em`) rather than the start of a quoted phrase.
fn elision(rest: impl Iterator<Item = char>) -> bool {
    const WORDS: [&str; 6] = ["til", "em", "tis", "twas", "cause", "bout"];
    let word = rest
        .take_while(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    // `'cause,` and `'til.` are the same elisions.
    let bare = word.trim_end_matches(|c: char| c.is_ascii_punctuation() && c != '\'');
    word.starts_with(|c: char| c.is_ascii_digit()) || WORDS.contains(&bare)
}

/// Whether an apostrophe in `rest` ends a word (followed by whitespace or the
/// end) before another word opens with one: in `'til death 'Rust Book'` the
/// first apostrophe is text and `'Rust Book'` is the phrase.
fn closes_later(mut rest: impl Iterator<Item = char>) -> bool {
    let mut prev = None;
    for c in rest.by_ref() {
        if prev == Some('\'') && c.is_whitespace() {
            return true;
        }
        if c == '\'' && prev.is_some_and(char::is_whitespace) {
            return false;
        }
        prev = Some(c);
    }
    prev == Some('\'')
}

pub fn parse(sub: Option<&str>, raw: &str) -> Result<WebCommand, String> {
    let action = sub.unwrap_or("status");
    let args = words(raw)?;
    if args
        .iter()
        .take_while(|a| a.quote.is_some() || a.text != "--")
        // In a query `-h` is search text (`C compiler -h option`), like any
        // single-dash word; `--help` still asks for help there.
        .any(|a| {
            a.quote.is_none()
                && (a.text == "--help" || a.text == "-h" && !matches!(action, "search" | "pages" | "research"))
        })
        || action == "help"
    {
        return Ok(WebCommand {
            action: "help".into(),
            body: json!({"topic":"web"}),
            notice: None,
        });
    }
    let mut positional = Vec::new();
    let mut group: Option<String> = None;
    let mut seeds = Vec::new();
    let mut urls = Vec::new();
    let mut count = None;
    let mut engine = None;
    let mut literal = false;
    let mut notice = None;
    let mut i = 0;
    while i < args.len() {
        let word = &args[i];
        let arg = &word.text;
        i += 1;
        if !literal && word.quote.is_none() && arg == "--" {
            literal = true;
            continue;
        }
        // In a query, a single-dash word is search text (`rust async -tokio`,
        // `i32 -1`, `->`), not a flag: every query flag is spelled `--name`.
        let query_word = matches!(action, "search" | "pages" | "research") && !arg.starts_with("--");
        let flag_shaped = if query_word {
            arg.starts_with("group=")
        } else {
            arg.starts_with('-') || arg.starts_with("group=")
        };
        if !literal && word.quote.is_none() && flag_shaped {
            let (flag, inline) = if let Some(value) = arg.strip_prefix("group=") {
                ("--group", Some(value))
            } else {
                arg.split_once('=').map_or((arg.as_str(), None), |(k, v)| (k, Some(v)))
            };
            let allowed = match flag {
                "--group" => matches!(action, "allow" | "check" | "fetch" | "search" | "pages" | "research"),
                "--seed" => action == "allow",
                "--url" => action == "research",
                "--count" | "--engine" => action == "search",
                _ => false,
            };
            if !allowed {
                let accepted = match action {
                    "allow" => "/web allow accepts --group and --seed",
                    "check" | "fetch" => "it accepts --group",
                    "search" => {
                        "it accepts --count, --engine and --group; put -- before query words that start with --"
                    }
                    "pages" => "it accepts --group; put -- before query words that start with --",
                    "research" => "it accepts --group and --url; put -- before question words that start with --",
                    "deny" => "deny takes one rule ID or pattern, with no flags",
                    _ => "it takes no flags",
                };
                return Err(format!(
                    "Unsupported flag {flag} for /web {action}: {accepted}. See /help web."
                ));
            }
            let value = if let Some(value) = inline {
                value.to_string()
            } else {
                let value = args
                    .get(i)
                    .filter(|s| s.quote.is_some() || !s.text.starts_with('-'))
                    .ok_or_else(|| format!("{flag} needs a value."))?;
                i += 1;
                value.text.clone()
            };
            match flag {
                "--group" if group.is_none() && valid_group(&value) => group = Some(value),
                "--seed" => seeds.push(value),
                "--url" => urls.push(value),
                "--count" if count.is_none() => {
                    count = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=10).contains(n))
                            .ok_or("--count must be 1–10.")?,
                    )
                }
                "--engine" if engine.is_none() && matches!(value.to_ascii_lowercase().as_str(), "google" | "pages") => {
                    engine = Some(value.to_ascii_lowercase())
                }
                "--engine" => return Err("Invalid or duplicate --engine; use google or pages.".into()),
                _ => return Err(format!("Invalid or duplicate {flag}.")),
            }
        } else {
            // Exact phrases are search semantics, not merely argument grouping.
            // A double-quoted query word, or a single-quoted multi-word span (as
            // chat treats `'tokio select'`), becomes a double-quoted phrase;
            // URL/flag values stay literal. A double quote inside single quotes
            // would collide with that phrase, so it is refused, as chat refuses it.
            if matches!(action, "search" | "pages" | "research") && word.quote == Some('\'') && arg.contains('"') {
                return Err("A quoted search term contains an embedded double quote.".into());
            }
            positional.push(
                if matches!(action, "search" | "pages" | "research")
                    && (word.quote == Some('"') || word.quote == Some('\'') && arg.contains(char::is_whitespace))
                {
                    format!("\"{arg}\"")
                } else {
                    arg.clone()
                },
            );
        }
    }
    let body = match action {
        "status" | "on" | "off" | "inject" | "forget" | "cancel" => {
            if !positional.is_empty() {
                return Err(format!("/web {action} takes no arguments."));
            }
            if matches!(action, "on" | "off") {
                json!({"enabled":action=="on"})
            } else {
                json!({})
            }
        }
        "allow" => {
            // Retain the old one-pattern positional group/seed syntax without
            // treating another URL as a group or discarding trailing operands.
            if group.is_none() && positional.len() >= 2 && valid_group(&positional[1]) {
                group = Some(positional.remove(1));
                if positional.len() == 2 {
                    seeds.push(positional.remove(1));
                }
            }
            if positional.is_empty() || positional.len() > MAX_BATCH_URLS || seeds.len() > 16 {
                return Err("Allow needs 1–32 URL patterns and at most 16 seeds.".into());
            }
            let group = group.unwrap_or_else(|| "default".into());
            let (mut patterns, mut rules, mut root_only) = (Vec::new(), Vec::new(), Vec::new());
            for raw in &positional {
                let pattern = checked(raw)?;
                let rule = Rule::new(&pattern, &group, &[]).map_err(|e| pattern_refused(raw, &e.message))?;
                // A host with no path (`example.org`, `https://*.example.org`) is
                // each host's root page only, which is rarely what was meant.
                if !pattern.split_once("://").is_some_and(|(_, rest)| rest.contains('/')) {
                    root_only.push(format!("{raw} permits only the root page {}", rule.pattern));
                }
                patterns.push(pattern);
                rules.push(rule);
            }
            let seeds = seeds.iter().map(|s| checked(s)).collect::<Result<Vec<_>, _>>()?;
            for seed in &seeds {
                let target = canonical_url(seed).map_err(|e| pattern_refused(seed, &e.message))?;
                if !rules.iter().any(|r| r.permits(&target)) {
                    return Err("Each seed must be inside a pattern in this grant.".into());
                }
            }
            if !root_only.is_empty() {
                notice = Some(format!(
                    "{}; end a pattern with /* (for example https://*.example.org/*) to include every path.",
                    root_only.join("; ")
                ));
            }
            json!({"urls":patterns,"group":group,"seeds":seeds})
        }
        "fetch" | "check" => {
            if positional.is_empty() || positional.len() > MAX_BATCH_URLS {
                return Err(format!("/web {action} needs 1–32 exact URLs. For links and site research, use /web research --url URL <question>."));
            }
            let urls = positional.iter().map(|u| checked(u)).collect::<Result<Vec<_>, _>>()?;
            for (raw, url) in positional.iter().zip(&urls) {
                canonical_url(url).map_err(|e| {
                    if raw.contains('*') {
                        format!("{raw}: use exact HTTP(S) URLs, not wildcard patterns or prose. For permitted internal links use /web research --url URL <question>.")
                    } else {
                        pattern_refused(raw, &e.message)
                    }
                })?;
            }
            json!({"urls":urls,"group":group})
        }
        "deny" => {
            if positional.len() != 1 {
                return Err("Use /web deny <rule-id-or-pattern>.".into());
            }
            let raw = &positional[0];
            let rule = if raw.len() == 32 && raw.bytes().all(|c| c.is_ascii_hexdigit()) {
                raw.clone()
            } else {
                let pattern = checked(raw)?;
                Rule::new(&pattern, "default", &[]).map_err(|e| pattern_refused(raw, &e.message))?;
                pattern
            };
            json!({"url":rule})
        }
        "search" | "pages" | "research" => {
            let query = positional.join(" ");
            let limit = if action == "research" {
                super::schemas::MAX_WEB_RESEARCH_QUESTION_LEN
            } else {
                super::schemas::MAX_WEB_QUERY_LEN
            };
            if query.trim().is_empty() || query.chars().count() > limit {
                return Err(format!("Supply a question or query of 1–{limit} characters."));
            }
            let urls = urls.iter().map(|u| checked(u)).collect::<Result<Vec<_>, _>>()?;
            for url in &urls {
                canonical_url(url).map_err(|e| pattern_refused(url, &e.message))?;
            }
            if urls.len() > MAX_BATCH_URLS {
                return Err("Too many research starting URLs.".into());
            }
            let engine = engine.unwrap_or_else(|| {
                if action == "search" && group.is_none() {
                    "google"
                } else {
                    "pages"
                }
                .into()
            });
            if engine == "google" && group.is_some() {
                return Err(
                    "A source group selects permitted pages; it cannot be combined with --engine google.".into(),
                );
            }
            let mut body = json!({"query":query,"group":group,"engine":engine,"count":count.unwrap_or(5)});
            if action == "research" {
                body["urls"] = json!(urls);
            }
            body
        }
        _ => return Err("Unknown web command; use /help web.".into()),
    };
    Ok(WebCommand {
        action: action.into(),
        body,
        notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_quotes_and_multi_source_intent_are_data_not_authority() {
        let c = parse(
            Some("allow"),
            "'https://www.veeam.com/*' https://example.org/* --group vendors --seed https://www.veeam.com/",
        )
        .unwrap();
        assert_eq!(c.body["urls"].as_array().unwrap().len(), 2);
        assert_eq!(c.body["group"], "vendors");
        assert_eq!(
            parse(Some("search"), "--count=3 \"backup security\"").unwrap().body["query"],
            "\"backup security\""
        );
        assert_eq!(
            parse(Some("research"), "--url https://www.veeam.com/ compare backup vendors")
                .unwrap()
                .body["urls"][0],
            "https://www.veeam.com/"
        );
        assert_eq!(
            parse(Some("allow"), "https://example.com/* docs https://example.com/a")
                .unwrap()
                .body["group"],
            "docs"
        );
        for (sub, arg) in [
            ("fetch", "https://www.veeam.com and its embedded internal links"),
            ("fetch", "https://example.com/*"),
            ("allow", "https://example.com/ --dry-run"),
            ("allow", "https://example.com/ --seed https://other.org/"),
            ("search", "--count 0 x"),
            ("search", "--count 2 --count 3 x"),
            ("off", "please"),
            ("search", "\"unclosed"),
        ] {
            assert!(parse(Some(sub), arg).is_err(), "{sub} {arg}");
        }
        assert_eq!(
            parse(Some("allow"), "https://example.com/* --help").unwrap().action,
            "help"
        );
    }

    #[test]
    fn quoted_query_flags_are_literal_and_exact_phrases_survive() {
        let c = parse(
            Some("search"),
            "--count 3 site:example.com \"backup security\" '--help'",
        )
        .unwrap();
        assert_eq!(c.action, "search");
        assert_eq!(c.body["query"], "site:example.com \"backup security\" --help");
        assert_eq!(c.body["count"], 3);
        assert_eq!(parse(Some("search"), "-- --help").unwrap().body["query"], "--help");
        assert_eq!(
            parse(Some("fetch"), "\"https://example.com/\"").unwrap().body["urls"][0],
            "https://example.com/"
        );
    }

    #[test]
    fn single_dash_words_are_query_text_and_double_dash_words_stay_flags() {
        for (arg, query) in [
            ("rust async -tokio", "rust async -tokio"),
            ("what does -> mean in rust", "what does -> mean in rust"),
            ("i32 -1 overflow", "i32 -1 overflow"),
            ("\"serde\" -yaml", "\"serde\" -yaml"),
        ] {
            assert_eq!(parse(Some("search"), arg).unwrap().body["query"], query, "{arg}");
        }
        assert_eq!(
            parse(Some("pages"), "--group docs retry -legacy").unwrap().body["group"],
            "docs"
        );
        assert!(parse(Some("search"), "--dry-run x").is_err());
        // Outside a query the old flag grammar is unchanged.
        assert!(parse(Some("allow"), "https://example.com/* -x").is_err());
    }

    #[test]
    fn single_quoted_phrases_stay_phrases_and_lone_apostrophes_are_text() {
        let query = |arg: &str| parse(Some("search"), arg).unwrap().body["query"].clone();
        assert_eq!(query("'tokio select' timeout"), "\"tokio select\" timeout");
        assert_eq!(query("'women's health' study"), "\"women's health\" study");
        assert_eq!(query("'til death do us part"), "'til death do us part");
        assert_eq!(
            query("'til death 'Rust Book' reviews"),
            "'til death \"Rust Book\" reviews"
        );
        assert_eq!(query("C compiler -h option"), "C compiler -h option");
        assert_eq!(
            query("'90s bands musicians' influences"),
            "'90s bands musicians' influences"
        );
        assert_eq!(
            query("'til death musicians' influences"),
            "'til death musicians' influences"
        );
        assert_eq!(query("'cause, musicians' influences"), "'cause, musicians' influences");
        assert_eq!(parse(Some("search"), "rust --help").unwrap().action, "help");
        assert_eq!(parse(Some("allow"), "-h").unwrap().action, "help");
        // A whole single-quoted word is argument grouping, as before.
        assert_eq!(query("rock 'n' roll"), "rock n roll");
        assert!(parse(Some("search"), "\"unclosed").is_err());
        assert!(parse(Some("search"), "'The \"Rust\" Book'")
            .unwrap_err()
            .contains("embedded double quote"));
        assert_eq!(
            parse(Some("search"), "--engine=Google tokio").unwrap().body["engine"],
            "google"
        );
        assert!(parse(Some("search"), "--engine bing tokio")
            .unwrap_err()
            .contains("google or pages"));
    }
    #[test]
    fn scheme_less_operands_get_https_only_and_refusals_name_the_operand() {
        let c = parse(Some("allow"), "*.google.com google.com/* --group google").unwrap();
        assert_eq!(c.body["urls"], json!(["https://*.google.com", "https://google.com/*"]));
        assert!(c
            .notice
            .unwrap()
            .starts_with("*.google.com permits only the root page https://*.google.com/;"));
        assert!(parse(Some("allow"), "https://www.google.com/*")
            .unwrap()
            .notice
            .is_none());
        assert!(parse(Some("allow"), "https://*.google.com").unwrap().notice.is_some());
        // A typed scheme is kept, never upgraded or downgraded; no host is guessed.
        assert_eq!(
            parse(Some("allow"), "http://example.org/*").unwrap().body["urls"][0],
            "http://example.org/*"
        );
        assert_eq!(
            parse(Some("check"), "www.google.com").unwrap().body["urls"][0],
            "https://www.google.com"
        );
        assert_eq!(
            parse(Some("deny"), "*.google.com/*").unwrap().body["url"],
            "https://*.google.com/*"
        );
        assert_eq!(
            parse(Some("research"), "--url www.google.com hvac near 30041")
                .unwrap()
                .body["urls"][0],
            "https://www.google.com"
        );
        assert_eq!(
            parse(Some("allow"), "`https://www.google.com/*` “https://example.org/*”")
                .unwrap()
                .body["urls"],
            json!(["https://www.google.com/*", "https://example.org/*"])
        );
        let err = parse(Some("allow"), "/*.google.com google.com/* --group google").unwrap_err();
        assert!(
            err.starts_with("/*.google.com:") && err.contains("Remove the leading /"),
            "{err}"
        );
        let err = parse(Some("allow"), "https://www.google.com, https://google.com/*").unwrap_err();
        assert!(
            err.starts_with("https://www.google.com,: the host contains ','"),
            "{err}"
        );
        assert!(parse(Some("deny"), "https://*.google.com/* --group google")
            .unwrap_err()
            .contains("with no flags"));
        assert!(parse(Some("search"), "--seed x hvac").unwrap_err().contains("put --"));
    }

    #[test]
    fn research_question_limit_is_separate_from_search_queries() {
        for action in ["search", "pages", "research"] {
            let limit = if action == "research" { 2000 } else { 200 };
            let query = "界".repeat(limit);
            let command = parse(Some(action), &query).unwrap();
            assert_eq!(command.body["query"], query);
            assert!(parse(Some(action), &format!("{query}x"))
                .unwrap_err()
                .contains(&format!("1–{limit}")));
        }
    }
}
