//! Thin natural-language intent in front of existing Google / SerpAPI search.
//! No new HTTP clients. No new hosts. Listings still do not grant destinations.

use serde::{Deserialize, Serialize};

const MAX_COUNT: usize = 10;
const DEFAULT_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEngine {
    Google,
    Serpapi,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebIntent {
    pub engine: SearchEngine,
    pub count: usize,
    pub terms: Vec<String>,
}

impl WebIntent {
    pub fn query(&self) -> String {
        self.terms.join(" ")
    }
}

/// Parse a chat line or `/web search` remainder into engine, count, and terms.
/// Returns None when the text is not a natural-language search request.
///
/// Two conditions must hold, otherwise the caller's query is passed through
/// untouched: the line must be command-shaped (it starts with `search` or
/// `google`, after optional politeness words, or is a `/web` remainder), and
/// it must carry an explicit signal, either balanced quoted terms or a
/// `first N` count. Ordinary Google syntax such as `site:x "search parser"`
/// or `vector search benchmarks` therefore never reaches the rewrite.
pub fn parse(text: &str) -> Option<WebIntent> {
    parse_with_count(text, DEFAULT_COUNT)
}

/// Like [`parse`], but a request without an explicit `first N` keeps
/// `fallback_count` (the caller's own count) instead of the default.
pub fn parse_with_count(text: &str, fallback_count: usize) -> Option<WebIntent> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !is_command_shaped(&lower) {
        return None;
    }
    let quoted = extract_quoted(trimmed);
    let explicit_count = extract_count(&lower);
    if quoted.is_empty() && explicit_count.is_none() {
        return None;
    }

    let engine = if lower.contains("serpapi") {
        SearchEngine::Serpapi
    } else if lower.contains("google") {
        SearchEngine::Google
    } else {
        SearchEngine::Default
    };

    let count = explicit_count.unwrap_or(fallback_count).clamp(1, MAX_COUNT);
    let mut terms = quoted;
    if terms.is_empty() {
        terms = tokenize_unquoted(trimmed);
    }
    terms.retain(|t| !t.is_empty());
    if terms.is_empty() {
        return None;
    }
    Some(WebIntent { engine, count, terms })
}

/// Politeness words that may precede the command verb.
const LEAD_IN: &[&str] = &[
    "please", "can", "could", "would", "you", "kindly", "hey", "ok", "okay", "now",
];

/// Command verbs that open a natural-language search request.
const VERBS: &[&str] = &["search", "google"];

fn is_command_shaped(lower: &str) -> bool {
    if lower.starts_with("/web") {
        return true;
    }
    lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .find(|w| !LEAD_IN.contains(w))
        .is_some_and(|w| VERBS.contains(&w))
}

fn word_of(tok: &str) -> &str {
    tok.trim_matches(|c: char| !c.is_ascii_alphanumeric())
}

fn is_count_pair(a: &str, b: &str) -> bool {
    let n = word_of(b);
    word_of(a).eq_ignore_ascii_case("first") && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
}

/// `first <ws> N` when present and N >= 1; None otherwise. Any amount of
/// whitespace between the two words is accepted, matching the console's
/// `first\s+\d+` predicate.
fn extract_count(lower: &str) -> Option<usize> {
    let toks: Vec<&str> = lower.split_whitespace().collect();
    toks.windows(2)
        .find(|w| is_count_pair(w[0], w[1]))
        .and_then(|w| match word_of(w[1]).parse::<usize>() {
            Ok(v) if v >= 1 => Some(v.min(MAX_COUNT)),
            _ => None,
        })
}

fn opens_quote(chars: &[char], i: usize) -> bool {
    i == 0 || matches!(chars[i - 1], c if c.is_whitespace() || matches!(c, '(' | ',' | ':' | '['))
}

fn closes_quote(chars: &[char], j: usize) -> bool {
    j + 1 == chars.len()
        || matches!(chars[j + 1], c if c.is_whitespace() || matches!(c, ',' | '.' | ';' | ':' | ')' | ']' | '!' | '?'))
}

