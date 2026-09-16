//! Bounded OpenAI SSE decoding. Partial text is provisional until normal completion.
use crate::common::errors::{HarnessError, Result};
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;

#[derive(Clone, Copy)]
pub struct Output<'a> {
    pub sender: &'a Sender<Value>,
    pub validate: &'a (dyn Fn() -> Result<()> + Sync),
}
impl Output<'_> {
    async fn text(self, text: &str) -> Result<()> {
        (self.validate)()?;
        self.sender
            .send(json!({"type":"delta","text":text}))
            .await
            .map_err(|_| invalid("chat stream disconnected"))
    }
}
fn invalid(message: &str) -> HarnessError {
    HarnessError::new(super::openai_chat::LLM_ERROR_CODE, message)
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    scanned: usize,
    data: String,
    response: Value,
    done: bool,
    content: String,
}
impl Decoder {
    fn event(&mut self) -> Result<Option<String>> {
        let data = std::mem::take(&mut self.data);
        if data.is_empty() {
            return Ok(None);
        }
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(None);
        }
        if self.done {
            return Err(invalid("model sent data after stream completion"));
        }
        let event: Value = serde_json::from_str(&data).map_err(|_| invalid("malformed model stream"))?;
        if event.get("error").is_some() {
            return Err(invalid("model stream failed"));
        }
        if self.response.is_null() {
            self.response = json!({"choices":[{"message":{"role":"assistant","content":""},"finish_reason":null}]});
        }
        if let Some(model) = event.get("model").filter(|v| v.is_string()) {
            self.response["model"] = model.clone();
        }
        if let Some(usage) = event.get("usage").filter(|v| v.is_object()) {
            self.response["usage"] = usage.clone();
        }
        let choices = event["choices"]
            .as_array()
            .ok_or_else(|| invalid("malformed model stream"))?;
        if choices.is_empty() {
            return Ok(None);
        }
        if choices.len() != 1 || choices[0]["index"] != 0 {
            return Err(invalid("unsupported model stream choices"));
        }
        let choice = &choices[0];
        let delta = &choice["delta"];
        let mut text = None;
        if let Some(value) = delta.get("content").filter(|v| !v.is_null()) {
            let value = value.as_str().ok_or_else(|| invalid("malformed model stream text"))?;
            self.content.push_str(value);
            text = Some(value.to_string());
        }
        if let Some(calls) = delta.get("tool_calls") {
            let calls = calls
                .as_array()
                .ok_or_else(|| invalid("malformed streamed tool call"))?;
            for call in calls {
                if call["index"] != 0 {
                    return Err(invalid("model must return one tool call"));
                }
                let message = &mut self.response["choices"][0]["message"];
                if message.get("tool_calls").is_none() {
                    message["tool_calls"] = json!([{"id":"","type":"function","function":{"name":"","arguments":""}}]);
                }
                let target = &mut message["tool_calls"][0];
                for (key, nested) in [("id", false), ("type", false), ("name", true), ("arguments", true)] {
                    let value = if nested { &call["function"][key] } else { &call[key] };
                    if value.is_null() {
                        continue;
                    }
                    let value = value.as_str().ok_or_else(|| invalid("malformed streamed tool call"))?;
                    let slot = if nested {
                        &mut target["function"][key]
                    } else {
                        &mut target[key]
                    };
                    if slot.as_str().unwrap_or("").len() + value.len() > 4096 {
                        return Err(invalid("streamed tool call exceeds limit"));
                    }
                    if key == "type" {
                        *slot = json!(value);
                    } else {
                        *slot = json!(format!("{}{value}", slot.as_str().unwrap_or("")));
                    }
                }
            }
        }
        if !choice["finish_reason"].is_null() {
            self.response["choices"][0]["finish_reason"] = choice["finish_reason"].clone();
        }
        Ok(text)
    }
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        let mut scan_from = self.scanned;
        let mut text = Vec::new();
        while let Some(end) = self.pending[scan_from..].iter().position(|b| *b == b'\n') {
            let end = scan_from + end;
            let line = std::str::from_utf8(&self.pending[consumed..end])
                .map_err(|_| invalid("invalid model stream UTF-8"))?
                .trim_end_matches('\r')
                .to_string();
            consumed = end + 1;
            scan_from = consumed;
            if line.is_empty() {
                if let Some(delta) = self.event()? {
                    text.push(delta);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            }
        }
        self.pending.drain(..consumed);
        self.scanned = self.pending.len();
        Ok(text)
    }
    fn finish(mut self) -> Result<Value> {
        if !self.done || self.response["choices"][0]["finish_reason"].is_null() {
            return Err(invalid("model stream ended before completion"));
        }
        self.response["choices"][0]["message"]["content"] = json!(self.content);
        Ok(self.response)
    }
}

pub async fn read(mut response: reqwest::Response, output: Output<'_>) -> Result<Value> {
    let mut decoder = Decoder::default();
    let mut bytes = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("model stream interrupted"))?
    {
        bytes = bytes.saturating_add(chunk.len());
        if bytes > 4_194_304 {
            return Err(invalid("model response exceeds limit"));
        }
        for text in decoder.push(&chunk)? {
            output.text(&text).await?;
        }
        if decoder.done {
            return decoder.finish();
        }
    }
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_utf8_usage_tools_and_incomplete_streams() {
        let wire = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hé\"},\"finish_reason\":null}]}\r\n\r\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );
        let mut decoder = Decoder::default();
        let mut text = String::new();
        for byte in wire.as_bytes() {
            text.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(text, "hé");
        let response = decoder.finish().unwrap();
        assert_eq!(response["usage"]["prompt_tokens"], 2);
        assert_eq!(
            super::super::openai_chat::parse_chat_response(&response, "fixture")
                .unwrap()
                .body_text,
            "hé"
        );
        assert!(Decoder::default().finish().is_err());
        let mut decoder = Decoder::default();
        assert!(decoder.push(b"data: {bad}\n\n").is_err());
        let mut decoder = Decoder::default();
        decoder.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"web_fetch\",\"arguments\":\"{\"}}]}}]}\n\n").unwrap();
        decoder.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n").unwrap();
        let response = decoder.finish().unwrap();
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
    }
}
