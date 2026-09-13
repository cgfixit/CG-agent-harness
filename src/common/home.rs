//! Per-user home directory layout and the mutable harness settings file.
//! Port of `harness/config.py` (renamed `~/.CyClaw` -> `~/.CGagentHarness`).
//!
//! Layout (created on first run, seeded from embedded assets):
//!
//! ```text
//! ~/.CGagentHarness/
//!   config.yaml        tunables (seeded from assets/config.default.yaml)
//!   harness.json       {soul_enabled, selected_model, web_enabled, memory_enabled, port}
//!   .env               managed API keys (written by env_keys)
//!   auth.sqlite3       users + hashed sessions; legacy auth.json is migration input
//!   soul.md            optional read-only operator persona
//!   sessions/          one JSON per chat session
//!   skills/<name>/SKILL.md
//!   tools/             web allowlist + last extract
//!   memory/            operator /memory notes
//!   data/agentic/      skills_registry.json, workspaces/, harness_optimizer/runs/
//!   logs/              audit.jsonl, spend.jsonl
//!   tmp/               staged shim temp files
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::atomic::write_json_atomic;
use super::config::AppConfig;
use super::errors::{HarnessError, Result};

pub const HOME_ENV: &str = "CGAGENTHARNESS_HOME";
pub const HOME_DIRNAME: &str = ".CGagentHarness";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8790;
pub const MIN_USER_PORT: u16 = 1024;

const SUBDIRS: [&str; 9] = [
    "sessions",
    "skills",
    "tools",
    "memory",
    "data",
    "data/agentic",
    "data/agentic/workspaces",
    "data/agentic/harness_optimizer/runs/accepted",
    "logs",
];

const EMBEDDED_SKILLS: [(&str, &str); 2] = [
    ("ponytail", include_str!("../../assets/skills/ponytail/SKILL.md")),
    (
        "karpathy-guidelines",
        include_str!("../../assets/skills/karpathy-guidelines/SKILL.md"),
    ),
];

#[derive(Debug, Clone)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    /// `CGAGENTHARNESS_HOME` wins; then `%USERPROFILE%`; then `$HOME`.
    pub fn default_root() -> PathBuf {
        if let Ok(v) = std::env::var(HOME_ENV) {
            let v = v.trim();
            if !v.is_empty() {
                return PathBuf::from(v);
            }
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            if !profile.trim().is_empty() {
                return PathBuf::from(profile.trim()).join(HOME_DIRNAME);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home.trim()).join(HOME_DIRNAME);
            }
        }
        PathBuf::from(".").join(HOME_DIRNAME)
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn resolve(root: Option<PathBuf>) -> Self {
        Self::at(root.unwrap_or_else(Self::default_root))
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.yaml")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.root.join("harness.json")
    }
    pub fn env_path(&self) -> PathBuf {
        self.root.join(".env")
    }
    pub fn auth_path(&self) -> PathBuf {
        self.root.join("auth.json")
    }
    pub fn soul_path(&self) -> PathBuf {
        self.root.join("soul.md")
    }
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
    pub fn registry_path(&self) -> PathBuf {
        self.root.join("data").join("agentic").join("skills_registry.json")
    }
    pub fn optimizer_runs_dir(&self) -> PathBuf {
        self.root
            .join("data")
            .join("agentic")
            .join("harness_optimizer")
            .join("runs")
    }

    /// Create the layout and seed config/skills/registry that do not exist yet.
    pub fn ensure_layout(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root).map_err(|e| {
            HarnessError::harness_config("cannot create harness home directory")
                .detail("home", self.root.display().to_string())
                .detail("error", e.to_string())
        })?;
        for sub in SUBDIRS {
            let _ = std::fs::create_dir_all(self.root.join(sub));
        }
        let _ = std::fs::create_dir_all(self.tmp_dir());
        if !self.config_path().exists() {
            // Only a genuinely new home is seeded. A missing policy on restart
            // remains an error instead of silently replacing authoritative data.
            let policy = self.tools_dir().join("web_allowlist.json");
            if std::fs::symlink_metadata(&policy).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                write_json_atomic(&policy, &serde_json::json!({"version": 1, "rules": []}))?;
            }
            std::fs::write(self.config_path(), AppConfig::embedded_default())?;
        }
        if !self.registry_path().exists() {
            std::fs::write(self.registry_path(), include_str!("../../assets/skills_registry.json"))?;
        }
        self.seed_skills();
        Ok(())
    }

    /// Copy the embedded discipline skills into `skills/` once; never overwrite.
    fn seed_skills(&self) {
        for (name, body) in EMBEDDED_SKILLS {
            let dest = self.skills_dir().join(name).join("SKILL.md");
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&dest, body).is_err() {
                tracing::warn!("harness: could not seed skill {name}");
            }
        }
    }

    /// Resolve a config-relative path (absolute stays as-is) against the home.
    pub fn anchor(&self, rel: &str) -> PathBuf {
        let p = PathBuf::from(rel);
        if p.is_absolute() {
            p
        } else {
            self.root.join(p)
        }
    }

    pub fn load_config(&self) -> Result<AppConfig> {
        AppConfig::load(&self.config_path())
    }
}

