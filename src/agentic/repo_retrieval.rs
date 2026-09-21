//! Bounded, local-only repository retrieval using the existing Tantivy engine.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing, TextOptions, Value as _, STORED};
use tantivy::{doc, Index};

use crate::common::errors::{HarnessError, Result};

use super::ctx::AgenticCtx;
use super::edits::{collect_checked, ReadContext};
use super::governance::inspect_candidate_text;
use super::real_repo_loop::{denied_read_basename, MAX_TOTAL_READ_CHARS};
use super::workspace::RepoWorkspace;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetrievedFile {
    pub path: String,
    pub selection: String,
    pub sha256: String,
    /// Conservative UTF-8 byte reservation, including headers; not vendor usage.
    pub token_reservation: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetrievalTrace {
    pub step: u64,
    pub indexed_files: usize,
    pub indexed_bytes: usize,
    pub limited: bool,
    pub files: Vec<RetrievedFile>,
}

/// Call only for an explicitly enabled local planner, after run gates pass.
pub(crate) fn retrieve(
    ctx: &AgenticCtx,
    tools: &RepoWorkspace<'_>,
    instruction: &str,
    step: u64,
    context: &mut ReadContext,
) -> Result<RetrievalTrace> {
    let cfg = &ctx.acfg.deepagent.retrieval;
    let mut trace = RetrievalTrace {
        step,
        indexed_files: 0,
        indexed_bytes: 0,
        limited: false,
        files: Vec::new(),
    };
    let terms: Vec<String> = instruction
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(32)
        .map(|s| crate::common::clip_chars(s, 64).to_lowercase())
        .collect();
    if terms.is_empty() {
        return Ok(trace);
    }

    let mut schema = Schema::builder();
    let text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let path_field = schema.add_text_field("path", text.clone());
    let body_field = schema.add_text_field("body", text);
    let row_field = schema.add_u64_field("row", STORED);
    // ponytail: rebuild a bounded RAM index each step; add a cache only if measured
    // scan cost warrants hash-validated reuse. No source bytes enter the web cache.
    let index = Index::create_in_ram(schema.build());
    let failed = |_| HarnessError::agentic("repository retrieval failed; planner was not called");
    let mut writer = index.writer_with_num_threads(1, 32_000_000).map_err(failed)?;
    let paths = tools.retrieval_paths()?;
    trace.limited = paths.len() > cfg.max_files;
    let mut evidence = Vec::new();
    for path in paths.into_iter().take(cfg.max_files) {
        if context.snapshots.contains_key(&path)
            || denied_read_basename(&path, &ctx.acfg.deepagent.denied_read_basenames)
        {
            continue;
        }
        let Ok(content) = tools.read_file(&path) else { continue };
        if content.is_empty() || content.contains('\0') || !inspect_candidate_text(&ctx.scanner, &content).is_empty() {
            continue;
        }
        if content.len() > cfg.max_index_bytes.saturating_sub(trace.indexed_bytes) {
            trace.limited = true;
            continue;
        }
        writer
            .add_document(
                doc!(path_field => path.as_str(), body_field => content.as_str(), row_field => evidence.len() as u64),
            )
            .map_err(failed)?;
        trace.indexed_bytes += content.len();
        let hash = crate::common::sha256_hex(&content);
        evidence.push((path, content, hash));
    }
    trace.indexed_files = evidence.len();
    if evidence.is_empty() {
        return Ok(trace);
    }
    writer.commit().map_err(failed)?;
    let reader = index.reader().map_err(failed)?;
    let searcher = reader.searcher();
    let mut parser = QueryParser::for_index(&index, vec![path_field, body_field]);
    parser.set_field_boost(path_field, 2.0);
    let expression = terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ");
    let query = parser
        .parse_query(&expression)
        .map_err(|_| HarnessError::agentic("repository retrieval query is invalid"))?;
    let hits = searcher
        .search(&query, &TopDocs::with_limit(cfg.top_k).order_by_score())
        .map_err(failed)?;
    let mut chars_left =
        MAX_TOTAL_READ_CHARS.saturating_sub(context.snapshots.values().map(|s| s.visible.chars().count()).sum());
    let mut tokens_left = cfg.token_budget;
    for (_, address) in hits {
        let doc: TantivyDocument = searcher.doc(address).map_err(failed)?;
        let Some(row) = doc.get_first(row_field).and_then(|v| v.as_u64()) else {
            continue;
        };
        let Some((path, content, hash)) = evidence.get(row as usize) else {
            continue;
        };
        // A UTF-8 scalar uses at most four bytes. Reserve room for the rendered
        // hash/path/selection before collecting, then check the exact byte total.
        let max_chars = chars_left.min(tokens_left.saturating_sub(path.len() * 2 + 256) / 4);
        if max_chars == 0 {
            trace.limited = true;
            break;
        }
        let start = excerpt_start(content, &terms, cfg.excerpt_lines, max_chars);
        let selection = format!("{path}#L{start}-L{}", start + cfg.excerpt_lines - 1);
        let mut selected = collect_checked(
            tools,
            std::slice::from_ref(&selection),
            &BTreeMap::from([(path.clone(), hash.clone())]),
            max_chars,
        );
        let Some(snapshot) = selected.snapshots.remove(path) else {
            continue;
        };
        let reservation = selected.rendered.len() + 2;
        if reservation > tokens_left || !inspect_candidate_text(&ctx.scanner, &selected.rendered).is_empty() {
            trace.limited = true;
            continue;
        }
        tokens_left -= reservation;
        chars_left -= snapshot.visible.chars().count();
        if !context.rendered.is_empty() {
            context.rendered.push_str("\n\n");
        }
        context.rendered.push_str(&selected.rendered);
        context.snapshots.insert(path.clone(), snapshot);
        trace.files.push(RetrievedFile {
            path: path.clone(),
            selection,
            sha256: hash.clone(),
            token_reservation: reservation,
        });
    }
    Ok(trace)
}

fn excerpt_start(content: &str, terms: &[String], lines: usize, max_chars: usize) -> usize {
    let rows: Vec<_> = content.split_inclusive('\n').collect();
    let mut best = (0, 0);
    for (row, line) in rows.iter().enumerate() {
        let lower = line.to_lowercase();
        let score = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| terms.iter().any(|term| word == term))
            .count();
        if score > best.1 {
            best = (row, score);
        }
    }
    let mut start = best.0;
    let mut leading_chars = 0;
    while start > best.0.saturating_sub(lines / 2) {
        leading_chars += rows[start - 1].chars().count();
        if leading_chars > max_chars / 2 {
            break;
        }
        start -= 1;
    }
    start + 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn excerpt_matches_words_instead_of_substrings_in_padding() {
        let content = "// retained padding\n".repeat(600) + "pub fn add(a: i32, b: i32) -> i32 { a - b }\n";
        let terms = [
            "fix",
            "add",
            "arithmetic",
            "in",
            "src",
            "lib",
            "rs",
            "to",
            "return",
            "a",
            "plus",
            "b",
        ]
        .map(str::to_string);
        let start = super::excerpt_start(&content, &terms, 40, 442);
        let window: String = content.split_inclusive('\n').skip(start - 1).take(40).collect();
        let excerpt: String = window.chars().take(442).collect();
        assert!(excerpt.contains("pub fn add"), "selected line {start}");
    }
}
