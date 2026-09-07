//! Validated `agentic:` block, port of `agentic/config.py`.
//! Data paths are forced under `<home>/data/`; the local planner URL must be
//! loopback; an enabled cloud provider with the master cloud gate off is a
//! config ERROR, not a silent no-op.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::backend::is_loopback_url;

pub const DEFAULT_REPO: &str = "cgfixit/CG-agent-harness";
pub const DEFAULT_ALLOWED_READ_OPS: [&str; 6] =
    ["pr_view", "pr_list", "pr_diff", "issue_view", "issue_list", "repo_view"];
pub const DEFAULT_PROTECTED_WRITE_PATH_PREFIXES: [&str; 19] = [
    "tests/",
    "conftest.py",
    ".github/",
    ".git/",
    "pyproject.toml",
    "setup.cfg",
    "pytest.ini",
    ".ruff.toml",
    "ruff.toml",
    "tox.ini",
    "noxfile.py",
    ".claude/",
    ".codex/",
    "config.yaml",
    "AGENTS.md",
    "Cargo.toml",
    "Cargo.lock",
    "deny.toml",
    "rustfmt.toml",
];
pub const DEFAULT_MAX_WRITE_BUDGET_BYTES: u64 = 100_000;
pub const DEFAULT_MAX_HANDOFF_CHARS: usize = 200_000;
pub const DEFAULT_PLANNER_TIMEOUT_SEC: u64 = 720;
pub const DEFAULT_PLANNER_MAX_TOKENS: u64 = 3072;
pub const VALID_DEEPAGENT_PROVIDERS: [&str; 2] = ["ollama", "openai_compatible"];
pub const CLOUD_PROVIDERS: [&str; 2] = ["grok", "claude"];
const SHELL_METACHARS: &str = ";|&$`<>(){}[]!*?\"'\\\n\r\t ";

pub fn cloud_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        "grok" => Some("GROK_API_KEY"),
        "claude" => Some("ANTHROPIC_API_KEY"),
        _ => None,
    }
}

pub fn repo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*\z").expect("static regex"))
}

fn cfg_err(message: impl Into<String>) -> HarnessError {
    HarnessError::agentic_config(message)
}

fn no_shell_metachars(value: &str, field: &str) -> Result<()> {
    let bad: Vec<char> = value.chars().filter(|c| SHELL_METACHARS.contains(*c)).collect();
    if !bad.is_empty() {
        return Err(cfg_err(format!("{field} contains forbidden characters")).detail("field", field));
    }
    Ok(())
}

