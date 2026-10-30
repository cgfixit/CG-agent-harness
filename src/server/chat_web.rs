//! The chat model may request bounded web reads, never authority changes.
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::{
    state::AppState,
    web_policy::error,
    web_research::authorize_owner,
    web_search::{WebTool, MIN_EVIDENCE_TOKENS},
};
use crate::common::errors::Result;
use crate::common::tool_broker::assert_allowed;
use crate::llm::openai_chat::{parse_chat_response, ChatMessage, ChatResult};
use crate::server::tool_inventory::chat_callable_names;

pub fn tools_for(mode: super::sessions::GoalWebMode) -> Vec<Value> {
    tools()
        .into_iter()
        .filter(|tool| mode.allows(tool["function"]["name"].as_str().unwrap_or("")))
        .collect()
}

pub fn tools() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"web_search","description":"Search Google for current ranked links and snippets. Listings are not fetched destination pages. Use for explicit search or current-web questions. Resolve references using conversation context; write one focused query with the relevant entities and constraints. Start with five results. Reuse evidence, avoid repeated queries, and stop once you can answer. Search again only for a specific unresolved gap. A WEB_SEARCH_COOLDOWN or WEB_SEARCH_PACED reply means search is paused: do not call web_search again this turn. Do not send credentials or private conversation text.","parameters":{"type":"object","properties":{"query":{"type":"string","maxLength":200},"count":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"web_fetch","description":"Read an exact public URL permitted by the administrator. Use when asked to read, retrieve, fetch or summarize a URL. Never grants permission or follows redirects.","parameters":{"type":"object","properties":{"url":{"type":"string","maxLength":2048}},"required":["url"],"additionalProperties":false}}}),
    ]
}

const OUTSIDE_SOURCE_PHRASES: &[&str] = &[
    "search the web",
    "search the internet",
    "web search",
    "on the internet",
    "from the internet",
    "from the web",
    "outside source",
    "outside sources",
    "internet source",
    "internet sources",
    "look online",
    "search online",
    "web source",
    "web sources",
];

const LOCAL_READ_TOOLS: &[&str] = &[
    "notes_read",
    "read_notes",
    "notes_corpus",
    "local_corpus",
    "read_corpus",
    "private_notes",
    "lookup_note",
];

fn is_web_search_name(name: &str) -> bool {
    matches!(name, "web_search" | "search_web" | "websearch")
}

fn is_local_read_name(name: &str) -> bool {
    LOCAL_READ_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(name.trim()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StickyRoute {
    local_answered: bool,
    user_asked_outside: bool,
}

impl StickyRoute {
    fn from_user_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        Self {
            local_answered: false,
            user_asked_outside: OUTSIDE_SOURCE_PHRASES.iter().any(|phrase| lower.contains(phrase)),
        }
    }

    fn observe_notes_in_prompt(&mut self, system: &str) {
        if system.contains("source=notes_corpus") {
            self.local_answered = true;
        }
    }

    fn observe_tool_result(&mut self, name: &str, ok: bool) {
        if ok && is_local_read_name(name) {
            self.local_answered = true;
        }
    }

    fn allows_web_search(&self) -> bool {
        !self.local_answered || self.user_asked_outside
    }
}

fn narrow_tools(definitions: &[Value], route: &StickyRoute) -> Vec<Value> {
    definitions
        .iter()
        .filter(|tool| {
            let name = tool["function"]["name"].as_str().unwrap_or("");
            !is_web_search_name(name) || route.allows_web_search()
        })
        .cloned()
        .collect()
}

fn hop_kind(loop_turn: bool, tools_offered: bool) -> crate::llm::openai_chat::TurnKind {
    if loop_turn || !tools_offered {
        crate::llm::openai_chat::TurnKind::Synthesis
    } else {
        crate::llm::openai_chat::TurnKind::Tool
    }
}

fn truncated_answer_needs_synthesis(
    hop_max_tokens: u64,
    synthesis_max_tokens: u64,
    has_calls: bool,
    finish_reason: Option<&str>,
) -> bool {
    hop_max_tokens < synthesis_max_tokens && !has_calls && finish_reason == Some("length")
}

fn response_has_calls(response: &Value) -> bool {
    match response["choices"][0]["message"].get("tool_calls") {
        None | Some(Value::Null) => false,
        Some(Value::Array(calls)) => !calls.is_empty(),
        Some(_) => true,
    }
}

fn latest_user_message(history: &[ChatMessage]) -> &str {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("")
}

const WEB_SEARCH_CLOSED: &str =
    "web search stays closed after a local notes or corpus read unless the message asks for outside sources";

// Small local models (Llama 3.x 8B, Qwen 7B) often write a number as a string
// (`"count":"5"`), send `null` for an optional field, or add a key the schema
// does not name. Only `query`/`url` and `count` are ever read, so an unnamed key
// is ignored rather than failing the whole chat turn; it grants nothing. A
// missing or non-string `query`/`url` still refuses the batch.
#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "five", deserialize_with = "lenient_count")]
    count: usize,
}
fn five() -> usize {
    5
}
/// A whole number, as a JSON number or a string holding one, clamped to the
/// 1–10 results a listing allows; `null` is the default five.
fn lenient_count<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<usize, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let whole = match &value {
        Value::Null => return Ok(five()),
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64)),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    whole
        .map(|n| n.clamp(1, 10) as usize)
        .ok_or_else(|| serde::de::Error::custom("count must be a whole number"))
}
#[derive(Deserialize)]
struct FetchArgs {
    url: String,
}

