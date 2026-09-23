//! Explicit cloud chat for Grok and Claude. Local chat remains the default.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use std::path::PathBuf;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::openai_chat::{ChatResult, LLM_ERROR_CODE};
use crate::llm::spend::{self, UsageTokens};

const GROK_ENDPOINT: &str = "https://api.x.ai/v1/responses";
const CLAUDE_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SEC: f64 = 90.0;
const DEFAULT_MAX_TOKENS: u64 = 4096;
/// Body bound for a paid cloud-provider reply; every cloud client uses it.
pub const MAX_RESPONSE_BYTES: usize = 4_194_304;

/// Pre-call estimate, never measured usage or a guaranteed invoice ceiling.
#[derive(Debug, serde::Serialize)]
pub struct SpendPrediction {
    pub provider: &'static str,
    pub model: String,
    pub input_tokens: u64,
    pub reserved_output_tokens: u64,
    pub estimate_source: &'static str,
    pub usd: Option<f64>,
    pub priced_as_of: &'static str,
    pub rates_stale: bool,
    pub max_usd_per_call: Option<f64>,
    pub budget_applies: bool,
    pub budget_exceeded: bool,
}

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
    endpoint: String,
}

/// One explicitly selected paid provider. It is never selected as a fallback.
pub struct CloudChat {
    enabled: bool,
    timeout: Duration,
    max_tokens: u64,
    count_timeout: Duration,
    max_usd_per_call: Option<f64>,
    budget_on_heuristic: bool,
    grok: ProviderConfig,
    claude: ProviderConfig,
    client: reqwest::Client,
    inflight: Mutex<BTreeMap<String, CancellationToken>>,
    spend_file: Option<PathBuf>,
    spend_max_bytes: u64,
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
        endpoint: match provider {
            Provider::Grok => GROK_ENDPOINT,
            Provider::Claude => CLAUDE_ENDPOINT,
        }
        .into(),
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
        let max_usd_per_call = match cfg.get("models.cloud_chat.max_usd_per_call") {
            None | Some(serde_yaml_ng::Value::Null) => None,
            Some(value) => Some(value.as_f64().filter(|n| n.is_finite() && *n > 0.0).ok_or_else(|| {
                HarnessError::config("models.cloud_chat.max_usd_per_call must be null or a finite positive number")
            })?),
        };
        let count_seconds = match cfg.get("models.cloud_chat.count_timeout_sec") {
            None => 2.0,
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && (0.1..=10.0).contains(n))
                .ok_or_else(|| {
                    HarnessError::config("models.cloud_chat.count_timeout_sec must be from 0.1 to 10 seconds")
                })?,
        };
        Ok(Self {
            enabled: cfg.flag_is_true("models.cloud_chat.enabled"),
            timeout: Duration::from_secs_f64(timeout_sec),
            max_tokens,
            count_timeout: Duration::from_secs_f64(count_seconds),
            max_usd_per_call,
            budget_on_heuristic: cfg.flag_is_true("models.cloud_chat.budget_on_heuristic"),
            grok: provider_config(cfg, Provider::Grok, "grok-4.6")?,
            claude: provider_config(cfg, Provider::Claude, "claude-sonnet-5")?,
            // Do not delegate paid credentials to ambient HTTP(S)_PROXY.
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs_f64(timeout_sec))
                .build()
                .map_err(|_| HarnessError::config("cannot construct cloud chat client"))?,
            inflight: Mutex::new(BTreeMap::new()),
            spend_file: None,
            spend_max_bytes: crate::common::bounded_log::limit(cfg),
        })
    }

    pub fn attach_spend(&mut self, path: PathBuf) {
        self.spend_file = Some(path);
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
        (self.timeout + self.count_timeout).as_secs_f64()
    }

    pub fn abort_in_flight(&self) {
        for (_, token) in std::mem::take(&mut *self.inflight.lock().unwrap_or_else(|p| p.into_inner())) {
            token.cancel();
        }
    }

    pub async fn chat(&self, selection: &str, message: &str) -> Result<ChatResult> {
        self.chat_with_source(selection, message, "chat").await
    }

    pub async fn chat_with_source(&self, selection: &str, message: &str, source: &str) -> Result<ChatResult> {
        let config = self.ready(selection)?;
        self.cancellable(self.request(config, message, source)).await
    }

    /// Counts only the explicitly supplied cloud message, without generating or recording spend.
    pub async fn predict(&self, selection: &str, message: &str) -> Result<SpendPrediction> {
        let config = self.ready(selection)?;
        self.cancellable(async { Ok(self.predict_request(config, message).await) })
            .await
    }

    fn ready(&self, selection: &str) -> Result<&ProviderConfig> {
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
        Ok(config)
    }

    async fn cancellable<T>(&self, operation: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        let request_id = crate::common::random_urlsafe(16);
        let cancel = CancellationToken::new();
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(request_id.clone(), cancel.clone());
        struct Call<'a> {
            chat: &'a CloudChat,
            id: String,
        }
        impl Drop for Call<'_> {
            fn drop(&mut self) {
                self.chat
                    .inflight
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&self.id);
            }
        }
        let _call = Call {
            chat: self,
            id: request_id,
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(err("cloud chat request cancelled")),
            result = operation => result,
        }
    }

    async fn count_input_tokens(&self, config: &ProviderConfig, message: &str) -> Option<u64> {
        if config.provider != Provider::Claude {
            return None;
        }
        // One deadline covers headers, a trickled body, and decoding. Failures
        // discard provider bodies and use the explicitly labelled heuristic.
        tokio::time::timeout(self.count_timeout, async {
            let mut response = self
                .client
                .post(format!("{}/count_tokens", config.endpoint.trim_end_matches('/')))
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .timeout(self.count_timeout)
                .json(&claude_input(config, message))
                .send()
                .await
                .ok()?;
            if !response.status().is_success()
                || response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
            {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value = serde_json::from_slice(&bytes).ok()?;
            body.get("input_tokens")?.as_u64().filter(|n| *n > 0)
        })
        .await
        .ok()
        .flatten()
    }

    async fn predict_request(&self, config: &ProviderConfig, message: &str) -> SpendPrediction {
        let counted = self.count_input_tokens(config, message).await;
        // ponytail: cloud bytes/4 is a rough English estimate, including on CJK;
        // use a provider counter when available, never borrow the local Qwen calibration.
        let input_tokens = counted.unwrap_or_else(|| message.len().div_ceil(4) as u64);
        let estimate = spend::estimate_usd(
            &config.model,
            &UsageTokens {
                input_tokens: Some(input_tokens),
                output_tokens: Some(self.max_tokens),
                ..UsageTokens::default()
            },
            config.provider.name(),
        );
        let budget_applies = self.max_usd_per_call.is_some()
            && estimate.usd.is_some()
            && (counted.is_some() || self.budget_on_heuristic);
        SpendPrediction {
            provider: config.provider.name(),
            model: config.model.clone(),
            input_tokens,
            reserved_output_tokens: self.max_tokens,
            estimate_source: if counted.is_some() { "vendor_count" } else { "heuristic" },
            usd: estimate.usd,
            priced_as_of: estimate.priced_as_of,
            rates_stale: spend::rates_are_stale(None),
            max_usd_per_call: self.max_usd_per_call,
            budget_applies,
            budget_exceeded: budget_applies
                && estimate
                    .usd
                    .zip(self.max_usd_per_call)
                    .is_some_and(|(usd, cap)| usd > cap),
        }
    }

    async fn request(&self, config: &ProviderConfig, message: &str, source: &str) -> Result<ChatResult> {
        let prediction = self.predict_request(config, message).await;
        if prediction.budget_exceeded {
            return Err(HarnessError::new(
                "CLOUD_CHAT_BUDGET",
                "cloud chat estimate exceeds max_usd_per_call; no generation was sent",
            )
            .detail("prediction", json!(prediction)));
        }
        let response = match config.provider {
            Provider::Grok => {
                self.client
                    .post(&config.endpoint)
                    .bearer_auth(&config.api_key)
                    .json(&json!({
                        "model": config.model,
                        "input": [{"role": "user", "content": message}],
                        "max_output_tokens": self.max_tokens,
                        "store": false,
                    }))
                    .send()
                    .await
            }
            Provider::Claude => {
                self.client
                    .post(&config.endpoint)
                    .header("x-api-key", &config.api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&{
                        let mut body = claude_input(config, message);
                        body["max_tokens"] = json!(self.max_tokens);
                        body
                    })
                    .send()
                    .await
            }
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
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| err(format!("{} cloud chat returned malformed JSON", config.provider.name())))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                self.record_cloud_spend(
                    config,
                    &json!({}),
                    spend::UsageTokens::default(),
                    source,
                    Some("failed_after_billing"),
                );
                return Err(err(format!(
                    "{} cloud chat response exceeds limit",
                    config.provider.name()
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                self.record_cloud_spend(
                    config,
                    &json!({}),
                    spend::UsageTokens::default(),
                    source,
                    Some("failed_after_billing"),
                );
                return Err(err(format!(
                    "{} cloud chat returned malformed JSON",
                    config.provider.name()
                )));
            }
        };
        let tokens = match config.provider {
            Provider::Grok => spend::parse_grok_usage(body.get("usage")),
            Provider::Claude => spend::parse_claude_usage(body.get("usage")),
        };
        let text = match config.provider {
            Provider::Grok => grok_text(&body),
            Provider::Claude => claude_text(&body),
        };
        let outcome = if text.is_err() {
            Some("failed_after_billing")
        } else {
            None
        };
        self.record_cloud_spend(config, &body, tokens.clone(), source, outcome);
        let body_text = text?;
        Ok(ChatResult {
            body_text,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&config.model)
                .to_string(),
            prompt_tokens: tokens.input_tokens.unwrap_or(0),
            completion_tokens: tokens.output_tokens.unwrap_or(0),
            usage_reported: tokens.usage_reported(),
            initial_prompt_tokens: None,
        })
    }

    fn record_cloud_spend(
        &self,
        config: &ProviderConfig,
        body: &Value,
        tokens: UsageTokens,
        source: &str,
        outcome: Option<&str>,
    ) {
        let Some(path) = &self.spend_file else {
            return;
        };
        let served = body.get("model").and_then(Value::as_str);
        spend::record(
            None,
            path,
            self.spend_max_bytes,
            spend::SpendEvent {
                provider: config.provider.name(),
                model: &config.model,
                served_model: served,
                source,
                tokens,
                outcome,
            },
        );
    }
}

