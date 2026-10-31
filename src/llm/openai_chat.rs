//! OpenAI-compatible chat with bounded JSON/SSE and per-call cancellation.
//! Dropping the caller drops its HTTP future; no detached model task survives.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

use crate::common::errors::{HarnessError, Result};

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
    /// Numeric provider counts, independently unknown when absent or malformed.
    pub reported_prompt_tokens: Option<u64>,
    pub reported_completion_tokens: Option<u64>,
    /// First request only, never the sum of a multi-call web turn. Numeric,
    /// positive prompt usage is needed for session estimator calibration.
    pub initial_prompt_tokens: Option<u64>,
    /// Whether that first request carried tool definitions; calibration counts
    /// them only then.
    pub initial_prompt_tools: bool,
    /// Whether the request that produced `body_text` offered tools; grounding
    /// treats them as available only then.
    pub final_prompt_tools: bool,
    /// Uncalibrated tokens the largest request of this turn carried beyond the
    /// system prompt and stored history: tool definitions when offered, plus a
    /// web turn's tool-call batches and results so far. Zero for a plain chat.
    pub peak_prompt_extra_tokens: u64,
}

pub struct ChatClient {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: f64,
    http: reqwest::Client,
    inflight: Mutex<BTreeMap<String, CancellationToken>>,
    /// A measured per-model deadline (`models.local_llm.auto_tune`) that
    /// replaces `timeout_sec` only for requests naming that exact model.
    timeout_override: Mutex<Option<(String, f64)>>,
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

/// The answer after a leading `<think>…</think>` block. Reasoning models served
/// without a reasoning parser (LM Studio, `mlx_lm.server`, llama.cpp without
/// `--reasoning-format`, older Ollama templates) return their chain of thought
/// inline. That text is not the answer: kept, it is shown as the reply, stored in
/// session history and re-sent with every later prompt, and it hides a JSON or
/// `GOAL_DONE` answer from its parser. Only a block that opens the reply and is
/// closed is removed. Some local templates omit the opening marker and emit
/// the identical answer on both sides of a standalone closing marker; keep
/// one copy only when equality proves no distinct text would be lost.
pub fn strip_leading_reasoning(text: &str) -> &str {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let Some(rest) = text.trim_start().strip_prefix(OPEN) else {
        if let Some((before, answer)) = text.split_once("\n</think>\n") {
            if !before.trim().is_empty() && before.trim() == answer.trim() {
                return answer.trim_start();
            }
        }
        return text;
    };
    match rest.find(CLOSE) {
        Some(end) => rest[end + CLOSE.len()..].trim_start(),
        None => text,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    Tool,
    Synthesis,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnSampling {
    pub max_tokens: u64,
    pub temperature: f64,
}

pub const DEFAULT_TOOL_MAX_TOKENS: u64 = 512;
const TOOL_MAX_TOKENS_MIN: u64 = 64;
const TOOL_MAX_TOKENS_MAX: u64 = 2048;

pub fn parse_tool_max_tokens(number: Option<u64>, key_present: bool) -> Result<u64> {
    if !key_present {
        return Ok(DEFAULT_TOOL_MAX_TOKENS);
    }
    match number {
        Some(n) if (TOOL_MAX_TOKENS_MIN..=TOOL_MAX_TOKENS_MAX).contains(&n) => Ok(n),
        _ => Err(HarnessError::config(
            "models.local_llm.tool_max_tokens must be an integer from 64 to 2048",
        )),
    }
}

pub fn sampling_for(
    kind: TurnKind,
    synthesis_max_tokens: u64,
    synthesis_temperature: f64,
    tool_max_tokens: u64,
) -> TurnSampling {
    match kind {
        TurnKind::Tool => TurnSampling {
            max_tokens: tool_max_tokens.min(synthesis_max_tokens).max(1),
            temperature: 0.0,
        },
        TurnKind::Synthesis => TurnSampling {
            max_tokens: synthesis_max_tokens,
            temperature: synthesis_temperature,
        },
    }
}

pub fn completion_payload(
    model: &str,
    messages: &[Value],
    sampling: TurnSampling,
    tools: &[Value],
    tool_protocol: bool,
    json_mode: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "max_tokens": sampling.max_tokens,
        "temperature": sampling.temperature,
        "stream": false,
    });
    if tool_protocol && !tools.is_empty() {
        payload["tools"] = json!(tools);
        payload["tool_choice"] = json!("auto");
        payload["parallel_tool_calls"] = json!(true);
    } else if tool_protocol {
        payload["tool_choice"] = json!("none");
    }
    if json_mode {
        payload["response_format"] = json!({"type": "json_object"});
    }
    if let Some(effort) = reasoning_effort {
        payload["reasoning_effort"] = json!(effort);
    }
    payload
}

fn is_json_object(text: &str) -> bool {
    let text = text.trim();
    text.starts_with('{')
        && text.ends_with('}')
        && serde_json::from_str::<Value>(text).ok().is_some_and(|v| v.is_object())
}

fn looks_like_plan(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "thinking process",
        "thinking:",
        "let me think",
        "my plan",
        "plan:",
        "planning",
        "i should",
        "i need to",
        "i will",
        "the user",
        "step 1",
        "reasoning",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

pub fn residual_think_leak(stripped: &str) -> bool {
    let trimmed = stripped.trim();
    if trimmed.is_empty() || is_json_object(trimmed) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("thinking process") || lower.starts_with("thinking:") {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        if looks_like_plan(rest) {
            return true;
        }
    }
    if let Some(start) = lower.find("<think>") {
        if let Some(end) = lower[start + "<think>".len()..].find("</think>") {
            let inner_start = start + "<think>".len();
            let inner = &trimmed[inner_start..inner_start + end];
            if looks_like_plan(inner) {
                return true;
            }
        }
    }
    if let Some((before, _)) = stripped.split_once("\n</think>\n") {
        if !before.trim().is_empty() && looks_like_plan(before) {
            return true;
        }
    }
    false
}

fn response_think_leak(parsed: &Value) -> bool {
    parsed["choices"][0]["message"]["content"]
        .as_str()
        .is_some_and(|content| residual_think_leak(strip_leading_reasoning(content)))
}

fn merge_usage(mut kept: Value, first: &Value) -> Value {
    if let (Some(extra), Some(base)) = (
        first["usage"]["completion_tokens"].as_u64(),
        kept["usage"]["completion_tokens"].as_u64(),
    ) {
        kept["usage"]["completion_tokens"] = json!(base.saturating_add(extra));
    }
    if let Some(prompt) = first["usage"]["prompt_tokens"].as_u64() {
        if kept.get("usage").is_some() {
            kept["usage"]["prompt_tokens"] = json!(prompt);
        }
    }
    kept
}

/// The JSON object a small model wrapped in prose or a Markdown fence:
/// `Here is the JSON: ```json {…} ``` ` yields `{…}`. Text that does not hold a
/// `{` … `}` span is returned unchanged so the caller's parser reports it. This
/// finds text to parse; the caller's schema still decides what it may contain.
pub fn reply_json_text(text: &str) -> &str {
    let text = strip_leading_reasoning(text).trim();
    if text.starts_with('{') && text.ends_with('}') {
        return text;
    }
    match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    }
}

/// Extract body text + usage or fail with a typed error (never echoes the body).
pub fn parse_chat_response(parsed: &Value, fallback_model: &str) -> Result<ChatResult> {
    let obj = parsed.as_object().ok_or_else(|| {
        llm_err("malformed response from model server").with_details(json!({
            "prompt_tokens": null, "completion_tokens": null, "usage_reported": false
        }))
    })?;
    let first = obj
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(json!({}));
    let usage = obj.get("usage").and_then(|u| u.as_object());
    let prompt_tokens = token_count(usage.and_then(|u| u.get("prompt_tokens")));
    let completion_tokens = token_count(usage.and_then(|u| u.get("completion_tokens")));
    let prompt_reported = usage.and_then(|u| u.get("prompt_tokens")).is_some_and(Value::is_number);
    let completion_reported = usage
        .and_then(|u| u.get("completion_tokens"))
        .is_some_and(Value::is_number);
    let usage_reported = prompt_reported && completion_reported;
    let incomplete = |message: &str| {
        llm_err(message).with_details(json!({
            "prompt_tokens": prompt_reported.then_some(prompt_tokens),
            "completion_tokens": completion_reported.then_some(completion_tokens),
            "usage_reported": usage_reported,
        }))
    };
    let body_text = first
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(strip_leading_reasoning)
        .ok_or_else(|| incomplete("malformed response from model server"))?
        .to_string();
    let incomplete = |message: &str| incomplete(message).detail("output_bytes", body_text.len());
    match first.get("finish_reason").and_then(Value::as_str) {
        Some("stop") => {
            if residual_think_leak(&body_text) {
                return Err(llm_err("model reply looked like leaked thinking and was not saved")
                    .detail("think_leak", true)
                    .detail("output_bytes", body_text.len()));
            }
        }
        Some("length") => return Err(incomplete("model output was truncated; reduce the requested output or adjust the configured token budget before retrying").detail("truncated", true)),
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
        reported_prompt_tokens: prompt_reported.then_some(prompt_tokens),
        reported_completion_tokens: completion_reported.then_some(completion_tokens),
        initial_prompt_tokens: initial_prompt_tokens(parsed),
        initial_prompt_tools: false,
        final_prompt_tools: false,
        peak_prompt_extra_tokens: 0,
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
        // The same shape the inventory probe accepts: a plain loopback URL. A
        // userinfo, query or fragment is refused here so no later error detail
        // or log line can echo a credential embedded in the URL.
        if !super::inventory::local_endpoint(base_url) {
            return Err(llm_err(
                "harness chat only targets a plain loopback model URL: http or https, no credentials, query or fragment",
            )
            .detail("key", "models.local_llm.base_url"));
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
            timeout_override: Mutex::new(None),
        })
    }