/// Lexically normalize (`.`/`..` collapsed) without touching the filesystem.
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A configured data path must resolve INSIDE `<home>/data/`.
pub fn resolve_data_path(raw: &str, field: &str, home_root: &Path) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(cfg_err(format!("{field} is required")).detail("hint", "Use a path under the home data/ tree."));
    }
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|_| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        home_root.join(expanded)
    };
    let resolved = lexical_normalize(&joined);
    let data_root = lexical_normalize(&home_root.join("data"));
    if !resolved.starts_with(&data_root) || resolved == data_root {
        return Err(
            cfg_err(format!("{field} must resolve to a path inside the home's data/ tree"))
                .detail("data_root", data_root.display().to_string())
                .detail("received", raw),
        );
    }
    Ok(resolved)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudProviderConfig {
    pub enabled: bool,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct DeepAgentConfig {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub workspace_root: PathBuf,
    pub allow_cloud_providers: bool,
    pub providers: BTreeMap<String, CloudProviderConfig>,
    pub allow_git_write_tools: bool,
    pub protected_write_paths: Vec<String>,
    pub max_write_budget_bytes: u64,
    pub max_handoff_chars: usize,
    pub planner_timeout_sec: u64,
    pub planner_max_tokens: u64,
    pub scan_code_shape: bool,
}

impl DeepAgentConfig {
    /// A provider's config only when BOTH the master cloud gate and its own flag are true.
    pub fn cloud_provider(&self, name: &str) -> Option<&CloudProviderConfig> {
        if !self.allow_cloud_providers {
            return None;
        }
        self.providers.get(name).filter(|p| p.enabled)
    }
}

#[derive(Debug, Clone)]
pub struct AgenticConfig {
    pub enabled: bool,
    pub repo: String,
    pub mode: String,
    pub writes_enabled: bool,
    pub gh_min_version: (u32, u32, u32),
    pub registry_path: PathBuf,
    pub allowed_read_ops: Vec<String>,
    pub gh_timeout_sec: u64,
    pub gh_retries: u32,
    pub deepagent: DeepAgentConfig,
}

impl AgenticConfig {
    pub fn is_write_mode(&self) -> bool {
        self.mode == "write"
    }
}

fn bool_field(cfg: &AppConfig, key: &str, default: bool) -> Result<bool> {
    match cfg.get(key) {
        None | Some(serde_yaml_ng::Value::Null) => Ok(default),
        Some(serde_yaml_ng::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(cfg_err(format!("{key} must be a boolean")).detail("field", key)),
    }
}

fn positive_int(cfg: &AppConfig, key: &str, default: u64) -> Result<u64> {
    match cfg.get(key) {
        None | Some(serde_yaml_ng::Value::Null) => Ok(default),
        Some(serde_yaml_ng::Value::Number(n)) => match n.as_i64() {
            Some(v) if v > 0 => Ok(v as u64),
            _ => Err(cfg_err(format!("{key} must be a positive integer"))),
        },
        Some(_) => Err(cfg_err(format!("{key} must be a positive integer"))),
    }
}

pub fn load_agentic_config(cfg: &AppConfig, home_root: &Path) -> Result<AgenticConfig> {
    let block = cfg.get("agentic").ok_or_else(|| {
        cfg_err("agentic: block missing from config.yaml").detail("hint", "Append the agentic: block to config.yaml.")
    })?;
    if !block.is_mapping() {
        return Err(cfg_err("agentic: block must be a mapping"));
    }
    let enabled = bool_field(cfg, "agentic.enabled", false)?;
    let repo = cfg.str_or("agentic.repo", DEFAULT_REPO);
    if !repo_re().is_match(&repo) {
        return Err(cfg_err("agentic.repo must match 'owner/name'").detail("received", repo.clone()));
    }
    no_shell_metachars(&repo, "agentic.repo")?;
    let mode = cfg.str_or("agentic.mode", "read");
    if mode != "read" && mode != "write" {
        return Err(cfg_err("agentic.mode must be 'read' or 'write'").detail("received", mode.clone()));
    }
    let writes_enabled = bool_field(cfg, "agentic.writes_enabled", false)?;
    let gh_min = cfg.str_or("agentic.gh_min_version", "2.40.0");
    let parts: Vec<u32> = gh_min.split('.').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 || gh_min.split('.').count() != 3 {
        return Err(cfg_err("agentic.gh_min_version must be 'X.Y.Z'").detail("received", gh_min.clone()));
    }
    let registry_path = resolve_data_path(
        &cfg.str_or("agentic.registry_path", "data/agentic/skills_registry.json"),
        "agentic.registry_path",
        home_root,
    )?;
    let mut allowed_read_ops = cfg.str_list("agentic.allowed_read_ops");
    if cfg.get("agentic.allowed_read_ops").is_none() {
        allowed_read_ops = DEFAULT_ALLOWED_READ_OPS.iter().map(|s| s.to_string()).collect();
    }
    let gh_timeout_sec = positive_int(cfg, "agentic.gh_timeout_sec", 30)?;
    let gh_retries = match cfg.get("agentic.gh_retries") {
        None | Some(serde_yaml_ng::Value::Null) => 2,
        Some(serde_yaml_ng::Value::Number(n)) => match n.as_i64() {
            Some(v) if v >= 0 => v as u32,
            _ => return Err(cfg_err("agentic.gh_retries must be an integer >= 0")),
        },
        Some(_) => return Err(cfg_err("agentic.gh_retries must be an integer >= 0")),
    };

    // deepagent_github
    let d = "agentic.deepagent_github";
    let provider = cfg.str_or(&format!("{d}.provider"), "ollama");
    if !VALID_DEEPAGENT_PROVIDERS.contains(&provider.as_str()) {
        return Err(
            cfg_err(format!("{d}.provider must be one of {VALID_DEEPAGENT_PROVIDERS:?}"))
                .detail("received", provider.clone()),
        );
    }
    let base_url = cfg.str_or(&format!("{d}.base_url"), "http://localhost:11434/v1");
    if base_url.trim().is_empty() {
        return Err(cfg_err(format!("{d}.base_url must be a non-empty string")));
    }
    let model = cfg.str_or(&format!("{d}.model"), "");
    no_shell_metachars(&base_url, &format!("{d}.base_url"))?;
    no_shell_metachars(&model, &format!("{d}.model"))?;
    if !is_loopback_url(&base_url) {
        return Err(cfg_err(format!(
            "{d}.base_url must be a loopback URL; it addresses the local model only"
        ))
        .detail("received", base_url.clone()));
    }
    let workspace_root = resolve_data_path(
        &cfg.str_or(&format!("{d}.workspace_root"), "data/agentic/workspaces"),
        &format!("{d}.workspace_root"),
        home_root,
    )?;
    let allow_cloud_providers = bool_field(cfg, &format!("{d}.allow_cloud_providers"), false)?;
    let mut providers = BTreeMap::new();
    if let Some(serde_yaml_ng::Value::Mapping(map)) = cfg.get(&format!("{d}.providers")) {
        for (k, v) in map {
            let name = k.as_str().unwrap_or("").to_string();
            if !CLOUD_PROVIDERS.contains(&name.as_str()) {
                return Err(cfg_err(format!("unknown {d}.providers entry '{name}'")).detail("received", name));
            }
            let enabled = matches!(v.get("enabled"), Some(serde_yaml_ng::Value::Bool(true)));
            if !matches!(v.get("enabled"), None | Some(serde_yaml_ng::Value::Bool(_))) {
                return Err(cfg_err(format!("{d}.providers.*.enabled must be a boolean")));
            }
            let pmodel = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
            no_shell_metachars(&pmodel, &format!("{d}.providers.*.model"))?;
            providers.insert(name, CloudProviderConfig { enabled, model: pmodel });
        }
    } else if cfg.get(&format!("{d}.providers")).is_some_and(|v| !v.is_null()) {
        return Err(cfg_err(format!("{d}.providers must be a mapping")));
    }
    if !allow_cloud_providers {
        let live: Vec<&String> = providers.iter().filter(|(_, p)| p.enabled).map(|(n, _)| n).collect();
        if !live.is_empty() {
            return Err(
                cfg_err(format!("{d}.providers enabled while allow_cloud_providers is false"))
                    .detail("enabled", serde_json::json!(live)),
            );
        }
    }
    let protected_write_paths = match cfg.get(&format!("{d}.protected_write_paths")) {
        None => DEFAULT_PROTECTED_WRITE_PATH_PREFIXES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some(serde_yaml_ng::Value::Sequence(items)) => {
            let mut out = Vec::new();
            for it in items {
                match it.as_str() {
                    Some(s) if !s.is_empty() => out.push(s.to_string()),
                    _ => {
                        return Err(cfg_err(format!(
                            "{d}.protected_write_paths must be a list of non-empty strings"
                        )))
                    }
                }
            }
            out
        }
        Some(_) => {
            return Err(cfg_err(format!(
                "{d}.protected_write_paths must be a list of non-empty strings"
            )))
        }
    };
    let deepagent = DeepAgentConfig {
        enabled: bool_field(cfg, &format!("{d}.enabled"), false)?,
        provider,
        base_url,
        model,
        workspace_root,
        allow_cloud_providers,
        providers,
        allow_git_write_tools: bool_field(cfg, &format!("{d}.allow_git_write_tools"), false)?,
        protected_write_paths,
        max_write_budget_bytes: positive_int(
            cfg,
            &format!("{d}.max_write_budget_bytes"),
            DEFAULT_MAX_WRITE_BUDGET_BYTES,
        )?,
        max_handoff_chars: positive_int(cfg, &format!("{d}.max_handoff_chars"), DEFAULT_MAX_HANDOFF_CHARS as u64)?
            as usize,
        planner_timeout_sec: positive_int(cfg, &format!("{d}.planner_timeout_sec"), DEFAULT_PLANNER_TIMEOUT_SEC)?,
        planner_max_tokens: positive_int(cfg, &format!("{d}.planner_max_tokens"), DEFAULT_PLANNER_MAX_TOKENS)?,
        scan_code_shape: bool_field(cfg, &format!("{d}.scan_code_shape"), true)?,
    };
    Ok(AgenticConfig {
        enabled,
        repo,
        mode,
        writes_enabled,
        gh_min_version: (parts[0], parts[1], parts[2]),
        registry_path,
        allowed_read_ops,
        gh_timeout_sec,
        gh_retries,
        deepagent,
    })
}
