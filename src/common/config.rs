//! `config.yaml` loaded once into an `AppConfig` and passed down.
//!
//! Mirrors `utils.logger._get_config`: the file is parsed once and every
//! consumer reads typed views off the same parsed tree. Boolean gates use
//! `flag_is_true`, which demands the literal YAML boolean `true` -- a quoted
//! `"true"` is a string and reads as OFF, the same rule CyClaw applies to
//! every security gate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_yaml_ng::Value;

use super::errors::{HarnessError, Result};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub raw: Value,
    pub path: PathBuf,
}

pub type SharedConfig = Arc<AppConfig>;

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            HarnessError::config(format!("cannot read config file: {e}")).detail("path", path.display().to_string())
        })?;
        Self::from_str(&text, path)
    }

    pub fn from_str(text: &str, path: &Path) -> Result<Self> {
        let raw: Value = serde_yaml_ng::from_str(text)
            .map_err(|e| HarnessError::config(format!("config file is not valid YAML: {e}")))?;
        if !raw.is_mapping() {
            return Err(HarnessError::config("config file must contain a YAML mapping"));
        }
        let config = Self {
            raw,
            path: path.to_path_buf(),
        };
        // Malformed protection switches cannot silently become an opt-out.
        // Absent fields retain compatibility with homes predating these features.
        for section in ["auth", "tls"] {
            if config.get(section).is_some_and(|v| !v.is_mapping())
                || config
                    .get(&format!("{section}.enabled"))
                    .is_some_and(|v| v.as_bool().is_none())
            {
                return Err(HarnessError::config(format!(
                    "{section}.enabled must be a literal YAML boolean"
                )));
            }
        }
        Ok(config)
    }

    /// Look up a dotted path such as `models.local_llm.base_url`.
    pub fn get(&self, dotted: &str) -> Option<&Value> {
        let mut cur = &self.raw;
        for key in dotted.split('.') {
            cur = cur.get(key)?;
        }
        Some(cur)
    }

    /// The literal boolean `true` only.
    pub fn flag_is_true(&self, dotted: &str) -> bool {
        matches!(self.get(dotted), Some(Value::Bool(true)))
    }

    pub fn str_or(&self, dotted: &str, default: &str) -> String {
        match self.get(dotted) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => default.to_string(),
        }
    }

    pub fn str_opt(&self, dotted: &str) -> Option<String> {
        match self.get(dotted) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn f64_or(&self, dotted: &str, default: f64) -> f64 {
        match self.get(dotted) {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
            _ => default,
        }
    }

    pub fn u64_or(&self, dotted: &str, default: u64) -> u64 {
        match self.get(dotted) {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(default),
            _ => default,
        }
    }

    pub fn i64_opt(&self, dotted: &str) -> Option<i64> {
        match self.get(dotted) {
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        }
    }

    pub fn bool_opt(&self, dotted: &str) -> Option<bool> {
        match self.get(dotted) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// A sequence of strings (non-string entries are skipped).
    pub fn str_list(&self, dotted: &str) -> Vec<String> {
        match self.get(dotted) {
            Some(Value::Sequence(items)) => items.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
            _ => Vec::new(),
        }
    }

    /// Sub-tree as JSON (for audit configuration and views).
    pub fn json(&self, dotted: &str) -> serde_json::Value {
        self.get(dotted)
            .and_then(|v| serde_json::to_value(v).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// The embedded shipped default, used to seed a fresh home.
    pub fn embedded_default() -> &'static str {
        include_str!("../../assets/config.default.yaml")
    }
}
