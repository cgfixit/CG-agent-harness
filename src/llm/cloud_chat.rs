//! Explicit cloud chat for Grok and Claude. Local chat remains the default.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::openai_chat::{ChatResult, LLM_ERROR_CODE};

const GROK_ENDPOINT: &str = "https://api.x.ai/v1/responses";
const CLAUDE_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SEC: f64 = 90.0;
const DEFAULT_MAX_TOKENS: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Grok,
    Claude,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Claude => "claude",
        }
    }

    fn key_env(self) -> &'static str {
        match self {
            Self::Grok => "GROK_API_KEY",
            Self::Claude => "ANTHROPIC_API_KEY",
        }
    }
}

#[derive(Debug, Clone)]
struct ProviderConfig {
    provider: Provider,
    enabled: bool,
    model: String,
    api_key: String,
}

/// One explicitly selected paid provider. It is never selected as a fallback.
pub struct CloudChat {
    enabled: bool,
    timeout: Duration,
    max_tokens: u64,
    grok: ProviderConfig,
    claude: ProviderConfig,
    client: reqwest::Client,
    inflight: Mutex<BTreeMap<String, CancellationToken>>,
}

impl std::fmt::Debug for CloudChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudChat")
            .field("enabled", &self.enabled)
            .field("grok_model", &self.grok.model)
            .field("claude_model", &self.claude.model)
            .finish()
    }
}

fn err(message: impl Into<String>) -> HarnessError {
    HarnessError::new(LLM_ERROR_CODE, message)
}

fn provider_config(cfg: &AppConfig, provider: Provider, fallback_model: &str) -> Result<ProviderConfig> {
    let root = format!("models.cloud_chat.{}", provider.name());
    let enabled = cfg.flag_is_true(&format!("{root}.enabled"));
    let model = cfg.str_or(&format!("{root}.model"), fallback_model).trim().to_string();
    if enabled && model.is_empty() {
        return Err(HarnessError::config(format!(
            "{root}.model must be non-empty when enabled"
        )));
    }
    Ok(ProviderConfig {
        provider,
        enabled,
        model,
        api_key: std::env::var(provider.key_env()).unwrap_or_default().trim().to_string(),
    })
}

