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
    /// `terms[i]` came from an ordinary quoted span (`"New York Times"`)
    /// and is sent to the provider as an exact phrase. Exclusions and
    /// operator arguments (`-"cats"`, `site:"x"`) already carry their quotes.
    #[serde(default)]
    pub quoted: Vec<bool>,
}

impl WebIntent {
    pub fn query(&self) -> String {
        self.terms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if self.quoted.get(i).copied().unwrap_or(false) {
                    format!("\"{t}\"")
                } else {
                    t.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
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
    let (quoted, bare, residual) = split_quoted(trimmed);
    // A double quote that did not pair up would otherwise be trimmed off a
    // subject word and silently broaden the search (`"C programming` → `C
    // programming`). Refuse the malformed request instead. Apostrophes are
    // ordinary characters (`women's`), so single quotes are left alone.
    if residual.chars().any(|c| matches!(c, '"' | '\u{201c}' | '\u{201d}')) {
        return WebIntentParse::Invalid("search request has an unbalanced double quote".into());
    }
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
    let (terms, quoted): (Vec<String>, Vec<bool>) = tokenize_unquoted(&residual)
        .into_iter()
        .filter_map(|tok| match placeholder_index(&tok) {
            Some(n) => quoted.get(n).cloned().map(|t| (t, bare[n])),
            None => Some((tok, false)),
        })
        .filter(|(t, _)| !t.is_empty())
        .unzip();
    if terms.is_empty() {
        // Command-shaped with a valid count clause but nothing to search for
        // (`search first 2 results`): refuse rather than search the literal
        // instruction with the fallback count.
        return WebIntentParse::Invalid("search request has no subject after the count clause".into());
    }
    WebIntentParse::Rewrite(WebIntent {
        engine,
        count,
        terms,
        quoted,
    })
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
    let raw: Vec<&str> = lower.split_whitespace().filter(|t| !word_of(t).is_empty()).collect();
    let mut i = 0;
    while i < raw.len() && is_plain_word(raw[i], LEAD_IN) {
        i += 1;
    }
    if raw.get(i).is_some_and(|t| is_plain_word(t, VERBS)) {
        return true;
    }
    raw.get(i..).is_some_and(|rest| find_count_clause(rest) == Some(0))
}

/// `tok` is one of `words` and carries no operator prefix: `-search "x"`
/// is Google's exclusion of the word `search`, not the command verb.
fn is_plain_word(tok: &str, words: &[&str]) -> bool {
    tok.chars().next().is_some_and(char::is_alphanumeric) && words.contains(&word_of(tok).to_ascii_lowercase().as_str())
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
    // Every clause is validated, not only the first: `first 2 results for
    // Rust first 99 links` is refused rather than searched with the second
    // clause left in the query. Two clauses are ambiguous and refused too.
    let clauses: Vec<usize> = (0..toks.len().saturating_sub(2))
        .filter(|&k| find_count_clause(&toks[k..k + 3]) == Some(0))
        .collect();
    let mut counts = Vec::new();
    for &k in &clauses {
        let tok = toks[k + 1];
        // `word_of` drops any leading sign, so `first -2 results` (or a
        // Unicode minus, `first \u{2212}2 results`) would otherwise be
        // searched as two results. Only an opening bracket or quote may
        // precede the digits; any other leading character makes the count
        // invalid rather than coerced.
        let core = tok
            .trim_start_matches(['(', '[', '"', '\''])
            .trim_end_matches(|c: char| !c.is_alphanumeric());
        match core.parse::<usize>() {
            Ok(v) if core.chars().all(|c| c.is_ascii_digit()) && (1..=MAX_COUNT).contains(&v) => counts.push(v),
            _ => {
                return Some(Err(format!(
                    "result count must be an unsigned number between 1 and {MAX_COUNT}"
                )))
            }
        }
    }
    match counts.as_slice() {
        [] => None,
        [one] => Some(Ok(*one)),
        _ => Some(Err("search request has more than one result-count clause".into())),
    }
}

/// The closing character that pairs with an opening quote: ASCII quotes
/// close themselves, typographic quotes (`“…”`, `‘…’`) close with their
/// right-hand form.
fn closing_quote(open: char) -> Option<char> {
    match open {
        '"' | '\'' => Some(open),
        '\u{201c}' => Some('\u{201d}'),
        '\u{2018}' => Some('\u{2019}'),
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
///
/// Google's quoted exclusion `-"cats"` is one span too: the operator and the
/// quotes stay attached (`-"cats"`, `-"Chris Grady"`), so the provider sees
/// the exclusion of the whole phrase instead of a detached `-"cats`.
fn split_quoted(text: &str) -> (Vec<String>, Vec<bool>, String) {
    let mut terms = Vec::new();
    let mut bare = Vec::new();
    let mut residual = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some(q_at) = opens_quote(&chars, i)
            .then(|| quote_after_operator(&chars, i))
            .flatten()
        {
            let q = chars[q_at];
            let close = closing_quote(q).unwrap_or(q);
            if let Some(j) = (q_at + 1..chars.len()).find(|&j| chars[j] == close && closes_quote(&chars, j)) {
                let t: String = chars[q_at + 1..j].iter().collect();
                let t = t.trim();
                if !t.is_empty() {
                    let prefix: String = chars[i..q_at].iter().collect();
                    residual.push(' ');
                    residual.push_str(&quote_placeholder(terms.len()));
                    residual.push(' ');
                    bare.push(prefix.is_empty());
                    terms.push(if prefix.is_empty() {
                        t.to_string()
                    } else {
                        format!("{prefix}{q}{t}{close}")
                    });
                }
                i = j + 1;
                continue;
            }
        }
        residual.push(chars[i]);
        i += 1;
    }
    (terms, bare, residual)
}

/// At word start `i`, the index of the opening quote of a bare quoted span
/// (`"x"`), a quoted exclusion (`-"x"`) or a quoted operator argument
/// (`intitle:"x"`, `site:"x"`, `-site:"x"`). The operator is kept with the
/// span so the provider sees it intact; `foo"x"` (no colon) is not a span.
fn quote_after_operator(chars: &[char], i: usize) -> Option<usize> {
    let mut k = i;
    // `-"x"`, `+"x"` and `~"x"`: the prefixes the console also treats as
    // operators on a quoted span.
    if chars.get(k).is_some_and(|c| matches!(c, '-' | '+' | '~')) {
        k += 1;
    }
    let op_start = k;
    while chars.get(k).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
        k += 1;
    }
    if k > op_start {
        if chars.get(k) != Some(&':') {
            return None;
        }
        k += 1;
    }
    chars.get(k).copied().filter(|c| closing_quote(*c).is_some()).map(|_| k)
}

/// Engine names that may follow the verb (`search Google`, `search serpapi`).
const ENGINES: &[&str] = &["google", "serpapi"];
/// `search on Google`, `search with serpapi`, `search via Google`,
/// `first 2 results from Google`.
const ENGINE_LINKS: &[&str] = &["on", "with", "using", "via", "across", "from"];
/// `search the web`, `search the internet`.
const ENGINE_PLACES: &[&str] = &["web", "internet"];
/// One optional word introducing the subject (`search for X`, `google about X`).
const INTRODUCERS: &[&str] = &["for", "about"];

/// Engine phrases, in any number, starting at `i`: `Google`, `(serpapi)`,
/// `on Google`, `the web`, `on the web`. Returns the index just past them. A
/// bare `internet` or `in` is not consumed, so subjects such as `Internet
/// Archive` or `in vitro fertilization` survive.
fn skip_engine_phrases(lower: &[String], mut i: usize) -> usize {
    loop {
        let at = |k: usize| lower.get(k).map(String::as_str);
        let linked_engine =
            at(i).is_some_and(|w| ENGINE_LINKS.contains(&w)) && at(i + 1).is_some_and(|w| ENGINES.contains(&w));
        // A bare engine word or a bare `the web` is scaffolding only when
        // scaffolding follows it (`Google for x`, `the web "x"`, `Google
        // first 2 links`, `Google (serpapi)`) or nothing does. Followed by a
        // plain word it begins the subject: `Google Trends first 2 results`,
        // `The Web Conference first 2 results`.
        let scaffolding_follows = |k: usize| {
            at(k).is_none_or(|next| {
                INTRODUCERS.contains(&next)
                    || ENGINES.contains(&next)
                    || ENGINE_LINKS.contains(&next)
                    || next == "the"
                    || next == "first"
                    || is_placeholder_word(next)
            })
        };
        let place =
            at(i) == Some("the") && at(i + 1).is_some_and(|w| ENGINE_PLACES.contains(&w)) && scaffolding_follows(i + 2);
        let linked_place = at(i).is_some_and(|w| ENGINE_LINKS.contains(&w))
            && at(i + 1) == Some("the")
            && at(i + 2).is_some_and(|w| ENGINE_PLACES.contains(&w));
        let bare_engine = at(i).is_some_and(|w| ENGINES.contains(&w)) && scaffolding_follows(i + 1);
        if bare_engine {
            i += 1;
        } else if linked_place {
            i += 3;
        } else if linked_engine || place {
            i += 2;
        } else {
            return i;
        }
    }
}

/// The lowercase word used for scaffolding matches. An operator-prefixed
/// token keeps its prefix (`-google` is Google's exclusion of the word, not
/// the engine), so it never matches an engine, link or introducer table.
fn scaffold_word(tok: &str) -> String {
    if tok.starts_with(['-', '+', '~', '@']) {
        tok.to_ascii_lowercase()
    } else {
        word_of(tok).to_ascii_lowercase()
    }
}

/// `word_of` of a quote placeholder (`\0q3\0` → `q3`).
fn is_placeholder_word(w: &str) -> bool {
    w.strip_prefix('q')
        .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

/// Length of a *linked* engine phrase at `k` (`on Google`, `the web`,
/// `from the internet`), or 0. Unlike [`skip_engine_phrases`] a bare engine
/// word does not count.
fn linked_engine_phrase_len(lower: &[String], k: usize) -> usize {
    let at = |i: usize| lower.get(i).map(String::as_str);
    let linked = at(k).is_some_and(|w| ENGINE_LINKS.contains(&w));
    if linked && at(k + 1).is_some_and(|w| ENGINES.contains(&w)) {
        2
    } else if linked && at(k + 1) == Some("the") && at(k + 2).is_some_and(|w| ENGINE_PLACES.contains(&w)) {
        3
    } else if at(k) == Some("the") && at(k + 1).is_some_and(|w| ENGINE_PLACES.contains(&w)) {
        2
    } else {
        0
    }
}

/// Unquoted request: remove the command scaffolding by position (lead-in
/// words, the verb, engine and preposition words right after it, and the
/// `first N <noun>` clause with its joining `the` / `for`), then keep every
/// remaining token verbatim. Nothing inside the subject is filtered, so
/// `The Who`, `and`, `you`, a year or a `-excluded` operator survive, and
/// non-ASCII terms such as `école` and `東京` are kept intact.
fn tokenize_unquoted(text: &str) -> Vec<String> {
    let raw: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = raw.iter().map(|t| scaffold_word(t)).collect();
    let mut i = 0;
    if raw.first().is_some_and(|t| t.eq_ignore_ascii_case("/web")) {
        i += 1;
    }
    while i < raw.len() && is_plain_word(raw[i], LEAD_IN) {
        i += 1;
    }
    if i < raw.len() && is_plain_word(raw[i], VERBS) {
        i = skip_engine_phrases(&lower, i + 1);
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
        // A word before the clause is never removed, even a preposition:
        // `Rust for the first 2 results` reaches the provider as `Rust for`
        // (a harmless stop word) rather than risk truncating a title such as
        // `What Are You Looking For first 2 results` or `Carry On first 2
        // results`. Dropping content is the costlier mistake.
        // The clause may carry its own engine phrase and connector:
        // `first 2 results on Google for Rust`, `first 2 links on the web
        // about rust`. Both are scaffolding; the subject starts after them.
        let lower_rest: Vec<String> = rest.iter().map(|t| scaffold_word(t)).collect();
        // Only an engine phrase with its connector is consumed here (`on
        // Google`, `from the web`): a bare engine word or a bare `the web`
        // after the count clause opens the subject (`first 2 results Google
        // Trends`, `first 2 results The Web Conference`).
        let mut end = k + 3;
        loop {
            let len = linked_engine_phrase_len(&lower_rest, end);
            if len == 0 || !ENGINE_LINKS.contains(&lower_rest[end].as_str()) {
                break;
            }
            end += len;
        }
        // `for` after the clause is the documented introducer (`first 2
        // links for rust`) and is always scaffolding. `about`, `of` and `on`
        // are consumed only before a quoted term: before an unquoted subject
        // they may be the title's first word (`first 2 results On the Road`,
        // `About Time`, `Of Mice and Men`), and losing it is the costlier
        // mistake.
        if end < rest.len() {
            let connector = lower_rest[end].as_str();
            let before_quote = end + 1 < rest.len() && placeholder_index(rest[end + 1]).is_some();
            if connector == "for" || (matches!(connector, "of" | "about" | "on") && before_quote) {
                end += 1;
            }
        }
        rest.drain(start..end);
    }
    // A connected engine phrase that closes the request right after a
    // quoted span (`"Rust" with Google`, `"Rust" on the web`, `'x' first 2
    // links from serpapi`) is scaffolding: the quoted span is the whole
    // subject. After unquoted subject words the same phrase is ambiguous
    // (`jobs with Google`, `history of the web`, `data from Google Trends`)
    // and is kept as content, as is a bare engine word (`"privacy policy"
    // Google`).
    loop {
        let lower_rest: Vec<String> = rest.iter().map(|t| scaffold_word(t)).collect();
        let trailing = (1..lower_rest.len()).find(|&k| {
            let len = linked_engine_phrase_len(&lower_rest, k);
            len > 0
                && k + len == lower_rest.len()
                && ENGINE_LINKS.contains(&lower_rest[k].as_str())
                && placeholder_index(rest[k - 1]).is_some()
        });
        match trailing {
            Some(k) => rest.truncate(k),
            None => break,
        }
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
    // `-pinterest`, `@handle`, `*` or `site:` survive. Quote characters are
    // kept: a double quote here is refused earlier, and an apostrophe is
    // part of the word.
    rest.iter()
        .map(|t| t.trim_end_matches([',', '.', ';', ':', '!', '?']).to_string())
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
        assert_eq!(p.query(), "\"cgfixit\" \"Chris Grady\"");
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
        assert_eq!(p.query(), "\"cats\" OR \"dogs\"");
        // Lowercase `or` (and `and` / `plus`) between quoted terms is still
        // natural-language scaffolding.
        let p = parse("search \"cats\" or \"dogs\"").expect("intent");
        assert_eq!(p.terms, vec!["cats".to_string(), "dogs".into()]);
        let p = parse("search 'a' and 'b' plus 'c'").expect("intent");
        assert_eq!(p.terms, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn engine_phrase_after_the_count_clause_is_scaffolding() {
        let p = parse("search first 2 results on Google for Rust").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.engine, SearchEngine::Google);
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        // `about` before an unquoted subject is kept (it may open a title);
        // the provider treats it as a stop word.
        let p = parse("search the first 3 links on the web about rust async").expect("intent");
        assert_eq!(p.terms, vec!["about".to_string(), "rust".into(), "async".into()]);
        let p = parse("search first 2 results from Google for Rust").expect("intent");
        assert_eq!(p.engine, SearchEngine::Google);
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        let p = parse("search first 2 links using serpapi for 'cgfixit'").expect("intent");
        assert_eq!(p.engine, SearchEngine::Serpapi);
        assert_eq!(p.terms, vec!["cgfixit".to_string()]);
        // `on` before an unquoted subject is kept (it may open a title such
        // as `On the Road`); the provider treats it as a stop word.
        let p = parse("search first 2 links on rust").expect("intent");
        assert_eq!(p.terms, vec!["on".to_string(), "rust".into()]);
        // A bare engine word after the count clause opens the subject.
        let p = parse("search first 2 results Google Trends").expect("intent");
        assert_eq!(p.terms, vec!["Google".to_string(), "Trends".into()]);
        let p = parse("search first 2 results The Web Conference").expect("intent");
        assert_eq!(p.terms, vec!["The".to_string(), "Web".into(), "Conference".into()]);
        let p = parse("search The Web Conference first 2 results").expect("intent");
        assert_eq!(p.terms, vec!["The".to_string(), "Web".into(), "Conference".into()]);
        let p = parse("search the web first 2 links for rust").expect("intent");
        assert_eq!(p.terms, vec!["rust".to_string()]);
        // A title-leading preposition after the count clause is kept; only
        // `for` (the documented introducer) is always scaffolding there.
        for (text, terms) in [
            ("search first 2 results On the Road", vec!["On", "the", "Road"]),
            ("search first 2 results About Time", vec!["About", "Time"]),
            (
                "search first 2 results Of Mice and Men",
                vec!["Of", "Mice", "and", "Men"],
            ),
            ("search first 2 results about \"rust\"", vec!["rust"]),
        ] {
            let p = parse(text).expect("intent");
            assert_eq!(
                p.terms,
                terms.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                "{text}"
            );
        }
        // ...and so does one right after the verb when a plain word follows.
        let p = parse("search Google Trends first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["Google".to_string(), "Trends".into()]);
        let p = parse("search Google \"privacy policy\"").expect("intent");
        assert_eq!(p.engine, SearchEngine::Google);
        assert_eq!(p.terms, vec!["privacy policy".to_string()]);
    }

    #[test]
    fn quoted_exclusions_keep_operator_and_quotes() {
        let p = parse("search -\"cats\" first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["-\"cats\"".to_string()]);
        let p = parse("search +\"rust async\" first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["+\"rust async\"".to_string()]);
        assert_eq!(p.query(), "+\"rust async\"");
        let p = parse("search ~\"cheap\" flights").expect("intent");
        assert_eq!(p.terms, vec!["~\"cheap\"".to_string(), "flights".into()]);
        let p = parse("search \"dogs\" -'Chris Grady'").expect("intent");
        assert_eq!(p.terms, vec!["dogs".to_string(), "-'Chris Grady'".into()]);
        assert_eq!(p.query(), "\"dogs\" -'Chris Grady'");
        // A hyphen inside a word is not an exclusion operator.
        let p = parse("search first 2 links for well-known rust").expect("intent");
        assert_eq!(p.terms, vec!["well-known".to_string(), "rust".into()]);
    }

    #[test]
    fn quoted_spans_reach_the_provider_as_exact_phrases() {
        let p = parse("search \"New York Times\"").expect("intent");
        assert_eq!(p.terms, vec!["New York Times".to_string()]);
        assert_eq!(p.quoted, vec![true]);
        assert_eq!(p.query(), "\"New York Times\"");
        let p = parse("search first 2 links for \"rust\" site:github.com async").expect("intent");
        assert_eq!(p.quoted, vec![true, false, false]);
        assert_eq!(p.query(), "\"rust\" site:github.com async");
    }

    #[test]
    fn words_before_a_trailing_count_clause_are_never_removed() {
        // `the` directly before the clause is the article of `the first N`;
        // anything else is the subject's own, preposition or not.
        let p = parse("search Rust for the first 2 results").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["Rust".to_string(), "for".into()]);
        for (text, terms) in [
            ("search Carry On first 2 results", vec!["Carry", "On"]),
            (
                "search What Are You Looking For first 2 results",
                vec!["What", "Are", "You", "Looking", "For"],
            ),
        ] {
            let p = parse(text).expect("intent");
            assert_eq!(
                p.terms,
                terms.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                "{text}"
            );
        }
    }

    #[test]
    fn unbalanced_double_quote_is_refused() {
        match parse_with_count("search first 2 results for \"C programming", 5) {
            WebIntentParse::Invalid(message) => assert!(message.contains("unbalanced"), "{message}"),
            other => panic!("should be refused, got {other:?}"),
        }
        // An apostrophe is an ordinary character.
        let p = parse("search first 2 results for women's health").expect("intent");
        assert_eq!(p.terms, vec!["women's".to_string(), "health".into()]);
    }

    #[test]
    fn excluded_engine_word_is_a_query_operator() {
        let p = parse("search -Google \"privacy policy\"").expect("intent");
        assert_eq!(p.engine, SearchEngine::Google, "engine detection still reads the text");
        assert_eq!(p.terms, vec!["-Google".to_string(), "privacy policy".into()]);
        assert_eq!(p.query(), "-Google \"privacy policy\"");
    }

    #[test]
    fn trailing_engine_phrases_are_scaffolding() {
        let p = parse("search \"Rust\" with Google").expect("intent");
        assert_eq!(p.engine, SearchEngine::Google);
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        let p = parse("search \"Rust\" on the web").expect("intent");
        assert_eq!(p.terms, vec!["Rust".to_string()]);
        let p = parse("search 'x' first 2 links from serpapi").expect("intent");
        assert_eq!(p.engine, SearchEngine::Serpapi);
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["x".to_string()]);
        // A bare engine word inside the subject is kept.
        let p = parse("search \"privacy policy\" Google").expect("intent");
        assert_eq!(p.terms, vec!["privacy policy".to_string(), "Google".into()]);
        // Engine-like words inside the subject are content, not scaffolding.
        let p = parse("search first 2 results for the web accessibility guidelines").expect("intent");
        assert_eq!(
            p.terms,
            vec![
                "the".to_string(),
                "web".into(),
                "accessibility".into(),
                "guidelines".into()
            ]
        );
        let p = parse("search \"trend\" data from Google Trends").expect("intent");
        assert_eq!(
            p.terms,
            vec![
                "trend".to_string(),
                "data".into(),
                "from".into(),
                "Google".into(),
                "Trends".into()
            ]
        );
        // After unquoted subject words the phrase is ambiguous and stays.
        let p = parse("search first 2 results for jobs with Google").expect("intent");
        assert_eq!(p.terms, vec!["jobs".to_string(), "with".into(), "Google".into()]);
        // A trailing `the web` without its connector belongs to the subject.
        let p = parse("search first 2 results for history of the web").expect("intent");
        assert_eq!(
            p.terms,
            vec!["history".to_string(), "of".into(), "the".into(), "web".into()]
        );
    }

    #[test]
    fn typographic_quotes_are_quoted_terms() {
        let p = parse("search Google for \u{201c}Rust async\u{201d}").expect("intent");
        assert_eq!(p.engine, SearchEngine::Google);
        assert_eq!(p.terms, vec!["Rust async".to_string()]);
        // The apostrophe inside `women’s` is not the closing quote.
        let p = parse("search \u{2018}women\u{2019}s health\u{2019} first 2 links").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["women\u{2019}s health".to_string()]);
        let p = parse("search -\u{201c}cats\u{201d} \"dogs\"").expect("intent");
        assert_eq!(p.terms, vec!["-\u{201c}cats\u{201d}".to_string(), "dogs".into()]);
    }

