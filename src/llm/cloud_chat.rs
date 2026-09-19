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
const MAX_RESPONSE_BYTES: usize = 4_194_304;

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
        Ok(Self {
            enabled: cfg.flag_is_true("models.cloud_chat.enabled"),
            timeout: Duration::from_secs_f64(timeout_sec),
            max_tokens,
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
        self.timeout.as_secs_f64()
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
            _ = cancel.cancelled() => Err(err("cloud chat request cancelled")),
            result = self.request(config, message, source) => result,
        }
    }

    async fn request(&self, config: &ProviderConfig, message: &str, source: &str) -> Result<ChatResult> {
        let response = match config.provider {
            Provider::Grok => self
                .client
                .post(&config.endpoint)
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
                .post(&config.endpoint)
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
