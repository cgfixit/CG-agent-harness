//! Prompt-window compaction for local chat sessions.
//!
//! Trigger uses next-prompt size (UTF-8 bytes / 4, calibrated from local usage)
//! plus an effective reply reservation. This remains an estimate, not a tokenizer.
//! System prompt and `Session.goal`
//! are composed each turn and are never stored in `messages`, so they cannot
//! be compacted. The first user message is kept verbatim.

use crate::common::errors::{HarnessError, Result};
use crate::common::now_ts;
use crate::llm::backend::ResolvedLocalBackend;
use crate::llm::openai_chat::{ChatClient, ChatMessage};
use crate::server::sessions::Message;
use serde::{Deserialize, Serialize};

pub const COMPACT_PREFIX: &str = "[session-compacted]\n";
pub const DEFAULT_PROMPT_TOKENS: u64 = 24_000;
pub const DEFAULT_KEEP_MESSAGES: u64 = 8;
pub const DEFAULT_REPLY_TOKENS: u64 = 4_096;
pub const MIN_PROMPT_HEADROOM: u64 = 4_096;
pub const MAX_PROMPT_TOKENS: u64 = 30_000;
pub const MAX_REPLY_TOKENS: u64 = MAX_PROMPT_TOKENS - MIN_PROMPT_HEADROOM;
pub const DEFAULT_SUMMARY_MAX_TOKENS: u64 = 768;
const SUMMARY_INPUT_CHARS: usize = 24_000;
const SUMMARY_TURN_CHARS: usize = 800;
const SUMMARY_SYSTEM: &str = "Summarize this chat history for a later local-model turn. Cover goals, decisions, files touched, leftover work, and key facts. Dense prose. No preamble.";

pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Conservative allowance for reasoning/unknown compatible backends. It is a
/// harness safety margin, not a promise about a provider's token accounting.
pub fn reply_reservation(backend: &ResolvedLocalBackend, max_tokens: u64) -> u64 {
    let multiplier = if backend.provider == "ollama" && backend.reasoning_effort.as_deref() == Some("none") {
        1
    } else {
        2
    };
    max_tokens.saturating_mul(multiplier)
}

pub fn summary_max_tokens(cfg: &crate::common::config::AppConfig) -> u64 {
    cfg.u64_or("compaction.summary_max_tokens", DEFAULT_SUMMARY_MAX_TOKENS)
        .clamp(128, 2048)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenCalibration {
    pub base_url: String,
    pub model: String,
    pub ratio: f64,
}

fn bounded_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(1.0, 3.0)
    } else {
        1.0
    }
}

pub fn token_ratio(calibration: Option<&TokenCalibration>, base_url: &str, model: &str) -> f64 {
    calibration
        .filter(|c| c.base_url == base_url && c.model == model)
        .map(|c| bounded_ratio(c.ratio))
        .unwrap_or(1.0)
}

pub fn calibrate(
    previous: Option<&TokenCalibration>,
    base_url: &str,
    model: &str,
    estimated: u64,
    observed: Option<u64>,
) -> Option<TokenCalibration> {
    let observed = observed.filter(|n| *n > 0 && estimated > 0)?;
    let sample = bounded_ratio(observed as f64 / estimated as f64);
    let old = token_ratio(previous, base_url, model);
    // React immediately to denser text; smooth decreases with a half-weight EMA.
    // ponytail: bounded usage calibration, not a tokenizer; abrupt shifts and
    // ratios above 3 still need a model-specific tokenizer if logs warrant it.
    Some(TokenCalibration {
        base_url: base_url.into(),
        model: model.into(),
        ratio: sample.max((old + sample) / 2.0),
    })
}

pub fn calibrated_tokens(estimate: u64, ratio: f64) -> u64 {
    (estimate as f64 * bounded_ratio(ratio)).ceil() as u64
}

