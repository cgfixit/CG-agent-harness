//! Cloud-backed proposer (xAI Grok / Anthropic Claude) behind the six-gate
//! chain, with the sanitized handoff audited AS EGRESS before any bytes leave.
//! Port of `deepagent_github/{chat_client,model_adapter,handoff}.py` without
//! LangChain: two thin HTTP shapes.
//!
//! Gates 1-2 (`agentic.enabled`, `deepagent_github.enabled`) and 6
//! (`--confirm-online`) belong to the caller; 3-4 are asserted by
//! `settings_for`, 5 (key presence) by `CloudProposerClient::new`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::common::audit::{Audit, Redactors};
use crate::common::errors::{HarnessError, Result};
use crate::common::injection::Scanner;

use super::config::{cloud_key_env, DeepAgentConfig};
use super::proposer::ProposerClient;

pub const HANDOFF_EVENT: &str = "agentic_deepagent_cloud_handoff";
const INVOKE_MAX_RETRIES: u32 = 2;
const BACKOFF_BASE_SEC: f64 = 1.0;
const BACKOFF_MAX_SEC: f64 = 30.0;
pub const XAI_ENDPOINT: &str = "https://api.x.ai/v1/chat/completions";
pub const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug, Clone)]
pub struct CloudSettings {
    pub provider: String,
    pub model: String,
    pub max_handoff_chars: usize,
    pub timeout_sec: u64,
}

/// Gates 3 and 4: the provider must be enabled under an enabled master cloud gate.
pub fn settings_for(cfg: &DeepAgentConfig, provider: &str) -> Result<CloudSettings> {
    let p = cfg.cloud_provider(provider).ok_or_else(|| {
        HarnessError::agentic(format!("cloud provider '{provider}' is not enabled"))
            .detail("provider", provider)
            .detail("allow_cloud_providers", cfg.allow_cloud_providers)
    })?;
    Ok(CloudSettings {
        provider: provider.to_string(),
        model: p.model.clone(),
        max_handoff_chars: cfg.max_handoff_chars,
        timeout_sec: cfg.planner_timeout_sec,
    })
}

/// Gate 5 as a pure predicate: key presence only, never a network probe.
pub fn cloud_key_available(provider: &str) -> bool {
    cloud_key_env(provider)
        .and_then(|env| std::env::var(env).ok())
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn require_cloud_key(provider: &str) -> Result<String> {
    let env =
        cloud_key_env(provider).ok_or_else(|| HarnessError::agentic(format!("unknown cloud provider '{provider}'")))?;
    let key = std::env::var(env).unwrap_or_default().trim().to_string();
    if key.is_empty() {
        // The env var's NAME, never its value.
        return Err(HarnessError::agentic(format!("{env} not set"))
            .detail("required_env", env)
            .detail("provider", provider));
    }
    Ok(key)
}

/// Injection-check, cap, redact, and audit an outbound prompt. Hard failure on a match.
pub fn sanitize_handoff(
    prompt: &str,
    provider: &str,
    scanner: &Scanner,
    redactors: &Redactors,
    audit: &Audit,
    max_chars: usize,
) -> Result<String> {
    if prompt.chars().count() > max_chars {
        return Err(HarnessError::injection(format!(
            "outbound prompt exceeds max_handoff_chars ({max_chars})"
        )));
    }
    let hits = scanner.scan(prompt);
    if !hits.is_empty() {
        return Err(HarnessError::injection("outbound prompt matches a banned pattern")
            .detail("pattern_count", hits.len() as u64));
    }
    let redacted = redactors.redact(prompt);
    audit.log(json!({
        "event": HANDOFF_EVENT, "provider": provider, "prompt_sha256": crate::common::sha256_hex(&redacted),
        "prompt_chars": redacted.chars().count(), "context_doc_ids": [], "had_redactions": redacted != prompt,
    }));
    Ok(redacted)
}

impl std::fmt::Debug for CloudProposerClient<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudProposerClient")
            .field("provider", &self.settings.provider)
            .finish()
    }
}

pub struct CloudProposerClient<'a> {
    pub settings: CloudSettings,
    key: String,
    audit: &'a Audit,
    scanner: &'a Scanner,
    redactors: &'a Redactors,
    spend_file: std::path::PathBuf,
    http: reqwest::blocking::Client,
    /// Test hook: replaces the provider endpoint (never read from config).
    pub endpoint_override: Option<String>,
}

