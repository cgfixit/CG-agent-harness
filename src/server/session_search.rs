//! Request-local Tantivy index over session transcripts. Not the web cache.

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing, TextOptions, Value as _, STORED};
use tantivy::{doc, Index};

use crate::common::errors::{HarnessError, Result};
use crate::server::sessions::OwnedSessionStore;

const MAX_SESSIONS: usize = 200;
const MAX_BYTES: usize = 2_000_000;
const CHUNK_CHARS: usize = 1200;
const MAX_QUERY_CHARS: usize = 200;
const MAX_TERMS: usize = 32;
const CANDIDATES: usize = 64;
const MAX_HITS: usize = 16;
const SNIPPET_CHARS: usize = 160;
const PER_SESSION: usize = 2;

#[derive(Debug, Clone)]
pub struct SessionHit {
    pub session_id: String,
    pub title: String,
    pub snippet: String,
    pub role: String,
    pub ts: f64,
    pub score: f32,
}

pub fn search_sessions(store: &OwnedSessionStore<'_>, query: &str) -> Result<Vec<SessionHit>> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
        return Ok(Vec::new());
    }
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(MAX_TERMS)
        .map(str::to_string)
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut schema_builder = Schema::builder();
    let text_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let stored = TextOptions::default().set_stored();
    let f_title = schema_builder.add_text_field("title", text_options.clone());
    let f_body = schema_builder.add_text_field("text", text_options);
    let f_sid = schema_builder.add_text_field("session_id", stored.clone());
    let f_role = schema_builder.add_text_field("role", stored);
    let f_ts = schema_builder.add_f64_field("ts", STORED);
    let f_row = schema_builder.add_u64_field("row", STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema.clone());
    let mut writer = index
        .writer_with_num_threads(1, 32_000_000)
        .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "could not build session index"))?;

    let summaries = store.list();
    let mut evidence: Vec<(String, String, String, f64, String)> = Vec::new();
    let mut bytes = 0usize;
    for (i, summary) in summaries.iter().take(MAX_SESSIONS).enumerate() {
        let Some(id) = summary["session_id"].as_str() else {
            continue;
        };
        let Ok(session) = store.get(id) else {
            continue;
        };
        let title = session.title.clone();
        for msg in &session.messages {
            bytes = bytes.saturating_add(msg.text.len());
            if bytes > MAX_BYTES {
                break;
            }
            for chunk in chunks(&msg.text) {
                let row = evidence.len() as u64;
                evidence.push((
                    session.session_id.clone(),
                    title.clone(),
                    msg.role.clone(),
                    msg.ts,
                    chunk.clone(),
                ));
                writer
                    .add_document(doc!(
                        f_title => title.as_str(),
                        f_body => chunk.as_str(),
                        f_sid => session.session_id.as_str(),
                        f_role => msg.role.as_str(),
                        f_ts => msg.ts,
                        f_row => row,
                    ))
                    .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "could not index session"))?;
            }
        }
        if bytes > MAX_BYTES {
            let _ = i;
            break;
        }
    }
    writer
        .commit()
        .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "could not commit session index"))?;
    if evidence.is_empty() {
        return Ok(Vec::new());
    }

    let reader = index
        .reader()
        .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "could not open session index"))?;
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(&index, vec![f_title, f_body]);
    let quoted = query.replace('\\', "\\\\").replace('"', "\\\"");
    let expression = format!(
        "\"{quoted}\"^2 {}",
        terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")
    );
    let parsed = parser
        .parse_query(&expression)
        .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "query cannot be indexed"))?;
    let top = searcher
        .search(&parsed, &TopDocs::with_limit(CANDIDATES).order_by_score())
        .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "session search failed"))?;

    let mut hits = Vec::new();
    let mut per_session = std::collections::HashMap::<String, usize>::new();
    for (score, addr) in top {
        let doc: TantivyDocument = searcher
            .doc(addr)
            .map_err(|_| HarnessError::new("SESSION_SEARCH_FAILED", "hit unreadable"))?;
        let row = doc.get_first(f_row).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let Some((sid, title, role, ts, text)) = evidence.get(row) else {
            continue;
        };
        if !text.to_lowercase().contains(&query.to_lowercase()) {
            continue;
        }
        let n = per_session.entry(sid.clone()).or_insert(0);
        if *n >= PER_SESSION {
            continue;
        }
        *n += 1;
        hits.push(SessionHit {
            session_id: sid.clone(),
            title: title.clone(),
            snippet: snippet(text, query),
            role: role.clone(),
            ts: *ts,
            score,
        });
        if hits.len() >= MAX_HITS {
            break;
        }
    }
    Ok(hits)
}

fn chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    chars.chunks(CHUNK_CHARS).map(|c| c.iter().collect()).collect()
}

fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    let idx = lower.find(&q).unwrap_or(0);
    // Map lowercase byte offsets back to original characters, including expansions.
    let mut lowered_bytes = 0;
    let match_char = text
        .chars()
        .take_while(|c| {
            lowered_bytes += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            lowered_bytes <= idx
        })
        .count();
    text.chars()
        .skip(match_char.saturating_sub(40))
        .take(SNIPPET_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_preserve_unicode_and_locate_case_expanded_matches() {
        for prefix in ["é".repeat(21), "界".repeat(21), "🙂".repeat(21)] {
            let text = format!("{prefix} needle");
            assert_eq!(snippet(&text, "NEEDLE"), text);
        }

        // İ expands when lowercased; ΟΣ needs contextual final-sigma casing.
        let text = format!("{} ΟΣ {}", "İ".repeat(200), "z".repeat(200));
        let expected = format!("{} ΟΣ {}", "İ".repeat(39), "z".repeat(117));
        assert_eq!(snippet(&text, "ος"), expected);
        assert_eq!(snippet(&text, "ΟΣ").chars().count(), SNIPPET_CHARS);
    }
}
