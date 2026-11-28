//! Bounded file snapshots and exact edits; model output is never a command.
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::common::errors::{HarnessError, Result};

use super::real_repo_loop::{parse_file_blocks, MAX_READ_FILE_CHARS, MAX_TOTAL_READ_CHARS};
use super::workspace::{canonical_repo_path, fs_equiv_path, RepoWorkspace};

pub struct Snapshot {
    pub original: String,
    pub(crate) visible: String,
    complete: bool,
}

pub struct ReadContext {
    pub rendered: String,
    pub snapshots: BTreeMap<String, Snapshot>,
}

/// Existing read selectors also accept an explicit inclusive line window.
/// Byte content is preserved, including CRLF, so the hash/old-text bind reality.
pub fn collect(tools: &RepoWorkspace<'_>, selectors: &[String]) -> ReadContext {
    collect_checked(tools, selectors, &BTreeMap::new(), MAX_TOTAL_READ_CHARS)
}

/// Re-read retrieved hits through the same jail; changed content is a miss.
pub(crate) fn collect_checked(
    tools: &RepoWorkspace<'_>,
    selectors: &[String],
    expected_hashes: &BTreeMap<String, String>,
    max_chars: usize,
) -> ReadContext {
    let mut rendered = Vec::new();
    let mut snapshots = BTreeMap::new();
    let mut remaining = max_chars.min(MAX_TOTAL_READ_CHARS);
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
        let hash = crate::common::sha256_hex(&original);
        if expected_hashes.get(&path).is_some_and(|expected| expected != &hash) {
            continue;
        }
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

/// How every label the outbound redactor writes in place of a value begins
/// (`[REDACTED_IP]`, `[REDACTED_EMAIL]`, `[REDACTED_SECRET]`).
const REDACTION_LABEL_START: &str = "[REDACTED_";

impl Proposal {
    /// The first path whose proposed text holds more redaction labels than the
    /// text it replaces, if any.
    ///
    /// A cloud planner is sent the files with addresses, emails and
    /// secret-shaped values replaced by a label. A file it returns whole, or an
    /// edit that repeats a line it was shown, then has the label where the
    /// clone has the value, and applying it would overwrite the value with the
    /// label. Labels the file already held are its own text: only a count that
    /// grew is refused. A planner that swaps a label for some other text is
    /// not caught here; the review diff's per-value tags show that line.
    pub fn writes_redaction_label(&self) -> Option<&str> {
        let labels = |text: &str| text.matches(REDACTION_LABEL_START).count();
        self.files
            .iter()
            .find(|(path, proposed)| {
                let before = self.originals.get(*path).and_then(Option::as_deref).map_or(0, labels);
                labels(proposed) > before
            })
            .map(|(path, _)| path.as_str())
    }
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
    fn a_retrieved_hash_changed_before_collection_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let cfg =
            crate::common::config::AppConfig::from_str(crate::common::config::AppConfig::embedded_default(), &path)
                .unwrap();
        let ctx = crate::agentic::ctx::AgenticCtx::new(cfg, &path).unwrap();
        let tools = RepoWorkspace::open_existing(&ctx, dir.path()).unwrap();
        let hashes = BTreeMap::from([("parser.rs".into(), crate::common::sha256_hex("old text"))]);
        std::fs::write(dir.path().join("parser.rs"), "new text").unwrap();
        let selected = ["parser.rs".into()];
        let stale = collect_checked(&tools, &selected, &hashes, MAX_TOTAL_READ_CHARS);
        assert!(stale.snapshots.is_empty());
        assert!(stale.rendered.is_empty());
        assert!(collect(&tools, &selected).rendered.contains("new text"));
    }

    /// A redaction label counts only where the proposal adds one.
    #[test]
    fn a_proposal_is_flagged_only_when_it_adds_a_redaction_label() {
        let original = "host = \"192.0.2.7\"\nnote = \"[REDACTED_SECRET] is the label\"\nport = 80\n";
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            "net.cfg".to_string(),
            Snapshot {
                original: original.into(),
                visible: original.into(),
                complete: true,
            },
        );
        let context = ReadContext {
            rendered: String::new(),
            snapshots,
        };
        let whole = |body: &str| {
            parse(
                &format!("=== FILE net.cfg ===\n{body}=== END FILE ===\n"),
                &context,
                10_000,
            )
            .unwrap()
        };
        let edit = |old: &str, new: &str| {
            let edits = serde_json::json!({"edits": [{"path": "net.cfg", "sha256": crate::common::sha256_hex(original), "old": old, "new": new}]});
            parse(
                &format!("=== EDITS ===\n{edits}\n=== END EDITS ===\n"),
                &context,
                10_000,
            )
            .unwrap()
        };
        // The file as a cloud planner was shown it: the address is a label now.
        let shown = original
            .replace("192.0.2.7", "[REDACTED_IP]")
            .replace("port = 80", "port = 8080");
        assert_eq!(whole(&shown).writes_redaction_label(), Some("net.cfg"));
        // An edit that repeats a shown line.
        assert_eq!(
            edit("port = 80", "port = 8080\nfallback = \"[REDACTED_IP]\"").writes_redaction_label(),
            Some("net.cfg")
        );
        // The label the file already held is its own text, whole or edited.
        assert_eq!(
            whole(&original.replace("port = 80", "port = 8080")).writes_redaction_label(),
            None
        );
        assert_eq!(edit("port = 80", "port = 8080").writes_redaction_label(), None);
        // Removing one is fine; a new file that holds one is not.
        assert_eq!(
            edit("[REDACTED_SECRET] is the label", "plain").writes_redaction_label(),
            None
        );
        let fresh = parse(
            "=== FILE new.cfg ===\npeer = \"[REDACTED_EMAIL]\"\n=== END FILE ===\n",
            &context,
            10_000,
        )
        .unwrap();
        assert_eq!(fresh.writes_redaction_label(), Some("new.cfg"));
        let clean = parse(
            "=== FILE new.cfg ===\npeer = \"none\"\n=== END FILE ===\n",
            &context,
            10_000,
        )
        .unwrap();
        assert_eq!(clean.writes_redaction_label(), None);
    }

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