fn tool_argv(name: &str, args: &str) -> Vec<String> {
    match name {
        "web_search" => serde_json::from_str::<SearchArgs>(args)
            .ok()
            .map(|a| vec![a.query])
            .unwrap_or_default(),
        "web_fetch" => serde_json::from_str::<FetchArgs>(args)
            .ok()
            .map(|a| vec![a.url])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Estimated tokens of a model call: UTF-8 bytes / 4 of what is sent, calibrated.
fn prompt_estimate(bytes: usize, token_ratio: f64) -> u64 {
    super::compaction::calibrated_tokens((bytes as u64).div_ceil(4), token_ratio)
}

/// Tokens of the messages a web turn appended to the history, counted the way
/// the chat route projects history: each content plus a batch's calls.
fn appended_tokens(messages: &[Value]) -> u64 {
    let estimate = super::compaction::estimate_tokens;
    messages
        .iter()
        .map(|m| {
            m["content"].as_str().map_or(0, estimate)
                + m.get("tool_calls")
                    .filter(|c| !c.is_null())
                    .map_or(0, |c| estimate(&c.to_string()))
        })
        .sum()
}

/// Room kept for one minimal tool result, calibrated like the estimate that
/// later charges it.
fn min_result_tokens(token_ratio: f64) -> u64 {
    super::compaction::calibrated_tokens(MIN_EVIDENCE_TOKENS, token_ratio)
}

/// The notice on every fetched page.
const FETCH_NOTICE: &str = "Untrusted fetched page text; excerpt may be truncated.";

/// The smallest form `fit_result` can cut a call's result to, from what is known
/// before the read: a page keeps its URL, which is never cut, and one character
/// of text; a listing keeps its query and search URL, and no results.
fn minimal_result(name: &str, args: &str) -> Value {
    if name == "web_fetch" {
        let url = serde_json::from_str::<FetchArgs>(args)
            .map(|a| a.url)
            .unwrap_or_default();
        let url = super::web_policy::canonical_url(&url).map_or(url, |u| u.to_string());
        return json!({"url":url,"title":"","text":"x","source_chars":u32::MAX,"notice":FETCH_NOTICE});
    }
    let query = serde_json::from_str::<SearchArgs>(args)
        .map(|a| a.query)
        .unwrap_or_default();
    let mut search_url = url::Url::parse("https://www.google.com/search").expect("static URL");
    search_url
        .query_pairs_mut()
        .append_pair("q", &query)
        .append_pair("num", "10");
    json!({"query":query,"provider":"google-serpapi","search_url":search_url.as_str(),"results":[],
        "omitted_results":10,"notice":super::web_google::SEARCH_NOTICE,"complete":false})
}

fn tool_message(id: &str, result: &Value) -> Result<Value> {
    Ok(json!({"role":"tool","tool_call_id":id,"content":serde_json::to_string(result)?}))
}

/// Largest form of a tool result that `fits` accepts, cutting text before
/// metadata. A fetched page keeps its longest fitting prefix; only when not
/// even one character fits beside its title is the title cut too. A search
/// listing drops trailing results, then cuts the last one's snippet and title,
/// then drops it too and says how many results it left out. URLs and the query
/// are never cut. `None` when even that does not fit.
fn fit_result(mut result: Value, mut fits: impl FnMut(&Value) -> Result<bool>) -> Result<Option<Value>> {
    if fits(&result)? {
        return Ok(Some(result));
    }
    let total = result.get("results").and_then(Value::as_array).map_or(0, Vec::len);
    let cuts: &[(&str, usize)] = if let Some(text) = result.get("text").and_then(Value::as_str) {
        // An empty page is no evidence.
        if text.is_empty() {
            return Ok(None);
        }
        &[("/text", 1), ("/title", 0)]
    } else {
        while let Some(listing) = result.get_mut("results").and_then(Value::as_array_mut) {
            if listing.len() <= 1 {
                break;
            }
            listing.pop();
            if fits(&result)? {
                return Ok(Some(result));
            }
        }
        &[("/results/0/snippet", 0), ("/results/0/title", 0)]
    };
    for &(pointer, min) in cuts {
        if cut(&mut result, pointer, min, &mut fits)? {
            return Ok(Some(result));
        }
    }
    // A listing whose last result does not fit even bare keeps its query and
    // search URL, and says how many results it left out.
    if let Some(listing) = result.get_mut("results").and_then(Value::as_array_mut) {
        if listing.len() == 1 {
            listing.clear();
            result["omitted_results"] = json!(total);
            if fits(&result)? {
                return Ok(Some(result));
            }
        }
    }
    Ok(None)
}

/// Cuts the string at `pointer`, which does not fit whole, to its longest prefix
/// of at least `min` characters that `fits` accepts. `false`, leaving that
/// minimum in place, when even the minimum does not fit or there is no string.
fn cut(result: &mut Value, pointer: &str, min: usize, fits: &mut impl FnMut(&Value) -> Result<bool>) -> Result<bool> {
    let Some(chars) = result
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(|text| text.chars().collect::<Vec<char>>())
        .filter(|chars| chars.len() >= min)
    else {
        return Ok(false);
    };
    let set = |result: &mut Value, len: usize| {
        if let Some(field) = result.pointer_mut(pointer) {
            *field = Value::String(chars[..len].iter().collect());
        }
    };
    set(result, min);
    if !fits(result)? {
        return Ok(false);
    }
    // Prefix `lo` fits and prefix `hi` does not; find the longest that does.
    let (mut lo, mut hi) = (min, chars.len());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        set(result, mid);
        if fits(result)? {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    set(result, lo);
    Ok(true)
}

fn check_evidence(state: &AppState, owner: &str, sources: &[String]) -> Result<()> {
    authorize_owner(state, owner)?;
    if !sources.is_empty() {
        let policy = state.web.require_enabled(true)?;
        for source in sources {
            policy.authorize(source, None)?;
        }
    }
    Ok(())
}

/// Cancellation covers model calls AND reads. The caller owns the generation gate.
#[allow(clippy::too_many_arguments)]
pub async fn run_stream(
    state: &AppState,
    web: &WebTool,
    owner: &str,
    system: &str,
    history: &[ChatMessage],
    model: &str,
    cap: u64,
    temperature: f64,
    token_ratio: f64,
    web_mode: super::sessions::GoalWebMode,
    sized_tuned: bool,
    loop_turn: bool,
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
    usage_snapshot: &std::sync::Mutex<Option<(crate::llm::spend::UsageTokens, u64)>>,
) -> Result<(ChatResult, Vec<Value>)> {
    let lease = web.chat_turn.start(owner)?;
    let mut usage = None;
    let mut known_completion_tokens = 0u64;
    let result = tokio::select! {
        biased;
        _ = lease.token.cancelled() => Err(error("WEB_CANCELLED", "chat web turn cancelled")),
        // A turn of several tool rounds gets more than one call's deadline; each
        // model call keeps its own (`ChatClient::timeout_for`).
        result = tokio::time::timeout(Duration::from_secs_f64(state.web_turn_deadline_sec(model, &web.limits).max(1.0)), run_inner(state, web, owner, system, history, model, cap, temperature, token_ratio, web_mode, sized_tuned, loop_turn, output, &mut usage, &mut known_completion_tokens, usage_snapshot)) =>
            result.unwrap_or_else(|_| Err(error("WEB_TIMEOUT", "chat web turn deadline exceeded"))),
    };
    result.map_err(|failure| match usage {
        Some(tokens) => failure
            .detail("prompt_tokens", json!(tokens.input_tokens))
            .detail("completion_tokens", json!(tokens.output_tokens))
            .detail("known_completion_tokens", known_completion_tokens)
            .detail("usage_reported", tokens.usage_reported()),
        None => failure,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_inner(
    state: &AppState,
    web: &WebTool,
    owner: &str,
    system: &str,
    history: &[ChatMessage],
    model: &str,
    cap: u64,
    temperature: f64,
    token_ratio: f64,
    web_mode: super::sessions::GoalWebMode,
    sized_tuned: bool,
    loop_turn: bool,
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
    usage: &mut Option<crate::llm::spend::UsageTokens>,
    known_completion_tokens: &mut u64,
    usage_snapshot: &std::sync::Mutex<Option<(crate::llm::spend::UsageTokens, u64)>>,
) -> Result<(ChatResult, Vec<Value>)> {
    let mut messages: Vec<Value> = history
        .iter()
        .map(|m| json!({"role":m.role,"content":m.content}))
        .collect();
    let mut events = Vec::new();
    let mut sources = Vec::new();
    let mut searches = std::collections::BTreeMap::new();
    let mut used_calls = 0;
    let mut prompt_tokens = 0u64;
    let mut completion_tokens = 0u64;
    let mut initial_prompt_tokens = None;
    let definitions = tools_for(web_mode);
    let mut route = StickyRoute::from_user_message(latest_user_message(history));
    route.observe_notes_in_prompt(system);
    let tool_key = "models.local_llm.tool_max_tokens";
    let tool_max = crate::llm::openai_chat::parse_tool_max_tokens(
        state.cfg.get(tool_key).and_then(|value| value.as_u64()),
        state.cfg.get(tool_key).is_some(),
    )?;
    let mut peak_extra = 0u64;
    let mut tools_withheld = false;
    let mut initial_prompt_tools = false;
    for turn in 0..=web.limits.chat_tool_calls {
        check_evidence(state, owner, &sources)?;
        let offered = narrow_tools(&definitions, &route);
        let offered_json = serde_json::to_string(&offered)?;
        let offered_bytes = offered_json.len();
        let offered_tokens = super::compaction::estimate_tokens(&offered_json);
        let sent = system.len() + serde_json::to_vec(&messages)?.len();
        let with_tools = prompt_estimate(sent + offered_bytes, token_ratio);
        let reservation = super::compaction::reply_reservation(&state.backend, cap);
        let remaining = web.limits.chat_tool_calls - used_calls;
        // A tool round is two calls: this one, whose reply carries the tool calls,
        // and a follow-up that sends this prompt again with the results. Each call
        // is one prompt inside `web.total_tokens`, the window-shaped budget: the
        // history is read again on the follow-up, not spent twice (a local runtime
        // normally serves the shared prefix from its prompt cache), so the prompt
        // is charged once. Offer tools only while this prompt with its definitions,
        // a minimal result and one reply fit; the batch the model returns is checked
        // again before its first read, with each result cut to the room the
        // follow-up leaves. Before, this charged the prompt twice and summed every
        // round, which withheld tools from any session past about a third of the
        // budget while compaction waited for chat.compact_prompt_tokens.
        let round_fits = remaining > 0
            && !tools_withheld
            && with_tools
                .saturating_add(min_result_tokens(token_ratio))
                .saturating_add(reservation)
                <= web.limits.total_tokens;
        // Charge what is sent: the tool definitions only when tools are offered.
        let (available, estimate) = if round_fits {
            (offered.as_slice(), with_tools)
        } else {
            (&[][..], prompt_estimate(sent, token_ratio))
        };
        if estimate.saturating_add(reservation) > web.limits.total_tokens {
            return Err(error("WEB_TOKEN_BUDGET", "chat web token budget exhausted"));
        }
        if !round_fits && remaining > 0 && !tools_withheld {
            tools_withheld = true;
            // Reserving synthesis after successful reads is normal completion,
            // not a failed read. With no evidence it remains a budget refusal.
            let have_evidence = !sources.is_empty() || !searches.is_empty();
            events.push(json!({"tool":"web","ok":have_evidence,
                "code":if have_evidence { "WEB_TOOLS_WITHHELD" } else { "WEB_TOKEN_BUDGET" },
                "message":if have_evidence {
                    "Remaining web budget reserves the final answer from gathered evidence; no further web tools were offered"
                } else {
                    "web.total_tokens has no room for a tool round (this prompt, its tool definitions, a result and a reply); web tools were not offered"
                }}));
        }
        if turn == 0 {
            initial_prompt_tools = round_fits;
        }
        // A middle round can be the largest request: earlier results plus the
        // definitions, which the final round may no longer carry.
        let extra = appended_tokens(&messages[history.len()..]) + if available.is_empty() { 0 } else { offered_tokens };
        peak_extra = peak_extra.max(extra);
        let validate = || check_evidence(state, owner, &sources);
        // A tool round may run long: the window must still allow this budget.
        state
            .ensure_window_allows(model, 0, web.limits.total_tokens, sized_tuned)
            .await?;
        let kind = hop_kind(loop_turn, !available.is_empty());
        let sampling = crate::llm::openai_chat::sampling_for(kind, cap, temperature, tool_max);
        let mut response = state
            .chat
            .chat_with_tools_stream(
                system,
                &messages,
                model,
                sampling.max_tokens,
                sampling.temperature,
                available,
                output.map(|sender| crate::llm::openai_stream::Output {
                    sender,
                    validate: &validate,
                }),
            )
            .await?;
        let totals = usage.get_or_insert_with(|| crate::llm::spend::UsageTokens {
            input_tokens: Some(0),
            output_tokens: Some(0),
            ..Default::default()
        });
        let reported = &response["usage"];
        if reported["completion_tokens"].is_number() {
            *known_completion_tokens = known_completion_tokens
                .saturating_add(crate::llm::openai_chat::token_count(reported.get("completion_tokens")));
        }
        // An unknown round makes that dimension's total unknown, never zero.
        for (total, field) in [
            (&mut totals.input_tokens, "prompt_tokens"),
            (&mut totals.output_tokens, "completion_tokens"),
        ] {
            *total = total
                .zip(reported.get(field).filter(|v| v.is_number()))
                .map(|(sum, count)| sum.saturating_add(crate::llm::openai_chat::token_count(Some(count))));
        }
        *usage_snapshot.lock().unwrap() = Some((totals.clone(), *known_completion_tokens));
        if turn == 0 {
            initial_prompt_tokens = crate::llm::openai_chat::initial_prompt_tokens(&response);
        }
        prompt_tokens =
            prompt_tokens.saturating_add(crate::llm::openai_chat::token_count(reported.get("prompt_tokens")));
        completion_tokens =
            completion_tokens.saturating_add(crate::llm::openai_chat::token_count(reported.get("completion_tokens")));
        check_evidence(state, owner, &sources)?;
        let mut final_tools = !available.is_empty();
        let synthesis = crate::llm::openai_chat::sampling_for(
            crate::llm::openai_chat::TurnKind::Synthesis,
            cap,
            temperature,
            tool_max,
        );
        if truncated_answer_needs_synthesis(
            sampling.max_tokens,
            synthesis.max_tokens,
            response_has_calls(&response),
            response["choices"][0]["finish_reason"].as_str(),
        ) {
            response = state
                .chat
                .chat_with_tools_stream(
                    system,
                    &messages,
                    model,
                    synthesis.max_tokens,
                    synthesis.temperature,
                    &[],
                    output.map(|sender| crate::llm::openai_stream::Output {
                        sender,
                        validate: &validate,
                    }),
                )
                .await?;
            let reported = &response["usage"];
            if reported["completion_tokens"].is_number() {
                *known_completion_tokens = known_completion_tokens
                    .saturating_add(crate::llm::openai_chat::token_count(reported.get("completion_tokens")));
            }
            for (total, field) in [
                (&mut totals.input_tokens, "prompt_tokens"),
                (&mut totals.output_tokens, "completion_tokens"),
            ] {
                *total = total
                    .zip(reported.get(field).filter(|v| v.is_number()))
                    .map(|(sum, count)| sum.saturating_add(crate::llm::openai_chat::token_count(Some(count))));
            }
            *usage_snapshot.lock().unwrap() = Some((totals.clone(), *known_completion_tokens));
            prompt_tokens =
                prompt_tokens.saturating_add(crate::llm::openai_chat::token_count(reported.get("prompt_tokens")));
            completion_tokens = completion_tokens
                .saturating_add(crate::llm::openai_chat::token_count(reported.get("completion_tokens")));
            final_tools = false;
        }
        let choice = &response["choices"][0];
        let message = &choice["message"];
        // `"tool_calls": null` (sent by some servers on a plain answer) is no calls.
        let has_calls = match message.get("tool_calls") {
            None | Some(Value::Null) => false,
            Some(Value::Array(calls)) => !calls.is_empty(),
            Some(_) => true,
        };
        if choice["finish_reason"] != "tool_calls" && !has_calls {
            let mut reply = parse_chat_response(&response, model)?;
            reply.prompt_tokens = prompt_tokens;
            reply.completion_tokens = completion_tokens;
            reply.usage_reported = totals.usage_reported();
            reply.reported_prompt_tokens = totals.input_tokens;
            reply.reported_completion_tokens = totals.output_tokens;
            reply.initial_prompt_tokens = initial_prompt_tokens;
            reply.initial_prompt_tools = initial_prompt_tools;
            reply.final_prompt_tools = final_tools;
            reply.peak_prompt_extra_tokens = peak_extra;
            return Ok((reply, events));
        }
        let calls = message["tool_calls"]
            .as_array()
            .filter(|calls| !calls.is_empty())
            .ok_or_else(|| {
                error(
                    "WEB_TOOL_RESPONSE",
                    "model must return valid read-only tool calls or a complete answer",
                )
            })?;
        // Some OpenAI-compatible servers (older Ollama streaming among them) end a
        // complete tool-call message with `stop` instead of `tool_calls`. Either
        // is a finished message; `length` or anything else is not.
        if !matches!(choice["finish_reason"].as_str(), Some("tool_calls" | "stop"))
            || available.is_empty()
            || calls.len() > web.limits.chat_tool_calls - used_calls
        {
            return Err(error("WEB_TOOL_LIMIT", "model exceeded the bounded tool protocol"));
        }
        // Validate the whole protocol batch before the first request. In
        // particular a malformed trailing call cannot hide behind a valid read.
        let mut call_ids = std::collections::BTreeSet::new();
        let mut closed_search = false;
        for call in calls {
            let id = call["id"]
                .as_str()
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                })
                .ok_or_else(|| error("WEB_TOOL_RESPONSE", "invalid tool call identifier"))?;
            if !call_ids.insert(id.to_string()) || call["type"] != "function" {
                return Err(error(
                    "WEB_TOOL_RESPONSE",
                    "duplicate identifier or unsupported call type",
                ));
            }
            let name = call["function"]["name"].as_str().unwrap_or("");
            let args = call["function"]["arguments"]
                .as_str()
                .filter(|s| s.len() <= 4096)
                .ok_or_else(|| error("WEB_TOOL_ARGUMENTS", "invalid tool arguments"))?;
            if is_web_search_name(name) && !route.allows_web_search() {
                closed_search = true;
                continue;
            }
            if !web_mode.allows(name) {
                return Err(error(
                    "WEB_TOOL_DENIED",
                    "model requested a tool excluded by the goal web mode",
                ));
            }
            match name {
                "web_search" => {
                    serde_json::from_str::<SearchArgs>(args)
                        .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid search arguments"))?;
                }
                "web_fetch" => {
                    serde_json::from_str::<FetchArgs>(args)
                        .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid fetch arguments"))?;
                }
                _ => return Err(error("WEB_TOOL_DENIED", "model requested an unavailable tool")),
            }
        }
        if closed_search {
            tools_withheld = true;
            events.push(json!({"tool":"web_search","ok":false,"code":"WEB_TOOL_DENIED","message":WEB_SEARCH_CLOSED}));
            continue;
        }
        // The follow-up resends this prompt with the batch and a result for each
        // call; the model may batch every call it has left. Each call needs room for
        // its minimal result, at least the calibrated floor, and more for a long URL
        // or query, which are never cut. When that does not fit, run none of the
        // reads and ask again without tools.
        let batch = json!({"role":"assistant","content":message["content"],"tool_calls":calls});
        let sent = system.len() + serde_json::to_vec(&messages)?.len() + 1 + serde_json::to_vec(&batch)?.len();
        let reserves = calls
            .iter()
            .map(|call| {
                let minimal = minimal_result(
                    call["function"]["name"].as_str().unwrap_or(""),
                    call["function"]["arguments"].as_str().unwrap_or(""),
                );
                let id = call["id"].as_str().unwrap_or("");
                let bytes = 1 + serde_json::to_vec(&tool_message(id, &minimal)?)?.len();
                Ok(prompt_estimate(bytes, token_ratio).max(min_result_tokens(token_ratio)))
            })
            .collect::<Result<Vec<u64>>>()?;
        if prompt_estimate(sent, token_ratio)
            .saturating_add(reserves.iter().sum::<u64>())
            .saturating_add(reservation)
            > web.limits.total_tokens
        {
            tools_withheld = true;
            events.push(json!({"tool":"web","ok":false,"code":"WEB_TOKEN_BUDGET",
                "message":format!("web.total_tokens has no room for a minimal result from each call in this {}-call batch; none ran, and web tools were withdrawn", calls.len())}));
            continue;
        }
        used_calls += calls.len();
        messages.push(batch);
        let mut batch_failures: Vec<(String, String)> = Vec::new();
        let mut batch_ok = 0usize;
        for (index, call) in calls.iter().enumerate() {
            let id = call["id"]
                .as_str()
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                })
                .ok_or_else(|| error("WEB_TOOL_RESPONSE", "invalid tool call identifier"))?;
            if call["type"] != "function" {
                return Err(error("WEB_TOOL_RESPONSE", "unsupported tool call type"));
            }
            let name = call["function"]["name"].as_str().unwrap_or("");
            let args = call["function"]["arguments"]
                .as_str()
                .filter(|s| s.len() <= 4096)
                .ok_or_else(|| error("WEB_TOOL_ARGUMENTS", "invalid tool arguments"))?;
            authorize_owner(state, owner)?;
            let argv = tool_argv(name, args);
            // The URL or query as asked, for the session's web ledger (`web_ledger`).
            let target = argv.first().cloned().unwrap_or_default();
            assert_allowed(name, &argv, &chat_callable_names(web_mode), &state.audit)
                .map_err(|e| error("WEB_TOOL_DENIED", &e.message))?;
            let result = if is_web_search_name(name) && !route.allows_web_search() {
                Err(error("WEB_TOOL_DENIED", WEB_SEARCH_CLOSED))
            } else {
                match name {
                    "web_search" => {
                        let args: SearchArgs = serde_json::from_str(args)
                            .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid search arguments"))?;
                        // The tool contract bounds the RAW query length; enforce it before
                        // whitespace normalisation and the intent rewrite so neither can
                        // shrink a long instruction under the limit. The count was already
                        // clamped to 1–10 when parsed. A refused argument is an ordinary
                        // tool failure (bounded reply, usage recorded), not a run error:
                        // the model already spent the turn that produced it.
                        let raw_len = args.query.chars().count();
                        let query = args.query.split_whitespace().collect::<Vec<_>>().join(" ");
                        let planned =
                            if !(1..=10).contains(&args.count) || raw_len > crate::server::schemas::MAX_WEB_QUERY_LEN {
                                Err(error(
                                    "WEB_BAD_QUERY",
                                    "search needs a query of 1–200 characters and 1–10 results",
                                ))
                            } else {
                                match crate::server::web_intent::parse_with_count(&query, args.count) {
                                    crate::server::web_intent::WebIntentParse::Rewrite(intent) => {
                                        Ok((intent.query(), intent.count))
                                    }
                                    crate::server::web_intent::WebIntentParse::PassThrough => Ok((query, args.count)),
                                    crate::server::web_intent::WebIntentParse::Invalid(message) => {
                                        Err(error("WEB_BAD_QUERY", &message))
                                    }
                                }
                            };
                        match planned {
                            Err(failure) => Err(failure),
                            Ok((query, count)) => {
                                let key = (query.to_lowercase(), count);
                                if let Some(result) = searches.get(&key) {
                                    Ok(Value::clone(result))
                                } else {
                                    let result = web.google_search(&query, count, true, &state.audit).await;
                                    if let Ok(value) = &result {
                                        searches.insert(key, value.clone());
                                    }
                                    result
                                }
                            }
                        }
                    }
                    "web_fetch" => {
                        let args: FetchArgs = serde_json::from_str(args)
                            .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid fetch arguments"))?;
                        web.fetch(&args.url, true, &state.audit, owner).await.map(|page| {
                    json!({"url":page["url"],"title":page["title"],"text":crate::common::clip_chars(page["text"].as_str().unwrap_or(""), (web.limits.evidence_tokens * 4) as usize),
                        "source_chars":page["chars"],"notice":FETCH_NOTICE})
                })
                    }
                    _ => return Err(error("WEB_TOOL_DENIED", "model requested an unavailable tool")),
                }
            };
            // The follow-up call sends everything again: cut the result to the room
            // web.total_tokens has left for it, keeping a minimal result's room for
            // each later call in this batch. Sized without the tool definitions: a
            // later call sends them only after its round check, which needs more room.
            let sent = system.len() + serde_json::to_vec(&messages)?.len();
            let later = reserves[index + 1..].iter().sum::<u64>();
            let result = match result {
                Ok(value) => fit_result(value, |candidate| {
                    let bytes = sent + 1 + serde_json::to_vec(&tool_message(id, candidate)?)?.len();
                    Ok(prompt_estimate(bytes, token_ratio)
                        .saturating_add(reservation)
                        .saturating_add(later)
                        <= web.limits.total_tokens)
                })?
                .ok_or_else(|| error("WEB_TOKEN_BUDGET", "web.total_tokens has no room left for this result")),
                Err(failure) => Err(failure),
            };
            check_evidence(state, owner, &sources)?;
            // A refused or failed read is that call's result, not the end of the
            // turn: the model is handed the harness's own code and message (never
            // page text) and answers from the reads that did succeed. A refused
            // URL is still never fetched, and the event stays `ok: false` for the
            // console. Before, one denied or unreachable URL in a batch discarded
            // every sibling result and ended the turn with no answer, which also
            // stopped an unattended /loop.
            let result = match result {
                Ok(result) => {
                    batch_ok += 1;
                    result
                }
                Err(failure) => {
                    events.push(
                        json!({"tool":name,"ok":false,"target":target,"code":failure.code,"message":failure.message}),
                    );
                    batch_failures.push((failure.code.clone(), failure.message.clone()));
                    json!({"error":failure.code,"message":failure.message,
                        "notice":"This read did not run or did not fit. Answer from the other results, or say what could not be read."})
                }
            };
            if result.get("error").is_none() {
                route.observe_tool_result(name, true);
                // Provider listings (SerpAPI, loopback SearXNG) are not URL evidence.
                if !matches!(result["provider"].as_str(), Some("google-serpapi" | "searxng")) {
                    if let Some(source) = result
                        .get("search_url")
                        .or_else(|| result.get("url"))
                        .and_then(Value::as_str)
                    {
                        sources.push(source.to_string());
                    }
                }
                check_evidence(state, owner, &sources)?;
                events.push(json!({"tool":name,"ok":true,"target":target,"result":result}));
            }
            messages.push(tool_message(id, &result)?);
        }
        if batch_ok == 0 {
            if sources.is_empty() && searches.is_empty() {
                // Every call failed and nothing has succeeded this turn: a refusal
                // the model cannot fix by asking again (no URL permitted, keyless
                // search, an unreachable site). End the turn as before, with the
                // first failure named, rather than spend the remaining rounds on it.
                let (code, message) = batch_failures.first().cloned().unwrap_or_default();
                return Ok((
                    ChatResult {
                        body_text: format!(
                            "Web operation failed: {code} — {message}. No successful result was obtained."
                        ),
                        model: model.into(),
                        prompt_tokens,
                        completion_tokens,
                        usage_reported: totals.usage_reported(),
                        reported_prompt_tokens: totals.input_tokens,
                        reported_completion_tokens: totals.output_tokens,
                        initial_prompt_tokens,
                        initial_prompt_tools,
                        // The last model call offered tools: it returned this batch.
                        final_prompt_tools: true,
                        peak_prompt_extra_tokens: peak_extra,
                    },
                    events,
                ));
            }
            // Evidence exists but this whole batch failed: the model is told and
            // answers from what it has; no further round is offered.
            tools_withheld = true;
        }
    }
    Err(error("WEB_TOOL_LIMIT", "chat tool limit exceeded"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_turn_keeps_the_loop_budget_and_a_truncated_answer_asks_again_at_synthesis() {
        let tool = crate::llm::openai_chat::sampling_for(
            hop_kind(false, true),
            4096,
            0.1,
            crate::llm::openai_chat::DEFAULT_TOOL_MAX_TOKENS,
        );
        let synthesis = crate::llm::openai_chat::sampling_for(
            hop_kind(false, false),
            4096,
            0.1,
            crate::llm::openai_chat::DEFAULT_TOOL_MAX_TOKENS,
        );
        let loop_hop = crate::llm::openai_chat::sampling_for(
            hop_kind(true, true),
            2048,
            0.1,
            crate::llm::openai_chat::DEFAULT_TOOL_MAX_TOKENS,
        );
        assert_eq!((tool.max_tokens, tool.temperature), (512, 0.0));
        assert_eq!((synthesis.max_tokens, synthesis.temperature), (4096, 0.1));
        assert_eq!(
            (loop_hop.max_tokens, loop_hop.temperature),
            (2048, 0.1),
            "loop turns use the loop token budget"
        );
        assert!(truncated_answer_needs_synthesis(512, 4096, false, Some("length")));
        assert!(!truncated_answer_needs_synthesis(512, 4096, true, Some("length")));
        assert!(!truncated_answer_needs_synthesis(512, 4096, false, Some("stop")));
        assert!(
            !truncated_answer_needs_synthesis(loop_hop.max_tokens, loop_hop.max_tokens, false, Some("length")),
            "a loop turn already ran at its answer budget"
        );
        let fallback =
            crate::llm::openai_chat::sampling_for(crate::llm::openai_chat::TurnKind::Synthesis, 4096, 0.1, 512);
        assert_eq!(fallback.max_tokens, 4096);
        assert_eq!(fallback.temperature, 0.1);
    }

    #[test]
    fn sticky_route_denies_web_search_after_a_local_read() {
        let mut route = StickyRoute::from_user_message("What does my note say about cedar?");
        assert!(route.allows_web_search());
        route.observe_tool_result("notes_corpus", true);
        assert!(!route.allows_web_search());
        assert!(narrow_tools(&tools(), &route)
            .iter()
            .all(|tool| tool["function"]["name"] != "web_search"));
        assert!(narrow_tools(&tools(), &route)
            .iter()
            .any(|tool| tool["function"]["name"] == "web_fetch"));
        route.observe_tool_result("web_search", true);
        assert!(!route.allows_web_search(), "a web hit does not reopen search");
        let mut failed = StickyRoute::from_user_message("What does my note say about cedar?");
        failed.observe_tool_result("read_notes", false);
        assert!(failed.allows_web_search());
        let mut asked = StickyRoute::from_user_message("Please search the web for cedar prices");
        asked.observe_tool_result("private_notes", true);
        asked.observe_notes_in_prompt("source=notes_corpus\ncedar: 5");
        assert!(asked.allows_web_search());
        let mut sources = StickyRoute::from_user_message("Compare my notes with web sources.");
        sources.observe_notes_in_prompt("source=notes_corpus\ncedar: 5");
        assert!(sources.allows_web_search());
        assert!(narrow_tools(&tools(), &sources)
            .iter()
            .any(|tool| tool["function"]["name"] == "web_search"));
        assert!(narrow_tools(&tools(), &asked)
            .iter()
            .any(|tool| tool["function"]["name"] == "web_search"));
        for alias in ["web_search", "search_web", "websearch"] {
            assert!(is_web_search_name(alias));
            assert!(!route.allows_web_search());
        }
    }

    #[test]
    fn web_search_tool_tells_the_model_not_to_retry_a_paced_or_cooldown_reply() {
        let tools = tools();
        let description = tools
            .iter()
            .find(|tool| tool["function"]["name"] == "web_search")
            .and_then(|tool| tool["function"]["description"].as_str())
            .unwrap_or("");
        assert!(
            description.contains("WEB_SEARCH_COOLDOWN or WEB_SEARCH_PACED"),
            "{description}"
        );
        assert!(
            description.contains("do not call web_search again this turn"),
            "{description}"
        );
    }

    fn size(value: &Value) -> usize {
        serde_json::to_vec(value).unwrap().len()
    }

    #[test]
    fn fit_result_keeps_the_longest_fitting_prefix_or_listing() {
        let page = json!({"url":"https://example.test/a","text":"abcdefghij","notice":"n"});
        let full = size(&page);
        assert_eq!(fit_result(page.clone(), |_| Ok(true)).unwrap().unwrap(), page);
        let fitted = fit_result(page.clone(), |v| Ok(size(v) <= full - 3)).unwrap().unwrap();
        assert_eq!(fitted["text"], "abcdefg");
        assert_eq!(fitted["url"], page["url"]);
        // An empty page is no evidence: not even one character fits.
        assert!(fit_result(page, |v| Ok(size(v) <= full - 10)).unwrap().is_none());
        // Prefixes are whole characters, never split UTF-8.
        let wide = json!({"text":"界界界界"});
        let fitted = fit_result(wide.clone(), |v| Ok(size(v) < size(&wide)))
            .unwrap()
            .unwrap();
        assert_eq!(fitted["text"], "界界界");
        // Listings drop trailing results and keep at least one.
        let listing = json!({"provider":"p","results":[{"title":"1"},{"title":"2"},{"title":"3"}]});
        let full = size(&listing);
        let fitted = fit_result(listing.clone(), |v| Ok(size(v) < full - 10))
            .unwrap()
            .unwrap();
        assert_eq!(fitted["results"].as_array().unwrap().len(), 2);
        assert!(fit_result(listing, |v| Ok(size(v) < 10)).unwrap().is_none());
    }

    #[test]
    fn fit_result_cuts_a_title_or_snippet_only_when_no_text_fits_beside_it() {
        let url = "https://example.test/a";
        let page = json!({"url":url,"title":"T".repeat(400),"text":"abcdefghij","notice":"n"});
        // While one character of text fits beside it, the title stays whole.
        let room = size(&json!({"url":url,"title":"T".repeat(400),"text":"abc","notice":"n"}));
        let fitted = fit_result(page.clone(), |v| Ok(size(v) <= room)).unwrap().unwrap();
        assert_eq!((&fitted["title"], &fitted["text"]), (&page["title"], &json!("abc")));
        // Otherwise the title is cut and one character of text stays.
        let room = size(&json!({"url":url,"title":"T".repeat(100),"text":"a","notice":"n"}));
        let fitted = fit_result(page.clone(), |v| Ok(size(v) <= room)).unwrap().unwrap();
        assert_eq!(fitted["title"], "T".repeat(100));
        assert_eq!((&fitted["text"], &fitted["url"]), (&json!("a"), &json!(url)));
        // The URL is never cut.
        let room = size(&json!({"url":url,"title":"","text":"a","notice":"n"}));
        assert!(fit_result(page, |v| Ok(size(v) < room)).unwrap().is_none());
        // A listing keeps one result, then cuts its snippet, then its title.
        let row = |title: usize, snippet: usize| json!({"rank":1,"title":"T".repeat(title),"url":url,"snippet":"S".repeat(snippet)});
        let listing = json!({"query":"q","results":[row(50, 300),row(1, 1)]});
        let room = size(&json!({"query":"q","results":[row(50, 20)]}));
        let fitted = fit_result(listing.clone(), |v| Ok(size(v) <= room)).unwrap().unwrap();
        assert_eq!(fitted["results"], json!([row(50, 20)]));
        let room = size(&json!({"query":"q","results":[row(10, 0)]}));
        let fitted = fit_result(listing.clone(), |v| Ok(size(v) <= room)).unwrap().unwrap();
        assert_eq!(fitted["results"], json!([row(10, 0)]));
        // A result that does not fit even bare is dropped too, and counted.
        let room = size(&json!({"query":"q","omitted_results":2,"results":[]}));
        let fitted = fit_result(listing.clone(), |v| Ok(size(v) <= room)).unwrap().unwrap();
        assert_eq!(
            (&fitted["results"], &fitted["omitted_results"]),
            (&json!([]), &json!(2))
        );
        assert!(fit_result(listing, |v| Ok(size(v) < room)).unwrap().is_none());
    }

    #[test]
    fn small_model_tool_arguments_parse_without_widening_what_is_read() {
        let search = |args: &str| serde_json::from_str::<SearchArgs>(args).map(|a| (a.query, a.count));
        assert_eq!(search(r#"{"query":"q"}"#).unwrap(), ("q".into(), 5));
        assert_eq!(search(r#"{"query":"q","count":"3"}"#).unwrap(), ("q".into(), 3));
        assert_eq!(search(r#"{"query":"q","count":null}"#).unwrap(), ("q".into(), 5));
        assert_eq!(search(r#"{"query":"q","count":4.0}"#).unwrap(), ("q".into(), 4));
        assert_eq!(search(r#"{"query":"q","count":50}"#).unwrap(), ("q".into(), 10));
        assert_eq!(search(r#"{"query":"q","count":0}"#).unwrap(), ("q".into(), 1));
        assert_eq!(search(r#"{"query":"q","num_results":3}"#).unwrap(), ("q".into(), 5));
        for refused in [
            r#"{}"#,
            r#"{"query":7}"#,
            r#"{"query":"q","count":"many"}"#,
            r#"{"query":"q","count":2.5}"#,
            r#"{"query":"q","count":-1}"#,
        ] {
            assert!(search(refused).is_err(), "{refused}");
        }
        // An extra key is ignored, never followed: only `url` is read.
        let fetch =
            serde_json::from_str::<FetchArgs>(r#"{"url":"https://a.test/","follow":"https://b.test/"}"#).unwrap();
        assert_eq!(fetch.url, "https://a.test/");
        assert_eq!(
            tool_argv("web_fetch", r#"{"url":"https://a.test/","method":"POST"}"#),
            vec!["https://a.test/"]
        );
        assert!(serde_json::from_str::<FetchArgs>(r#"{"href":"https://a.test/"}"#).is_err());
    }
}
