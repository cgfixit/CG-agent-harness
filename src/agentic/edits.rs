//! Bounded file snapshots and exact edits; model output is never a command.
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::common::errors::{HarnessError, Result};

use super::real_repo_loop::{parse_file_blocks, MAX_READ_FILE_CHARS, MAX_TOTAL_READ_CHARS};
use super::workspace::{canonical_repo_path, fs_equiv_path, RepoWorkspace};

pub struct Snapshot {
    pub original: String,
    visible: String,
    complete: bool,
}

pub struct ReadContext {
    pub rendered: String,
    pub snapshots: BTreeMap<String, Snapshot>,
}

/// Existing read selectors also accept an explicit inclusive line window.
/// Byte content is preserved, including CRLF, so the hash/old-text bind reality.
pub fn collect(tools: &RepoWorkspace<'_>, selectors: &[String]) -> ReadContext {
    let mut rendered = Vec::new();
    let mut snapshots = BTreeMap::new();
    let mut remaining = MAX_TOTAL_READ_CHARS;
    for selector in selectors {
        if remaining == 0 {
            rendered.push(format!("[{selector} omitted: total context budget reached]"));
            continue;
        }
        let selection = (|| -> Option<(String, usize, usize)> {
            let (path, start, end) = if let Some((path, range)) = selector.rsplit_once("#L") {
                let (start, end) = range.split_once("-L")?;
                (path, start.parse::<usize>().ok()?, end.parse::<usize>().ok()?)
            } else {
                (selector.as_str(), 1, usize::MAX)
            };
            if start == 0 || end < start {
                return None;
            }
            Some((canonical_repo_path(path)?, start, end))
        })();
        let Some((path, start, end)) = selection else {
            rendered.push(format!("[{selector} omitted: invalid line selector]"));
            continue;
        };
        if snapshots.contains_key(&path) {
            rendered.push(format!("[{selector} omitted: select one window per file]"));
            continue;
        }
        let Ok(original) = tools.read_file(&path) else {
            rendered.push(format!(
                "[{selector} omitted: unreadable, unsafe, or larger than file-read ceiling]"
            ));
            continue;
        };
        let window: String = original
            .split_inclusive('\n')
            .skip(start - 1)
            .take(end - start + 1)
            .collect();
        let visible = crate::common::clip_chars(&window, MAX_READ_FILE_CHARS.min(remaining));
        if visible.is_empty() && !original.is_empty() {
            rendered.push(format!("[{selector} omitted: selected lines are empty]"));
            continue;
        }
        remaining = remaining.saturating_sub(visible.chars().count());
        let complete = visible == original;
        let hash = crate::common::sha256_hex(&original);
        rendered.push(format!(
            "--- EXISTING FILE: {path} ---\nsha256: {hash}\nselection: {selector}; complete: {complete}\n{visible}\n--- END EXISTING FILE ---{}",
            if complete { "" } else { "\n[omitted or truncated context: use exact edits only]" }
        ));
        snapshots.insert(
            path,
            Snapshot {
                original,
                visible,
                complete,
            },
        );
    }
    ReadContext {
        rendered: rendered.join("\n\n"),
        snapshots,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactProposal {
    edits: Vec<ExactEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactEdit {
    path: String,
    sha256: String,
    old: String,
    new: String,
}

#[derive(Debug)]
pub struct Proposal {
    pub files: BTreeMap<String, String>,
    /// None means the destination must still be absent.
    pub originals: BTreeMap<String, Option<String>>,
}

pub fn parse(response: &str, context: &ReadContext, max_bytes: usize) -> Result<Proposal> {
    let fail = |why: &str| HarnessError::agentic(format!("proposal refused: {why}; no files applied"));
    if response.len() > max_bytes {
        return Err(fail("model output exceeds aggregate response budget"));
    }
    let mut files = BTreeMap::new();
    let mut originals = BTreeMap::new();
    if response.contains("=== EDITS") {
        if response.matches("=== EDITS ===").count() != 1
            || response.matches("=== END EDITS ===").count() != 1
            || response.contains("=== FILE ")
        {
            return Err(fail("malformed/truncated or mixed EDITS response"));
        }
        let (before, body) = response
            .split_once("=== EDITS ===")
            .ok_or_else(|| fail("missing EDITS marker"))?;
        let (body, after) = body
            .split_once("=== END EDITS ===")
            .ok_or_else(|| fail("missing END EDITS"))?;
        if before.contains("===") || after.contains("===") {
            return Err(fail("unrecognized or truncated proposal marker"));
        }
        let proposal: ExactProposal = serde_json::from_str(body).map_err(|_| fail("invalid exact-edit JSON"))?;
        let mut destinations = std::collections::BTreeSet::new();
        for edit in proposal.edits {
            let path = canonical_repo_path(&edit.path).ok_or_else(|| fail("unsafe edit path"))?;
            if !destinations.insert(fs_equiv_path(&path)) {
                return Err(fail("duplicate edit destination"));
            }
            let snapshot = context
                .snapshots
                .get(&path)
                .ok_or_else(|| fail("exact edit needs explicitly selected context"))?;
            if edit.sha256 != crate::common::sha256_hex(&snapshot.original) {
                return Err(fail("stale or incorrect original hash"));
            }
            if edit.old.is_empty() || !snapshot.visible.contains(&edit.old) {
                return Err(fail(
                    "old text must be nonempty and contained in the actual displayed excerpt",
                ));
            }
            if snapshot.original.find(&edit.old) != snapshot.original.rfind(&edit.old) {
                return Err(fail("old text is ambiguous; select a larger unique span"));
            }
            let replacement = snapshot.original.replacen(&edit.old, &edit.new, 1);
            files.insert(path.clone(), replacement);
            originals.insert(path, Some(snapshot.original.clone()));
        }
    } else {
        files = parse_file_blocks(response)?;
        for path in files.keys() {
            let original = if let Some(snapshot) = context.snapshots.get(path) {
                if !snapshot.complete {
                    return Err(fail(
                        "whole-file replacement requires full current context; use exact edits",
                    ));
                }
                Some(snapshot.original.clone())
            } else {
                None
            };
            originals.insert(path.clone(), original);
        }
    }
    Ok(Proposal { files, originals })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_trailing_partial_or_mixed_markers_and_overlapping_old_text() {
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            "a.rs".into(),
            Snapshot {
                original: "aaa".into(),
                visible: "aaa".into(),
                complete: true,
            },
        );
        let context = ReadContext {
            rendered: String::new(),
            snapshots,
        };
        let valid = format!(
            "=== EDITS ===\n{}\n=== END EDITS ===",
            serde_json::json!({"edits":[{"path":"a.rs","sha256":crate::common::sha256_hex("aaa"),"old":"aaa","new":"b"}]})
        );
        assert!(parse(&valid, &context, 10000).is_ok());
        for tail in ["\n=== EDITS ==", "\n=== END FILE ===", "\n=== EDI"] {
            assert!(parse(&format!("{valid}{tail}"), &context, 10000).is_err());
        }
        let overlapping = valid.replace("\"old\":\"aaa\"", "\"old\":\"aa\"");
        assert!(parse(&overlapping, &context, 10000)
            .unwrap_err()
            .message
            .contains("ambiguous"));
    }
}
