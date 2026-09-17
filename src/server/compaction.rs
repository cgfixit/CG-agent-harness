//! Prompt-window compaction for local chat sessions.
//!
//! Trigger uses estimated next-prompt size (UTF-8 bytes / 4 plus the reply
//! ceiling). That is the English-model money ruler, not a CJK tokenizer.
//! System prompt and `Session.goal`
//! are composed each turn and are never stored in `messages`, so they cannot
//! be compacted. The first user message is kept verbatim.

use crate::common::now_ts;
use crate::llm::openai_chat::ChatMessage;
use crate::server::sessions::Message;

pub const COMPACT_PREFIX: &str = "[session-compacted]\n";
pub const DEFAULT_PROMPT_TOKENS: u64 = 24_000;
pub const DEFAULT_KEEP_MESSAGES: u64 = 8;
pub const DEFAULT_REPLY_TOKENS: u64 = 4_096;
pub const MIN_PROMPT_HEADROOM: u64 = 4_096;
pub const MAX_PROMPT_TOKENS: u64 = 30_000;
pub const MAX_REPLY_TOKENS: u64 = MAX_PROMPT_TOKENS - MIN_PROMPT_HEADROOM;

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

pub fn compact_messages(messages: &[Message], keep_recent: usize) -> Vec<Message> {
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
    let middle_start = match first_user {
        Some(idx) if idx < tail_start => idx + 1,
        _ => 0,
    };
    if middle_start < tail_start {
        let middle = &messages[middle_start..tail_start];
        if !middle.is_empty() {
            out.push(Message {
                role: "assistant".into(),
                text: format_summary(middle),
                ts: now_ts(),
            });
        }
    }
    out.extend(messages[tail_start..].iter().cloned());
    out
}

fn format_summary(middle: &[Message]) -> String {
    let mut lines = Vec::new();
    for msg in middle {
        let snippet: String = msg.text.chars().take(160).collect();
        lines.push(format!("{}: {snippet}", msg.role));
    }
    let mut out = String::from(COMPACT_PREFIX);
    out.push_str("Older turns were summarized to keep the next prompt under the compact threshold.\n");
    out.push_str("Turns:\n");
    for line in lines.into_iter().take(24) {
        out.push_str("- ");
        out.push_str(&line);
        out.push('\n');
    }
    out
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
        let out = compact_messages(&messages, 2);
        assert_eq!(out[0].text, "original prompt about parser.rs");
        assert!(out[1].text.starts_with(COMPACT_PREFIX));
        assert_eq!(out[2].text, "latest");
        assert_eq!(out[3].text, "done");
        assert!(!out.iter().any(|m| m.role == "system"));
    }

    #[test]
    fn small_histories_are_unchanged() {
        let messages = vec![msg("user", "hi"), msg("assistant", "hello")];
        assert_eq!(compact_messages(&messages, 8), messages);
    }

    #[test]
    fn projection_uses_next_prompt_not_lifetime_tally() {
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "abcd".into(),
        }];
        let projected = projected_prompt_tokens("sys", &history, "efgh", 10);
        assert_eq!(projected, 1 + 1 + 1 + 10);
        assert_eq!(estimate_tokens("abcd"), 1);
    }
}