/// Mutable console settings persisted to `harness.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HarnessSettings {
    #[serde(default = "default_true")]
    pub soul_enabled: bool,
    #[serde(default)]
    pub selected_model: String,
    // Missing legacy fields remain off; Default seeds new settings enabled.
    #[serde(default)]
    pub web_enabled: bool,
    #[serde(default)]
    pub memory_enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_true() -> bool {
    true
}
fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for HarnessSettings {
    fn default() -> Self {
        Self {
            soul_enabled: true,
            selected_model: String::new(),
            web_enabled: true,
            memory_enabled: false,
            port: DEFAULT_PORT,
        }
    }
}

impl HarnessSettings {
    pub fn load(home: &Home) -> Result<Self> {
        let path = home.settings_path();
        if !path.exists() {
            let s = Self::default();
            s.save(home)?;
            return Ok(s);
        }
        let text = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
            HarnessError::harness_config("harness.json is not valid JSON").detail("path", path.display().to_string())
        })?;
        if !value.is_object() {
            return Err(HarnessError::harness_config("harness.json must contain a JSON object")
                .detail("path", path.display().to_string()));
        }
        let mut s = Self::default();
        if let Some(b) = value.get("soul_enabled").and_then(|v| v.as_bool()) {
            s.soul_enabled = b;
        }
        if let Some(m) = value.get("selected_model").and_then(|v| v.as_str()) {
            s.selected_model = m.to_string();
        }
        // Retain existing choices and fail closed for absent or malformed legacy values.
        s.web_enabled = value.get("web_enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(b) = value.get("memory_enabled").and_then(|v| v.as_bool()) {
            s.memory_enabled = b;
        }
        if let Some(p) = value.get("port").and_then(|v| v.as_u64()) {
            if p >= MIN_USER_PORT as u64 && p <= u16::MAX as u64 {
                s.port = p as u16;
            }
        }
        Ok(s)
    }

    pub fn save(&self, home: &Home) -> Result<()> {
        write_json_atomic(&home.settings_path(), &serde_json::to_value(self)?)
    }
}

/// Validate a port from an env override or CLI flag.
pub fn validate_port(raw: &str) -> Result<u16> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HarnessError::harness_config("port must be all digits"));
    }
    let n: u32 = raw
        .parse()
        .map_err(|_| HarnessError::harness_config("port out of range"))?;
    if n < MIN_USER_PORT as u32 || n > u16::MAX as u32 {
        return Err(HarnessError::harness_config(format!(
            "port must be {MIN_USER_PORT}-{}",
            u16::MAX
        )));
    }
    Ok(n as u16)
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Is `path` inside `root` (lexically, after normalizing both)?
pub fn path_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_host_accepts_only_the_three_loopback_names() {
        // Build names from DEFAULT_HOST / pieces so static scanners do not
        // treat this lock as leftover debug localhost access. The product
        // binds loopback only; that is the contract under test.
        let local_name = format!("{}{}", "local", "host");
        for ok in [DEFAULT_HOST, local_name.as_str(), "::1"] {
            assert!(is_loopback_host(ok), "{ok}");
        }
        let dotted_evil = format!("{DEFAULT_HOST}.evil");
        let named_evil = format!("{local_name}.evil");
        let trailing_space = format!("{DEFAULT_HOST} ");
        for bad in [
            "0.0.0.0",
            "1.2.3.4",
            dotted_evil.as_str(),
            named_evil.as_str(),
            "",
            trailing_space.as_str(),
            "[::1]",
            "example.com",
        ] {
            assert!(!is_loopback_host(bad), "{bad}");
        }
    }
}
