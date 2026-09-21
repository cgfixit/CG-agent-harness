//! Request-local RAM BM25 over already-authorized text.
//!
//! Ranking never confers permission. Callers read bytes through
//! `AttachmentStore` or the notes jail, then pass the classified text here.
//! This index is not `web_index` and is not session-search.

use std::collections::{BTreeMap, BTreeSet};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing, TextOptions, Value as _, STORED};
use tantivy::{doc, Index};

use crate::common::errors::{HarnessError, Result};

const CHUNK_CHARS: usize = 1200;
const MAX_TERMS: usize = 32;
const CANDIDATE_LIMIT: usize = 64;
const HIT_CAP: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Attachment,
    NotesCorpus,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Attachment => "attachment",
            SourceKind::NotesCorpus => "notes_corpus",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PassageDoc {
    pub source: SourceKind,
    pub source_id: String,
    pub heading: String,
    pub text: String,
    pub sha256: String,
    pub start: usize,
    pub end: usize,
    pub score: f32,
}

impl PassageDoc {
    fn passage_id(&self) -> String {
        format!("{}:{}:{}", self.source_id, self.start, self.end)
    }
}

/// Split `text` into ~1200-scalar windows. Offsets are UTF-8 bytes.
/// Snap back to the last newline or space after `len/2`. No overlap.
/// Heading is the last `# ` line seen in that chunk.
pub fn chunk_text(text: &str) -> Vec<(usize, usize, String, String)> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut heading = String::new();
    while start < text.len() {
        let tail = &text[start..];
        let mut len = tail
            .char_indices()
            .nth(CHUNK_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        if len < tail.len() {
            if let Some(end) = tail[..len].rfind(['\n', ' ']).filter(|n| *n > len / 2) {
                len = end + 1;
            }
        }
        let end = start + len;
        let chunk = &text[start..end];
        if let Some(line) = chunk.lines().rev().find(|l| l.starts_with("# ")) {
            heading = line.trim_start_matches("# ").trim().to_string();
        }
        if !chunk.trim().is_empty() {
            output.push((start, end, heading.clone(), chunk.to_string()));
        }
        start = end;
    }
    output
}

