//! The chat model may request bounded web reads, never authority changes.
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::{state::AppState, web_policy::error, web_research::authorize_owner, web_search::WebTool};
use crate::common::errors::Result;
use crate::common::tool_broker::assert_allowed;
use crate::llm::openai_chat::{parse_chat_response, ChatMessage, ChatResult};
use crate::server::tool_inventory::chat_callable_names;

pub fn tools() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"web_search","description":"Search Google for current ranked links and snippets. Listings are not fetched destination pages. Use for explicit search or current-web questions. Resolve references using conversation context; write one focused query with the relevant entities and constraints. Start with five results. Reuse evidence, avoid repeated queries, and stop once you can answer. Search again only for a specific unresolved gap. Do not send credentials or private conversation text.","parameters":{"type":"object","properties":{"query":{"type":"string","maxLength":200},"count":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"web_fetch","description":"Read an exact public URL permitted by the administrator. Use when asked to read, retrieve, fetch or summarize a URL. Never grants permission or follows redirects.","parameters":{"type":"object","properties":{"url":{"type":"string","maxLength":2048}},"required":["url"],"additionalProperties":false}}}),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default = "five")]
    count: usize,
}
fn five() -> usize {
    5
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
) -> Result<(ChatResult, Vec<Value>)> {
    let lease = web.chat_turn.start(owner)?;
    tokio::select! {
        biased;
        _ = lease.token.cancelled() => Err(error("WEB_CANCELLED", "chat web turn cancelled")),
        result = tokio::time::timeout(Duration::from_secs_f64(state.chat.timeout_sec.max(1.0)), run_inner(state, web, owner, system, history, model, cap, temperature, token_ratio, output)) =>
            result.map_err(|_| error("WEB_TIMEOUT", "chat web turn deadline exceeded"))?,
    }
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
    output: Option<&tokio::sync::mpsc::Sender<Value>>,
) -> Result<(ChatResult, Vec<Value>)> {
    let mut messages: Vec<Value> = history
        .iter()
        .map(|m| json!({"role":m.role,"content":m.content}))
        .collect();
    let mut events = Vec::new();
    let mut sources = Vec::new();
    let mut searches = std::collections::BTreeMap::new();
    let mut prompt_tokens = 0u64;
    let mut completion_tokens = 0u64;
    let mut reported = true;
    let mut initial_prompt_tokens = None;
    let mut budget_used = 0u64;
    let definitions = tools();
    for turn in 0..=web.limits.chat_tool_calls {
        check_evidence(state, owner, &sources)?;
        let estimate = ((system.len() + serde_json::to_vec(&messages)?.len() + serde_json::to_vec(&definitions)?.len())
            as u64)
            .div_ceil(4);
        let estimate = super::compaction::calibrated_tokens(estimate, token_ratio);
        let reservation = super::compaction::reply_reservation(&state.backend, cap);
        if budget_used.saturating_add(estimate).saturating_add(reservation) > web.limits.total_tokens {
            return Err(error("WEB_TOKEN_BUDGET", "chat web token budget exhausted"));
        }
        let available = if turn == web.limits.chat_tool_calls {
            &[][..]
        } else {
            definitions.as_slice()
        };
        let validate = || check_evidence(state, owner, &sources);
        let response = state
            .chat
            .chat_with_tools_stream(
                system,
                &messages,
                model,
                cap,
                temperature,
                available,
                output.map(|sender| crate::llm::openai_stream::Output {
                    sender,
                    validate: &validate,
                }),
            )
            .await?;
        let usage = &response["usage"];
        if turn == 0 {
            initial_prompt_tokens = crate::llm::openai_chat::initial_prompt_tokens(&response);
        }
        reported &= usage["prompt_tokens"].is_u64() && usage["completion_tokens"].is_u64();
        budget_used = budget_used
            .saturating_add(usage["prompt_tokens"].as_u64().unwrap_or(estimate))
            .saturating_add(usage["completion_tokens"].as_u64().unwrap_or(cap));
        prompt_tokens = prompt_tokens.saturating_add(crate::llm::openai_chat::token_count(usage.get("prompt_tokens")));
        completion_tokens =
            completion_tokens.saturating_add(crate::llm::openai_chat::token_count(usage.get("completion_tokens")));
        check_evidence(state, owner, &sources)?;
        let choice = &response["choices"][0];
        let message = &choice["message"];
        if choice["finish_reason"] != "tool_calls"
            && message
                .get("tool_calls")
                .is_none_or(|c| c.as_array().is_some_and(Vec::is_empty))
        {
            let mut reply = parse_chat_response(&response, model)?;
            reply.prompt_tokens = prompt_tokens;
            reply.completion_tokens = completion_tokens;
            reply.usage_reported = reported;
            reply.initial_prompt_tokens = initial_prompt_tokens;
            return Ok((reply, events));
        }
        let calls = message["tool_calls"]
            .as_array()
            .filter(|calls| calls.len() == 1)
            .ok_or_else(|| {
                error(
                    "WEB_TOOL_RESPONSE",
                    "model must return one valid read-only tool call or a complete answer",
                )
            })?;
        if choice["finish_reason"] != "tool_calls" || available.is_empty() {
            return Err(error("WEB_TOOL_LIMIT", "model exceeded the bounded tool protocol"));
        }
        let call = &calls[0];
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
        assert_allowed(name, &argv, &chat_callable_names(true), &state.audit)
            .map_err(|e| error("WEB_TOOL_DENIED", &e.message))?;
        let result = match name {
            "web_search" => {
                let args: SearchArgs =
                    serde_json::from_str(args).map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid search arguments"))?;
                // The tool contract bounds the RAW arguments; enforce both before
                // whitespace normalisation and the intent rewrite so neither can
                // shrink a long instruction under the limit and a textual
                // `first N` cannot mask an out-of-range count. A refused argument
                // is an ordinary tool failure (bounded reply, usage recorded), not
                // a run error: the model already spent the turn that produced it.
                let raw_len = args.query.chars().count();
                let query = args.query.split_whitespace().collect::<Vec<_>>().join(" ");
                let planned = if !(1..=10).contains(&args.count) || raw_len > crate::server::schemas::MAX_WEB_QUERY_LEN
                {
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
                let args: FetchArgs =
                    serde_json::from_str(args).map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid fetch arguments"))?;
                web.fetch(&args.url, true, &state.audit, owner).await.map(|page| {
                    json!({"url":page["url"],"title":page["title"],"text":crate::common::clip_chars(page["text"].as_str().unwrap_or(""), (web.limits.evidence_tokens * 4) as usize),
                        "source_chars":page["chars"],"notice":"Untrusted fetched page text; excerpt may be truncated."})
                })
            }
            _ => return Err(error("WEB_TOOL_DENIED", "model requested an unavailable tool")),
        };
        check_evidence(state, owner, &sources)?;
        let result = match result {
            Ok(result) => result,
            Err(failure) => {
                events.push(json!({"tool":name,"ok":false,"code":failure.code,"message":failure.message}));
                return Ok((
                    ChatResult {
                        body_text: format!(
                            "Web operation failed: {} — {}. No successful result was obtained.",
                            failure.code, failure.message
                        ),
                        model: model.into(),
                        prompt_tokens,
                        completion_tokens,
                        usage_reported: reported,
                        initial_prompt_tokens,
                    },
                    events,
                ));
            }
        };
        if result["provider"] != "google-serpapi" {
            if let Some(source) = result
                .get("search_url")
                .or_else(|| result.get("url"))
                .and_then(Value::as_str)
            {
                sources.push(source.to_string());
            }
        }
        check_evidence(state, owner, &sources)?;
        events.push(json!({"tool":name,"ok":true,"result":result}));
        messages.push(json!({"role":"assistant","content":message["content"],"tool_calls":calls}));
        messages.push(json!({"role":"tool","tool_call_id":id,"content":serde_json::to_string(&result)?}));
    }
    Err(error("WEB_TOOL_LIMIT", "chat tool limit exceeded"))
}
