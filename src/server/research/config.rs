//! Typed view of the `research` section.
//!
//! Numeric defaults live in `assets/config.default.yaml`. Missing keys on an
//! older home use that shipped file. A present but ill-typed or out-of-range
//! value is a config error. Boolean gates use `flag_is_true`: only an
//! unquoted `true` arms a gate. The embeddings endpoint must be loopback, and
//! that check is enforced only when the feature is enabled.

use std::path::Path;

use serde_yaml_ng::Value;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

const EMBEDDINGS_TIMEOUT_SEC: (u64, u64) = (5, 3600);
const CHUNK_CHARS: (u64, u64) = (100, 8000);
const CHUNK_OVERLAP: (u64, u64) = (0, 4000);
const TOP_K: (u64, u64) = (1, 50);
const MAX_DB_BYTES: (u64, u64) = (16_777_216, 2_147_483_648);
const MAX_FILE_BYTES: (u64, u64) = (1024, 16_777_216);
const MAX_TOTAL_BYTES: (u64, u64) = (65_536, 268_435_456);
const MIN_SCORE: (f64, f64) = (0.0, 10.0);
const MIN_SEMANTIC_SCORE: (f64, f64) = (0.0, 1.0);
const MAX_URL_CHARS: usize = 256;
const MAX_MODEL_CHARS: usize = 200;
const MAX_PATH_CHARS: usize = 4096;

/// Effective research-corpus settings.
#[derive(Debug, Clone)]
pub struct ResearchConfig {
    pub enabled: bool,
    /// Used exactly as written (no home anchoring); an absolute path is
    /// recommended. Empty ships disabled behavior: reindex refuses.
    pub corpus_dir: String,
    /// Loopback OpenAI-compatible embeddings base URL; empty = keyword-only.
    pub embeddings_url: String,
    pub embeddings_model: String,
    /// Per-request embeddings timeout; also bounds one reindex run.
    pub embeddings_timeout_sec: u64,
    pub chunk_chars: usize,
    pub chunk_overlap: usize,
    pub top_k: usize,
    pub min_score: f64,
    pub min_semantic_score: f64,
    pub max_db_bytes: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl ResearchConfig {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        if cfg.get("research").is_some_and(|value| !value.is_mapping()) {
            return Err(HarnessError::config("research must be a mapping"));
        }
        let defaults = AppConfig::from_str(AppConfig::embedded_default(), Path::new("defaults"))?;
        let enabled = cfg.flag_is_true("research.enabled");
        let corpus_dir = text(cfg, &defaults, "corpus_dir", MAX_PATH_CHARS)?;
        let embeddings_url = text(cfg, &defaults, "embeddings_url", MAX_URL_CHARS)?;
        if enabled && !embeddings_url.is_empty() && !crate::llm::backend::is_loopback_url(&embeddings_url) {
            return Err(HarnessError::config(
                "research.embeddings_url must be a loopback URL when research is enabled",
            ));
        }
        let embeddings_model = text(cfg, &defaults, "embeddings_model", MAX_MODEL_CHARS)?;
        let chunk_chars = integer(cfg, &defaults, "chunk_chars", CHUNK_CHARS)? as usize;
        let chunk_overlap = integer(cfg, &defaults, "chunk_overlap", CHUNK_OVERLAP)? as usize;
        if chunk_overlap >= chunk_chars {
            return Err(HarnessError::config(
                "research.chunk_overlap must stay below research.chunk_chars",
            ));
        }
        Ok(Self {
            enabled,
            corpus_dir,
            embeddings_url,
            embeddings_model,
            embeddings_timeout_sec: integer(cfg, &defaults, "embeddings_timeout_sec", EMBEDDINGS_TIMEOUT_SEC)?,
            chunk_chars,
            chunk_overlap,
            top_k: integer(cfg, &defaults, "top_k", TOP_K)? as usize,
            min_score: float(cfg, &defaults, "min_score", MIN_SCORE)?,
            min_semantic_score: float(cfg, &defaults, "min_semantic_score", MIN_SEMANTIC_SCORE)?,
            max_db_bytes: integer(cfg, &defaults, "max_db_bytes", MAX_DB_BYTES)?,
            max_file_bytes: integer(cfg, &defaults, "max_file_bytes", MAX_FILE_BYTES)?,
            max_total_bytes: integer(cfg, &defaults, "max_total_bytes", MAX_TOTAL_BYTES)?,
        })
    }
}

fn integer(cfg: &AppConfig, defaults: &AppConfig, name: &str, bounds: (u64, u64)) -> Result<u64> {
    let key = format!("research.{name}");
    let (min, max) = bounds;
    let value = if cfg.get(&key).is_some() {
        cfg.get(&key)
    } else {
        defaults.get(&key)
    };
    let Some(Value::Number(number)) = value else {
        return Err(HarnessError::config(format!(
            "{key} must be an integer from {min} to {max}"
        )));
    };
    number
        .as_u64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| HarnessError::config(format!("{key} must be an integer from {min} to {max}")))
}

