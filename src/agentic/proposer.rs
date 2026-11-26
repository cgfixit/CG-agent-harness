//! Planner-model contract and the local OpenAI-compatible proposer.
//! Port of `agentic/harness_optimizer/model_adapter.py::LocalProposerClient`.
//! The child process is synchronous; a blocking reqwest client is correct here.

use std::io::Read;
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
    /// True when `invoke` sends the prompt off-machine.
    ///
    /// Distinguishes the two concrete clients `run_real_repo_loop` actually
    /// receives: `LocalProposerClient` (always `"ollama"`) vs
    /// `CloudProposerClient` (`CLOUD_PROVIDERS`: `grok`, `claude`). Test
    /// doubles inherit this default, so a mock that reports a cloud provider
    /// name is treated as cloud.
    fn is_cloud(&self) -> bool {
        super::config::CLOUD_PROVIDERS.contains(&self.provider())
    }
}

pub struct LocalProposerClient<'a> {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: u64,
    /// The timeout came from `--planner-timeout-sec` (the server's measured
    /// value), so the configured setting does not apply to this run.
    pub timeout_measured: bool,
    audit: &'a Audit,
    http: reqwest::blocking::Client,
    /// Set for an Ollama planner: the loopback native origin whose `/api/show`
    /// every invocation reads first, so an alias of a cloud model is refused.
    ollama_native: Option<String>,
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
            .dns_resolver(std::sync::Arc::new(crate::llm::backend::LoopbackOnlyDns))
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
            timeout_measured: false,
            audit,
            http,
            ollama_native: None,
        })
    }

    /// The planner endpoint is Ollama: refuse, before every invocation, a model
    /// whose `/api/show` names a remote host or cannot be read.
    pub fn refuse_remote_ollama_models(&mut self) -> Result<()> {
        let native = crate::llm::ollama::native_base_url(&self.base_url).ok_or_else(|| {
            HarnessError::agentic("agentic.deepagent_github.base_url is not a plain loopback Ollama origin")
        })?;
        self.ollama_native = Some(native);
        Ok(())
    }

    /// Ollama routes by the remote host stored in the model's config, not its
    /// name: a Modelfile `FROM glm-4.6:cloud` alias forwards like the tag.
    fn refuse_unless_shown_local(&self) -> Result<()> {
        use crate::llm::ollama::{refuse_unless_shown_local, show_payload, ShowRead};
        let Some(native) = &self.ollama_native else {
            return Ok(());
        };
        let mut request = self
            .http
            .post(format!("{native}/api/show"))
            .json(&show_payload(&self.model));
        // The same origin and credential the planner's chat requests use.
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = request.send();
        let cap = crate::llm::MAX_RESPONSE_BYTES;
        let mut bytes = Vec::new();
        let body;
        let read = match response {
            Ok(response) if response.status().is_success() => {
                match response.take(cap as u64 + 1).read_to_end(&mut bytes) {
                    Ok(_) if bytes.len() <= cap => match serde_json::from_slice::<Value>(&bytes) {
                        Ok(value) => {
                            body = value;
                            ShowRead::Body(&body)
                        }
                        Err(_) => ShowRead::Failed,
                    },
                    _ => ShowRead::Failed,
                }
            }
            Ok(response) => ShowRead::Status(response.status().as_u16()),
            Err(_) => ShowRead::Failed,
        };
        refuse_unless_shown_local(read).map_err(|err| {
            HarnessError::agentic(format!("the local proposer model is refused: {}", err.message))
                .detail("reason", err.details["reason"].clone())
        })
    }

    fn failure_detail(&self, e: &reqwest::Error) -> String {
        if e.is_timeout() && self.timeout_measured {
            format!(
                "ReadTimeout after {}s, a timeout models.local_llm.auto_tune derived from this model's measured speed; \
                 run /model use again to re-measure, or turn auto_tune off to use agentic.deepagent_github.planner_timeout_sec",
                self.timeout_sec
            )
        } else if e.is_timeout() {
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

    fn is_cloud(&self) -> bool {
        false
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
        // `real-repo-run` refuses this first; every other caller is stopped here.
        if crate::llm::ollama::cloud_tag(&self.model) {
            return Err(HarnessError::agentic(
                "the local proposer model is an Ollama cloud model (a :cloud or -cloud tag), which sends the \
                 planner's prompt to ollama.com",
            ));
        }
        self.refuse_unless_shown_local()?;
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
        // Bound model-controlled JSON independently of the requested tokens:
        // the cap the cloud proposer and local chat already apply.
        let cap = crate::llm::MAX_RESPONSE_BYTES;
        let mut body = Vec::new();
        if resp.take(cap as u64 + 1).read_to_end(&mut body).is_err() {
            failed("ValueError");
            return Err(HarnessError::agentic("local proposer invocation failed (ValueError)"));
        }
        if body.len() > cap {
            failed("ResponseTooLarge");
            return Err(
                HarnessError::agentic("local proposer response exceeds limit; this response was not applied")
                    .detail("error_type", "ResponseTooLarge"),
            );
        }
        let data: Value = serde_json::from_slice(&body).map_err(|_| {
            failed("ValueError");
            HarnessError::agentic("local proposer invocation failed (ValueError)")
        })?;
        let choice = data.get("choices").and_then(Value::as_array).and_then(|a| a.first());
        match choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str) {
            Some("stop") => {}
            Some("length") => {
                failed("IncompleteOutput");
                return Err(HarnessError::config("local planner output was truncated; this response was not applied. Reduce the task or adjust agentic.deepagent_github.planner_max_tokens before retrying"));
            }
            _ => {
                failed("IncompleteOutput");
                return Err(HarnessError::config("local planner did not report a normal completion (finish_reason=stop required); this response was not applied"));
            }
        }
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
