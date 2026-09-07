//! One typed error envelope for the whole crate: `{code, message, details}`.
//!
//! CyClaw's `utils/errors.py` builds a class hierarchy whose only observable
//! surface is the `.code` string, the message, and a details dict. A single
//! struct carrying the same three fields is the faithful port; the helper
//! constructors below reproduce every code the harness and the agentic
//! pipeline emit, so the console's `detail.code` contract is unchanged.

use serde_json::{json, Value};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct HarnessError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl HarnessError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: json!({}),
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        if !self.details.is_object() {
            self.details = json!({});
        }
        if let Some(map) = self.details.as_object_mut() {
            map.insert(key.to_string(), value.into());
        }
        self
    }

    // -- generic ---------------------------------------------------------
    pub fn agentic(message: impl Into<String>) -> Self {
        Self::new("AGENTIC_ERROR", message)
    }
    pub fn config(message: impl Into<String>) -> Self {
        Self::new("CONFIG_ERROR", message)
    }
    pub fn agentic_config(message: impl Into<String>) -> Self {
        Self::new("AGENTIC_CONFIG_INVALID", message)
    }
    pub fn write_refused(message: impl Into<String>) -> Self {
        Self::new("AGENTIC_WRITE_REFUSED", message)
    }
    pub fn gh_not_installed(message: impl Into<String>) -> Self {
        Self::new("GH_NOT_INSTALLED", message)
    }
    pub fn gh_version(message: impl Into<String>) -> Self {
        Self::new("GH_VERSION_TOO_OLD", message)
    }
    pub fn registry(message: impl Into<String>) -> Self {
        Self::new("SKILL_REGISTRY_ERROR", message)
    }
    pub fn injection(message: impl Into<String>) -> Self {
        Self::new("PROMPT_INJECTION_BLOCKED", message)
    }
    pub fn tool_denied(message: impl Into<String>) -> Self {
        Self::new("TOOL_DENIED", message)
    }
    pub fn harness_config(message: impl Into<String>) -> Self {
        Self::new("HARNESS_CONFIG_ERROR", message)
    }
    pub fn sandbox_unavailable(message: impl Into<String>) -> Self {
        Self::new("HARD_SANDBOX_UNAVAILABLE", message)
    }

    // -- auth ------------------------------------------------------------
    pub fn auth_login_failed() -> Self {
        Self::new("AUTH_LOGIN_FAILED", "invalid username or password")
    }
    pub fn auth_bootstrap_complete() -> Self {
        Self::new("AUTH_BOOTSTRAP_COMPLETE", "admin password is already set")
    }
    pub fn auth_locked(retry_after_sec: f64, username: &str) -> Self {
        Self::new(
            "AUTH_ACCOUNT_LOCKED",
            format!("account temporarily locked, retry in {}s", retry_after_sec as i64 + 1),
        )
        .with_details(json!({"retry_after_sec": retry_after_sec, "username": username}))
    }
    pub fn auth_user_exists(username: &str) -> Self {
        Self::new("AUTH_USER_EXISTS", format!("user already exists: {username}"))
            .with_details(json!({"username": username}))
    }
    pub fn auth_user_not_found(username: &str) -> Self {
        Self::new("AUTH_USER_NOT_FOUND", format!("unknown user: {username}"))
            .with_details(json!({"username": username}))
    }
    pub fn auth_last_admin() -> Self {
        Self::new("AUTH_LAST_ADMIN", "cannot modify the last enabled admin")
    }
    pub fn password_policy(message: impl Into<String>) -> Self {
        Self::new("PASSWORD_POLICY", message)
    }

    /// True when this error carries the given code.
    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }

    /// JSON body shape the console parses: `{"code","message","details"}`.
    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message, "details": self.details})
    }
}

impl From<std::io::Error> for HarnessError {
    fn from(err: std::io::Error) -> Self {
        HarnessError::new("IO_ERROR", err.to_string())
    }
}

impl From<String> for HarnessError {
    fn from(message: String) -> Self {
        HarnessError::agentic(message)
    }
}

impl From<serde_json::Error> for HarnessError {
    fn from(err: serde_json::Error) -> Self {
        HarnessError::new("JSON_ERROR", err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, HarnessError>;

/// Raise `AGENTIC_ERROR` unless `value` is non-empty after trimming.
pub fn require_non_empty(value: &str, field_name: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(
            HarnessError::agentic(format!("{field_name} must be a non-empty string"))
                .with_details(json!({"field": field_name})),
        );
    }
    Ok(())
}
