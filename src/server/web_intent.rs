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
/// `google` after optional politeness words, is a `/web` remainder, or opens
/// with `first N links|results|...` as the console's `/web search` remainder
/// does), and it must carry an explicit signal, either balanced quoted terms
/// or a `first N` count outside those quotes. Ordinary Google syntax such as
/// `site:x "search parser"` or `vector search benchmarks` therefore never
/// reaches the rewrite.
pub fn parse(text: &str) -> Option<WebIntent> {
    parse_with_count(text, DEFAULT_COUNT)
}

/// Like [`parse`], but a request without an explicit `first N` keeps
/// `fallback_count` exactly as the caller supplied it, so the downstream
/// range validation still sees an out-of-range caller value.
pub fn parse_with_count(text: &str, fallback_count: usize) -> Option<WebIntent> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !is_command_shaped(&lower) {
        return None;
    }
    let (quoted, residual) = split_quoted(trimmed);
    let explicit_count = extract_count(&residual.to_ascii_lowercase());
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

    let count = explicit_count.unwrap_or(fallback_count);
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

/// Nouns that make a leading `first N` read as a result-count clause
/// (`first 2 links for rust`, the console's `/web search` remainder) rather
/// than a query about the first two of something.
const COUNT_NOUNS: &[&str] = &["links", "link", "results", "result", "hits", "hit", "pages", "page"];

fn is_command_shaped(lower: &str) -> bool {
    if lower.starts_with("/web") {
        return true;
    }
    let words: Vec<&str> = lower
        .split_whitespace()
        .map(word_of)
        .filter(|w| !w.is_empty())
        .collect();
    let mut i = 0;
    while i < words.len() && LEAD_IN.contains(&words[i]) {
        i += 1;
    }
    if words.get(i).is_some_and(|w| VERBS.contains(w)) {
        return true;
    }
    words.get(i..).is_some_and(|rest| find_count_clause(rest) == Some(0))
}

fn word_of(tok: &str) -> &str {
    tok.trim_matches(|c: char| !c.is_alphanumeric())
}

fn is_count_pair(a: &str, b: &str) -> bool {
    let n = word_of(b);
    word_of(a).eq_ignore_ascii_case("first") && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
}

/// Index of a `first N <noun>` result-count clause in `toks`, if any.
/// The noun is required so `first 2 amendments` stays part of the subject.
fn find_count_clause(toks: &[&str]) -> Option<usize> {
    toks.windows(3)
        .position(|w| is_count_pair(w[0], w[1]) && COUNT_NOUNS.contains(&word_of(w[2]).to_ascii_lowercase().as_str()))
}

/// `first N <noun>` when present and N >= 1; None otherwise. Any amount of
/// whitespace between the words is accepted, matching the console's
/// `first\s+\d+` predicate. Callers pass the text with quoted spans removed
/// so a phrase such as `"first 2 steps of Rust"` is never read as a count.
fn extract_count(lower: &str) -> Option<usize> {
    let toks: Vec<&str> = lower.split_whitespace().collect();
    let k = find_count_clause(&toks)?;
    match word_of(toks[k + 1]).parse::<usize>() {
        Ok(v) if v >= 1 => Some(v.min(MAX_COUNT)),
        _ => None,
    }
}

fn opens_quote(chars: &[char], i: usize) -> bool {
    i == 0 || matches!(chars[i - 1], c if c.is_whitespace() || matches!(c, '(' | ',' | ':' | '['))
}

fn closes_quote(chars: &[char], j: usize) -> bool {
    j + 1 == chars.len()
        || matches!(chars[j + 1], c if c.is_whitespace() || matches!(c, ',' | '.' | ';' | ':' | ')' | ']' | '!' | '?'))
}

/// Balanced quoted terms plus the text that remains once those spans are
/// removed. A quote opens at the start of a word and closes at the end of
/// one, so an apostrophe inside `women's` or `what's` is an ordinary
/// character and an unclosed quote is ignored.
fn split_quoted(text: &str) -> (Vec<String>, String) {
    let mut terms = Vec::new();
    let mut residual = String::new();
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
                residual.push(' ');
                i = j + 1;
                continue;
            }
        }
        residual.push(c);
        i += 1;
    }
    (terms, residual)
}

/// Words that may sit between the command verb and the subject
/// (`search Google (serpapi) for the ...`).
const AFTER_VERB: &[&str] = &["google", "serpapi", "for", "the", "on", "in", "using", "with", "web"];

