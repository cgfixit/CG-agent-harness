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

/// Estimated tokens of a model call: UTF-8 bytes / 4 of what is sent, calibrated.
fn prompt_estimate(bytes: usize, token_ratio: f64) -> u64 {
    super::compaction::calibrated_tokens((bytes as u64).div_ceil(4), token_ratio)
}

/// Room kept for one minimal tool result, calibrated like the estimate that
/// later charges it.
fn min_result_tokens(token_ratio: f64) -> u64 {
    super::compaction::calibrated_tokens(MIN_EVIDENCE_TOKENS, token_ratio)
}

fn tool_message(id: &str, result: &Value) -> Result<Value> {
    Ok(json!({"role":"tool","tool_call_id":id,"content":serde_json::to_string(result)?}))
}

/// Largest form of a tool result that `fits` accepts. A fetched page keeps its
/// longest fitting prefix, a search listing drops trailing results. `None` when
/// not even one character or one result fits.
fn fit_result(mut result: Value, mut fits: impl FnMut(&Value) -> Result<bool>) -> Result<Option<Value>> {
    if fits(&result)? {
        return Ok(Some(result));
    }
    if let Some(text) = result.get("text").and_then(Value::as_str) {
        let chars: Vec<char> = text.chars().collect();
        // Prefix `hi` does not fit; find the longest prefix `lo` that does.
        let (mut lo, mut hi) = (0, chars.len());
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            result["text"] = Value::String(chars[..mid].iter().collect());
            if fits(&result)? {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        result["text"] = Value::String(chars[..lo].iter().collect());
        return Ok((lo > 0 && fits(&result)?).then_some(result));
    }
    while let Some(listing) = result.get_mut("results").and_then(Value::as_array_mut) {
        if listing.len() <= 1 {
            break;
        }
        listing.pop();
        if fits(&result)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
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
    let mut used_calls = 0;
    let mut prompt_tokens = 0u64;
    let mut completion_tokens = 0u64;
    let mut reported = true;
    let mut initial_prompt_tokens = None;
    let mut budget_used = 0u64;
    let definitions = tools();
    let definition_bytes = serde_json::to_vec(&definitions)?.len();
    let mut tools_withheld = false;
    for turn in 0..=web.limits.chat_tool_calls {
        check_evidence(state, owner, &sources)?;
        let estimate = prompt_estimate(
            system.len() + serde_json::to_vec(&messages)?.len() + definition_bytes,
            token_ratio,
        );
        let reservation = super::compaction::reply_reservation(&state.backend, cap);
        if budget_used.saturating_add(estimate).saturating_add(reservation) > web.limits.total_tokens {
            return Err(error("WEB_TOKEN_BUDGET", "chat web token budget exhausted"));
        }
        // A tool call sends this whole prompt again with its result. Offer tools
        // only while that follow-up still fits beside a minimal result.
        let round_fits = budget_used
            .saturating_add(estimate.saturating_mul(2))
            .saturating_add(min_result_tokens(token_ratio))
            .saturating_add(reservation)
            <= web.limits.total_tokens;
        if !round_fits && used_calls < web.limits.chat_tool_calls && !tools_withheld {
            tools_withheld = true;
            events.push(json!({"tool":"web","ok":false,"code":"WEB_TOKEN_BUDGET",
                "message":"web.total_tokens has no room to send this prompt again with a result; web tools were not offered"}));
        }
        let available = if used_calls == web.limits.chat_tool_calls || !round_fits {
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
            .filter(|calls| !calls.is_empty())
            .ok_or_else(|| {
                error(
                    "WEB_TOOL_RESPONSE",
                    "model must return valid read-only tool calls or a complete answer",
                )
            })?;
        if choice["finish_reason"] != "tool_calls"
            || available.is_empty()
            || calls.len() > web.limits.chat_tool_calls - used_calls
        {
            return Err(error("WEB_TOOL_LIMIT", "model exceeded the bounded tool protocol"));
        }
        // Validate the whole protocol batch before the first request. In
        // particular a malformed trailing call cannot hide behind a valid read.
        let mut call_ids = std::collections::BTreeSet::new();
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
            let args = call["function"]["arguments"]
                .as_str()
                .filter(|s| s.len() <= 4096)
                .ok_or_else(|| error("WEB_TOOL_ARGUMENTS", "invalid tool arguments"))?;
            match call["function"]["name"].as_str() {
                Some("web_search") => {
                    serde_json::from_str::<SearchArgs>(args)
                        .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid search arguments"))?;
                }
                Some("web_fetch") => {
                    serde_json::from_str::<FetchArgs>(args)
                        .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid fetch arguments"))?;
                }
                _ => return Err(error("WEB_TOOL_DENIED", "model requested an unavailable tool")),
            }
        }
        used_calls += calls.len();
        messages.push(json!({"role":"assistant","content":message["content"],"tool_calls":calls}));
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
            assert_allowed(name, &argv, &chat_callable_names(true), &state.audit)
                .map_err(|e| error("WEB_TOOL_DENIED", &e.message))?;
            let result = match name {
                "web_search" => {
                    let args: SearchArgs = serde_json::from_str(args)
                        .map_err(|_| error("WEB_TOOL_ARGUMENTS", "invalid search arguments"))?;
                    // The tool contract bounds the RAW arguments; enforce both before
                    // whitespace normalisation and the intent rewrite so neither can
                    // shrink a long instruction under the limit and a textual
                    // `first N` cannot mask an out-of-range count. A refused argument
                    // is an ordinary tool failure (bounded reply, usage recorded), not
                    // a run error: the model already spent the turn that produced it.
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
                        "source_chars":page["chars"],"notice":"Untrusted fetched page text; excerpt may be truncated."})
                })
                }
                _ => return Err(error("WEB_TOOL_DENIED", "model requested an unavailable tool")),
            };
            // The follow-up call sends everything again: cut the result to the room
            // web.total_tokens has left for it, with the estimate the next call uses,
            // keeping a minimal result's room for each later call in this batch.
            let sent = system.len() + serde_json::to_vec(&messages)?.len() + definition_bytes;
            let later = min_result_tokens(token_ratio).saturating_mul((calls.len() - index - 1) as u64);
            let result = match result {
                Ok(value) => fit_result(value, |candidate| {
                    let bytes = sent + 1 + serde_json::to_vec(&tool_message(id, candidate)?)?.len();
                    Ok(budget_used
                        .saturating_add(prompt_estimate(bytes, token_ratio))
                        .saturating_add(reservation)
                        .saturating_add(later)
                        <= web.limits.total_tokens)
                })?
                .ok_or_else(|| error("WEB_TOKEN_BUDGET", "web.total_tokens has no room left for this result")),
                Err(failure) => Err(failure),
            };
            check_evidence(state, owner, &sources)?;
            let result = match result {
                Ok(result) => result,
                Err(failure) => {
                    events.push(json!({"tool":name,"ok":false,"code":failure.code,"message":failure.message}));
                    return Ok((
                        ChatResult {
                            body_text: format!(
                                "Web operation failed: {} — {}. {}",
                                failure.code,
                                failure.message,
                                if sources.is_empty() && searches.is_empty() {
                                    "No successful result was obtained."
                                } else {
                                    "Earlier reads succeeded, but this research is incomplete; no synthesis was produced."
                                }
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
            messages.push(tool_message(id, &result)?);
        }
    }
    Err(error("WEB_TOOL_LIMIT", "chat tool limit exceeded"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
