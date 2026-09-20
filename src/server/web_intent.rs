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

/// Outcome of the natural-language intent check for one query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebIntentParse {
    /// A command-shaped request with a clear signal: send `terms` / `count`.
    Rewrite(WebIntent),
    /// Not a natural-language request: send the caller's query untouched.
    PassThrough,
    /// A request that is command-shaped but carries an invalid result count
    /// (`first 0 results`, `first 99 links`); refuse instead of guessing.
    Invalid(String),
}

/// Parse a chat line or `/web search` remainder into engine, count, and terms.
/// Returns None unless the text is a natural-language search request with a
/// valid count; see [`parse_with_count`] for the three-way outcome.
///
/// Two conditions must hold for a rewrite: the line must be command-shaped
/// (it starts with `search` after optional politeness words, is
/// a `/web` remainder, or opens with `first N links|results|...` as the
/// console's `/web search` remainder does), and it must carry an explicit
/// signal, either balanced quoted terms or a `first N <noun>` count outside
/// those quotes. Ordinary Google syntax such as `site:x "search parser"` or
/// `vector search benchmarks` therefore never reaches the rewrite.
pub fn parse(text: &str) -> Option<WebIntent> {
    match parse_with_count(text, DEFAULT_COUNT) {
        WebIntentParse::Rewrite(intent) => Some(intent),
        _ => None,
    }
}

/// Like [`parse`], but a request without an explicit `first N` keeps
/// `fallback_count` exactly as the caller supplied it, so the downstream
/// range validation still sees an out-of-range caller value.
pub fn parse_with_count(text: &str, fallback_count: usize) -> WebIntentParse {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return WebIntentParse::PassThrough;
    }
    let lower = trimmed.to_ascii_lowercase();
    // `/web` needs a word boundary: `/webpack "x"` is a raw query, not a command.
    if !is_command_shaped(&lower) {
        return WebIntentParse::PassThrough;
    }
    let (quoted, residual) = split_quoted(trimmed);
    let explicit_count = match extract_count(&residual.to_ascii_lowercase()) {
        Some(Ok(n)) => Some(n),
        Some(Err(message)) => return WebIntentParse::Invalid(message),
        None => None,
    };
    if quoted.is_empty() && explicit_count.is_none() {
        return WebIntentParse::PassThrough;
    }

    let engine = if lower.contains("serpapi") {
        SearchEngine::Serpapi
    } else if lower.contains("google") {
        SearchEngine::Google
    } else {
        SearchEngine::Default
    };

    let count = explicit_count.unwrap_or(fallback_count);
    // Quoted phrases stay single terms; unquoted constraints around them
    // (`site:github.com`, extra words) are kept in their original order.
    let terms: Vec<String> = tokenize_unquoted(&residual)
        .into_iter()
        .filter_map(|tok| match placeholder_index(&tok) {
            Some(n) => quoted.get(n).cloned(),
            None => Some(tok),
        })
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        // Command-shaped with a valid count clause but nothing to search for
        // (`search first 2 results`): refuse rather than search the literal
        // instruction with the fallback count.
        return WebIntentParse::Invalid("search request has no subject after the count clause".into());
    }
    WebIntentParse::Rewrite(WebIntent { engine, count, terms })
}

/// Politeness words that may precede the command verb.
const LEAD_IN: &[&str] = &[
    "please", "can", "could", "would", "you", "kindly", "hey", "ok", "okay", "now",
];

/// Command verbs that open a natural-language search request. `google` is
/// deliberately not a verb: as the first word it is far more often the
/// entity (`Google "privacy policy"`) than an instruction, and `search`
/// followed by `Google` already covers the command form.
const VERBS: &[&str] = &["search"];

/// Nouns that make a leading `first N` read as a result-count clause
/// (`first 2 links for rust`, the console's `/web search` remainder) rather
/// than a query about the first two of something.
const COUNT_NOUNS: &[&str] = &["links", "link", "results", "result", "hits", "hit", "pages", "page"];

