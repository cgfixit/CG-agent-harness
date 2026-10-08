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
/// The loaded window the prompt and web caps were sized for.
pub const BASE_WINDOW: u64 = 32_768;
/// `web.total_tokens` ceiling at [`BASE_WINDOW`].
pub const BASE_WEB_TOTAL_TOKENS: u64 = 32_000;
/// Largest window the caps scale to.
pub const MAX_WINDOW: u64 = 131_072;
/// `web.total_tokens` ceiling at [`MAX_WINDOW`]: what config may set.
pub const MAX_WEB_TOTAL_TOKENS: u64 = BASE_WEB_TOTAL_TOKENS * (MAX_WINDOW / BASE_WINDOW);
pub const DEFAULT_SUMMARY_MAX_TOKENS: u64 = 768;
const SUMMARY_INPUT_CHARS: usize = 24_000;
const SUMMARY_TURN_CHARS: usize = 800;
const SUMMARY_SYSTEM: &str = "Summarize this chat history for a later local-model turn. Cover goals, decisions, files touched, leftover work, and key facts. Dense prose. No preamble.";

/// `base` at [`BASE_WINDOW`], scaled to a measured `window` above it (capped at
/// [`MAX_WINDOW`]). An unknown or smaller window keeps `base`: the caps only grow
/// for a window `auto_tune` has measured.
fn window_cap(base: u64, window: Option<u64>) -> u64 {
    match window.filter(|window| *window > BASE_WINDOW) {
        Some(window) => base * window.min(MAX_WINDOW) / BASE_WINDOW,
        None => base,
    }
}

/// The prompt cap for a model loaded at `window`: [`MAX_PROMPT_TOKENS`] unless
/// a measured window is larger.
pub fn prompt_cap(window: Option<u64>) -> u64 {
    window_cap(MAX_PROMPT_TOKENS, window)
}

/// The `web.total_tokens` cap for a model loaded at `window`, like [`prompt_cap`].
pub fn web_total_cap(window: Option<u64>) -> u64 {
    window_cap(BASE_WEB_TOTAL_TOKENS, window)
}

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

/// Largest projected prompt (input plus one reply reservation) that web-enabled
/// chat admits. The projection already carries one reservation, so subtracting
/// one more leaves room for a second reply, after a tool round, inside
/// `web.total_tokens`. Subtracting two here charged the reservation three times,
/// which on a doubled-reservation backend is six times `max_tokens`.
pub fn web_prompt_limit(web_total_tokens: u64, reservation: u64) -> u64 {
    web_total_tokens.saturating_sub(reservation)
}

/// Why web-enabled chat cannot fit a usable prompt with these settings, if it
/// cannot: under [`MIN_PROMPT_HEADROOM`] input tokens remain once both replies
/// and the tool definitions are reserved, and the system prompt alone can fill that.
pub fn web_budget_warning(web_total_tokens: u64, reservation: u64, tool_tokens: u64) -> Option<String> {
    let input = web_prompt_limit(web_total_tokens, reservation)
        .saturating_sub(reservation)
        .saturating_sub(tool_tokens);
    (input < MIN_PROMPT_HEADROOM).then(|| {
        format!(
            "web.total_tokens {web_total_tokens} leaves {input} prompt tokens for web-enabled chat after two \
             {reservation}-token reply reservations (from models.local_llm.max_tokens) and {tool_tokens} tokens of \
             tool definitions; below {MIN_PROMPT_HEADROOM}, web chat refuses almost every message. Lower \
             models.local_llm.max_tokens, raise web.total_tokens, or turn web off"
        )
    })
}

/// The prompt limit a chat turn is held to and the setting that binds it:
/// `chat.compact_prompt_tokens` (held at `cap`, from [`prompt_cap`]), tightened to
/// `web_room` for web chat, but never below `floor` (reply reservation, headroom
/// and tool definitions), where the turn's reply setting binds instead.
pub fn prompt_limit(
    configured: u64,
    cap: u64,
    web_room: Option<u64>,
    floor: u64,
    reply_setting: &'static str,
) -> (u64, &'static str) {
    let mut limit = (configured.min(cap), "chat.compact_prompt_tokens");
    if let Some(room) = web_room.filter(|room| *room < limit.0) {
        limit = (room, "web.total_tokens");
    }
    if limit.0 < floor {
        limit = (floor, reply_setting);
    }
    limit
}