    #[test]
    fn operator_prefixed_verb_is_a_query_word() {
        let p = parse("/web search -search \"rust\"").expect("intent");
        assert_eq!(p.terms, vec!["-search".to_string(), "rust".into()]);
        assert_eq!(parse_with_count("-search \"rust\"", 5), WebIntentParse::PassThrough);
        assert_eq!(
            parse_with_count("-please search \"rust\"", 5),
            WebIntentParse::PassThrough
        );
    }

    #[test]
    fn quoted_operator_arguments_stay_one_term() {
        let p = parse("search intitle:\"rust async\"").expect("intent");
        assert_eq!(p.terms, vec!["intitle:\"rust async\"".to_string()]);
        let p = parse("search site:\"example.com\" first 2 links").expect("intent");
        assert_eq!(p.count, 2);
        assert_eq!(p.terms, vec!["site:\"example.com\"".to_string()]);
        let p = parse("search -site:'pinterest.com' \"rust\"").expect("intent");
        assert_eq!(p.terms, vec!["-site:'pinterest.com'".to_string(), "rust".into()]);
        assert_eq!(p.query(), "-site:'pinterest.com' \"rust\"");
        // A verb glued to its quote was never command-shaped (unchanged), and
        // `foo"x"` (no colon) is no span: it stays an ordinary subject token,
        // trimmed of its trailing quote like any other unquoted word.
        assert_eq!(parse_with_count("search:\"rust\"", 5), WebIntentParse::PassThrough);
        match parse_with_count("search foo\"bar\" first 2 links", 5) {
            WebIntentParse::Invalid(message) => assert!(message.contains("unbalanced"), "{message}"),
            other => panic!("should be refused, got {other:?}"),
        }
    }

