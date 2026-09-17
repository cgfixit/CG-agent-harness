//! Bounded FTS5 MATCH builder for structured-memory facts.
//!
//! Natural-language input is tokenized, length-capped, and quoted. Field
//! prefixes (`title:` / `value:` / `tags:`) keep MATCH from treating the query
//! as a free FTS5 expression that could glue operators across rows. This is
//! not a secret scanner and not episode search.

const FTS_RESERVED: &[&str] = &["AND", "OR", "NOT", "NEAR"];

/// Split `raw` into bounded alphanumeric tokens. Punctuation, quotes, FTS
/// operators, and column selectors are discarded — they never reach MATCH.
pub fn tokenize_query(raw: &str, max_tokens: usize, max_token_chars: usize) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, tokens: &mut Vec<String>| {
        if current.is_empty() {
            return;
        }
        let token = std::mem::take(current);
        if is_reserved(&token) {
            return;
        }
        let clipped: String = token.chars().take(max_token_chars.max(1)).collect();
        if !clipped.is_empty() {
            tokens.push(clipped);
        }
    };
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else {
            flush(&mut current, &mut tokens);
            if tokens.len() >= max_tokens {
                break;
            }
        }
    }
    if tokens.len() < max_tokens {
        flush(&mut current, &mut tokens);
    }
    tokens.truncate(max_tokens);
    tokens
}

fn is_reserved(token: &str) -> bool {
    FTS_RESERVED.iter().any(|word| token.eq_ignore_ascii_case(word))
}

pub fn quote_token(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

/// Field-prefixed AND of quoted tokens. `None` means "match nothing" — never
/// a bare `*` or an unquoted operator string.
pub fn match_expression(tokens: &[String]) -> Option<String> {
    joined_expression(tokens, " AND ")
}

fn joined_expression(tokens: &[String], join: &str) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    let clauses: Vec<String> = tokens
        .iter()
        .map(|token| {
            let quoted = quote_token(token);
            format!("(title: {quoted} OR value: {quoted} OR tags: {quoted})")
        })
        .collect();
    Some(clauses.join(join))
}

pub fn safe_match(raw: &str, max_tokens: usize, max_token_chars: usize) -> Option<String> {
    match_expression(&tokenize_query(raw, max_tokens, max_token_chars))
}

/// Chat contains question words absent from saved facts. Match any meaningful
/// term and let BM25 rank the bounded candidates; explicit keyword search stays AND.
pub fn safe_prompt_match(raw: &str, max_tokens: usize, max_token_chars: usize) -> Option<String> {
    const QUESTION_WORDS: &[&str] = &[
        "a", "an", "the", "i", "me", "my", "we", "our", "you", "your", "it", "its", "is", "are", "am", "was", "were",
        "be", "been", "do", "does", "did", "have", "has", "what", "which", "who", "when", "where", "why", "how", "can",
        "could", "would", "should", "please", "tell", "about", "for", "to", "of", "in", "on", "with", "and", "or",
    ];
    let mut tokens = tokenize_query(raw, raw.chars().count(), max_token_chars);
    tokens.retain(|token| !QUESTION_WORDS.iter().any(|word| token.eq_ignore_ascii_case(word)));
    tokens.truncate(max_tokens);
    joined_expression(&tokens, " OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_operators_quotes_and_unicode_cannot_alter_match() {
        assert_eq!(
            tokenize_query(r#"metric OR NEAR/3 "units" title:secret"#, 8, 32),
            vec!["metric", "3", "units", "title", "secret"]
        );
        assert_eq!(tokenize_query("AND OR NOT NEAR", 8, 32), Vec::<String>::new());
        assert_eq!(tokenize_query("café-unité", 8, 32), vec!["café", "unité"]);
        assert_eq!(
            tokenize_query("foo*bar^baz+quux", 8, 32),
            vec!["foo", "bar", "baz", "quux"]
        );
        let expr = safe_match(r#"Prefer "metric" OR NEAR/3 units"#, 8, 32).unwrap();
        assert!(expr.contains("title: \"Prefer\""));
        assert!(expr.contains("value: \"metric\""));
        assert!(expr.contains("value: \"units\""));
        assert!(!expr.contains("NEAR"));
        assert!(!expr.contains(" OR NEAR"));
        assert!(!expr.contains("title:secret"));
        assert!(safe_match("AND OR NOT * ^ +", 8, 32).is_none());
        assert!(safe_match("", 8, 32).is_none());
    }

    #[test]
    fn tokens_are_bounded_and_quoted() {
        let long = "abcdefghij".repeat(8);
        let tokens = tokenize_query(&long, 2, 4);
        assert_eq!(tokens, vec!["abcd"]);
        assert_eq!(quote_token(r#"say "hi""#), r#""say ""hi""""#);
    }
}