/// Estimate the next prompt from the same bounded history that will be sent,
/// never the full persisted session: stored turns outside the send window must
/// not drive the compaction decision.
pub fn projected_prompt_tokens(
    system: &str,
    history: &[ChatMessage],
    user: &str,
    reply_tokens: u64,
    ratio: f64,
    tool_tokens: u64,
) -> u64 {
    let input = estimate_tokens(system)
        + history.iter().map(|m| estimate_tokens(&m.content)).sum::<u64>()
        + estimate_tokens(user)
        + tool_tokens;
    calibrated_tokens(input, ratio).saturating_add(reply_tokens)
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

/// First user plus recent tail. These turns are never summarized away.
pub fn retained_messages(messages: &[Message], keep_recent: usize) -> Vec<Message> {
    compact_messages(messages, keep_recent, "")
        .into_iter()
        .filter(|m| !m.text.is_empty())
        .collect()
}

fn summary_input(middle: &[Message], max_bytes: usize) -> Result<String> {
    let turns: Vec<_> = middle
        .iter()
        .map(|msg| {
            let preserved = msg.text.starts_with(COMPACT_PREFIX);
            let text = if preserved {
                msg.text.clone()
            } else {
                msg.text.chars().take(SUMMARY_TURN_CHARS).collect()
            };
            (preserved, format!("{}: {}\n", msg.role, text))
        })
        .collect();
    let mut chars: usize = turns
        .iter()
        .filter(|(preserved, _)| *preserved)
        .map(|(_, text)| text.chars().count())
        .sum();
    let mut bytes: usize = turns
        .iter()
        .filter(|(preserved, _)| *preserved)
        .map(|(_, text)| text.len())
        .sum();
    if chars > SUMMARY_INPUT_CHARS || bytes > max_bytes {
        return Err(HarnessError::new(
            crate::llm::openai_chat::LLM_ERROR_CODE,
            "prior compaction summaries exceed the summary input budget; history was preserved",
        ));
    }
    // Reserve opaque summaries first, then fill with the most recent ordinary
    // turns. A suffix truncation here would silently cut the previous summary.
    let mut selected = Vec::new();
    for (preserved, text) in turns.into_iter().rev() {
        if !preserved {
            let size = text.chars().count();
            if chars + size > SUMMARY_INPUT_CHARS || bytes + text.len() > max_bytes {
                continue;
            }
            chars += size;
            bytes += text.len();
        }
        selected.push(text);
    }
    selected.reverse();
    Ok(selected.concat())
}

/// One bounded local-model call. Empty or failed output must not persist.
pub async fn summarize_turns(
    chat: &ChatClient,
    model: &str,
    middle: &[Message],
    max_tokens: u64,
    reservation: u64,
    ratio: f64,
) -> Result<(String, u64, u64)> {
    let raw_budget = (MAX_PROMPT_TOKENS.saturating_sub(reservation) as f64 / bounded_ratio(ratio)).floor() as u64;
    let max_bytes = raw_budget
        .saturating_sub(estimate_tokens(SUMMARY_SYSTEM))
        .saturating_mul(4) as usize;
    let clipped = summary_input(middle, max_bytes)?;
    let reply = chat
        .chat(
            SUMMARY_SYSTEM,
            &[ChatMessage {
                role: "user".into(),
                content: clipped,
            }],
            Some(model),
            max_tokens,
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
    Ok((out, reply.prompt_tokens, reply.completion_tokens))
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
        let projected = projected_prompt_tokens("sys", &history, "efgh", 10, 1.0, 0);
        assert_eq!(projected, 1 + 1 + 1 + 10);
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn summary_input_keeps_later_turns() {
        let early = msg("user", &"x".repeat(30_000));
        let later = msg("assistant", "DECISION_KEEP");
        let input = summary_input(&[early, later], usize::MAX).unwrap();
        assert!(input.contains("DECISION_KEEP"), "{input}");
        assert!(input.chars().count() <= SUMMARY_INPUT_CHARS);
    }

    #[test]
    fn second_compaction_preserves_the_entire_previous_summary_within_the_input_bound() {
        let original = vec![
            msg("user", "first"),
            msg("assistant", "old"),
            msg("user", "recent"),
            msg("assistant", "last"),
        ];
        let summary = format!("{COMPACT_PREFIX}{}TAIL_FACT\n", "決定".repeat(900));
        let mut once = compact_messages(&original, 2, &summary);
        for _ in 0..40 {
            once.push(msg("user", &"ordinary".repeat(200)));
        }
        let input = summary_input(middle_turns(&once, 2), usize::MAX).unwrap();
        assert!(input.contains(&summary), "prior summary must survive byte-identical");
        assert!(input.chars().count() <= SUMMARY_INPUT_CHARS);
        let twice = compact_messages(&once, 2, "next summary");
        assert_eq!(twice[0], original[0]);
        assert_eq!(&twice[2..], &once[once.len() - 2..]);
    }

    #[test]
    fn opaque_summaries_fail_closed_when_they_cannot_fit() {
        let summary = msg("assistant", &format!("{COMPACT_PREFIX}{}", "界".repeat(1000)));
        assert!(summary_input(std::slice::from_ref(&summary), 2000).is_err());
        assert!(summary_input(
            &[msg(
                "assistant",
                &format!("{COMPACT_PREFIX}{}", "x".repeat(SUMMARY_INPUT_CHARS))
            )],
            usize::MAX
        )
        .is_err());
        let ordinary = msg("user", &"界".repeat(1000));
        let input = summary_input(&[ordinary, summary.clone()], 4000).unwrap();
        assert!(input.contains(&summary.text));
        assert!(input.len() <= 4000);
    }

    #[test]
    fn usage_calibration_rises_immediately_decays_slowly_and_resets_for_other_models() {
        let first = calibrate(None, "local", "qwen", 100, Some(200)).unwrap();
        assert_eq!(first.ratio, 2.0);
        assert_eq!(projected_prompt_tokens("abcd", &[], "abcd", 10, first.ratio, 2), 18);
        let next = calibrate(Some(&first), "local", "qwen", 100, Some(100)).unwrap();
        assert_eq!(next.ratio, 1.5);
        assert_eq!(token_ratio(Some(&first), "other", "qwen"), 1.0);
        assert_eq!(token_ratio(Some(&first), "local", "other"), 1.0);
        assert!(calibrate(Some(&first), "local", "qwen", 100, None).is_none());
        assert!(calibrate(None, "local", "qwen", 0, Some(100)).is_none());
        assert!(calibrate(None, "local", "qwen", 100, Some(0)).is_none());
        assert_eq!(calibrate(None, "local", "qwen", 100, Some(1)).unwrap().ratio, 1.0);
        assert_eq!(
            calibrate(None, "local", "qwen", 100, Some(u64::MAX)).unwrap().ratio,
            3.0
        );
        assert_eq!(calibrated_tokens(100, f64::NAN), 100);
    }

    #[test]
    fn summary_budget_defaults_and_clamps() {
        for (yaml, expected) in [
            ("{}", 768),
            ("compaction: {summary_max_tokens: 0}", 128),
            ("compaction: {summary_max_tokens: 1024}", 1024),
            ("compaction: {summary_max_tokens: 99999}", 2048),
        ] {
            let cfg = crate::common::config::AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap();
            assert_eq!(summary_max_tokens(&cfg), expected);
        }
    }
}