/// Balanced quoted terms only. A quote opens at the start of a word and
/// closes at the end of one, so an apostrophe inside `women's` or `what's`
/// is an ordinary character and an unclosed quote is ignored.
fn extract_quoted(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c == '"' || c == '\'') && opens_quote(&chars, i) {
            if let Some(j) = (i + 1..chars.len()).find(|&j| chars[j] == c && closes_quote(&chars, j)) {
                let t: String = chars[i + 1..j].iter().collect();
                let t = t.trim();
                if !t.is_empty() {
                    terms.push(t.to_string());
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    terms
}

/// Unquoted request: drop the command scaffolding and the `first N` pair,
/// keep every other token (years and other numbers included).
fn tokenize_unquoted(text: &str) -> Vec<String> {
    const DROP: &[&str] = &[
        "search", "google", "serpapi", "for", "the", "links", "link", "and", "please", "can", "could", "would", "you",
        "me", "kindly", "hey", "ok", "okay", "now", "a", "an", "of", "to", "using", "/web", "web",
    ];
    let raw: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .map(|t| t.trim().trim_matches(|c: char| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if i + 1 < raw.len() && is_count_pair(raw[i], raw[i + 1]) {
            i += 2;
            continue;
        }
        if !DROP.contains(&raw[i].to_ascii_lowercase().as_str()) {
            out.push(raw[i].to_string());
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_quoted_terms_and_count() {
        let p = parse("search Google (serpapi) for the first 2 links for 'cgfixit' and 'Chris Grady'").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["cgfixit".to_string(), "Chris Grady".to_string()]);
        assert_eq!(p.engine, SearchEngine::Serpapi);
        assert_eq!(p.query(), "cgfixit Chris Grady");
    }

    #[test]
    fn web_search_remainder_keeps_quoted_term() {
        let p = parse("/web search first 2 links for \"cgfixit\"").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
    }

    #[test]
    fn ordinary_chat_is_not_a_search() {
        assert!(parse("what is the loopback bind?").is_none());
        assert!(parse("please research the write gates").is_none());
    }

    #[test]
    fn memory_slash_is_not_stolen() {
        assert!(parse("/memory consolidate insights from this session").is_none());
    }

    #[test]
    fn plain_query_containing_search_is_left_alone() {
        // No quotes and no `first N`: not a natural-language request, so the
        // existing search path keeps the query and the caller's count intact.
        assert!(parse("vector search benchmarks").is_none());
        assert!(parse("search engine optimization").is_none());
        assert!(parse_with_count("elasticsearch tuning", 3).is_none());
    }

    #[test]
    fn caller_count_survives_when_no_first_n_is_given() {
        let p = parse_with_count("search google for 'cgfixit'", 3).expect("intent");
        assert_eq!(p.count, 3);
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
        let p = parse_with_count("search google for the first 2 links for 'cgfixit'", 7).expect("intent");
        assert_eq!(p.count, 2);
    }

    #[test]
    fn raw_google_syntax_is_never_rewritten() {
        // Quoted phrase contains "search" but the line is not command-shaped.
        assert!(parse("site:github.com \"search parser\"").is_none());
        assert!(parse("\"vector search\" benchmarks 2024").is_none());
    }

    #[test]
    fn apostrophes_inside_words_are_not_quotes() {
        assert!(parse("women's search trends").is_none());
        let p = parse("search google for the first 3 links for what's new in rust").expect("intent");
        assert_eq!(p.count, 3);
        assert_eq!(
            p.terms,
            vec!["what's".to_string(), "new".into(), "in".into(), "rust".into()]
        );
        let p = parse("search for 'women's health' news").expect("intent");
        assert_eq!(p.terms, vec!["women's health".to_string()]);
    }

    #[test]
    fn count_accepts_any_whitespace_and_keeps_other_numbers() {
        let p = parse("search Google for the first   2 links for 2024 election results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["2024".to_string(), "election".into(), "results".into()]);
    }

    #[test]
    fn politeness_prefix_still_counts_as_a_command() {
        let p = parse("please search google for 'cgfixit'").expect("intent");
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
        assert!(parse("tell me about 'cgfixit' search").is_none());
    }

    #[test]
    fn count_is_capped_at_ten() {
        let p = parse("search google for the first 99 links for 'x'").expect("intent");
        assert_eq!(p.count, 10);
        assert_eq!(p.terms, vec!["x".to_string()]);
    }
}