    #[test]
    fn every_count_clause_is_validated_and_only_one_is_allowed() {
        match parse_with_count("search first 2 results for Rust first 99 links", 5) {
            WebIntentParse::Invalid(message) => assert!(message.contains("between 1 and 10"), "{message}"),
            other => panic!("should be refused, got {other:?}"),
        }
        match parse_with_count("search first 2 results for Rust first 3 links", 5) {
            WebIntentParse::Invalid(message) => assert!(message.contains("more than one"), "{message}"),
            other => panic!("should be refused, got {other:?}"),
        }
    }

    #[test]
    fn signed_result_counts_are_refused_not_coerced() {
        for text in [
            "search \"Rust\" first -2 results",
            "search \"Rust\" first +2 results",
            "search \"Rust\" first \u{2212}2 results",
            "search \"Rust\" first \u{2013}2 results",
            "search first -2 links for rust",
            "search first (-2) links for rust",
        ] {
            match parse_with_count(text, 5) {
                WebIntentParse::Invalid(message) => assert!(message.contains("unsigned"), "{text:?}: {message}"),
                other => panic!("{text:?} should be refused, got {other:?}"),
            }
        }
        // A plain number with trailing punctuation or brackets is still a count.
        let p = parse("search first 2, links for rust").expect("intent");
        assert_eq!(p.count, 2);
        let p = parse("search first (2) links for rust").expect("intent");
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