/// What to change when a prompt exceeds `limit`, bound by `source`. Raising
/// `chat.compact_prompt_tokens` is suggested only while the limit is under `cap`.
pub fn prompt_limit_remedy(source: &str, limit: u64, cap: u64, reply_setting: &str, web_chat: bool) -> String {
    let below_cap = limit < cap;
    match source {
        "web.total_tokens" => {
            format!("shorten the message, lower {reply_setting}, raise web.total_tokens, or turn web off")
        }
        "chat.compact_prompt_tokens" if below_cap => "shorten the message or raise chat.compact_prompt_tokens".into(),
        "chat.compact_prompt_tokens" => format!("shorten the message or lower {reply_setting}"),
        _ if web_chat => format!("shorten the message, lower {reply_setting}, or turn web off"),
        _ if below_cap => format!("shorten the message, lower {reply_setting}, or raise chat.compact_prompt_tokens"),
        _ => format!("shorten the message or lower {reply_setting}"),
    }
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
    fn web_prompt_limit_reserves_the_reply_once_on_top_of_the_projection() {
        // Shipped web budget with a doubled reservation (MLX / LM Studio, max_tokens 4096).
        assert_eq!(web_prompt_limit(28_000, 8_192), 19_808);
        assert!(web_budget_warning(28_000, 8_192, 272).is_none());
        let warning = web_budget_warning(16_000, 8_192, 272).unwrap();
        assert!(warning.contains("models.local_llm.max_tokens") && warning.contains("web.total_tokens"));
        // Exactly MIN_PROMPT_HEADROOM input tokens after both replies and the tools is usable.
        let usable = 2 * 8_192 + 272 + MIN_PROMPT_HEADROOM;
        assert!(web_budget_warning(usable, 8_192, 272).is_none());
        assert!(web_budget_warning(usable - 1, 8_192, 272).is_some());
        assert!(web_budget_warning(0, 8_192, 272).is_some());
    }

    #[test]
    fn prompt_limit_names_the_bound_that_wins_and_only_remedies_that_help() {
        let chat = "models.local_llm.max_tokens";
        // Configured, web room and the floor each win in turn.
        assert_eq!(
            prompt_limit(24_000, MAX_PROMPT_TOKENS, None, 8_288, chat),
            (24_000, "chat.compact_prompt_tokens")
        );
        assert_eq!(
            prompt_limit(24_000, MAX_PROMPT_TOKENS, Some(15_096), 12_560, chat),
            (15_096, "web.total_tokens")
        );
        // A doubled 12,952-token reply: the floor, capped at 30000, binds even at
        // the largest web.total_tokens, so raising that setting cannot help.
        let room = web_prompt_limit(32_000, 25_904);
        let (limit, source) = prompt_limit(24_000, MAX_PROMPT_TOKENS, Some(room), MAX_PROMPT_TOKENS, chat);
        assert_eq!((limit, source), (MAX_PROMPT_TOKENS, chat));
        let remedy = prompt_limit_remedy(source, limit, MAX_PROMPT_TOKENS, chat, true);
        assert_eq!(
            remedy,
            "shorten the message, lower models.local_llm.max_tokens, or turn web off"
        );
        // A configured value above the cap is held at the cap; raising it cannot help.
        let (limit, source) = prompt_limit(40_000, MAX_PROMPT_TOKENS, None, 8_288, chat);
        assert_eq!((limit, source), (MAX_PROMPT_TOKENS, "chat.compact_prompt_tokens"));
        assert_eq!(
            prompt_limit_remedy(source, limit, MAX_PROMPT_TOKENS, chat, false),
            "shorten the message or lower models.local_llm.max_tokens"
        );
        // Under the cap, raising the configured limit lifts a floor-bound limit too.
        let loop_setting = "api.harness_loop_rate_limit.max_tokens";
        let (limit, source) = prompt_limit(8_000, MAX_PROMPT_TOKENS, None, 12_288, loop_setting);
        assert_eq!((limit, source), (12_288, loop_setting));
        assert_eq!(
            prompt_limit_remedy(source, limit, MAX_PROMPT_TOKENS, loop_setting, false),
            "shorten the message, lower api.harness_loop_rate_limit.max_tokens, or raise chat.compact_prompt_tokens"
        );
        assert_eq!(
            prompt_limit_remedy(source, MAX_PROMPT_TOKENS, MAX_PROMPT_TOKENS, loop_setting, false),
            "shorten the message or lower api.harness_loop_rate_limit.max_tokens"
        );
        assert!(
            prompt_limit_remedy("web.total_tokens", 15_096, MAX_PROMPT_TOKENS, chat, true)
                .contains("raise web.total_tokens")
        );
        assert_eq!(
            prompt_limit_remedy("chat.compact_prompt_tokens", 24_000, MAX_PROMPT_TOKENS, chat, true),
            "shorten the message or raise chat.compact_prompt_tokens"
        );
        // A measured 64k window lifts the cap, so 40000 is held only at its own value.
        let cap = prompt_cap(Some(65_536));
        assert_eq!(
            prompt_limit(40_000, cap, None, 8_288, chat),
            (40_000, "chat.compact_prompt_tokens")
        );
        assert_eq!(
            prompt_limit_remedy("chat.compact_prompt_tokens", MAX_PROMPT_TOKENS, cap, chat, false),
            "shorten the message or raise chat.compact_prompt_tokens"
        );
    }

    #[test]
    fn caps_grow_only_with_a_measured_larger_window() {
        assert_eq!(prompt_cap(None), MAX_PROMPT_TOKENS);
        assert_eq!(prompt_cap(Some(16_384)), MAX_PROMPT_TOKENS);
        assert_eq!(prompt_cap(Some(BASE_WINDOW)), MAX_PROMPT_TOKENS);
        assert_eq!(prompt_cap(Some(65_536)), 60_000);
        assert_eq!(web_total_cap(None), 32_000);
        assert_eq!(web_total_cap(Some(49_152)), 48_000);
        // Past MAX_WINDOW the caps stop growing; config can never exceed the top one.
        assert_eq!(prompt_cap(Some(1 << 20)), 120_000);
        assert_eq!(web_total_cap(Some(1 << 20)), MAX_WEB_TOTAL_TOKENS);
        assert_eq!(MAX_WEB_TOTAL_TOKENS, 128_000);
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
