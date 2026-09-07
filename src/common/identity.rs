//! Agent git identity: committer defaults and the branch-name namespace.
//!
//! Port of `utils/agent_identity.py`. Every allowed prefix is alphanumeric-led
//! so `git checkout -b <name>` / `gh pr create --head` can never reparse a
//! branch name as a flag. The preferred prefix env override is validated and
//! FAILS CLOSED (an invalid value is an error, not a fallback to the default).

use std::sync::OnceLock;

use regex::Regex;

use super::errors::{HarnessError, Result};

pub const DEFAULT_COMMIT_NAME: &str = "CGagentHarness Agent";
pub const DEFAULT_COMMIT_EMAIL: &str = "cgagentharness-agent@users.noreply.github.com";
pub const DEFAULT_BRANCH_PREFIX: &str = "agent";
pub const ENV_COMMIT_NAME: &str = "CGAGENTHARNESS_AGENT_COMMIT_NAME";
pub const ENV_COMMIT_EMAIL: &str = "CGAGENTHARNESS_AGENT_COMMIT_EMAIL";
pub const ENV_BRANCH_PREFIX: &str = "CGAGENTHARNESS_AGENT_BRANCH_PREFIX";

/// From CyClaw's `.github/PULL_REQUEST_TEMPLATE.md` (plus lowercase CyClaw + generic agent).
pub const TEMPLATE_BRANCH_PREFIXES: [&str; 7] = ["claude", "codex", "grok", "kimi", "CyClaw", "cyclaw", "agent"];

const TOPIC: &str = r"[A-Za-z0-9][A-Za-z0-9._/-]{0,79}";

#[derive(Debug, Clone)]
pub struct Identity {
    pub commit_name: String,
    pub commit_email: String,
    pub branch_prefix: String,
    pub allowed_prefixes: Vec<String>,
    pub branch_name_re: Regex,
}

fn prefix_shape() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9._-]{0,31}\z").expect("static regex"))
}

pub fn validate_prefix(raw: &str, env_name: &str) -> Result<String> {
    if !prefix_shape().is_match(raw) {
        return Err(HarnessError::config(format!(
            "{env_name} must be 1-32 chars from [A-Za-z0-9._-] and start alphanumeric"
        )));
    }
    Ok(raw.to_string())
}

pub fn compile_branch_name_re(prefixes: &[String]) -> Regex {
    let mut ordered: Vec<&String> = prefixes.iter().collect();
    // Longer prefixes first so alternation is unambiguous if one prefixes another.
    ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let alt: Vec<String> = ordered.iter().map(|p| regex::escape(p)).collect();
    Regex::new(&format!(r"\A({})/{}\z", alt.join("|"), TOPIC)).expect("branch regex")
}

impl Identity {
    /// Build from explicit values (tests) or the environment (`from_env`).
    pub fn build(commit_name: String, commit_email: String, preferred_prefix: &str) -> Result<Self> {
        let branch_prefix = validate_prefix(preferred_prefix, ENV_BRANCH_PREFIX)?;
        let mut allowed: Vec<String> = TEMPLATE_BRANCH_PREFIXES.iter().map(|s| s.to_string()).collect();
        if !allowed.contains(&branch_prefix) {
            allowed.push(branch_prefix.clone());
        }
        let branch_name_re = compile_branch_name_re(&allowed);
        Ok(Self {
            commit_name,
            commit_email,
            branch_prefix,
            allowed_prefixes: allowed,
            branch_name_re,
        })
    }

    pub fn from_env() -> Result<Self> {
        let name = std::env::var(ENV_COMMIT_NAME).ok().filter(|s| !s.trim().is_empty());
        let email = std::env::var(ENV_COMMIT_EMAIL).ok().filter(|s| !s.trim().is_empty());
        let prefix = std::env::var(ENV_BRANCH_PREFIX).unwrap_or_else(|_| DEFAULT_BRANCH_PREFIX.to_string());
        Self::build(
            name.unwrap_or_else(|| DEFAULT_COMMIT_NAME.to_string()),
            email.unwrap_or_else(|| DEFAULT_COMMIT_EMAIL.to_string()),
            &prefix,
        )
    }

    pub fn branch_is_valid(&self, branch: &str) -> bool {
        self.branch_name_re.is_match(branch)
    }

    /// Human-readable list for error messages (e.g. `agent/, claude/, codex/`).
    pub fn allowed_prefixes_help(&self) -> String {
        let mut names: Vec<String> = self.allowed_prefixes.clone();
        names.sort_by_key(|a| a.to_lowercase());
        names.iter().map(|p| format!("{p}/")).collect::<Vec<_>>().join(", ")
    }
}

/// Process-wide identity resolved once from the environment (fail-closed).
pub fn identity() -> Result<&'static Identity> {
    static CELL: OnceLock<std::result::Result<Identity, HarnessError>> = OnceLock::new();
    match CELL.get_or_init(Identity::from_env) {
        Ok(id) => Ok(id),
        Err(e) => Err(e.clone()),
    }
}
