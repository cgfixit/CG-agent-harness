//! Bounded OpenAI SSE decoding. Partial text is provisional until normal completion.
use crate::common::errors::{HarnessError, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::sync::mpsc::Sender;

/// Protocol allocation ceiling; the web dispatcher enforces the configured
/// (possibly smaller) remaining budget before executing any assembled calls.
/// It is also the `web.chat_tool_calls` ceiling: a model may batch every call it
/// has left in one reply, so a smaller per-reply bound would refuse in-budget
/// batches. `web.total_tokens` and the turn deadline still bind first.
pub(crate) const MAX_TOOL_CALLS: u64 = 30;

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

/// Ends a local reply early when one sentence or line repeats `limit` times in
/// a row (#941: some tags loop on a short answer while tools are offered and
/// would otherwise spend the whole reply budget). A piece is the text between
/// `\n`, `.`, `!` or `?`, compared trimmed, lowercased and with whitespace
/// collapsed. Pieces with no letter neither count nor trip it; one with a
/// digit (a list number, a table cell) also breaks the run, so numbered
/// answers and numeric tables do not trip it. `limit` 0 is off.
#[derive(Default)]
pub(crate) struct RepeatGuard {
    limit: usize,
    partial: String,
    last: Option<String>,
    run: usize,
}

/// A piece longer than this is not a short loop; it is dropped and ends a run.
const REPEAT_PIECE_CHARS: usize = 512;

impl RepeatGuard {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    /// Feed streamed text; `true` once a piece has repeated `limit` times.
    pub(crate) fn push(&mut self, text: &str) -> bool {
        if self.limit == 0 {
            return false;
        }
        for ch in text.chars() {
            if matches!(ch, '\n' | '.' | '!' | '?') {
                let piece = std::mem::take(&mut self.partial);
                if self.piece(&piece) {
                    return true;
                }
            } else if self.partial.chars().count() < REPEAT_PIECE_CHARS {
                self.partial.push(ch);
            } else {
                self.partial.clear();
                self.last = None;
                self.run = 0;
            }
        }
        false
    }

    fn piece(&mut self, raw: &str) -> bool {
        if !raw.chars().any(char::is_alphabetic) {
            // A list number (`2.` splits to `2`) separates items: identical
            // numbered answers are a list, not a loop. Other letterless
            // pieces (`}`, `|---|`) are skipped.
            if raw.chars().any(|c| c.is_ascii_digit()) {
                self.last = None;
                self.run = 0;
            }
            return false;
        }
        let piece = raw.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if self.last.as_deref() == Some(piece.as_str()) {
            self.run += 1;
        } else {
            self.last = Some(piece);
            self.run = 1;
        }
        self.run >= self.limit
    }
}

/// The error a fired [`RepeatGuard`] ends the call with. The partial reply is
/// never returned, so nothing is saved.
pub(crate) fn repeat_error(limit: usize) -> HarnessError {
    invalid(&format!(
        "model reply repeated one line {limit} times in a row; it was stopped early and not saved"
    ))
    .detail("repeat_detector", true)
    .detail("finish_reason", "repeat")
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    scanned: usize,
    data: String,
    response: Value,
    done: bool,
    content: String,
    tool_calls: BTreeMap<u64, Value>,
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
                let index = call["index"]
                    .as_u64()
                    .filter(|n| *n < MAX_TOOL_CALLS)
                    .ok_or_else(|| invalid("invalid or excessive streamed tool call index"))?;
                let target = self
                    .tool_calls
                    .entry(index)
                    .or_insert_with(|| json!({"id":"","type":"function","function":{"name":"","arguments":""}}));
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
                    match slot {
                        Value::String(text) if key != "type" => text.push_str(value),
                        _ => *slot = json!(value),
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
        self.response["choices"][0]["message"]["content"] = Value::String(std::mem::take(&mut self.content));
        if !self.tool_calls.is_empty() {
            if self.tool_calls.keys().copied().ne(0..self.tool_calls.len() as u64) {
                return Err(invalid("streamed tool call indices must be contiguous"));
            }
            self.response["choices"][0]["message"]["tool_calls"] =
                json!(self.tool_calls.into_values().collect::<Vec<_>>());
        }
        Ok(self.response)
    }
}

pub async fn read(mut response: reqwest::Response, output: Output<'_>, repeat_limit: usize) -> Result<Value> {
    let mut decoder = Decoder::default();
    let mut repeats = RepeatGuard::new(repeat_limit);
    let mut bytes = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("model stream interrupted"))?
    {
        bytes = bytes.saturating_add(chunk.len());
        if bytes > super::MAX_RESPONSE_BYTES {
            return Err(invalid("model response exceeds limit"));
        }
        for text in decoder.push(&chunk)? {
            // Returning drops the response, which closes the connection, so the
            // model server stops generating instead of running to the cap.
            if repeats.push(&text) {
                return Err(repeat_error(repeat_limit));
            }
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

    fn fires(limit: usize, chunks: &[&str]) -> bool {
        let mut guard = RepeatGuard::new(limit);
        chunks.iter().any(|chunk| guard.push(chunk))
    }

    #[test]
    fn the_repeat_guard_stops_a_looping_short_answer_and_spares_ordinary_output() {
        // #941's shape: one word repeated, streamed in small deltas.
        let loop_reply: Vec<&str> = std::iter::repeat_n(["Jun", "iper", ". "], 8).flatten().collect();
        assert!(fires(8, &loop_reply), "eight in a row stop the stream");
        assert!(!fires(8, &loop_reply[..loop_reply.len() - 1]), "seven do not");
        assert!(
            fires(8, &["juniper\n", "JUNIPER!  ", "Juniper?\n"].repeat(3)),
            "case, spacing and the delimiter do not matter"
        );
        assert!(!fires(0, &loop_reply), "0 turns the guard off");
        // Legitimate output that repeats without looping on one sentence.
        let code = "fn a() {\n    if x {\n        y();\n    }\n}\n".repeat(4) + &"}\n".repeat(20);
        let table = "| a | b |\n|---|---|\n".to_string() + &"| 0 | 0.0 |\n".repeat(40);
        let list = (1..=30).map(|n| format!("- item {n}.\n")).collect::<String>();
        let alternating = "Yes. No. ".repeat(20);
        for text in [code.as_str(), table.as_str(), list.as_str(), alternating.as_str()] {
            assert!(!fires(8, &[text]), "{text}");
        }
        // A piece with no letter and no digit (`}`, `|---|`) neither counts nor breaks a run.
        assert!(fires(3, &["Juniper.\n}\nJuniper.\n|---|\nJuniper."]));
        // A list number breaks it: eight numbered answers that read the same are a list,
        // not a loop (Codex review on #981).
        let numbered: String = (1..=12).map(|n| format!("{n}. Yes.\n")).collect();
        assert!(!fires(8, &[numbered.as_str()]), "{numbered}");
        // A piece too long to be a short loop resets the run.
        let long = "w".repeat(REPEAT_PIECE_CHARS + 1);
        assert!(!fires(3, &["Juniper. Juniper. ", &long, " Juniper. "]));
        let err = repeat_error(8);
        assert_eq!(err.code, crate::llm::openai_chat::LLM_ERROR_CODE);
        assert_eq!(err.details["repeat_detector"], true);
        assert_eq!(err.details["finish_reason"], "repeat");
    }
    #[test]
    fn streamed_tool_indices_have_a_bounded_contiguous_domain() {
        for index in [
            Value::Null,
            json!(-1),
            json!(1.5),
            json!(MAX_TOOL_CALLS),
            json!(u64::MAX),
        ] {
            let mut decoder = Decoder::default();
            let event = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":index}]}}]});
            assert!(decoder.push(format!("data: {event}\n\n").as_bytes()).is_err());
        }
        for count in [1, MAX_TOOL_CALLS] {
            let mut decoder = Decoder::default();
            let calls: Vec<_> = (0..count).map(|index| json!({"index":index})).collect();
            let event = json!({"choices":[{"index":0,"delta":{"tool_calls":calls},"finish_reason":"tool_calls"}]});
            decoder
                .push(format!("data: {event}\n\ndata: [DONE]\n\n").as_bytes())
                .unwrap();
            assert_eq!(
                decoder.finish().unwrap()["choices"][0]["message"]["tool_calls"]
                    .as_array()
                    .unwrap()
                    .len(),
                count as usize
            );
        }
        let mut decoder = Decoder::default();
        let event = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":1}]},"finish_reason":"tool_calls"}]});
        decoder
            .push(format!("data: {event}\n\ndata: [DONE]\n\n").as_bytes())
            .unwrap();
        assert!(decoder.finish().is_err(), "a missing index cannot be invented");
    }

    #[test]
    fn interleaved_multiple_tool_calls_are_reassembled_by_index() {
        let mut decoder = Decoder::default();
        for (calls, finish) in [
            (
                json!([
                    {"index":1,"id":"call_b","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":"}},
                    {"index":0,"id":"call_a","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":"}}
                ]),
                Value::Null,
            ),
            (
                json!([
                    {"index":0,"function":{"arguments":"\"https://example.com/\"}"}},
                    {"index":1,"function":{"arguments":"\"https://example.org/\"}"}}
                ]),
                json!("tool_calls"),
            ),
        ] {
            let event = json!({"choices":[{"index":0,"delta":{"tool_calls":calls},"finish_reason":finish}]});
            for byte in format!("data: {event}\n\n").bytes() {
                decoder.push(&[byte]).unwrap();
            }
        }
        decoder.push(b"data: [DONE]\n\n").unwrap();
        let result = decoder.finish().unwrap();
        let calls = result["choices"][0]["message"]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        for (call, id, url) in [
            (&calls[0], "call_a", "https://example.com/"),
            (&calls[1], "call_b", "https://example.org/"),
        ] {
            assert_eq!(call["id"], id);
            assert_eq!(
                serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap()["url"],
                url
            );
        }
    }

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