fn float(cfg: &AppConfig, defaults: &AppConfig, name: &str, bounds: (f64, f64)) -> Result<f64> {
    let key = format!("research.{name}");
    let (min, max) = bounds;
    let value = if cfg.get(&key).is_some() {
        cfg.get(&key)
    } else {
        defaults.get(&key)
    };
    let Some(Value::Number(number)) = value else {
        return Err(HarnessError::config(format!(
            "{key} must be a number from {min} to {max}"
        )));
    };
    number
        .as_f64()
        .filter(|v| v.is_finite() && *v >= min && *v <= max)
        .ok_or_else(|| HarnessError::config(format!("{key} must be a number from {min} to {max}")))
}

fn text(cfg: &AppConfig, defaults: &AppConfig, name: &str, max_chars: usize) -> Result<String> {
    let key = format!("research.{name}");
    let value = if cfg.get(&key).is_some() {
        cfg.get(&key)
    } else {
        defaults.get(&key)
    };
    let Some(Value::String(raw)) = value else {
        return Err(HarnessError::config(format!("{key} must be a string")));
    };
    let trimmed = raw.trim();
    if trimmed.chars().count() > max_chars || trimmed.chars().any(|c| c.is_control()) {
        return Err(HarnessError::config(format!(
            "{key} must be at most {max_chars} characters without control characters"
        )));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(yaml: &str) -> AppConfig {
        AppConfig::from_str(yaml, Path::new("fixture.yaml")).unwrap()
    }

    fn load(yaml: &str) -> ResearchConfig {
        ResearchConfig::from_config(&app(yaml)).unwrap()
    }

    #[test]
    fn shipped_defaults_are_closed_and_keyword_only() {
        let cfg = load(AppConfig::embedded_default());
        assert!(!cfg.enabled);
        assert!(cfg.corpus_dir.is_empty());
        assert!(cfg.embeddings_url.is_empty());
        assert_eq!(cfg.embeddings_model, "all-minilm");
        assert_eq!(cfg.embeddings_timeout_sec, 120);
        assert_eq!(cfg.chunk_chars, 900);
        assert_eq!(cfg.chunk_overlap, 120);
        assert_eq!(cfg.top_k, 5);
        assert_eq!(cfg.min_score, 0.028);
        assert_eq!(cfg.min_semantic_score, 0.30);
        assert_eq!(cfg.max_db_bytes, 536_870_912);
        assert_eq!(cfg.max_file_bytes, 1_048_576);
        assert_eq!(cfg.max_total_bytes, 67_108_864);
    }

    #[test]
    fn absent_section_stays_closed() {
        let cfg = load("app:\n  name: test\n");
        assert!(!cfg.enabled);
        assert!(cfg.corpus_dir.is_empty());
    }

    #[test]
    fn quoted_true_stays_off() {
        let cfg = load("research:\n  enabled: \"true\"\n");
        assert!(!cfg.enabled);
    }

    #[test]
    fn non_loopback_embeddings_are_refused_only_when_enabled() {
        let remote = "research:\n  enabled: true\n  embeddings_url: \"https://203.0.113.9/v1\"\n";
        let err = ResearchConfig::from_config(&app(remote)).unwrap_err();
        assert!(err.is("CONFIG_ERROR"));
        assert!(!err.message.contains("203.0.113.9"));
        // The same URL is tolerated while the feature is off: nothing dials it.
        let off = load("research:\n  embeddings_url: \"https://203.0.113.9/v1\"\n");
        assert!(!off.enabled);
        assert_eq!(off.embeddings_url, "https://203.0.113.9/v1");
        // Loopback is accepted when enabled.
        let local = load("research:\n  enabled: true\n  embeddings_url: \"http://127.0.0.1:11434/v1\"\n");
        assert!(local.enabled);
        assert_eq!(local.embeddings_url, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn overlap_must_stay_below_chunk_chars() {
        assert!(ResearchConfig::from_config(&app("research:\n  chunk_chars: 100\n  chunk_overlap: 100\n")).is_err());
        assert!(ResearchConfig::from_config(&app("research:\n  chunk_chars: 100\n  chunk_overlap: 120\n")).is_err());
        let ok = load("research:\n  chunk_chars: 200\n  chunk_overlap: 120\n");
        assert_eq!(ok.chunk_chars, 200);
    }

    #[test]
    fn limits_reject_out_of_range_and_ill_typed() {
        for bad in [
            "research:\n  top_k: 0\n",
            "research:\n  top_k: 51\n",
            "research:\n  top_k: \"5\"\n",
            "research:\n  chunk_chars: 99\n",
            "research:\n  embeddings_timeout_sec: 4\n",
            "research:\n  max_file_bytes: 1023\n",
            "research:\n  min_score: -0.1\n",
            "research:\n  min_score: 10.5\n",
            "research:\n  min_semantic_score: 1.5\n",
            "research:\n  min_semantic_score: \"0.3\"\n",
        ] {
            assert!(ResearchConfig::from_config(&app(bad)).is_err(), "{bad}");
        }
        assert!(ResearchConfig::from_config(&app("research: 4\n")).is_err());
        let ok = load("research:\n  top_k: 50\n  min_score: 0\n  min_semantic_score: 1\n");
        assert_eq!(ok.top_k, 50);
    }
}
