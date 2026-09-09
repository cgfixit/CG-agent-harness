//! OpenAI-compatible chat client with token-usage extraction and in-flight
//! abort. Port of `harness/ollama.py` (+ `harness/chat_cancel.py`, which is
//! unnecessary here: the request runs in its own tokio task, and aborting that
//! task drops the connection).

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::task::AbortHandle;

use crate::common::errors::{HarnessError, Result};
use crate::llm::backend::is_loopback_url;

pub const DEFAULT_CHAT_TIMEOUT_SEC: f64 = 720.0;
pub const LLM_ERROR_CODE: &str = "HARNESS_LLM_ERROR";

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub body_text: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub struct ChatClient {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: f64,
    http: reqwest::Client,
    inflight: Mutex<Option<AbortHandle>>,
}

impl std::fmt::Debug for ChatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

fn llm_err(message: impl Into<String>) -> HarnessError {
    HarnessError::new(LLM_ERROR_CODE, message)
}

fn token_count(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

/// Extract body text + usage or fail with a typed error (never echoes the body).
pub fn parse_chat_response(parsed: &Value, fallback_model: &str) -> Result<ChatResult> {
    let obj = parsed
        .as_object()
        .ok_or_else(|| llm_err("malformed response from model server"))?;
    let first = obj
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(json!({}));
    let body_text = first
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| llm_err("malformed response from model server"))?
        .to_string();
    match first.get("finish_reason").and_then(Value::as_str) {
        Some("stop") => {}
        Some("length") => return Err(llm_err("model output was truncated; reduce the requested output or adjust the configured token budget before retrying")),
        _ => return Err(llm_err("model response did not report a normal completion (finish_reason=stop required)")),
    }
    let usage = obj.get("usage").and_then(|u| u.as_object());
    Ok(ChatResult {
        body_text,
        model: obj
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(fallback_model)
            .to_string(),
        prompt_tokens: token_count(usage.and_then(|u| u.get("prompt_tokens"))),
        completion_tokens: token_count(usage.and_then(|u| u.get("completion_tokens"))),
    })
}

impl ChatClient {
    pub fn new(
        base_url: &str,
        model: &str,
        timeout_sec: f64,
        api_key: &str,
        reasoning_effort: Option<String>,
    ) -> Result<Self> {
        if !is_loopback_url(base_url) {
            return Err(llm_err("harness chat only targets loopback model servers").detail("base_url", base_url));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs_f64(timeout_sec.max(1.0)))
            .build()
            .map_err(|e| llm_err(format!("cannot build http client: {e}")))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key: api_key.trim().to_string(),
            reasoning_effort,
            timeout_sec,
            http,
            inflight: Mutex::new(None),
        })
    }

    /// Abort the in-flight POST (if any). Idempotent.
    pub fn abort_in_flight(&self) {
        if let Some(handle) = self.inflight.lock().unwrap_or_else(|p| p.into_inner()).take() {
            handle.abort();
        }
    }

    /// One chat completion. `messages` are prior turns (user/assistant).
    pub async fn chat(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        model: Option<&str>,
        max_tokens: u64,
        temperature: f64,
    ) -> Result<ChatResult> {
        let use_model = model
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
            .unwrap_or(self.model.trim())
            .to_string();
        if use_model.is_empty() {
            return Err(llm_err("no model selected; set one with /model use <name>"));
        }
        let mut all: Vec<Value> = vec![json!({"role": "system", "content": system_prompt})];
        for m in messages {
            all.push(json!({"role": m.role, "content": m.content}));
        }
        let mut payload = json!({
            "model": use_model,
            "messages": all,
            "max_tokens": max_tokens,
            "temperature": temperature,
            "stream": false,
        });
        if let Some(effort) = &self.reasoning_effort {
            payload["reasoning_effort"] = Value::String(effort.clone());
        }
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&payload);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let base_url = self.base_url.clone();
        let task = tokio::spawn(async move {
            let resp = req.send().await.map_err(|e| {
                let kind = if e.is_timeout() {
                    "ReadTimeout"
                } else if e.is_connect() {
                    "ConnectError"
                } else {
                    "TransportError"
                };
                llm_err("model server unreachable - is the model server running?")
                    .detail("base_url", base_url.clone())
                    .detail("error", kind)
            })?;
            let status = resp.status();
            if !status.is_success() {
                // Body stays out of the browser: a proxy 401 can echo credentials.
                let preview: String = resp.text().await.unwrap_or_default().chars().take(200).collect();
                tracing::debug!("harness chat upstream HTTP {status}: {preview}");
                return Err(llm_err(format!("model server returned HTTP {}", status.as_u16())));
            }
            let parsed: Value = resp
                .json()
                .await
                .map_err(|_| llm_err("malformed response from model server"))?;
            Ok::<Value, HarnessError>(parsed)
        });
        *self.inflight.lock().unwrap_or_else(|p| p.into_inner()) = Some(task.abort_handle());
        let outcome = task.await;
        *self.inflight.lock().unwrap_or_else(|p| p.into_inner()) = None;
        match outcome {
            Ok(Ok(parsed)) => parse_chat_response(&parsed, &use_model),
            Ok(Err(e)) => Err(e),
            Err(join) if join.is_cancelled() => Err(llm_err("cancelled").detail("cancelled", true)),
            Err(_) => Err(llm_err("model call failed")),
        }
    }
}
