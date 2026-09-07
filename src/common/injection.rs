//! Prompt-injection pattern lists and scanner.
//!
//! One internal module holds what CyClaw spreads across `utils/personality.py`
//! (`ENFORCED_SOUL_PATTERNS` / `OWASP_INJECTION_PATTERNS`) and
//! `guardrails.rails` (the union with `policy.prompt_filter.banned_patterns`).
//! Patterns compile case-insensitively; an invalid configured pattern is
//! skipped with a warning naming only its index.

use regex::{Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;

use super::config::AppConfig;

/// The enforced core set (CyClaw's `_CORE_INJECTION_PATTERNS`).
pub const CORE_INJECTION_PATTERNS: [&str; 9] = [
    r"ignore\s+(previous|all|prior)\s+instructions",
    r"disregard\s+(previous|all|prior)",
    r"forget\s+(previous|all|prior)\s+instructions",
    r"new\s+instructions\s*:",
    r"system\s+prompt\s*:",
    r"override\s+instructions",
    r"jailbreak",
    r"DAN\s+mode",
    r"developer\s+mode",
];

/// Advisory extras layered on the core set (`OWASP_INJECTION_PATTERNS`).
pub const OWASP_EXTRA_PATTERNS: [&str; 4] = [
    r"you\s+are\s+now",
    r"pretend\s+(you\s+are|to\s+be)",
    r"act\s+as",
    r"<\s*script\s*>",
];

#[derive(Debug, Clone)]
pub struct Scanner {
    rules: Vec<(String, Regex)>,
}

fn compile(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .ok()
}

impl Scanner {
    pub fn from_patterns<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Self {
        let mut rules = Vec::new();
        for (idx, p) in patterns.into_iter().enumerate() {
            match compile(p) {
                Some(re) => rules.push((p.to_string(), re)),
                None => tracing::warn!("injection pattern #{idx} failed to compile; skipped"),
            }
        }
        Self { rules }
    }

    /// The enforced core set only (what `/memory add` and soul writes use).
    pub fn core() -> Self {
        Self::from_patterns(CORE_INJECTION_PATTERNS)
    }

    /// OWASP set union the configured `banned_patterns`, order preserving, deduplicated.
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut seen = std::collections::BTreeSet::new();
        let mut ordered: Vec<String> = Vec::new();
        for p in CORE_INJECTION_PATTERNS.iter().chain(OWASP_EXTRA_PATTERNS.iter()) {
            if seen.insert(p.to_string()) {
                ordered.push(p.to_string());
            }
        }
        for p in cfg.str_list("policy.prompt_filter.banned_patterns") {
            if seen.insert(p.clone()) {
                ordered.push(p);
            }
        }
        Self::from_patterns(ordered.iter().map(|s| s.as_str()))
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Patterns (source text) that matched `text`.
    pub fn scan(&self, text: &str) -> Vec<String> {
        self.rules
            .iter()
            .filter(|(_, re)| re.is_match(text))
            .map(|(src, _)| src.clone())
            .collect()
    }

    /// Scan both the raw text and its confusable-normalized form.
    pub fn scan_normalized(&self, text: &str) -> Vec<String> {
        let mut hits = self.scan(text);
        let normalized = normalize_for_scan(text);
        if normalized != text {
            for h in self.scan(&normalized) {
                if !hits.contains(&h) {
                    hits.push(h);
                }
            }
        }
        hits
    }

    pub fn count_matches(&self, text: &str) -> usize {
        self.scan(text).len()
    }
}

fn is_format_control(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180E | 0x200B..=0x200F
            | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0
            | 0x1D173..=0x1D17A | 0xE0000..=0xE007F
    )
}

fn fold_confusable(c: char) -> char {
    match c {
        // Cyrillic / Greek look-alikes commonly used to dodge ASCII regexes.
        'а' => 'a',
        'е' => 'e',
        'о' => 'o',
        'р' => 'p',
        'с' => 'c',
        'х' => 'x',
        'у' => 'y',
        'і' => 'i',
        'ј' => 'j',
        'ѕ' => 's',
        'һ' => 'h',
        'к' => 'k',
        'т' => 't',
        'ν' => 'v',
        'ο' => 'o',
        'α' => 'a',
        'ι' => 'i',
        'ρ' => 'p',
        'τ' => 't',
        'ϲ' => 'c',
        'ɡ' => 'g',
        'ⅼ' => 'l',
        'ℓ' => 'l',
        other => other,
    }
}

/// NFKD-normalize, drop combining marks and invisible format characters, and
/// fold common homoglyphs so an obfuscated directive still matches.
pub fn normalize_for_scan(text: &str) -> String {
    text.nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .filter(|c| !is_format_control(*c))
        .map(fold_confusable)
        .collect()
}
