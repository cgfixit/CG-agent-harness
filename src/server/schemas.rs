//! Request bodies for the control plane, port of `harness/schemas.py`.
//!
//! Every struct forbids unknown fields and carries a `validate()` that returns
//! the names of failing DECLARED fields. The `ValidJson<T>` extractor turns
//! serde failures and validation failures into the 422 envelope, substituting
//! `(unexpected field)` for a caller-supplied key so a key like
//! `sk-ant-<secret>` is never echoed.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

use super::errors::ApiError;
use crate::common::repo_paths::canonical_repo_relative_path;

pub const MAX_MESSAGE_LEN: usize = 32768;
pub const MAX_TITLE_LEN: usize = 200;
pub const MAX_MODEL_LEN: usize = 200;
pub const MAX_INSTRUCTION_LEN: usize = 8192;
pub const MAX_REASON_LEN: usize = 1000;
pub const MAX_COMMIT_MESSAGE_LEN: usize = 500;
pub const MAX_BRANCH_LEN: usize = 88;
pub const MAX_CHECK_PROFILES: usize = 8;
pub const MAX_READ_FILES: usize = 8;
pub const MAX_READ_FILE_LEN: usize = 1024;
pub const MAX_GOAL_LEN: usize = 2000;
pub const MAX_WEB_URL_LEN: usize = 500;
pub const MAX_WEB_QUERY_LEN: usize = 200;
pub const MAX_NOTE_LEN: usize = 500;
pub const MAX_NOTE_ID_LEN: usize = 32;
pub const MAX_PLAN_CHARS: usize = 6_100;
pub const MAX_ITERATIONS_CEILING: u32 = 10;
pub const MAX_API_KEYS_PER_REQUEST: usize = 16;
pub const MAX_API_KEY_LEN: usize = 4096;
pub const MAX_ENV_NAME_LEN: usize = 64;
/// Request bodies larger than this are rejected before parsing.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

const MAX_ERROR_FIELD_LEN: usize = 120;
pub const UNEXPECTED_FIELD: &str = "(unexpected field)";

pub trait Validate {
    /// Names of declared fields that fail their bounds.
    fn validate(&self) -> Vec<String>;
}

fn len_ok(s: &str, min: usize, max: usize) -> bool {
    let n = s.chars().count();
    n >= min && n <= max
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// True only for console /loop turns (JSON key `loop`).
    #[serde(default, rename = "loop")]
    pub loop_turn: bool,
}

impl Validate for ChatRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !len_ok(&self.message, 1, MAX_MESSAGE_LEN) {
            bad.push("message".into());
        }
        if let Some(m) = &self.model {
            if !len_ok(m, 0, MAX_MODEL_LEN) {
                bad.push("model".into());
            }
        }
        bad
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    #[serde(default)]
    pub title: String,
}

