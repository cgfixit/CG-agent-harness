//! Optional prose-quality nudge for the planner (`unslop.enabled`, ships false).
//! A deliberately small stand-in for CyClaw's vendored `unslop` scanner: a
//! banned-phrase scan over the response's rationale text. Hits become
//! feedback appended to the next planning prompt, never a gate.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::common::config::AppConfig;

const BANNED_PHRASES: [&str; 16] = [
    "delve",
    "tapestry",
    "in today's fast-paced",
    "it's important to note",
    "as an ai",
    "i hope this helps",
    "certainly!",
    "let's dive in",
    "game-changer",
    "leverage synergies",
    "seamlessly",
    "robust and scalable",
    "a testament to",
    "navigate the complexities",
    "unlock the potential",
    "at the end of the day",
];

pub struct UnslopProbe {
    metrics_path: std::path::PathBuf,
}

impl UnslopProbe {
    /// `None` unless `unslop.enabled` is the literal true.
    pub fn from_config(cfg: &AppConfig, home_root: &std::path::Path) -> Option<Self> {
        if !cfg.flag_is_true("unslop.enabled") {
            return None;
        }
        let raw = cfg.str_or("unslop.metrics_path", "logs/unslop.jsonl");
        let p = std::path::PathBuf::from(&raw);
        Some(Self {
            metrics_path: if p.is_absolute() { p } else { home_root.join(p) },
        })
    }

    /// Scan the prose outside file blocks; returns `{"nudge": ...}` when anything hit.
    pub fn probe(&self, response: &str, proposed_files: &BTreeMap<String, String>, step: u64) -> Value {
        let mut prose = response.to_string();
        for body in proposed_files.values() {
            prose = prose.replace(body, "");
        }
        let lowered = prose.to_lowercase();
        let hits: Vec<&str> = BANNED_PHRASES.iter().copied().filter(|p| lowered.contains(p)).collect();
        let record = json!({"step": step, "hits": hits.len(), "response_sha256": crate::common::sha256_hex(response), "ts": crate::common::iso_now()});
        if let Some(parent) = self.metrics_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.metrics_path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{record}");
        }
        if hits.is_empty() {
            json!({})
        } else {
            json!({"nudge": format!("Style note: avoid filler phrasing in rationale ({} flagged phrase(s)); keep explanations terse and concrete.", hits.len()), "hits": hits.len()})
        }
    }
}