impl CloudChat {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        let timeout_sec = cfg.f64_or("models.cloud_chat.timeout_sec", DEFAULT_TIMEOUT_SEC);
        if !timeout_sec.is_finite() || !(1.0..=720.0).contains(&timeout_sec) {
            return Err(HarnessError::config(
                "models.cloud_chat.timeout_sec must be from 1 to 720",
            ));
        }
        let max_tokens = cfg.u64_or("models.cloud_chat.max_tokens", DEFAULT_MAX_TOKENS);
        if !(1..=32768).contains(&max_tokens) {
            return Err(HarnessError::config(
                "models.cloud_chat.max_tokens must be from 1 to 32768",
            ));
        }
        Ok(Self {
            enabled: cfg.flag_is_true("models.cloud_chat.enabled"),
            timeout: Duration::from_secs_f64(timeout_sec),
            max_tokens,
            grok: provider_config(cfg, Provider::Grok, "grok-4.6")?,
            claude: provider_config(cfg, Provider::Claude, "claude-sonnet-5")?,
            // Do not delegate paid credentials to ambient HTTP(S)_PROXY.
            client: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs_f64(timeout_sec))
                .build()
                .map_err(|_| HarnessError::config("cannot construct cloud chat client"))?,
            inflight: Mutex::new(BTreeMap::new()),
        })
    }

    fn configured(&self, selection: &str) -> Option<&ProviderConfig> {
        let selected = selection.trim();
        [(&self.grok, Provider::Grok), (&self.claude, Provider::Claude)]
            .into_iter()
            .find_map(|(config, provider)| {
                (selected.eq_ignore_ascii_case(provider.name()) || selected == config.model).then_some(config)
            })
    }

    pub fn is_cloud_selection(&self, selection: &str) -> bool {
        self.configured(selection).is_some()
    }

    pub fn provider_name(&self, selection: &str) -> Option<&'static str> {
        self.configured(selection).map(|config| config.provider.name())
    }

    pub fn timeout_sec(&self) -> f64 {
        self.timeout.as_secs_f64()
    }

    pub fn abort_in_flight(&self) {
        for (_, token) in std::mem::take(&mut *self.inflight.lock().unwrap_or_else(|p| p.into_inner())) {
            token.cancel();
        }
    }

    pub async fn chat(&self, selection: &str, message: &str) -> Result<ChatResult> {
        let config = self
            .configured(selection)
            .ok_or_else(|| err("selected cloud chat model is not configured"))?;
        if !self.enabled || !config.enabled {
            return Err(err("cloud chat is disabled by local configuration"));
        }
        if config.api_key.is_empty() {
            return Err(err(format!(
                "{} is not configured; save it in API Keys and restart",
                config.provider.key_env()
            )));
        }
        let request_id = crate::common::random_urlsafe(16);
        let cancel = CancellationToken::new();
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(request_id.clone(), cancel.clone());
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(err("cloud chat request cancelled")),
            result = self.request(config, message) => result,
        };
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&request_id);
        result
    }

    async fn request(&self, config: &ProviderConfig, message: &str) -> Result<ChatResult> {
        let response = match config.provider {
            Provider::Grok => self
                .client
                .post(GROK_ENDPOINT)
                .bearer_auth(&config.api_key)
                .json(&json!({
                    "model": config.model,
                    "input": [{"role": "user", "content": message}],
                    "max_output_tokens": self.max_tokens,
                    "store": false,
                }))
                .send()
                .await,
            Provider::Claude => self
                .client
                .post(CLAUDE_ENDPOINT)
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&json!({"model":config.model,"max_tokens":self.max_tokens,"messages":[{"role":"user","content":message}]}))
                .send()
                .await,
        }
        .map_err(|_| err(format!("{} cloud chat request failed", config.provider.name())))?;
        let status = response.status();
        if !status.is_success() {
            return Err(err(format!(
                "{} cloud chat rejected the request (HTTP {})",
                config.provider.name(),
                status.as_u16()
            )));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| err(format!("{} cloud chat returned malformed JSON", config.provider.name())))?;
        let (body_text, prompt_tokens, completion_tokens) = match config.provider {
            Provider::Grok => parse_grok(&body)?,
            Provider::Claude => parse_claude(&body)?,
        };
        Ok(ChatResult {
            body_text,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&config.model)
                .to_string(),
            prompt_tokens,
            completion_tokens,
            usage_reported: true,
        })
    }
}

fn parse_grok(body: &Value) -> Result<(String, u64, u64)> {
    let text = body
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|content| content.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return Err(err("Grok cloud chat returned no text"));
    }
    Ok((
        text,
        body.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0),
        body.pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    ))
}

fn parse_claude(body: &Value) -> Result<(String, u64, u64)> {
    let text = body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return Err(err("Claude cloud chat returned no text"));
    }
    Ok((
        text,
        body.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0),
        body.pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parsers_accept_only_nonempty_text() {
        let grok = json!({
            "output": [{"type":"message","content":[{"type":"output_text","text":"hi"}]}],
            "usage":{"input_tokens":2,"output_tokens":3}
        });
        assert_eq!(parse_grok(&grok).unwrap(), ("hi".into(), 2, 3));
        let claude = json!({"content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":4,"output_tokens":5}});
        assert_eq!(parse_claude(&claude).unwrap(), ("hello".into(), 4, 5));
        assert!(parse_grok(&json!({})).is_err());
        assert!(parse_claude(&json!({"content":[]})).is_err());
    }

    #[test]
    fn shipped_config_selects_only_the_named_cloud_models() {
        let cfg = AppConfig::from_str(AppConfig::embedded_default(), std::path::Path::new("config.yaml")).unwrap();
        let cloud = CloudChat::from_config(&cfg).unwrap();
        assert!(cloud.is_cloud_selection("grok"));
        assert!(cloud.is_cloud_selection("grok-4.6"));
        assert!(cloud.is_cloud_selection("claude"));
        assert!(cloud.is_cloud_selection("claude-sonnet-5"));
        assert!(!cloud.is_cloud_selection("qwen3.8:27b-mlx"));
    }
}