impl<'a> CloudProposerClient<'a> {
    pub fn new(
        settings: CloudSettings,
        audit: &'a Audit,
        scanner: &'a Scanner,
        redactors: &'a Redactors,
        spend_file: std::path::PathBuf,
    ) -> Result<Self> {
        let key = require_cloud_key(&settings.provider)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(settings.timeout_sec.max(1)))
            .build()
            .map_err(|e| HarnessError::agentic(format!("cannot build http client: {e}")))?;
        Ok(Self {
            settings,
            key,
            audit,
            scanner,
            redactors,
            spend_file,
            http,
            endpoint_override: None,
        })
    }

    fn endpoint(&self) -> String {
        if let Some(e) = &self.endpoint_override {
            return e.clone();
        }
        if self.settings.provider == "grok" {
            XAI_ENDPOINT.into()
        } else {
            ANTHROPIC_ENDPOINT.into()
        }
    }

    fn record_spend(&self, usage: Option<&Value>, outcome: &str) {
        let (prompt_tokens, completion_tokens) = match (self.settings.provider.as_str(), usage) {
            ("claude", Some(u)) => (u.get("input_tokens").cloned(), u.get("output_tokens").cloned()),
            (_, Some(u)) => (u.get("prompt_tokens").cloned(), u.get("completion_tokens").cloned()),
            _ => (None, None),
        };
        let record = json!({
            "ts": crate::common::iso_now(), "provider": self.settings.provider, "model": self.settings.model,
            "prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens, "source": "agentic", "outcome": outcome,
        });
        if let Some(parent) = self.spend_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.spend_file)
        {
            use std::io::Write;
            let _ = writeln!(f, "{record}");
        }
    }

    fn build_request(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u64,
        temperature: Option<f64>,
    ) -> reqwest::blocking::RequestBuilder {
        if self.settings.provider == "grok" {
            let mut body = json!({
                "model": self.settings.model,
                "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": user_prompt}],
                "max_tokens": max_tokens,
            });
            if let Some(t) = temperature {
                body["temperature"] = json!(t);
            }
            self.http.post(self.endpoint()).bearer_auth(&self.key).json(&body)
        } else {
            // Anthropic rejects a non-default temperature on the Claude 5 family: omit it.
            let body = json!({
                "model": self.settings.model,
                "max_tokens": max_tokens,
                "system": system_prompt,
                "messages": [{"role": "user", "content": user_prompt}],
            });
            self.http
                .post(self.endpoint())
                .header("x-api-key", &self.key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
        }
    }

    fn extract_content(&self, data: &Value) -> Option<String> {
        if self.settings.provider == "grok" {
            data.get("choices")?
                .as_array()?
                .first()?
                .get("message")?
                .get("content")?
                .as_str()
                .map(|s| s.to_string())
        } else {
            let blocks = data.get("content")?.as_array()?;
            let parts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            Some(parts.join("\n"))
        }
    }
}

impl ProposerClient for CloudProposerClient<'_> {
    fn provider(&self) -> &str {
        &self.settings.provider
    }

    fn invoke(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u64,
        temperature: Option<f64>,
    ) -> Result<String> {
        let provider = self.settings.provider.clone();
        let sanitized = sanitize_handoff(
            user_prompt,
            &provider,
            self.scanner,
            self.redactors,
            self.audit,
            self.settings.max_handoff_chars,
        )
        .map_err(|e| {
            HarnessError::agentic(format!(
                "outbound prompt to {provider} blocked by the injection scan: {}",
                e.message
            ))
            .detail("provider", provider.clone())
        })?;
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let req = self.build_request(system_prompt, &sanitized, max_tokens, temperature);
            let outcome = req.send();
            let (retryable, error_type, retry_after, response) = match outcome {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if (200..300).contains(&status) {
                        (false, String::new(), None, Some(resp))
                    } else {
                        let retry_after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .filter(|s| s.is_finite() && *s >= 0.0)
                            .map(|s| s.min(BACKOFF_MAX_SEC));
                        (
                            status == 429 || status >= 500,
                            format!("HTTPStatusError {status}"),
                            retry_after,
                            None,
                        )
                    }
                }
                Err(e) => {
                    if e.is_timeout() {
                        (false, "Timeout".into(), None, None)
                    } else {
                        (e.is_connect() || e.is_request(), "ConnectionError".into(), None, None)
                    }
                }
            };
            if let Some(resp) = response {
                let data: Value = resp.json().map_err(|_| {
                    HarnessError::agentic("cloud proposer invocation failed (ValueError)")
                        .detail("provider", provider.clone())
                })?;
                let usage = data.get("usage").cloned();
                let content = self.extract_content(&data);
                self.record_spend(usage.as_ref(), "ok");
                self.audit.log(json!({"event": "agentic_deepagent_cloud_model_succeeded", "provider": provider, "model": self.settings.model}));
                return content
                    .filter(|c| !c.trim().is_empty())
                    .ok_or_else(|| HarnessError::agentic("cloud proposer returned empty content"));
            }
            if attempts <= INVOKE_MAX_RETRIES && retryable {
                tracing::warn!(
                    "cloud proposer attempt {attempts}/{} failed retryably ({error_type}); backing off",
                    INVOKE_MAX_RETRIES + 1
                );
                let delay = retry_after
                    .unwrap_or_else(|| (BACKOFF_BASE_SEC * 2f64.powi(attempts as i32 - 1)).min(BACKOFF_MAX_SEC));
                std::thread::sleep(Duration::from_secs_f64(delay));
                continue;
            }
            self.audit.log(json!({"event": "agentic_deepagent_cloud_model_failed", "provider": provider, "model": self.settings.model, "error_type": error_type, "attempts": attempts}));
            return Err(
                HarnessError::agentic(format!("cloud proposer invocation failed ({error_type})"))
                    .detail("provider", provider)
                    .detail("error_type", error_type)
                    .detail("attempts", attempts),
            );
        }
    }
}
