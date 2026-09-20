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
/// Returns None when the text is not a search request.
pub fn parse(text: &str) -> Option<WebIntent> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.starts_with('/') && !lower.starts_with("/web") {
        return None;
    }
    let looks_like = has_word(&lower, "search")
        || (lower.starts_with("/web") && (lower.contains("first ") || !extract_quoted(trimmed).is_empty()));
    if !looks_like {
        return None;
    }

    let engine = if lower.contains("serpapi") {
        SearchEngine::Serpapi
    } else if lower.contains("google") {
        SearchEngine::Google
    } else {
        SearchEngine::Default
    };

    let count = extract_count(&lower).clamp(1, MAX_COUNT);
    let mut terms = extract_quoted(trimmed);
    if terms.is_empty() {
        terms = tokenize_unquoted(trimmed);
    }
    terms.retain(|t| !t.is_empty());
    if terms.is_empty() {
        return None;
    }
    Some(WebIntent { engine, count, terms })
}

fn has_word(lower: &str, word: &str) -> bool {
    lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == word)
}

fn extract_count(lower: &str) -> usize {
    let bytes = lower.as_bytes();
    if let Some(idx) = lower.find("first ") {
        let rest = &lower[idx + 6..];
        let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(v) = n.parse::<usize>() {
            if v >= 1 {
                return v.min(MAX_COUNT);
            }
        }
    }
    let _ = bytes;
    DEFAULT_COUNT
}

fn extract_quoted(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut buf = String::new();
            while i < chars.len() && chars[i] != quote {
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && chars[i] == quote {
                i += 1;
            }
            let t = buf.trim();
            if !t.is_empty() {
                terms.push(t.to_string());
            }
            continue;
        }
        i += 1;
    }
    terms
}

fn tokenize_unquoted(text: &str) -> Vec<String> {
    const DROP: &[&str] = &[
        "search", "google", "serpapi", "for", "the", "first", "links", "link", "and", "please", "can", "you", "me",
        "a", "an", "of", "to", "using", "/web", "web",
    ];
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .map(|t| t.trim().trim_matches(|c: char| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()))
        .filter(|t| !DROP.contains(&t.to_ascii_lowercase().as_str()))
        .map(|t| t.to_string())
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
    fn count_is_capped_at_ten() {
        let p = parse("search google for the first 99 links for 'x'").expect("intent");
        assert_eq!(p.count, 10);
        assert_eq!(p.terms, vec!["x".to_string()]);
    }
}