fn is_command_shaped(lower: &str) -> bool {
    if lower == "/web" || lower.starts_with("/web ") {
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

/// `first N <noun>` when present. `Some(Ok(n))` for 1..=MAX_COUNT,
/// `Some(Err(_))` when the clause is present but N is outside that range
/// (the request is refused rather than silently searched differently), and
/// `None` when there is no clause. Any amount of whitespace between the
/// words is accepted, matching the console's `first\s+\d+` predicate.
/// Callers pass the text with quoted spans removed so a phrase such as
/// `"first 2 steps of Rust"` is never read as a count.
fn extract_count(lower: &str) -> Option<Result<usize, String>> {
    let toks: Vec<&str> = lower.split_whitespace().collect();
    let k = find_count_clause(&toks)?;
    let tok = toks[k + 1];
    // `word_of` drops a leading sign, so `first -2 results` would otherwise
    // be searched as two results. A signed count is refused, never coerced.
    let signed = tok
        .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '-')
        .starts_with(['+', '-']);
    Some(match word_of(tok).parse::<usize>() {
        Ok(v) if !signed && (1..=MAX_COUNT).contains(&v) => Ok(v),
        _ => Err(format!(
            "result count must be an unsigned number between 1 and {MAX_COUNT}"
        )),
    })
}

fn opens_quote(chars: &[char], i: usize) -> bool {
    i == 0 || matches!(chars[i - 1], c if c.is_whitespace() || matches!(c, '(' | ',' | ':' | '['))
}

fn closes_quote(chars: &[char], j: usize) -> bool {
    j + 1 == chars.len()
        || matches!(chars[j + 1], c if c.is_whitespace() || matches!(c, ',' | '.' | ';' | ':' | ')' | ']' | '!' | '?'))
}

/// Placeholder token that stands in for the n-th quoted span inside the
/// residual text. NUL cannot appear in a chat line, so it never collides.
fn quote_placeholder(n: usize) -> String {
    format!("\u{0}q{n}\u{0}")
}

fn placeholder_index(tok: &str) -> Option<usize> {
    tok.strip_prefix("\u{0}q")?.strip_suffix('\u{0}')?.parse().ok()
}

/// Balanced quoted terms plus the text that remains once each span is
/// replaced by a placeholder token. A quote opens at the start of a word and
/// closes at the end of one, so an apostrophe inside `women's` or `what's`
/// is an ordinary character and an unclosed quote is ignored.
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
                    residual.push(' ');
                    residual.push_str(&quote_placeholder(terms.len()));
                    residual.push(' ');
                    terms.push(t.to_string());
                }
                i = j + 1;
                continue;
            }
        }
        residual.push(c);
        i += 1;
    }
    (terms, residual)
}

/// Engine names that may follow the verb (`search Google`, `search serpapi`).
const ENGINES: &[&str] = &["google", "serpapi"];
/// `search on Google`, `search with serpapi`, `search via Google`.
const ENGINE_LINKS: &[&str] = &["on", "with", "using", "via", "across"];
/// `search the web`, `search the internet`.
const ENGINE_PLACES: &[&str] = &["web", "internet"];
/// One optional word introducing the subject (`search for X`, `google about X`).
const INTRODUCERS: &[&str] = &["for", "about"];