fn claude_input(config: &ProviderConfig, message: &str) -> Value {
    json!({"model":config.model,"messages":[{"role":"user","content":message}]})
}

fn grok_text(body: &Value) -> Result<String> {
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
    Ok(text)
}

#[cfg(test)]
fn parse_grok(body: &Value) -> Result<(String, u64, u64)> {
    let text = grok_text(body)?;
    let tokens = spend::parse_grok_usage(body.get("usage"));
    Ok((
        text,
        tokens.input_tokens.unwrap_or(0),
        tokens.output_tokens.unwrap_or(0),
    ))
}

fn claude_text(body: &Value) -> Result<String> {
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
    Ok(text)
}

#[cfg(test)]
fn parse_claude(body: &Value) -> Result<(String, u64, u64)> {
    let text = claude_text(body)?;
    let tokens = spend::parse_claude_usage(body.get("usage"));
    Ok((
        text,
        tokens.input_tokens.unwrap_or(0),
        tokens.output_tokens.unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Response, StatusCode};
    use axum::response::Redirect;
    use axum::routing::post;
    use axum::Router;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn enabled_chat() -> CloudChat {
        let cfg = AppConfig::from_str(
            "models:\n  cloud_chat:\n    enabled: true\n    timeout_sec: 5\n    max_tokens: 16\n    grok: { enabled: true, model: grok-test }\n    claude: { enabled: true, model: claude-test }\n",
            std::path::Path::new("config.yaml"),
        )
        .unwrap();
        let mut chat = CloudChat::from_config(&cfg).unwrap();
        chat.grok.api_key = "grok-test-key".into();
        chat.claude.api_key = "claude-test-key".into();
        chat
    }

    #[tokio::test]
    async fn configured_heuristic_budget_refuses_before_any_generation_or_ledger_write() {
        let hits = Arc::new(AtomicUsize::new(0));
        let generated = hits.clone();
        let (origin, server) = serve(Router::new().route(
            "/responses",
            post(move || {
                generated.fetch_add(1, Ordering::SeqCst);
                async { axum::Json(json!({"output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":1,"output_tokens":1}})) }
            }),
        )).await;
        let cfg = AppConfig::from_str(
            "models:\n  cloud_chat:\n    enabled: true\n    max_tokens: 100\n    max_usd_per_call: 0.0001\n    budget_on_heuristic: true\n    grok: {enabled: true, model: grok-4.6}\n",
            std::path::Path::new("config.yaml"),
        ).unwrap();
        let mut chat = CloudChat::from_config(&cfg).unwrap();
        chat.grok.api_key = "fixture-only".into();
        chat.grok.endpoint = format!("{origin}/responses");
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("spend.jsonl");
        chat.attach_spend(ledger.clone());
        let result = chat.chat("grok", "中文预算 SECRET_PROMPT").await;
        server.abort();
        assert_eq!(result.unwrap_err().code, "CLOUD_CHAT_BUDGET");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(!ledger.exists());
    }

    #[test]
    fn budget_configuration_is_explicit_and_invalid_caps_fail_closed() {
        let config = |key: &str, value: &str| {
            AppConfig::from_str(
                &format!("models:\n  cloud_chat:\n    {key}: {value}\n"),
                std::path::Path::new("config.yaml"),
            )
            .unwrap()
        };
        for value in ["0", "-1", ".inf", ".nan", "true", "'0.1'", "[]"] {
            assert!(
                CloudChat::from_config(&config("max_usd_per_call", value)).is_err(),
                "{value}"
            );
        }
        for value in ["0", "11", ".nan", "null", "'2'"] {
            assert!(
                CloudChat::from_config(&config("count_timeout_sec", value)).is_err(),
                "{value}"
            );
        }
        assert!(CloudChat::from_config(&config("max_usd_per_call", "null"))
            .unwrap()
            .max_usd_per_call
            .is_none());
        assert_eq!(
            CloudChat::from_config(&config("max_usd_per_call", "0.1"))
                .unwrap()
                .max_usd_per_call,
            Some(0.1)
        );
        assert!(
            !CloudChat::from_config(&config("budget_on_heuristic", "'true'"))
                .unwrap()
                .budget_on_heuristic
        );
        let seed = AppConfig::from_str(AppConfig::embedded_default(), std::path::Path::new("config.yaml")).unwrap();
        let chat = CloudChat::from_config(&seed).unwrap();
        assert!(chat.max_usd_per_call.is_none());
        assert!(!chat.budget_on_heuristic);
    }

    #[tokio::test]
    async fn prediction_reserves_output_keeps_cjk_heuristic_explicit_and_unknown_models_unpriced() {
        let mut chat = enabled_chat();
        chat.grok.model = "grok-4.6".into();
        chat.max_usd_per_call = Some(0.000001);
        // An invalid endpoint proves Grok prediction makes no network request.
        chat.grok.endpoint = "invalid endpoint".into();
        let message = "中文budget";
        let prediction = chat.predict("grok", message).await.unwrap();
        assert_eq!(prediction.input_tokens, 3); // Two Han characters (6 bytes) plus six ASCII bytes.
        assert_eq!(prediction.reserved_output_tokens, 16);
        assert_eq!(prediction.estimate_source, "heuristic");
        assert!((prediction.usd.unwrap() - (3.0 * 2.0 + 16.0 * 6.0) / 1_000_000.0).abs() < 1e-12);
        assert!(!prediction.budget_applies);
        assert!(!prediction.budget_exceeded);
        chat.grok.model = "grok-unknown".into();
        chat.budget_on_heuristic = true;
        let unknown = chat.predict("grok", message).await.unwrap();
        assert!(unknown.usd.is_none());
        assert!(!unknown.budget_applies);
        assert!(!unknown.budget_exceeded);
        assert!(chat.predict("qwen3.8:27b-mlx", message).await.is_err());
        chat.grok.api_key.clear();
        assert!(chat.predict("grok", message).await.is_err());
    }

    #[tokio::test]
    async fn claude_counter_prices_identical_input_and_refuses_only_above_the_cap() {
        let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let counted = requests.clone();
        let generated = requests.clone();
        let (origin, server) = serve(Router::new()
            .route("/messages/count_tokens", post(move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                assert_eq!(headers["x-api-key"], "claude-test-key");
                assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
                counted.lock().unwrap().push(("count".into(), body));
                async { axum::Json(json!({"input_tokens":25})) }
            }))
            .route("/messages", post(move |axum::Json(body): axum::Json<Value>| {
                generated.lock().unwrap().push(("generate".into(), body));
                async { axum::Json(json!({"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":25,"output_tokens":1}})) }
            }))).await;
        let mut chat = enabled_chat();
        chat.claude.model = "claude-sonnet-5".into();
        chat.claude.endpoint = format!("{origin}/messages");
        chat.max_usd_per_call = Some(0.0001);
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("spend.jsonl");
        chat.attach_spend(ledger.clone());
        let prediction = chat.predict("claude", "中文 private draft").await.unwrap();
        assert_eq!(prediction.estimate_source, "vendor_count");
        assert_eq!(prediction.input_tokens, 25);
        assert!((prediction.usd.unwrap() - 0.00021).abs() < 1e-12);
        assert!(prediction.budget_exceeded);
        assert!(!ledger.exists());
        let refused = chat.chat("claude", "中文 private draft").await.unwrap_err();
        assert_eq!(refused.code, "CLOUD_CHAT_BUDGET");
        assert!(!serde_json::to_string(&refused.details)
            .unwrap()
            .contains("private draft"));
        assert!(requests.lock().unwrap().iter().all(|(kind, _)| kind == "count"));
        assert!(!ledger.exists());
        chat.max_usd_per_call = prediction.usd; // Equal to cap is allowed.
        assert_eq!(chat.chat("claude", "中文 private draft").await.unwrap().body_text, "ok");
        let requests = requests.lock().unwrap();
        let count = &requests[0].1;
        let generation = &requests.last().unwrap().1;
        assert_eq!(
            count,
            &json!({"model":"claude-sonnet-5","messages":[{"role":"user","content":"中文 private draft"}]})
        );
        assert_eq!(count["messages"], generation["messages"]);
        assert_eq!(count["model"], generation["model"]);
        assert_eq!(generation["max_tokens"], 16);
        let recorded = std::fs::read_to_string(ledger).unwrap();
        assert_eq!(recorded.lines().count(), 1);
        assert!(!recorded.contains("private draft"));
        assert!(!recorded.contains("claude-test-key"));
        server.abort();
    }

    #[tokio::test]
    async fn counter_failures_fall_back_without_following_redirects_or_blocking_on_a_trickled_body() {
        let mode = Arc::new(AtomicUsize::new(0));
        let counter_mode = mode.clone();
        let redirected = Arc::new(AtomicUsize::new(0));
        let sink = redirected.clone();
        let (origin, server) = serve(
            Router::new()
                .route(
                    "/messages/count_tokens",
                    post(move || {
                        let mode = counter_mode.load(Ordering::SeqCst);
                        async move {
                            match mode {
                                0 => Response::builder()
                                    .status(500)
                                    .body(Body::from("PRIVATE_PROVIDER_ERROR"))
                                    .unwrap(),
                                1 => Response::new(Body::from("bad JSON")),
                                2 => Response::new(Body::from(r#"{"input_tokens":"25"}"#)),
                                3 => Response::new(Body::from(r#"{"input_tokens":0}"#)),
                                4 => Response::builder()
                                    .status(307)
                                    .header(header::LOCATION, "/sink")
                                    .body(Body::empty())
                                    .unwrap(),
                                5 => Response::new(Body::from_stream(futures_util::stream::iter([
                                    Ok::<_, std::io::Error>(vec![b' '; MAX_RESPONSE_BYTES / 2]),
                                    Ok(vec![b' '; MAX_RESPONSE_BYTES / 2 + 1]),
                                ]))),
                                6 => Response::new(Body::from_stream(futures_util::stream::unfold(
                                    false,
                                    |sent| async move {
                                        if sent {
                                            std::future::pending::<()>().await;
                                        }
                                        Some((Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{")), true))
                                    },
                                ))),
                                _ => std::future::pending::<Response<Body>>().await,
                            }
                        }
                    }),
                )
                .route(
                    "/sink",
                    post(move || {
                        sink.fetch_add(1, Ordering::SeqCst);
                        async { StatusCode::OK }
                    }),
                ),
        )
        .await;
        let mut chat = enabled_chat();
        chat.claude.model = "claude-sonnet-5".into();
        chat.claude.endpoint = format!("{origin}/messages");
        chat.count_timeout = Duration::from_millis(100);
        chat.max_usd_per_call = Some(0.000001);
        for case in 0..8 {
            mode.store(case, Ordering::SeqCst);
            let prediction = tokio::time::timeout(Duration::from_secs(2), chat.predict("claude", "中文abcdef"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(prediction.estimate_source, "heuristic", "case {case}");
            assert_eq!(prediction.input_tokens, 3);
            assert!(!prediction.budget_applies);
            assert!(!serde_json::to_string(&prediction)
                .unwrap()
                .contains("PRIVATE_PROVIDER_ERROR"));
        }
        assert_eq!(redirected.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn cancelling_during_token_count_never_generates_and_unregisters_the_call() {
        let counted = Arc::new(tokio::sync::Notify::new());
        let arrived = counted.clone();
        let generated = Arc::new(AtomicUsize::new(0));
        let hits = generated.clone();
        let (origin, server) = serve(
            Router::new()
                .route(
                    "/messages/count_tokens",
                    post(move || {
                        arrived.notify_one();
                        async { std::future::pending::<StatusCode>().await }
                    }),
                )
                .route(
                    "/messages",
                    post(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                        async { StatusCode::OK }
                    }),
                ),
        )
        .await;
        let mut chat = enabled_chat();
        chat.claude.endpoint = format!("{origin}/messages");
        let chat = Arc::new(chat);
        let call_chat = chat.clone();
        let call = tokio::spawn(async move { call_chat.chat("claude", "private").await });
        tokio::time::timeout(Duration::from_secs(1), counted.notified())
            .await
            .unwrap();
        chat.abort_in_flight();
        assert!(call.await.unwrap().unwrap_err().message.contains("cancelled"));
        assert!(chat.inflight.lock().unwrap().is_empty());
        assert_eq!(generated.load(Ordering::SeqCst), 0);
        assert!(chat.predict("grok", "next request").await.is_ok());
        server.abort();
    }

    async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
        let origin = format!("http://{}", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this test-only provider has no real credentials and binds only to loopback.
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (origin, task)
    }

    fn grok_body(len: usize) -> Vec<u8> {
        let prefix =
            br#"{"output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"usage":{},"padding":""#;
        let suffix = br#""}"#;
        assert!(len >= prefix.len() + suffix.len());
        [
            prefix.as_slice(),
            &vec![b'x'; len - prefix.len() - suffix.len()],
            suffix.as_slice(),
        ]
        .concat()
    }

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
        let missing = json!({"output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}],"usage":{}});
        assert!(!spend::parse_grok_usage(missing.get("usage")).usage_reported());
    }

    #[tokio::test]
    async fn empty_text_2xx_appends_failed_after_billing_without_prompt_or_key() {
        let dir = tempfile::tempdir().unwrap();
        let spend = dir.path().join("spend.jsonl");
        let (origin, server) = serve(Router::new().route(
            "/empty",
            post(|| async {
                Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"output":[],"usage":{"input_tokens":2,"output_tokens":0,"cost_in_usd_ticks":1}}"#,
                    ))
                    .unwrap()
            }),
        ))
        .await;
        let mut chat = enabled_chat();
        chat.attach_spend(spend.clone());
        chat.grok.endpoint = format!("{origin}/empty");
        let err = chat.chat("grok", "secret-prompt-text").await.unwrap_err();
        assert!(err.message.contains("no text"), "{}", err.message);
        let text = std::fs::read_to_string(&spend).unwrap();
        assert!(text.contains("failed_after_billing"));
        assert!(text.contains("\"usage_missing\":false"));
        assert!(!text.contains("secret-prompt-text"));
        assert!(!text.contains("grok-test-key"));
        assert!(!text.contains("\"usd\""));
        server.abort();
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

    #[tokio::test]
    async fn cloud_transport_refuses_redirects_and_bounds_success_bodies() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sink_hits = hits.clone();
        let (sink, sink_task) = serve(Router::new().route(
            "/sink",
            post(move || {
                let hits = sink_hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            }),
        ))
        .await;
        let redirect_target = format!("{sink}/sink");
        let (redirect, redirect_task) = serve(Router::new().route(
            "/redirect",
            post(move || {
                let target = redirect_target.clone();
                async move { Redirect::temporary(&target) }
            }),
        ))
        .await;

        let exact = axum::body::Bytes::from(grok_body(MAX_RESPONSE_BYTES));
        let over = axum::body::Bytes::from(grok_body(MAX_RESPONSE_BYTES + 1));
        let (responses, response_task) = serve(
            Router::new()
                .route(
                    "/exact",
                    post(move || {
                        let body = exact.clone();
                        async move {
                            Response::builder()
                                .header(header::CONTENT_TYPE, "application/json")
                                .body(Body::from(body))
                                .unwrap()
                        }
                    }),
                )
                .route(
                    "/over",
                    post(move || {
                        let body = over.clone();
                        async move {
                            let split = MAX_RESPONSE_BYTES / 2;
                            let chunks = vec![
                                Ok::<_, std::convert::Infallible>(body.slice(..split)),
                                Ok(body.slice(split..)),
                            ];
                            Response::builder()
                                .header(header::CONTENT_TYPE, "application/json")
                                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                                .unwrap()
                        }
                    }),
                ),
        )
        .await;

        let mut chat = enabled_chat();
        chat.claude.endpoint = format!("{redirect}/redirect");
        let refused = chat.chat("claude", "secret").await.unwrap_err();
        assert!(refused.message.contains("HTTP 307"), "{}", refused.message);
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        chat.grok.endpoint = format!("{responses}/exact");
        assert_eq!(chat.chat("grok", "hello").await.unwrap().body_text, "ok");
        chat.grok.endpoint = format!("{responses}/over");
        assert!(chat
            .chat("grok", "hello")
            .await
            .unwrap_err()
            .message
            .contains("response exceeds limit"));

        sink_task.abort();
        redirect_task.abort();
        response_task.abort();
    }

    #[tokio::test]
    async fn dropping_a_cloud_call_unregisters_it() {
        let (origin, server) = serve(Router::new().route(
            "/pending",
            post(|| async { std::future::pending::<StatusCode>().await }),
        ))
        .await;
        let mut chat = enabled_chat();
        chat.grok.endpoint = format!("{origin}/pending");
        let chat = Arc::new(chat);
        let task_chat = chat.clone();
        let call = tokio::spawn(async move { task_chat.chat("grok", "hello").await });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if chat.inflight.lock().unwrap_or_else(|p| p.into_inner()).len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        call.abort();
        let _ = call.await;
        assert!(chat.inflight.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
        server.abort();
    }
}
