//! Prompt-window compaction for local chat sessions.
//!
//! Trigger uses estimated next-prompt size (UTF-8 bytes / 4 plus the reply
//! ceiling), not lifetime `TokenTally::total`. System prompt and `Session.goal`
//! are composed each turn and are never stored in `messages`, so they cannot
//! be compacted. The first user message is kept verbatim.

use crate::common::errors::{HarnessError, Result};
use crate::common::now_ts;
use crate::llm::openai_chat::{ChatClient, ChatMessage};
use crate::server::sessions::Message;

pub const COMPACT_PREFIX: &str = "[session-compacted]\n";
pub const DEFAULT_PROMPT_TOKENS: u64 = 24_000;
pub const DEFAULT_KEEP_MESSAGES: u64 = 8;
pub const DEFAULT_REPLY_TOKENS: u64 = 4_096;
pub const MIN_PROMPT_HEADROOM: u64 = 4_096;
pub const MAX_PROMPT_TOKENS: u64 = 30_000;
pub const MAX_REPLY_TOKENS: u64 = MAX_PROMPT_TOKENS - MIN_PROMPT_HEADROOM;
pub const SUMMARY_MAX_TOKENS: u64 = 400;
const SUMMARY_INPUT_CHARS: usize = 24_000;
const SUMMARY_SYSTEM: &str = "Summarize this chat history for a later local-model turn. Cover goals, decisions, files touched, leftover work, and key facts. Dense prose. No preamble.";

pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Estimate the next prompt from the same bounded history that will be sent,
/// never the full persisted session: stored turns outside the send window must
/// not drive the compaction decision.
pub fn projected_prompt_tokens(system: &str, history: &[ChatMessage], user: &str, max_tokens: u64) -> u64 {
    estimate_tokens(system)
        + history.iter().map(|m| estimate_tokens(&m.content)).sum::<u64>()
        + estimate_tokens(user)
        + max_tokens
}

pub fn middle_turns(messages: &[Message], keep_recent: usize) -> &[Message] {
    let keep_recent = keep_recent.max(1);
    if messages.len() <= keep_recent + 1 {
        return &[];
    }
    let tail_start = messages.len() - keep_recent;
    let first_user = messages.iter().position(|m| m.role == "user");
    let middle_start = match first_user {
        Some(idx) if idx < tail_start => idx + 1,
        _ => 0,
    };
    if middle_start < tail_start {
        &messages[middle_start..tail_start]
    } else {
        &[]
    }
}

pub fn compact_messages(messages: &[Message], keep_recent: usize, summary: &str) -> Vec<Message> {
    let keep_recent = keep_recent.max(1);
    if messages.len() <= keep_recent + 1 {
        return messages.to_vec();
    }
    let tail_start = messages.len() - keep_recent;
    let first_user = messages.iter().position(|m| m.role == "user");
    let mut out = Vec::new();
    if let Some(idx) = first_user {
        if idx < tail_start {
            out.push(messages[idx].clone());
        }
    }
    let middle = middle_turns(messages, keep_recent);
    if !middle.is_empty() {
        out.push(Message {
            role: "assistant".into(),
            text: summary.to_string(),
            ts: now_ts(),
        });
    }
    out.extend(messages[tail_start..].iter().cloned());
    out
}

/// One bounded local-model call. Empty or failed output must not persist.
pub async fn summarize_turns(chat: &ChatClient, model: &str, middle: &[Message]) -> Result<String> {
    let mut body = String::new();
    for msg in middle {
        body.push_str(&msg.role);
        body.push_str(": ");
        body.push_str(&msg.text);
        body.push('\n');
    }
    let clipped: String = body.chars().take(SUMMARY_INPUT_CHARS).collect();
    let reply = chat
        .chat(
            SUMMARY_SYSTEM,
            &[ChatMessage {
                role: "user".into(),
                content: clipped,
            }],
            Some(model),
            SUMMARY_MAX_TOKENS,
            0.0,
        )
        .await?;
    let text = reply.body_text.trim();
    if text.is_empty() {
        return Err(HarnessError::new(
            crate::llm::openai_chat::LLM_ERROR_CODE,
            "compaction summary was empty",
        ));
    }
    let mut out = String::from(COMPACT_PREFIX);
    out.push_str(text);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            text: text.into(),
            ts: 1.0,
        }
    }

    #[test]
    fn keeps_first_user_and_recent_tail() {
        let messages = vec![
            msg("user", "original prompt about parser.rs"),
            msg("assistant", "looking"),
            msg("user", "try again"),
            msg("assistant", "still looking"),
            msg("user", "latest"),
            msg("assistant", "done"),
        ];
        let summary = format!("{COMPACT_PREFIX}goals, decisions, files, leftover work\n");
        let out = compact_messages(&messages, 2, &summary);
        assert_eq!(out[0].text, "original prompt about parser.rs");
        assert_eq!(out[1].text, summary);
        assert_eq!(out[2].text, "latest");
        assert_eq!(out[3].text, "done");
        assert!(!out.iter().any(|m| m.role == "system"));
        assert!(!out[1].text.contains("looking"));
    }

    #[test]
    fn small_histories_are_unchanged() {
        let messages = vec![msg("user", "hi"), msg("assistant", "hello")];
        assert_eq!(compact_messages(&messages, 8, "unused"), messages);
    }

    #[test]
    fn projection_uses_next_prompt_not_lifetime_tally() {
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "abcd".into(),
        }];
        let projected = projected_prompt_tokens("sys", &history, "efgh", 10);
        assert_eq!(projected, 1 + 1 + 1 + 10);
    }
}