/// Unquoted request: remove the command scaffolding by position (lead-in
/// words, the verb, engine and preposition words right after it, and the
/// `first N <noun>` clause with its joining `the` / `for`), then keep every
/// remaining token verbatim. Nothing inside the subject is filtered, so
/// `The Who`, `and`, `you`, a year or a `-excluded` operator survive, and
/// non-ASCII terms such as `école` and `東京` are kept intact.
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
        // Engine phrases, in any number: `Google`, `(serpapi)`, `on Google`,
        // `the web`, `on the web`. A bare `internet` or `in` is not consumed,
        // so subjects such as `Internet Archive` or `in vitro fertilization`
        // survive.
        loop {
            let at = |k: usize| lower.get(k).map(String::as_str);
            let linked_engine =
                at(i).is_some_and(|w| ENGINE_LINKS.contains(&w)) && at(i + 1).is_some_and(|w| ENGINES.contains(&w));
            let place = at(i) == Some("the") && at(i + 1).is_some_and(|w| ENGINE_PLACES.contains(&w));
            let linked_place = at(i).is_some_and(|w| ENGINE_LINKS.contains(&w))
                && at(i + 1) == Some("the")
                && at(i + 2).is_some_and(|w| ENGINE_PLACES.contains(&w));
            if at(i).is_some_and(|w| ENGINES.contains(&w)) {
                i += 1;
            } else if linked_place {
                i += 3;
            } else if linked_engine || place {
                i += 2;
            } else {
                break;
            }
        }
        // At most one introducer; whatever follows is the subject.
        if i < lower.len() && INTRODUCERS.contains(&lower[i].as_str()) {
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
    // `'a' and 'b'`: a joiner between two quoted spans is scaffolding too.
    // Uppercase `OR` is Google's operator, not a joiner: `"cats" OR "dogs"`
    // must reach the provider with the operator between the terms.
    let is_joiner = |tok: &str| {
        let w = word_of(tok);
        w.eq_ignore_ascii_case("and") || w.eq_ignore_ascii_case("plus") || (w.eq_ignore_ascii_case("or") && w != "OR")
    };
    let mut keep = vec![true; rest.len()];
    for k in 1..rest.len().saturating_sub(1) {
        if is_joiner(rest[k]) && placeholder_index(rest[k - 1]).is_some() && placeholder_index(rest[k + 1]).is_some() {
            keep[k] = false;
        }
    }
    let rest: Vec<&str> = rest.iter().zip(keep).filter(|(_, k)| *k).map(|(t, _)| *t).collect();
    // Subject tokens are kept verbatim apart from surrounding quote characters
    // and trailing sentence punctuation, so Google operators such as
    // `-pinterest`, `@handle`, `*` or `site:` survive.
    rest.iter()
        .map(|t| {
            t.trim_matches(|c: char| c == '"' || c == '\'')
                .trim_end_matches([',', '.', ';', ':', '!', '?'])
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(text: &str, fallback: usize) -> WebIntent {
        match parse_with_count(text, fallback) {
            WebIntentParse::Rewrite(intent) => intent,
            other => panic!("{text:?} should rewrite, got {other:?}"),
        }
    }

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
        assert_eq!(parse_with_count("elasticsearch tuning", 3), WebIntentParse::PassThrough);
    }

    #[test]
    fn caller_count_survives_when_no_first_n_is_given() {
        let p = rewrite("search google for 'cgfixit'", 3);
        assert_eq!(p.count, 3);
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
        let p = rewrite("search google for the first 2 links for 'cgfixit'", 7);
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
        assert_eq!(p.terms, vec!["women's health".to_string(), "news".into()]);
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
        let p = rewrite("first 2 links for rust", 5);
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["rust".to_string()]);
        let p = rewrite("first 3 results for election results", 5);
        assert_eq!(p.count, 3);
        assert_eq!(p.terms, vec!["election".to_string(), "results".into()]);
        // A raw query that merely starts with "first N" is not a command.
        assert!(parse("first 2 amendments").is_none());
    }

    #[test]
    fn count_inside_quotes_does_not_override_the_caller() {
        let p = rewrite("search Google for \"first 2 steps of Rust\"", 7);
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
        for fallback in [0usize, 99] {
            match parse_with_count("search google for 'rust'", fallback) {
                WebIntentParse::Rewrite(intent) => assert_eq!(intent.count, fallback),
                other => panic!("expected a rewrite, got {other:?}"),
            }
        }
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
    fn out_of_range_textual_counts_are_refused_not_guessed() {
        for line in [
            "search first 0 results for Rust",
            "search google for the first 99 links for 'x'",
        ] {
            match parse_with_count(line, 5) {
                WebIntentParse::Invalid(message) => assert!(message.contains("between 1 and 10"), "{message}"),
                other => panic!("{line:?} should be refused, got {other:?}"),
            }
        }
        // A clause with a valid count still rewrites; no clause still passes through.
        assert!(matches!(
            parse_with_count("search first 10 links for Rust", 5),
            WebIntentParse::Rewrite(_)
        ));
        assert_eq!(
            parse_with_count("vector search benchmarks", 5),
            WebIntentParse::PassThrough
        );
    }

    #[test]
    fn quoted_terms_keep_their_unquoted_constraints_in_order() {
        let p = parse("search Google for \"rust async\" site:github.com first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["rust async".to_string(), "site:github.com".into()]);
        let p = parse("search for -reddit 'tokio tutorial' 2025").expect("intent");
        assert_eq!(
            p.terms,
            vec!["-reddit".to_string(), "tokio tutorial".into(), "2025".into()]
        );
    }

    #[test]
    fn trailing_count_clause_keeps_a_leading_article() {
        let p = parse("search for The Who first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["The".to_string(), "Who".into()]);
        let p = parse("search the web for the first 2 links for The Who").expect("intent");
        assert_eq!(p.terms, vec!["The".to_string(), "Who".into()]);
    }

    #[test]
    fn subject_leading_scaffold_lookalikes_are_kept() {
        let p = parse("search for in vitro fertilization first 2 results").expect("intent");
        assert_eq!(p.terms, vec!["in".to_string(), "vitro".into(), "fertilization".into()]);
        let p = parse("search Internet Archive first 2 results").expect("intent");
        assert_eq!(p.terms, vec!["Internet".to_string(), "Archive".into()]);
        let p = parse("search on Google the web for 'x'").expect("intent");
        assert_eq!(p.terms, vec!["x".to_string()]);
        let p = parse("search on the web for first 2 results for Rust").expect("intent");
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        let p = parse("search across the internet for the first 2 links for Rust").expect("intent");
        assert_eq!(p.terms, vec!["Rust".to_string()]);
    }

    #[test]
    fn count_clause_without_a_subject_is_refused() {
        match parse_with_count("search first 2 results", 5) {
            WebIntentParse::Invalid(message) => assert!(message.contains("no subject"), "{message}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
        match parse_with_count("search Google for the first 3 links", 5) {
            WebIntentParse::Invalid(_) => {}
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn a_leading_entity_name_is_not_a_verb() {
        // `/web search Google "privacy policy"` reaches the server as the
        // remainder `Google "privacy policy"`: a literal query, not a command.
        assert_eq!(
            parse_with_count("Google \"privacy policy\"", 5),
            WebIntentParse::PassThrough
        );
        assert_eq!(
            parse_with_count("google first 2 links for rust", 5),
            WebIntentParse::PassThrough
        );
        let p = parse("search Google \"privacy policy\"").expect("intent");
        assert_eq!(p.terms, vec!["privacy policy".to_string()]);
    }

    #[test]
    fn slash_web_needs_a_word_boundary() {
        assert_eq!(
            parse_with_count("/webpack \"module federation\"", 5),
            WebIntentParse::PassThrough
        );
        assert_eq!(
            parse_with_count("/webhook first 2 links for x", 5),
            WebIntentParse::PassThrough
        );
        let p = parse("/web search first 2 links for \"cgfixit\"").expect("intent");
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
    }

    #[test]
    fn uppercase_or_between_quoted_terms_is_a_query_operator() {
        let p = parse("search \"cats\" OR \"dogs\"").expect("intent");
        assert_eq!(p.terms, vec!["cats".to_string(), "OR".into(), "dogs".into()]);
        assert_eq!(p.query(), "cats OR dogs");
        // Lowercase `or` (and `and` / `plus`) between quoted terms is still
        // natural-language scaffolding.
        let p = parse("search \"cats\" or \"dogs\"").expect("intent");
        assert_eq!(p.terms, vec!["cats".to_string(), "dogs".into()]);
        let p = parse("search 'a' and 'b' plus 'c'").expect("intent");
        assert_eq!(p.terms, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn signed_result_counts_are_refused_not_coerced() {
        for text in [
            "search \"Rust\" first -2 results",
            "search \"Rust\" first +2 results",
            "search first -2 links for rust",
            "search first (-2) links for rust",
        ] {
            match parse_with_count(text, 5) {
                WebIntentParse::Invalid(message) => assert!(message.contains("unsigned"), "{text:?}: {message}"),
                other => panic!("{text:?} should be refused, got {other:?}"),
            }
        }
        // A plain number with trailing punctuation is still a count.
        let p = parse("search first 2, links for rust").expect("intent");
        assert_eq!(p.count, 2);
    }

    #[test]
    fn leading_search_operators_survive_in_the_subject() {
        let p = parse("search first 2 results for -pinterest rust").expect("intent");
        assert_eq!(p.terms, vec!["-pinterest".to_string(), "rust".into()]);
        let p = parse("search first 2 links for @cgfixit site:github.com *harness*").expect("intent");
        assert_eq!(
            p.terms,
            vec!["@cgfixit".to_string(), "site:github.com".into(), "*harness*".into()]
        );
    }
}