impl Validate for SessionCreateRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.title, 0, MAX_TITLE_LEN) {
            vec![]
        } else {
            vec!["title".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameRequest {
    pub title: String,
}

impl Validate for RenameRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.title, 1, MAX_TITLE_LEN) {
            vec![]
        } else {
            vec!["title".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToggleRequest {
    pub enabled: bool,
}

impl Validate for ToggleRequest {
    fn validate(&self) -> Vec<String> {
        vec![]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryNoteRequest {
    pub text: String,
}

impl Validate for MemoryNoteRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.text, 1, MAX_NOTE_LEN) {
            vec![]
        } else {
            vec!["text".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryForgetRequest {
    pub id: String,
}

impl Validate for MemoryForgetRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.id, 1, MAX_NOTE_ID_LEN) {
            vec![]
        } else {
            vec!["id".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUrlRequest {
    pub url: String,
}

impl Validate for WebUrlRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.url, 1, MAX_WEB_URL_LEN) {
            vec![]
        } else {
            vec!["url".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchRequest {
    pub query: String,
}

impl Validate for WebSearchRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.query, 1, MAX_WEB_QUERY_LEN) {
            vec![]
        } else {
            vec!["query".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GoalRequest {
    #[serde(default)]
    pub goal: String,
}

impl Validate for GoalRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.goal, 0, MAX_GOAL_LEN) {
            vec![]
        } else {
            vec!["goal".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelectRequest {
    pub model: String,
}

impl Validate for ModelSelectRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.model, 1, MAX_TITLE_LEN) {
            vec![]
        } else {
            vec!["model".into()]
        }
    }
}

/// Start one real-repo coding run. `checks` carries profile NAMES, never
/// commands; `confirm` is NOT defaulted on.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRunRequest {
    pub instruction: String,
    pub branch: String,
    pub commit_message: String,
    pub reason: String,
    #[serde(default)]
    pub confirm: bool,
    #[serde(default)]
    pub checks: Option<Vec<String>>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub read_files: Vec<String>,
    #[serde(default)]
    pub max_iterations: Option<i64>,
    #[serde(default)]
    pub pr: Option<i64>,
    #[serde(default)]
    pub issue: Option<i64>,
}

impl AgentRunRequest {
    /// Canonical, deduplicated `read_files`, or `None` when any entry is unsafe.
    pub fn canonical_read_files(&self) -> Option<Vec<String>> {
        if self.read_files.len() > MAX_READ_FILES {
            return None;
        }
        let mut out: Vec<String> = Vec::new();
        for raw in &self.read_files {
            if raw.chars().count() > MAX_READ_FILE_LEN {
                return None;
            }
            let canonical = canonical_repo_relative_path(raw)?;
            if !out.contains(&canonical) {
                out.push(canonical);
            }
        }
        Some(out)
    }
}

impl Validate for AgentRunRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !len_ok(&self.instruction, 1, MAX_INSTRUCTION_LEN) {
            bad.push("instruction".into());
        }
        let branch_ok = len_ok(&self.branch, 1, MAX_BRANCH_LEN)
            && crate::common::identity::identity()
                .map(|id| id.branch_is_valid(&self.branch))
                .unwrap_or(false);
        if !branch_ok {
            bad.push("branch".into());
        }
        if !len_ok(&self.commit_message, 1, MAX_COMMIT_MESSAGE_LEN) {
            bad.push("commit_message".into());
        }
        if !len_ok(&self.reason, 1, MAX_REASON_LEN) {
            bad.push("reason".into());
        }
        if let Some(checks) = &self.checks {
            if checks.is_empty() || checks.len() > MAX_CHECK_PROFILES {
                bad.push("checks".into());
            }
        }
        if let Some(plan) = &self.plan {
            if plan.trim().is_empty() || !len_ok(plan, 1, MAX_PLAN_CHARS) {
                bad.push("plan".into());
            }
        }
        if self.canonical_read_files().is_none() {
            bad.push("read_files".into());
        }
        if let Some(n) = self.max_iterations {
            if n < 1 || n > MAX_ITERATIONS_CEILING as i64 {
                bad.push("max_iterations".into());
            }
        }
        if matches!(self.pr, Some(n) if n < 1) {
            bad.push("pr".into());
        }
        if matches!(self.issue, Some(n) if n < 1) {
            bad.push("issue".into());
        }
        if self.pr.is_some() && self.issue.is_some() {
            bad.push("pr/issue".into());
        }
        bad
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approve,
    Reject,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Approve => "approve",
            Decision::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDecisionRequest {
    pub decision: Decision,
}

impl Validate for AgentDecisionRequest {
    fn validate(&self) -> Vec<String> {
        vec![]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPublishRequest {
    pub reason: String,
    #[serde(default)]
    pub confirm: bool,
}

impl Validate for AgentPublishRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.reason, 1, MAX_REASON_LEN) {
            vec![]
        } else {
            vec!["reason".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeysRequest {
    pub keys: BTreeMap<String, String>,
}

impl Validate for ApiKeysRequest {
    fn validate(&self) -> Vec<String> {
        if self.keys.is_empty() || self.keys.len() > MAX_API_KEYS_PER_REQUEST {
            return vec!["keys".into()];
        }
        for (name, secret) in &self.keys {
            if name.chars().count() > MAX_ENV_NAME_LEN || secret.chars().count() > MAX_API_KEY_LEN {
                return vec!["keys".into()];
            }
        }
        vec![]
    }
}

// ---------------------------------------------------------------- auth bodies

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthLoginRequest {
    pub username: String,
    pub password: String,
}

impl Validate for AuthLoginRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !len_ok(&self.username, 1, 64) {
            bad.push("username".into());
        }
        if !len_ok(&self.password, 1, 1024) {
            bad.push("password".into());
        }
        bad
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSetPasswordRequest {
    pub password: String,
}

impl Validate for AuthSetPasswordRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.password, 1, 1024) {
            vec![]
        } else {
            vec!["password".into()]
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthCreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
}

impl Validate for AuthCreateUserRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !len_ok(&self.username, 1, 64) {
            bad.push("username".into());
        }
        if !len_ok(&self.password, 1, 1024) {
            bad.push("password".into());
        }
        bad
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSetRoleRequest {
    pub role: String,
}

impl Validate for AuthSetRoleRequest {
    fn validate(&self) -> Vec<String> {
        if len_ok(&self.role, 1, 32) {
            vec![]
        } else {
            vec!["role".into()]
        }
    }
}

// ---------------------------------------------------------------- extractor

/// JSON body extractor that emits the console's 422 envelope.
pub struct ValidJson<T>(pub T);

fn validation_error(fields: Vec<String>) -> ApiError {
    let named = if fields.is_empty() {
        "unknown field".to_string()
    } else {
        fields.join(", ")
    };
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
        format!("request body failed validation: {named}"),
    )
    .details(json!({"fields": fields}))
}

/// Map a serde error message to a reportable field location; the caller's
/// own key (unknown field) is never echoed.
pub fn field_from_serde_error(msg: &str) -> String {
    if msg.starts_with("unknown field") {
        return UNEXPECTED_FIELD.to_string();
    }
    if let Some(rest) = msg.strip_prefix("missing field `") {
        if let Some(end) = rest.find('`') {
            return rest[..end].chars().take(MAX_ERROR_FIELD_LEN).collect();
        }
    }
    if let Some(rest) = msg.strip_prefix("invalid type") {
        // "invalid type: string \"x\", expected a boolean" -> "body"
        let _ = rest;
        return "body".to_string();
    }
    "body".to_string()
}

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = match Bytes::from_request(req, state).await {
            Ok(b) => b,
            Err(_) => return Err(validation_error(vec!["body".into()])),
        };
        if bytes.len() > MAX_BODY_BYTES {
            return Err(validation_error(vec!["body".into()]));
        }
        let parsed: T = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return Err(validation_error(vec![field_from_serde_error(&e.to_string())])),
        };
        let bad = parsed.validate();
        if !bad.is_empty() {
            return Err(validation_error(bad));
        }
        Ok(ValidJson(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn confirm_is_never_defaulted_on() {
        let req: AgentRunRequest = serde_json::from_value(json!({
            "instruction": "x",
            "branch": "claude/x",
            "commit_message": "m",
            "reason": "r"
        }))
        .unwrap();
        assert!(!req.confirm, "absent confirm must deserialize as false, never true");
        let on: AgentRunRequest = serde_json::from_value(json!({
            "instruction": "x",
            "branch": "claude/x",
            "commit_message": "m",
            "reason": "r",
            "confirm": true
        }))
        .unwrap();
        assert!(on.confirm);
    }
}
