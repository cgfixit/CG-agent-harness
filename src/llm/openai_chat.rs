//! OpenAI-compatible chat with bounded JSON/SSE and per-call cancellation.
//! Dropping the caller drops its HTTP future; no detached model task survives.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

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
    pub usage_reported: bool,
    /// First request only, never the sum of a multi-call web turn. Numeric,
    /// positive prompt usage is needed for session estimator calibration.
    pub initial_prompt_tokens: Option<u64>,
}

pub struct ChatClient {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: f64,
    http: reqwest::Client,
    inflight: Mutex<BTreeMap<String, CancellationToken>>,
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

pub(crate) fn token_count(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

pub(crate) fn initial_prompt_tokens(parsed: &Value) -> Option<u64> {
    parsed["usage"]["prompt_tokens"].as_u64().filter(|n| *n > 0)
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
    let usage = obj.get("usage").and_then(|u| u.as_object());
    let prompt_tokens = token_count(usage.and_then(|u| u.get("prompt_tokens")));
    let completion_tokens = token_count(usage.and_then(|u| u.get("completion_tokens")));
    let usage_reported = usage.is_some_and(|u| {
        u.get("prompt_tokens").is_some_and(Value::is_number) && u.get("completion_tokens").is_some_and(Value::is_number)
    });
    let incomplete = |message: &str| {
        llm_err(message).with_details(json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens, "usage_reported":usage_reported, "output_bytes":body_text.len()}))
    };
    match first.get("finish_reason").and_then(Value::as_str) {
        Some("stop") => {}
        Some("length") => return Err(incomplete("model output was truncated; reduce the requested output or adjust the configured token budget before retrying")),
        _ => return Err(incomplete("model response did not report a normal completion (finish_reason=stop required)")),
    }
    Ok(ChatResult {
        body_text,
        model: obj
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(fallback_model)
            .to_string(),
        prompt_tokens,
        completion_tokens,
        usage_reported,
        initial_prompt_tokens: initial_prompt_tokens(parsed),
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
            inflight: Mutex::new(BTreeMap::new()),
        })
    }

    /// Abort every registered POST. Production single-flights via GenerationGate;
    /// per-call records also keep independent direct callers from orphaning handles.
    pub fn abort_in_flight(&self) {
        for (_, token) in std::mem::take(&mut *self.inflight.lock().unwrap_or_else(|p| p.into_inner())) {
            token.cancel();
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
        self.chat_stream(system_prompt, messages, model, max_tokens, temperature, None)
            .await
    }

    pub async fn chat_stream(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        model: Option<&str>,
        max_tokens: u64,
        temperature: f64,
        output: Option<super::openai_stream::Output<'_>>,
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
        let parsed = self.send_payload(payload, output).await?;
        parse_chat_response(&parsed, &use_model)
    }

    /// Standard OpenAI tool protocol; the server validates and executes the
    /// narrow read-only tool schema. This client never dispatches a tool.
    pub async fn chat_with_tools(
        &self,
        system: &str,
        messages: &[Value],
        model: &str,
        max_tokens: u64,
        temperature: f64,
        tools: &[Value],
    ) -> Result<Value> {
        self.chat_with_tools_stream(system, messages, model, max_tokens, temperature, tools, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_with_tools_stream(
        &self,
        system: &str,
        messages: &[Value],
        model: &str,
        max_tokens: u64,
        temperature: f64,
        tools: &[Value],
        output: Option<super::openai_stream::Output<'_>>,
    ) -> Result<Value> {
        let mut all = vec![json!({"role":"system","content":system})];
        all.extend_from_slice(messages);
        let mut payload = json!({"model":model,"messages":all,"max_tokens":max_tokens,
            "temperature":temperature,"stream":false});
        if !tools.is_empty() {
            payload["tools"] = json!(tools);
            payload["tool_choice"] = json!("auto");
            // The dispatcher accepts a bounded batch and checks each read's
            // authority. This permits multiple calls, not unbounded execution.
            payload["parallel_tool_calls"] = json!(true);
        }
        if let Some(effort) = &self.reasoning_effort {
            payload["reasoning_effort"] = json!(effort);
        }
        self.send_payload(payload, output).await
    }

    async fn send_payload(
        &self,
        mut payload: Value,
        output: Option<super::openai_stream::Output<'_>>,
    ) -> Result<Value> {
        if output.is_some() {
            payload["stream"] = json!(true);
            payload["stream_options"] = json!({"include_usage":true});
        }
        let id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        self.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), token.clone());
        struct Call<'a> {
            client: &'a ChatClient,
            id: String,
        }
        impl Drop for Call<'_> {
            fn drop(&mut self) {
                self.client
                    .inflight
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&self.id);
            }
        }
        let _call = Call { client: self, id };
        tokio::select! {
            _ = token.cancelled() => Err(llm_err("cancelled").detail("cancelled", true)),
            result = tokio::time::timeout(Duration::from_secs_f64(self.timeout_sec.max(1.0)), self.send_request(payload, output)) =>
                result.map_err(|_| llm_err("model request deadline exceeded"))?,
        }
    }

    async fn send_request(&self, payload: Value, output: Option<super::openai_stream::Output<'_>>) -> Result<Value> {
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&payload);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let base_url = self.base_url.clone();
        async {
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
                // Error bodies can echo credentials or never finish; drop them unread.
                tracing::debug!("harness chat upstream HTTP {status}");
                return Err(llm_err(format!("model server returned HTTP {}", status.as_u16())));
            }
            if let Some(output) = output {
                if resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.split(';').next().unwrap_or("").trim() == "text/event-stream")
                {
                    return super::openai_stream::read(resp, output).await;
                }
            }
            // Bound model-controlled JSON independently of the requested tokens.
            const MAX_RESPONSE_BYTES: usize = 4_194_304;
            let mut resp = resp;
            let mut body = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|_| llm_err("malformed response from model server"))?
            {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(llm_err("model response exceeds limit"));
                }
                body.extend_from_slice(&chunk);
            }
            let parsed: Value =
                serde_json::from_slice(&body).map_err(|_| llm_err("malformed response from model server"))?;
            Ok::<Value, HarnessError>(parsed)
        }
        .await
    }
}