pub fn retrieve_passages(docs: &[PassageDoc], query: &str, limit: usize) -> Result<Vec<PassageDoc>> {
    if docs.is_empty() || query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let failed = |_| HarnessError::new("ATTACHMENT_INDEX_FAILED", "derived passage index failed");
    let mut schema = Schema::builder();
    let text_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let title = schema.add_text_field("title", text_options.clone());
    let heading = schema.add_text_field("heading", text_options.clone());
    let body = schema.add_text_field("text", text_options);
    let row = schema.add_u64_field("row", STORED);
    let index = Index::create_in_ram(schema.build());
    let mut writer = index
        .writer_with_num_threads::<TantivyDocument>(1, 32_000_000)
        .map_err(failed)?;
    for (i, p) in docs.iter().enumerate() {
        writer
            .add_document(doc!(
                title => "",
                heading => p.heading.clone(),
                body => p.text.clone(),
                row => i as u64
            ))
            .map_err(failed)?;
    }
    writer.commit().map_err(failed)?;
    let reader = index.reader().map_err(failed)?;
    let searcher = reader.searcher();
    let mut parser = QueryParser::for_index(&index, vec![title, heading, body]);
    parser.set_field_boost(title, 1.6);
    parser.set_field_boost(heading, 1.3);

    let terms: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(MAX_TERMS)
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let quoted = query.replace('\\', "\\\\").replace('"', "\\\"");
    let expression = format!(
        "\"{quoted}\"^2 {}",
        terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")
    );
    let query_expr = parser
        .parse_query(&expression)
        .map_err(|_| HarnessError::new("ATTACHMENT_INDEX_FAILED", "query cannot be indexed"))?;
    let identifier = if query.contains('_') || query.contains("::") {
        Some(
            regex::RegexBuilder::new(&format!(
                r"(?:^|[^\p{{L}}\p{{N}}_]){}(?:$|[^\p{{L}}\p{{N}}_])",
                regex::escape(query)
            ))
            .case_insensitive(true)
            .build()
            .map_err(|_| HarnessError::new("ATTACHMENT_INDEX_FAILED", "invalid identifier"))?,
        )
    } else {
        None
    };
    let mut candidates = Vec::new();
    for (score, address) in searcher
        .search(&query_expr, &TopDocs::with_limit(CANDIDATE_LIMIT).order_by_score())
        .map_err(failed)?
    {
        let found: TantivyDocument = searcher.doc(address).map_err(failed)?;
        let i = found
            .get_first(row)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| HarnessError::new("ATTACHMENT_INDEX_FAILED", "invalid passage index row"))?
            as usize;
        let mut p = docs
            .get(i)
            .cloned()
            .ok_or_else(|| HarnessError::new("ATTACHMENT_INDEX_FAILED", "invalid passage index row"))?;
        if identifier
            .as_ref()
            .is_some_and(|re| !re.is_match(&p.text) && !re.is_match(&p.heading))
        {
            continue;
        }
        p.score = score
            + if p.text.to_lowercase().contains(&query.to_lowercase()) {
                2.0
            } else {
                0.0
            };
        candidates.push(p);
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.passage_id().cmp(&b.passage_id()))
    });
    let mut output = Vec::new();
    let mut seen_text = BTreeSet::new();
    let mut source_counts: BTreeMap<String, usize> = BTreeMap::new();
    for cap in [1, 2] {
        for p in &candidates {
            if output.len() >= limit.min(HIT_CAP) {
                return Ok(output);
            }
            let normalized = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let hash = crate::common::sha256_hex(&normalized);
            if source_counts.get(&p.source_id).copied().unwrap_or(0) >= cap || seen_text.contains(&hash) {
                continue;
            }
            let words: BTreeSet<_> = normalized.split_whitespace().collect();
            if output.iter().any(|prior: &PassageDoc| {
                let old: BTreeSet<_> = prior.text.split_whitespace().collect();
                !words.is_empty() && words.intersection(&old).count() * 10 > words.union(&old).count() * 9
            }) {
                continue;
            }
            seen_text.insert(hash);
            *source_counts.entry(p.source_id.clone()).or_default() += 1;
            output.push(p.clone());
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_snaps_and_keeps_heading() {
        let mut body = "# Title\n".to_string();
        body.push_str(&"alpha ".repeat(400));
        body.push_str("\n# Later\nUNIQUE_TOKEN ");
        body.push_str(&"beta ".repeat(50));
        let chunks = chunk_text(&body);
        assert!(chunks.len() >= 2, "{}", chunks.len());
        assert!(chunks.iter().any(|c| c.2 == "Title" || c.3.contains("# Title")));
        assert!(chunks.iter().any(|c| c.3.contains("UNIQUE_TOKEN")));
        for (start, end, _, text) in &chunks {
            assert_eq!(&body[*start..*end], text);
        }
    }

    #[test]
    fn retrieve_finds_identifier_and_ignores_syntax() {
        let text = "fn foo::bar_baz() { 1 }\n".to_string() + &"padding ".repeat(80);
        let docs: Vec<PassageDoc> = chunk_text(&text)
            .into_iter()
            .map(|(start, end, heading, chunk)| PassageDoc {
                source: SourceKind::Attachment,
                source_id: "blob-a".into(),
                heading,
                text: chunk,
                sha256: "abc".into(),
                start,
                end,
                score: 0.0,
            })
            .collect();
        let hits = retrieve_passages(&docs, "foo::bar_baz", 8).unwrap();
        assert!(hits.iter().any(|h| h.text.contains("foo::bar_baz")), "{hits:?}");
        let empty = retrieve_passages(&docs, "\".*\"", 8).unwrap();
        assert!(empty.is_empty(), "{empty:?}");
    }

    #[test]
    fn notes_corpus_variant_exists_for_later_c() {
        assert_eq!(SourceKind::NotesCorpus.as_str(), "notes_corpus");
        assert_eq!(SourceKind::Attachment.as_str(), "attachment");
    }
}