/// Unquoted request: remove the command scaffolding by position (lead-in
/// words, the verb, engine and preposition words right after it, and the
/// `first N <noun>` clause with its joining `the` / `for`), then keep every
/// remaining token verbatim. Nothing inside the subject is filtered, so
/// `The Who`, `and`, `you` or a year survive; trimming of outer punctuation
/// is Unicode-aware so `école` and `東京` are kept intact.
fn tokenize_unquoted(text: &str) -> Vec<String> {
    let raw: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = raw.iter().map(|t| word_of(t).to_ascii_lowercase()).collect();
    let mut i = 0;
    if raw.first().is_some_and(|t| t.eq_ignore_ascii_case("/web")) {
        i += 1;
    }
    while i < lower.len() && LEAD_IN.contains(&lower[i].as_str()) {
        i += 1;
    }
    if i < lower.len() && VERBS.contains(&lower[i].as_str()) {
        i += 1;
        while i < lower.len() && AFTER_VERB.contains(&lower[i].as_str()) {
            i += 1;
        }
    }
    let mut rest: Vec<&str> = raw[i..].to_vec();
    if let Some(k) = find_count_clause(&rest) {
        let mut start = k;
        if start > 0 && word_of(rest[start - 1]).eq_ignore_ascii_case("the") {
            start -= 1;
        }
        let mut end = k + 3;
        if end < rest.len()
            && matches!(
                word_of(rest[end]).to_ascii_lowercase().as_str(),
                "for" | "of" | "about" | "on"
            )
        {
            end += 1;
        }
        rest.drain(start..end);
    }
    rest.iter()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '+' | '#'))
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect()
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
    fn console_web_search_remainder_is_command_shaped() {
        // The console posts only the remainder of `/web search ...`.
        let p = parse_with_count("first 2 links for rust", 5).expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["rust".to_string()]);
        let p = parse_with_count("first 3 results for election results", 5).expect("intent");
        assert_eq!(p.count, 3);
        assert_eq!(p.terms, vec!["election".to_string(), "results".into()]);
        // A raw query that merely starts with "first N" is not a command.
        assert!(parse("first 2 amendments").is_none());
    }

    #[test]
    fn count_inside_quotes_does_not_override_the_caller() {
        let p = parse_with_count("search Google for \"first 2 steps of Rust\"", 7).expect("intent");
        assert_eq!(p.count, 7);
        assert_eq!(p.terms, vec!["first 2 steps of Rust".to_string()]);
    }

    #[test]
    fn punctuated_scaffolding_is_dropped() {
        let p = parse("search Google: for the first 2 links for Rust").expect("intent");
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        let p = parse("search: first 2 links for Rust").expect("intent");
        assert_eq!(p.terms, vec!["Rust".to_string()]);
    }

    #[test]
    fn fallback_count_is_passed_through_unclamped() {
        // Out-of-range caller counts must still reach the downstream range
        // check instead of being silently corrected here.
        assert_eq!(parse_with_count("search google for 'rust'", 0).unwrap().count, 0);
        assert_eq!(parse_with_count("search google for 'rust'", 99).unwrap().count, 99);
    }

    #[test]
    fn non_ascii_terms_are_preserved() {
        let p = parse("search Google for first 2 links for 東京").expect("intent");
        assert_eq!(p.terms, vec!["東京".to_string()]);
        let p = parse("search first 2 links for école normale").expect("intent");
        assert_eq!(p.terms, vec!["école".to_string(), "normale".into()]);
    }

    #[test]
    fn subject_words_that_look_like_scaffolding_survive() {
        let p = parse("search first 2 links for The Who").expect("intent");
        assert_eq!(p.terms, vec!["The".to_string(), "Who".into()]);
        let p = parse("search google for the first 3 results for can you search and google").expect("intent");
        assert_eq!(
            p.terms,
            vec![
                "can".to_string(),
                "you".into(),
                "search".into(),
                "and".into(),
                "google".into()
            ]
        );
    }

    #[test]
    fn first_n_without_a_result_noun_is_part_of_the_subject() {
        // No count clause and no quotes: not a natural-language request, the
        // whole query goes to the provider untouched.
        assert!(parse("search Google for first 2 amendments").is_none());
        let p = parse("search Google for first 2 links for first 2 amendments").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["first".to_string(), "2".into(), "amendments".into()]);
    }

    #[test]
    fn count_is_capped_at_ten() {
        let p = parse("search google for the first 99 links for 'x'").expect("intent");
        assert_eq!(p.count, 10);
        assert_eq!(p.terms, vec!["x".to_string()]);
    }
}
