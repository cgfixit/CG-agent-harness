//! Planner-model contract and the local OpenAI-compatible proposer.
//! Port of `agentic/harness_optimizer/model_adapter.py::LocalProposerClient`.
//! The child process is synchronous; a blocking reqwest client is correct here.

use std::time::Duration;

use serde_json::{json, Value};

use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};

/// What the loop reads off a planner response: only `content`.
pub trait ProposerClient {
    fn invoke(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u64,
        temperature: Option<f64>,
    ) -> Result<String>;
    fn provider(&self) -> &str;
}

pub struct LocalProposerClient<'a> {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: u64,
    audit: &'a Audit,
    http: reqwest::blocking::Client,
}

impl<'a> LocalProposerClient<'a> {
    pub fn new(
        audit: &'a Audit,
        base_url: &str,
        model: &str,
        timeout_sec: u64,
        api_key: &str,
        reasoning_effort: Option<String>,
    ) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(timeout_sec.max(1)))
            .build()
            .map_err(|e| HarnessError::agentic(format!("cannot build http client: {e}")))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key: api_key.trim().to_string(),
            reasoning_effort,
            timeout_sec,
            audit,
            http,
        })
    }

    fn failure_detail(&self, e: &reqwest::Error) -> String {
        if e.is_timeout() {
            format!(
                "ReadTimeout after {}s; raise agentic.deepagent_github.planner_timeout_sec if the model needs longer",
                self.timeout_sec
            )
        } else if e.is_connect() {
            "ConnectError".into()
        } else {
            "TransportError".into()
        }
    }
}

impl ProposerClient for LocalProposerClient<'_> {
    fn provider(&self) -> &str {
        "ollama"
    }

    fn invoke(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: u64,
        temperature: Option<f64>,
    ) -> Result<String> {
        if self.model.trim().is_empty() {
            return Err(HarnessError::agentic(
                "local proposer model must be configured before invocation",
            ));
        }
        self.audit.log(json!({
            "event": "agentic_harness_proposer_model_invoked", "provider": "ollama", "model": self.model,
            "system_prompt_hash": crate::common::sha256_hex(system_prompt), "user_prompt_hash": crate::common::sha256_hex(user_prompt),
        }));
        let mut payload = json!({
            "model": self.model,
            "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": user_prompt}],
            "max_tokens": max_tokens,
            "temperature": temperature.unwrap_or(0.0),
        });
        if let Some(effort) = &self.reasoning_effort {
            payload["reasoning_effort"] = json!(effort);
        }
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&payload);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let failed = |error_type: &str| {
            self.audit.log(json!({"event": "agentic_harness_proposer_model_failed", "provider": "ollama", "model": self.model, "error_type": error_type}));
        };
        let resp = req.send().map_err(|e| {
            let detail = self.failure_detail(&e);
            failed(&detail);
            HarnessError::agentic(format!("local proposer invocation failed ({detail})")).detail("error_type", detail)
        })?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            failed(&format!("HTTPStatusError {code}"));
            return Err(
                HarnessError::agentic(format!("local proposer invocation failed (HTTP {code})"))
                    .detail("error_type", "HTTPStatusError"),
            );
        }
        let data: Value = resp.json().map_err(|_| {
            failed("ValueError");
            HarnessError::agentic("local proposer invocation failed (ValueError)")
        })?;
        let content = data
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
        match content {
            Some(c) if !c.trim().is_empty() => {
                self.audit.log(json!({"event": "agentic_harness_proposer_model_succeeded", "provider": "ollama", "model": self.model}));
                Ok(c)
            }
            _ => {
                failed("AgenticError");
                Err(HarnessError::agentic("local proposer returned empty content"))
            }
        }
    }
}
