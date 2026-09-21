//! Pure slash grammar. Produces data, never URLs with inferred hosts or authority.
//! HTTP handlers independently validate every request and permission.
use serde::Serialize;
use serde_json::{json, Value};

use super::web_policy::{canonical_url, valid_group, Rule, MAX_RULES};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebCommand {
    pub action: String,
    pub body: Value,
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
        if matches!(c, '\'' | '"') {
            let mut closed = false;
            for next in chars.by_ref() {
                if next == c {
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
            quote: matches!(c, '\'' | '"').then_some(c),
        });
    }
    Ok(out)
}

pub fn parse(sub: Option<&str>, raw: &str) -> Result<WebCommand, String> {
    let action = sub.unwrap_or("status");
    let args = words(raw)?;
    if args
        .iter()
        .take_while(|a| a.quote.is_some() || a.text != "--")
        .any(|a| a.quote.is_none() && matches!(a.text.as_str(), "--help" | "-h"))
        || action == "help"
    {
        return Ok(WebCommand {
            action: "help".into(),
            body: json!({"topic":"web"}),
        });
    }
    let mut positional = Vec::new();
    let mut group: Option<String> = None;
    let mut seeds = Vec::new();
    let mut urls = Vec::new();
    let mut count = None;
    let mut engine = None;
    let mut literal = false;
    let mut i = 0;
    while i < args.len() {
        let word = &args[i];
        let arg = &word.text;
        i += 1;
        if !literal && word.quote.is_none() && arg == "--" {
            literal = true;
            continue;
        }
        if !literal && word.quote.is_none() && (arg.starts_with('-') || arg.starts_with("group=")) {
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
                return Err(format!("Unsupported flag {flag} for /web {action}. Use /help web."));
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
                "--engine" if engine.is_none() && matches!(value.as_str(), "google" | "pages") => engine = Some(value),
                _ => return Err(format!("Invalid or duplicate {flag}.")),
            }
        } else {
            // Exact phrases are search semantics, not merely argument grouping.
            // Preserve double quotes for queries while URL/flag values stay literal.
            positional.push(
                if word.quote == Some('"') && matches!(action, "search" | "pages" | "research") {
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
            if positional.is_empty() || positional.len() > MAX_RULES || seeds.len() > 16 {
                return Err("Allow needs 1–32 URL patterns and at most 16 seeds.".into());
            }
            let group = group.unwrap_or_else(|| "default".into());
            let rules = positional
                .iter()
                .map(|p| Rule::new(p, &group, &[]))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.message)?;
            for seed in &seeds {
                let target = canonical_url(seed).map_err(|e| e.message)?;
                if !rules.iter().any(|r| r.permits(&target)) {
                    return Err("Each seed must be inside a pattern in this grant.".into());
                }
            }
            json!({"urls":positional,"group":group,"seeds":seeds})
        }
        "fetch" | "check" => {
            if positional.is_empty() || positional.len() > MAX_RULES {
                return Err(format!("/web {action} needs 1–32 exact URLs. For links and site research, use /web research --url URL <question>."));
            }
            for url in &positional {
                canonical_url(url).map_err(|_| "Use exact HTTP(S) URLs, not wildcard patterns or prose. For permitted internal links use /web research --url URL <question>.".to_string())?;
            }
            json!({"urls":positional,"group":group})
        }
        "deny" => {
            if positional.len() != 1 {
                return Err("Use /web deny <rule-id-or-pattern>.".into());
            }
            let rule = &positional[0];
            if !(rule.len() == 32 && rule.bytes().all(|c| c.is_ascii_hexdigit())) {
                Rule::new(rule, "default", &[]).map_err(|e| e.message)?;
            }
            json!({"url":rule})
        }
        "search" | "pages" | "research" => {
            let query = positional.join(" ");
            if query.trim().is_empty() || query.chars().count() > 200 {
                return Err("Supply a question or query of 1–200 characters.".into());
            }
            for url in &urls {
                canonical_url(url).map_err(|e| e.message)?;
            }
            if urls.len() > MAX_RULES {
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
}