    /// Install (or clear) the measured deadline for one model.
    pub fn set_timeout_override(&self, model_seconds: Option<(String, f64)>) {
        *self.timeout_override.lock().unwrap_or_else(|p| p.into_inner()) = model_seconds;
    }

    /// The deadline a request for `model` gets: its measured override, else `timeout_sec`.
    pub fn timeout_for(&self, model: &str) -> f64 {
        match &*self.timeout_override.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((tuned, seconds)) if tuned == model => *seconds,
            _ => self.timeout_sec,
        }
        .max(1.0)
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
        self.chat_inner(
            system_prompt,
            messages,
            model,
            max_tokens,
            temperature,
            output,
            None,
            false,
        )
        .await
    }

    /// Request JSON syntax; callers still validate the schema and source bindings.
    pub async fn chat_json(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        model: Option<&str>,
        max_tokens: u64,
        temperature: f64,
    ) -> Result<ChatResult> {
        self.chat_inner(
            system_prompt,
            messages,
            model,
            max_tokens,
            temperature,
            None,
            None,
            true,
        )
        .await
    }

    /// `chat` with a caller-held deadline, for a run that snapshots its model's
    /// limits at start: a later retune or selection change does not move it.
    pub async fn chat_with_deadline(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        model: &str,
        max_tokens: u64,
        temperature: f64,
        deadline_sec: f64,
    ) -> Result<ChatResult> {
        self.chat_inner(
            system_prompt,
            messages,
            Some(model),
            max_tokens,
            temperature,
            None,
            Some(deadline_sec),
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn chat_inner(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        model: Option<&str>,
        max_tokens: u64,
        temperature: f64,
        output: Option<super::openai_stream::Output<'_>>,
        deadline: Option<f64>,
        json_mode: bool,
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
        let payload = completion_payload(
            &use_model,
            &all,
            TurnSampling {
                max_tokens,
                temperature,
            },
            &[],
            false,
            json_mode,
            self.reasoning_effort.as_deref(),
        );
        let parsed = self.post_without_think_leak(payload, output, deadline).await?;
        parse_chat_response(&parsed, &use_model)
    }

    async fn post_without_think_leak(
        &self,
        payload: Value,
        output: Option<super::openai_stream::Output<'_>>,
        deadline: Option<f64>,
    ) -> Result<Value> {
        let first = self.send_payload(payload.clone(), output, deadline).await?;
        if !response_think_leak(&first) {
            return Ok(first);
        }
        let second = self.send_payload(payload, output, deadline).await?;
        if response_think_leak(&second) {
            return Err(
                llm_err("model reply still looked like leaked thinking after one retry; it was not saved")
                    .detail("think_leak", true),
            );
        }
        Ok(merge_usage(second, &first))
    }

    /// Standard OpenAI tool protocol; the server validates and executes the
    /// narrow read-only tool schema. This client never dispatches a tool.
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
        let payload = completion_payload(
            model,
            &all,
            TurnSampling {
                max_tokens,
                temperature,
            },
            tools,
            true,
            false,
            self.reasoning_effort.as_deref(),
        );
        // Some local servers replay historical tool calls at the end of an SSE
        // synthesis even with tool_choice=none. Their complete JSON response
        // correctly finishes with stop. Buffer only this tool-free final call;
        // the caller still validates completion, authority and any returned calls
        // before delivery, and cancellation/deadlines remain in send_payload.
        let output = if tools.is_empty() { None } else { output };
        self.post_without_think_leak(payload, output, None).await
    }

    async fn send_payload(
        &self,
        mut payload: Value,
        output: Option<super::openai_stream::Output<'_>>,
        deadline: Option<f64>,
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
        let deadline = deadline
            .unwrap_or_else(|| self.timeout_for(payload["model"].as_str().unwrap_or(&self.model)))
            .max(1.0);
        tokio::select! {
            _ = token.cancelled() => Err(llm_err("cancelled").detail("cancelled", true)),
            result = tokio::time::timeout(Duration::from_secs_f64(deadline), self.send_request(payload, output, deadline)) =>
                result.map_err(|_| llm_err("model request deadline exceeded"))?,
        }
    }

    async fn send_request(
        &self,
        payload: Value,
        output: Option<super::openai_stream::Output<'_>>,
        deadline: f64,
    ) -> Result<Value> {
        // A per-request timeout replaces the client-wide one, so a measured
        // deadline can be longer or shorter than the configured timeout_sec.
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .timeout(Duration::from_secs_f64(deadline))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(content: &str) -> Value {
        json!({"choices":[{"finish_reason":"stop","message":{"content":content}}],"usage":{"prompt_tokens":3,"completion_tokens":2}})
    }

    #[test]
    fn a_base_url_with_credentials_query_or_fragment_is_refused_without_echo() {
        // A synthetic marker, not a credential: the assertions only check that
        // no error surface repeats what sat in the userinfo, query or fragment,
        // and a failure names the URL shape rather than printing the URL.
        let marker = "synthetic-url-marker";
        for (shape, url) in [
            ("userinfo", format!("http://user:{marker}@127.0.0.1:9/v1")), // DevSkim: ignore DS162092,DS137138 because this refusal fixture never connects.
            ("username only", format!("http://{marker}@127.0.0.1:9/v1")), // DevSkim: ignore DS162092,DS137138 because this refusal fixture never connects.
            ("query", format!("http://127.0.0.1:9/v1?q={marker}")), // DevSkim: ignore DS162092,DS137138 because this refusal fixture never connects.
            ("fragment", format!("http://127.0.0.1:9/v1#{marker}")), // DevSkim: ignore DS162092,DS137138 because this refusal fixture never connects.
        ] {
            let err = ChatClient::new(&url, "local", 5.0, "", None).unwrap_err();
            let shown = format!("{err:?} {}", err.to_json());
            assert!(!shown.contains(marker), "{shape}: the refusal echoed the URL");
            assert_eq!(err.to_json()["details"]["key"], "models.local_llm.base_url", "{shape}");
        }
        // The constructor never connects, so a plain loopback URL still builds.
        let plain = "http://127.0.0.1:9/v1"; // DevSkim: ignore DS162092,DS137138 because the control fixture binds nothing.
        assert!(ChatClient::new(plain, "local", 5.0, "", None).is_ok());
    }

    /// Both local reply readers stop at 4 MiB of model-controlled bytes: the
    /// JSON body and one server-sent event. Exactly 4 MiB still parses.
    #[tokio::test]
    async fn oversized_local_replies_are_refused_at_the_byte_ceiling() {
        const LIMIT: usize = 4_194_304;
        // A valid completion padded with JSON whitespace to exactly `len` bytes.
        fn padded(len: usize) -> String {
            let body = reply("ok").to_string();
            format!("{body}{}", " ".repeat(len - body.len()))
        }
        let json_body =
            |len: usize| async move { ([(reqwest::header::CONTENT_TYPE, "application/json")], padded(len)) };
        let app = axum::Router::new()
            .route(
                "/exact/v1/chat/completions",
                axum::routing::post(move || json_body(LIMIT)),
            )
            .route(
                "/over/v1/chat/completions",
                axum::routing::post(move || json_body(LIMIT + 1)),
            )
            .route(
                "/sse/v1/chat/completions",
                axum::routing::post(|| async {
                    (
                        [(reqwest::header::CONTENT_TYPE, "text/event-stream")],
                        format!("data: {}\n\n", padded(LIMIT)),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only ephemeral loopback.
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            |path: &str| ChatClient::new(&format!("http://{addr}/{path}/v1"), "local", 30.0, "", None).unwrap(); // DevSkim: ignore DS137138 because this fixture uses no credentials and binds only to loopback.
        let exact = client("exact").chat("s", &[], None, 16, 0.0).await.unwrap();
        assert_eq!(exact.body_text, "ok");
        let over = client("over").chat("s", &[], None, 16, 0.0).await.unwrap_err();
        assert_eq!(over.message, "model response exceeds limit");
        let (sender, _receiver) = tokio::sync::mpsc::channel(4);
        let output = super::super::openai_stream::Output {
            sender: &sender,
            validate: &|| Ok(()),
        };
        let streamed = client("sse")
            .chat_stream("s", &[], None, 16, 0.0, Some(output))
            .await
            .unwrap_err();
        assert_eq!(streamed.message, "model response exceeds limit");
        server.abort();
    }

    #[tokio::test]
    async fn json_mode_is_requested_only_for_json_calls() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(
                |axum::Json(body): axum::Json<Value>| async move { axum::Json(reply(&body.to_string())) },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only ephemeral loopback.
        let url = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this fixture uses no credentials and binds only to loopback.
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ChatClient::new(&url, "local", 5.0, "", None).unwrap();
        let ordinary = client.chat("Return JSON", &[], None, 1024, 0.0).await.unwrap();
        let structured = client.chat_json("Return JSON", &[], None, 1024, 0.0).await.unwrap();
        let ordinary: Value = serde_json::from_str(&ordinary.body_text).unwrap();
        let mut structured: Value = serde_json::from_str(&structured.body_text).unwrap();
        assert_eq!(
            structured.as_object_mut().unwrap().remove("response_format"),
            Some(json!({"type":"json_object"}))
        );
        assert_eq!(structured, ordinary);
        assert!(ordinary.get("response_format").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn a_caller_held_deadline_outlives_a_cleared_override() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                axum::Json(reply("late but fine"))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the test fixture binds only an ephemeral loopback port.
        let url = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ChatClient::new(&url, "slow", 1.0, "", None).unwrap();
        // The run snapshotted a 10s measured deadline; a retune then cleared it.
        client.set_timeout_override(Some(("slow".into(), 10.0)));
        let held = client.timeout_for("slow");
        client.set_timeout_override(None);
        let user = [ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        // Without the held deadline the configured 1s timeout cuts the 1.5s reply.
        let started = std::time::Instant::now();
        assert!(client.chat("s", &user, Some("slow"), 8, 0.0).await.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(1400),
            "{:?}",
            started.elapsed()
        );
        let ok = client
            .chat_with_deadline("s", &user, "slow", 8, 0.0, held)
            .await
            .unwrap();
        assert_eq!(ok.body_text, "late but fine");
        server.abort();
    }

    #[test]
    fn a_leading_closed_reasoning_block_is_not_the_answer() {
        let parsed = parse_chat_response(&reply("\n<think>\nplan GOAL_DONE\n</think>\n\nThe answer."), "m").unwrap();
        assert_eq!(parsed.body_text, "The answer.");
        // Only a block that opens the reply and is closed is removed.
        for kept in [
            "Use <think>…</think> tags.",
            "<think>never closed",
            "Answer.\n<think>x</think>",
            "</think> stray close",
        ] {
            assert_eq!(parse_chat_response(&reply(kept), "m").unwrap().body_text, kept);
        }
        // A reply that was only reasoning is empty, so chat refuses it as no answer.
        assert_eq!(
            parse_chat_response(&reply("<think>x</think>"), "m").unwrap().body_text,
            ""
        );
    }

    #[test]
    fn reply_json_text_unwraps_fences_prose_and_reasoning_only() {
        let object = r#"{"a":[1]}"#;
        for wrapped in [
            object.to_string(),
            format!("```json\n{object}\n```"),
            format!("Here is the JSON:\n{object}\nDone."),
            format!("<think>{{\"no\":1}}</think>\n```\n{object}\n```"),
        ] {
            assert_eq!(reply_json_text(&wrapped), object, "{wrapped}");
        }
        assert_eq!(reply_json_text("no object here"), "no object here");
        assert_eq!(reply_json_text("} backwards {"), "} backwards {");
    }

    #[test]
    fn identical_answers_around_a_bare_reasoning_close_keep_one_copy() {
        // Observed from qwen3.8:27b-mlx-cg with reasoning_effort=none, both
        // through native chat and directly through Ollama's compatible API.
        for answer in [
            "7",
            "Cedar: 5/0. Alder: 2/1. Birch: 0/2.\n\nGOAL_DONE",
            r#"{"supported":[]}"#,
        ] {
            let repeated = format!("{answer}\n</think>\n\n{answer}");
            let parsed = parse_chat_response(&reply(&repeated), "local-model").unwrap();
            assert_eq!(parsed.body_text, answer);
            assert_eq!(reply_json_text(&repeated), answer);
        }
        for kept in [
            "7</think>7", // a literal inline delimiter is not this provider shape
            "</think>\n7",
            "\n</think>\n",                 // no complete answer to recover
            "```text\n7\n</think>\n7\n```", // quoted example
            "GOAL_DONE\n</think>\nStill working.",
        ] {
            assert_eq!(
                parse_chat_response(&reply(kept), "local-model").unwrap().body_text,
                kept
            );
        }
    }

    #[test]
    fn a_residual_plan_after_the_think_close_is_not_appendable() {
        let plan = "Planning is distinct.\n</think>\nThe answer.";
        let err = parse_chat_response(&reply(plan), "qwen3.8").unwrap_err();
        let shown = format!("{err:?} {}", err.to_json());
        assert_eq!(err.details["think_leak"], true);
        assert!(!shown.contains("Planning is distinct"), "{shown}");
        assert!(!shown.contains("The answer"), "{shown}");
        assert!(residual_think_leak(plan));
        assert!(!residual_think_leak("The cedar count is 5."));
        assert!(!residual_think_leak(
            r#"{"name":"web_search","arguments":{"query":"cedar"}}"#
        ));
        let repeated = "The cedar count is 5.\n</think>\n\nThe cedar count is 5.";
        assert_eq!(
            parse_chat_response(&reply(repeated), "qwen3.8").unwrap().body_text,
            "The cedar count is 5."
        );
        assert!(residual_think_leak("Thinking process: read the notes, then answer."));
        assert!(residual_think_leak(
            "I need to check the notes.\n</think>\nThe count is 5."
        ));
        assert!(!residual_think_leak("Use <think>…</think> tags."));
    }

    #[test]
    fn tool_and_synthesis_payloads_use_different_budgets() {
        let messages = [json!({"role":"user","content":"count the cedars"})];
        let tool = sampling_for(TurnKind::Tool, 4096, 0.2, DEFAULT_TOOL_MAX_TOKENS);
        let synthesis = sampling_for(TurnKind::Synthesis, 4096, 0.2, DEFAULT_TOOL_MAX_TOKENS);
        let tool_payload = completion_payload("qwen3.8:27b-mlx-cg", &messages, tool, &[], true, false, Some("none"));
        let synthesis_payload = completion_payload(
            "qwen3.8:27b-mlx-cg",
            &messages,
            synthesis,
            &[],
            true,
            false,
            Some("none"),
        );
        assert_eq!(tool_payload["max_tokens"], 512);
        assert_eq!(tool_payload["temperature"].as_f64(), Some(0.0));
        assert_eq!(tool_payload["reasoning_effort"], "none");
        assert_eq!(tool_payload["tool_choice"], "none");
        assert_eq!(synthesis_payload["max_tokens"], 4096);
        assert_eq!(synthesis_payload["temperature"].as_f64(), Some(0.2));
        assert_eq!(synthesis_payload["reasoning_effort"], "none");
        let shipped = sampling_for(TurnKind::Synthesis, 4096, 0.1, DEFAULT_TOOL_MAX_TOKENS);
        assert_eq!(
            (shipped.max_tokens, shipped.temperature),
            (4096, 0.1),
            "synthesis keeps the configured chat budget"
        );
        assert_eq!(parse_tool_max_tokens(None, false).unwrap(), 512);
        assert!(parse_tool_max_tokens(Some(32), true).is_err());
        assert_eq!(parse_tool_max_tokens(Some(1024), true).unwrap(), 1024);
    }

    #[test]
    fn the_tool_budget_accepts_only_integers_inside_its_bounds() {
        for (value, ok) in [(63, false), (64, true), (2048, true), (2049, false), (0, false)] {
            assert_eq!(parse_tool_max_tokens(Some(value), true).is_ok(), ok, "{value}");
        }
        // A present key that is not an integer (quoted, float, null) reaches the
        // parser as None and must fail closed instead of using the default.
        assert!(parse_tool_max_tokens(None, true).is_err());
        let hop = sampling_for(TurnKind::Tool, 256, 0.1, 2048);
        assert_eq!(hop.max_tokens, 256, "a hop never exceeds the answer ceiling");
        assert_eq!(hop.temperature, 0.0);
    }

    #[tokio::test]
    async fn a_think_leak_is_retried_once_and_not_returned() {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    let (body, prompt, completion) = if n == 0 {
                        ("Planning is distinct.\n</think>\nThe answer.", 10, 2)
                    } else {
                        ("The cedar count is 5.", 40, 3)
                    };
                    axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"content":body}}],"usage":{"prompt_tokens":prompt,"completion_tokens":completion}}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only ephemeral loopback.
        let url = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this fixture uses no credentials and binds only to loopback.
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ChatClient::new(&url, "qwen3.8:27b-mlx-cg", 5.0, "", Some("none".into())).unwrap();
        let parsed = client.chat("s", &[], None, 4096, 0.2).await.unwrap();
        assert_eq!(parsed.body_text, "The cedar count is 5.");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(parsed.prompt_tokens, 10, "calibration keeps the first prompt count");
        assert_eq!(parsed.initial_prompt_tokens, Some(10));
        assert_ne!(parsed.initial_prompt_tokens, Some(50));
        assert_eq!(parsed.completion_tokens, 5);

        hits.store(0, std::sync::atomic::Ordering::SeqCst);
        let counter = hits.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let _n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move { axum::Json(reply("Planning is distinct.\n</think>\nThe answer.")) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only ephemeral loopback.
        let url = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this fixture uses no credentials and binds only to loopback.
        let server_both = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ChatClient::new(&url, "qwen3.8:27b-mlx-cg", 5.0, "", Some("none".into())).unwrap();
        let err = client.chat("s", &[], None, 512, 0.0).await.unwrap_err();
        let shown = format!("{err:?} {}", err.to_json());
        assert!(err.message.contains("not saved"), "{err:?}");
        assert!(!shown.contains("Planning is distinct"), "{shown}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
        server_both.abort();
    }
}
