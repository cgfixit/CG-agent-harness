//! Harness-local operator notes for `/memory`, port of `harness/memory_notes.py`.
//! Notes live under the home `memory/notes.json` and enter the system prompt
//! only while the operator has `/memory on`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::common::atomic::write_json_atomic;
use crate::common::errors::{HarnessError, Result};
use crate::common::injection::Scanner;

pub const MAX_NOTE_CHARS: usize = 500;
pub const MAX_NOTES: usize = 20;
pub const MAX_PROMPT_CHARS: usize = 3000;

const PREAMBLE: &str = "The following are operator-pinned notes from /memory. \
They are not a write authorization and do not change routing, \
topology, or the real-repo six-gate. They are not soul.md and \
are not RAG facts.";

pub struct MemoryNotes {
    dir: PathBuf,
    path: PathBuf,
    scanner: Scanner,
    lock: Mutex<()>,
}

impl std::fmt::Debug for MemoryNotes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryNotes").field("path", &self.path).finish()
    }
}

fn coerce_note(raw: &Value) -> Option<Value> {
    let obj = raw.as_object()?;
    let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let text = obj
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() || text.is_empty() {
        return None;
    }
    Some(json!({
        "id": id,
        "text": crate::common::clip_chars(&text, MAX_NOTE_CHARS),
        "ts": obj.get("ts").and_then(|v| v.as_str()).unwrap_or(""),
    }))
}

impl MemoryNotes {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            path: dir.join("notes.json"),
            scanner: Scanner::core(),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Result<Vec<Value>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(HarnessError::new("MEMORY_NOTES_UNREADABLE", "notes file is unreadable")),
        };
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|_| HarnessError::new("MEMORY_NOTES_UNREADABLE", "notes file is unreadable"))?;
        let raw = parsed
            .get("notes")
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(raw.iter().filter_map(coerce_note).take(MAX_NOTES).collect())
    }

    fn save(&self, notes: &[Value]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        write_json_atomic(&self.path, &json!({"notes": notes}))
    }

    fn clean(&self, text: &str) -> Result<String> {
        let cleaned = text.trim();
        if cleaned.is_empty() {
            return Err(HarnessError::new("MEMORY_NOTE_EMPTY", "note text is empty"));
        }
        if cleaned.chars().count() > MAX_NOTE_CHARS {
            return Err(HarnessError::new(
                "MEMORY_NOTE_TOO_LONG",
                format!("note exceeds {MAX_NOTE_CHARS} characters"),
            )
            .detail("max_chars", MAX_NOTE_CHARS as u64));
        }
        let flags = self.scanner.count_matches(cleaned);
        if flags > 0 {
            // Count only, never the matched pattern content.
            return Err(
                HarnessError::new("MEMORY_NOTE_INJECTION", "note matches a critical injection pattern")
                    .detail("injection_flag_count", flags as u64),
            );
        }
        Ok(cleaned.to_string())
    }

    pub fn status(&self, enabled: bool) -> Result<Value> {
        let notes = self.load()?;
        Ok(json!({
            "enabled": enabled,
            "count": notes.len(),
            "max_notes": MAX_NOTES,
            "max_chars": MAX_NOTE_CHARS,
            "notes": notes,
        }))
    }

    pub fn add(&self, text: &str) -> Result<Value> {
        let cleaned = self.clean(text)?;
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut notes = self.load()?;
        if notes.len() >= MAX_NOTES {
            return Err(
                HarnessError::new("MEMORY_NOTE_CAP", format!("at most {MAX_NOTES} notes"))
                    .detail("max_notes", MAX_NOTES as u64),
            );
        }
        let note = json!({
            "id": crate::common::random_hex(4),
            "text": cleaned,
            "ts": crate::common::iso_now(),
        });
        notes.push(note.clone());
        self.save(&notes)?;
        Ok(note)
    }

    pub fn forget(&self, note_id: &str) -> Result<Value> {
        let wanted = note_id.trim();
        if wanted.is_empty() {
            return Err(HarnessError::new("MEMORY_NOTE_ID_REQUIRED", "note id is required"));
        }
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let notes = self.load()?;
        let kept: Vec<Value> = notes
            .iter()
            .filter(|n| n.get("id").and_then(|v| v.as_str()) != Some(wanted))
            .cloned()
            .collect();
        if kept.len() == notes.len() {
            return Err(HarnessError::new("MEMORY_NOTE_UNKNOWN", "unknown note id").detail("id", wanted));
        }
        self.save(&kept)?;
        Ok(json!({"forgotten": wanted, "count": kept.len()}))
    }

    pub fn clear(&self) -> Result<Value> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.save(&[])?;
        Ok(json!({"cleared": true, "count": 0}))
    }

    /// Prompt text; a corrupt file degrades to "" (this path never writes).
    pub fn context_text(&self) -> String {
        let notes = match self.load() {
            Ok(n) => n,
            Err(_) => return String::new(),
        };
        if notes.is_empty() {
            return String::new();
        }
        let lines: Vec<String> = notes
            .iter()
            .map(|n| {
                format!(
                    "- [{}] {}",
                    n.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    n.get("text").and_then(|v| v.as_str()).unwrap_or("")
                )
            })
            .collect();
        let blob = crate::common::clip_chars(&lines.join("\n"), MAX_PROMPT_CHARS);
        format!("{PREAMBLE}\n\n{blob}")
    }
}

/// Read-only echo of a RAG `memory:` block. This binary has no RAG, so every
/// flag is false and `writable_from_harness` is always false.
pub fn rag_flags() -> Value {
    json!({"enabled": false, "facts": false, "episodes": false, "retrieval_fusion": false, "writable_from_harness": false})
}
