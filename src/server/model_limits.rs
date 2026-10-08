//! Per-model limits: proposals for `/model profile`, and the opt-in
//! `models.local_llm.auto_tune` that applies them on `/model use` and startup.
//!
//! The shipped budgets were sized for one verified 32,768-token Ollama window
//! (the `web.total_tokens` comment in `assets/config.default.yaml`). A model
//! loaded with a smaller window gets those shipped values scaled in proportion,
//! then checked by the validators startup and reload use. Shipped values, not
//! the home's current ones, are scaled, so a home already tuned for a small
//! window is not halved twice. Applied budgets only ever tighten the configured
//! ones; the chat timeout follows the measured speed instead.
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use serde_yaml_ng::Value as Yaml;

use super::compaction::{self, MIN_PROMPT_HEADROOM};
use super::state::AppState;
use super::web_search::Limits;
use crate::common::config::AppConfig;
use crate::llm::backend::ResolvedLocalBackend;
use crate::llm::inventory::InventoryLimits;
use crate::llm::openai_chat::DEFAULT_CHAT_TIMEOUT_SEC;
use crate::llm::profile;

/// The loaded window the shipped budgets were sized for.
pub const TUNED_WINDOW: u64 = compaction::BASE_WINDOW;

/// Margin over the estimated worst-case turn.
const TIMEOUT_SAFETY: f64 = 2.0;
/// Tokens a tool-call round adds to the reply side of the estimate.
const TOOL_ROUND_TOKENS: u64 = 128;
const MIN_TIMEOUT_SEC: u64 = 120;
/// Ceiling for derived chat and planner timeouts; the agentic child mirrors it
/// (`agentic::commands::MAX_PLANNER_TIMEOUT_SEC`, checked by `invariant_guard`).
pub const MAX_TIMEOUT_SEC: u64 = 3600;
/// Synthesis prompt tokens beside the evidence: instructions, question, framing.
const SYNTHESIS_PROMPT_OVERHEAD: u64 = 1_000;
const MIN_SYNTHESIS_SEC: u64 = 60;
const MAX_SYNTHESIS_SEC: u64 = 1_800;
/// Generation-gate owner while the speed sample runs.
pub const TUNE_OWNER: &str = "model_tune";
/// A cold load of a large model can take minutes; the sample itself takes seconds.
const MEASURE_TIMEOUT: Duration = Duration::from_secs(180);
/// A busy gate (a chat turn, or the sample for the previous selection) is
/// retried every half second for up to two minutes.
const GATE_ATTEMPTS: u32 = 240;
const GATE_RETRY: Duration = Duration::from_millis(500);
/// The coding planner's prompt: about 24k characters (see the shipped
/// `agentic.deepagent_github.planner_max_tokens` comment) at 4 bytes a token.
const PLANNER_PROMPT_TOKENS: u64 = 6_000;
const DEFAULT_PLANNER_MAX_TOKENS: u64 = 3_072;

/// Settings scaled with the window: (key, rounding step, minimum).
const SCALED: [(&str, u64, u64); 6] = [
    ("models.local_llm.max_tokens", 256, 256),
    ("chat.compact_prompt_tokens", 500, 500),
    ("web.total_tokens", 500, 500),
    ("web.evidence_tokens", 250, 250),
    ("web.model_tokens", 64, 256),
    ("web.chat_tool_calls", 1, 1),
];

/// `shipped × window / TUNED_WINDOW`, rounded down to `step` (tool calls round
/// up so a usable window keeps at least one call), never below `min`.
fn scale(key: &str, shipped: u64, window: u64, step: u64, min: u64) -> u64 {
    let product = shipped.saturating_mul(window);
    let value = if key == "web.chat_tool_calls" {
        product.div_ceil(TUNED_WINDOW)
    } else {
        product / TUNED_WINDOW / step * step
    };
    value.max(min)
}

fn set_path(raw: &mut Yaml, key: &str, value: u64) {
    let mut node = raw;
    let parts: Vec<&str> = key.split('.').collect();
    for (index, part) in parts.iter().enumerate() {
        if !node.is_mapping() {
            *node = Yaml::Mapping(Default::default());
        }
        let map = node.as_mapping_mut().expect("mapping set above");
        let name = Yaml::String((*part).to_string());
        if index + 1 == parts.len() {
            map.insert(name, Yaml::Number(value.into()));
            return;
        }
        node = map.entry(name).or_insert(Yaml::Null);
    }
}

fn shipped() -> Option<AppConfig> {
    AppConfig::from_str(
        AppConfig::embedded_default(),
        std::path::Path::new("config.default.yaml"),
    )
    .ok()
}

/// Seconds the longest turn the limits allow should take at the measured speed:
/// load, the largest prompt at prefill speed, one reply reservation plus a tool
/// call per round at decode speed; doubled, rounded up to 30 s, kept in 120–3600.
fn derived_timeout(speed: &Value, prompt_tokens: u64, reply_tokens: u64, tool_rounds: u64) -> Option<u64> {
    let written = reply_tokens.saturating_add(tool_rounds.saturating_mul(TOOL_ROUND_TOKENS));
    padded_seconds(speed, prompt_tokens, written, MIN_TIMEOUT_SEC, MAX_TIMEOUT_SEC)
}

/// `/web research`'s answer call at the measured speed: the evidence plus the
/// answer instructions read, one `web.model_tokens` reply written; doubled,
/// rounded up to 30 s, kept in 60–1800 (`web.synthesis_seconds` allows 10–1800).
/// A 27B model on CPU needs far more than the shipped 300 s; a GPU 7B far less.
fn derived_synthesis(speed: &Value, evidence_tokens: u64, model_tokens: u64) -> Option<u64> {
    let prompt = evidence_tokens.saturating_add(SYNTHESIS_PROMPT_OVERHEAD);
    padded_seconds(speed, prompt, model_tokens, MIN_SYNTHESIS_SEC, MAX_SYNTHESIS_SEC)
}

/// Load, then `prompt` tokens at prefill speed and `written` at decode speed;
/// doubled, rounded up to 30 s and kept in `min..=max`.
fn padded_seconds(speed: &Value, prompt: u64, written: u64, min: u64, max: u64) -> Option<u64> {
    let prefill = speed["prefill_tps"].as_f64().filter(|v| *v > 0.0)?;
    let decode = speed["decode_tps"].as_f64().filter(|v| *v > 0.0)?;
    let load = speed["load_seconds"].as_f64().unwrap_or(0.0).max(0.0);
    let estimate = load + prompt as f64 / prefill + written as f64 / decode;
    let seconds = (estimate * TIMEOUT_SAFETY).ceil().min(max as f64) as u64;
    Some((seconds.div_ceil(30) * 30).clamp(min, max))
}

/// Proposed limits for the profiled model. `profile` is a `profiled` probe
/// result; `speed` an optional `profile::parse_speed` sample; `web` the live
/// snapshot and `cfg` the running config.
pub fn propose(
    profile: &Value,
    speed: Option<&Value>,
    cfg: &AppConfig,
    web: &Limits,
    backend: &ResolvedLocalBackend,
) -> Value {
    let current = |key: &str| -> u64 {
        match key {
            "web.total_tokens" => web.total_tokens,
            "web.evidence_tokens" => web.evidence_tokens,
            "web.model_tokens" => web.model_tokens,
            "web.chat_tool_calls" => web.chat_tool_calls as u64,
            "chat.compact_prompt_tokens" => cfg.u64_or(key, compaction::DEFAULT_PROMPT_TOKENS),
            _ => cfg.u64_or(key, compaction::DEFAULT_REPLY_TOKENS),
        }
    };
    let mut notes = Vec::new();
    let declared = &profile["declared"];
    if declared["tools"] == false {
        notes.push("This model declares no tool support, so web tools cannot work with it. Turn web off (/web off) while it is selected.".to_string());
    }
    if declared["thinking"] == true && compaction::reply_reservation(backend, 1) == 1 {
        notes.push("This model can reason, but reasoning_effort is none, so each reply is reserved once. Turning reasoning on doubles the reservation.".to_string());
    }
    if let Some(fraction) = profile["loaded"]["gpu_fraction"].as_f64().filter(|f| *f < 1.0) {
        notes.push(format!(
            "Only {:.0}% of the model is in GPU memory; the rest runs on CPU and decodes far slower. Use a smaller quantization or window.",
            fraction * 100.0
        ));
    }
    if speed.is_none() {
        notes.push("No speed sample yet, so the chat timeout stays as configured. With models.local_llm.auto_tune on, /model use measures it.".to_string());
    }
    let Some(window) = profile["loaded"]["context_length"].as_u64() else {
        return json!({"state": "unknown_window", "tuned_window": TUNED_WINDOW, "values": [], "notes": notes,
            "detail": "The loaded window is unknown: the model is not resident or /api/ps did not answer. Send one chat, then run /model profile again."});
    };
    let Some(shipped) = shipped() else {
        return json!({"state": "unknown_window", "tuned_window": TUNED_WINDOW, "values": [], "notes": notes,
            "detail": "The shipped defaults could not be read."});
    };
    let effective = window.min(compaction::MAX_WINDOW);
    if window > TUNED_WINDOW {
        notes.push(format!(
            "The loaded window ({window}) is larger than the {TUNED_WINDOW} the shipped budgets were sized for. auto_tune only lowers budgets, so set these larger values in config.yaml; with auto_tune on, the prompt cap ({}) and web.total_tokens cap ({}) follow this window.",
            compaction::prompt_cap(Some(window)),
            compaction::web_total_cap(Some(window)),
        ));
    }
    if window > compaction::MAX_WINDOW {
        notes.push(format!("Budgets stop growing at a {} window.", compaction::MAX_WINDOW));
    }
    // Each setting's own code ceiling, so a large window never proposes a value
    // startup would refuse.
    let ceiling = |key: &str| match key {
        "models.local_llm.max_tokens" => compaction::MAX_REPLY_TOKENS / compaction::reply_reservation(backend, 1),
        "chat.compact_prompt_tokens" => compaction::prompt_cap(Some(window)),
        "web.total_tokens" => compaction::web_total_cap(Some(window)),
        "web.evidence_tokens" => super::web_search::MAX_EVIDENCE_TOKENS,
        "web.model_tokens" => super::web_search::MAX_MODEL_TOKENS,
        "web.chat_tool_calls" => crate::llm::openai_stream::MAX_TOOL_CALLS,
        _ => u64::MAX,
    };
    let mut candidate = cfg.raw.clone();
    let mut values = Vec::new();
    let mut proposed_reply = compaction::DEFAULT_REPLY_TOKENS;
    for (key, step, min) in SCALED {
        let Some(base) = shipped.get(key).and_then(|v| v.as_u64()) else {
            continue;
        };
        let mut value = scale(key, base, effective, step, min).min(ceiling(key));
        match key {
            "models.local_llm.max_tokens" => proposed_reply = value,
            // Compaction never triggers below one reply reservation plus headroom.
            "chat.compact_prompt_tokens" => {
                value = value.max(compaction::reply_reservation(backend, proposed_reply) + MIN_PROMPT_HEADROOM)
            }
            _ => {}
        }
        set_path(&mut candidate, key, value);
        values.push(json!({"key": key, "current": current(key), "proposed": value}));
    }
    // The timeout is installed as proposed, but budgets apply as
    // min(configured, proposed): size it for the budgets that will be in force.
    if let Some(seconds) = speed.and_then(|speed| {
        let in_force = |key: &str| {
            values
                .iter()
                .find(|row| row["key"] == key)
                .and_then(|row| row["proposed"].as_u64())
                .map(|proposed| proposed.min(current(key)))
        };
        derived_timeout(
            speed,
            in_force("chat.compact_prompt_tokens")?,
            compaction::reply_reservation(backend, proposed_reply.min(current("models.local_llm.max_tokens"))),
            in_force("web.chat_tool_calls").unwrap_or(0),
        )
    }) {
        let configured = cfg
            .f64_or("models.local_llm.timeout_sec", DEFAULT_CHAT_TIMEOUT_SEC)
            .round() as u64;
        values.push(json!({"key": "models.local_llm.timeout_sec", "current": configured, "proposed": seconds}));
    }
    // Research's answer deadline follows the same sample. It replaces the
    // configured value, like the chat timeout, so a slow model gets longer.
    if let Some(seconds) = speed.and_then(|speed| {
        let proposed = |key: &str| {
            values
                .iter()
                .find(|row| row["key"] == key)
                .and_then(|row| row["proposed"].as_u64())
        };
        derived_synthesis(
            speed,
            proposed("web.evidence_tokens")?.min(web.evidence_tokens),
            proposed("web.model_tokens")?.min(web.model_tokens),
        )
    }) {
        set_path(&mut candidate, "web.synthesis_seconds", seconds);
        values.push(json!({"key": "web.synthesis_seconds", "current": web.synthesis_seconds, "proposed": seconds}));
    }
    // The coding planner, when it runs this model: its prompt at prefill speed
    // and one planner_max_tokens reply at decode speed, padded like chat.
    let planner_model = cfg.str_or("agentic.deepagent_github.model", "");
    if profile["model"]
        .as_str()
        .is_some_and(|model| crate::llm::profile::same_model(model, &planner_model))
    {
        let planner_reply = cfg.u64_or(
            "agentic.deepagent_github.planner_max_tokens",
            DEFAULT_PLANNER_MAX_TOKENS,
        );
        if let Some(seconds) = speed.and_then(|speed| derived_timeout(speed, PLANNER_PROMPT_TOKENS, planner_reply, 0)) {
            let configured = cfg.u64_or(
                "agentic.deepagent_github.planner_timeout_sec",
                crate::shim::REAL_REPO_RUN_FALLBACK_PLANNER_SEC,
            );
            values.push(json!({"key": "agentic.deepagent_github.planner_timeout_sec", "current": configured, "proposed": seconds}));
        }
    }
    // The same validators startup and reload run; a failure means this window is
    // too small for the shipped shape, not that the proposal should be forced.
    let mut state = match window.cmp(&TUNED_WINDOW) {
        std::cmp::Ordering::Less => "scaled",
        std::cmp::Ordering::Equal => "shipped",
        std::cmp::Ordering::Greater => "scaled_up",
    };
    let candidate = AppConfig {
        raw: candidate,
        path: cfg.path.clone(),
    };
    let checked = super::validate_reply_budget("models.local_llm.max_tokens", proposed_reply, backend)
        .and_then(|_| Limits::load(&candidate));
    match checked {
        Err(error) => {
            state = "not_viable";
            notes.push(format!("At this window the scaled values fail validation: {error}."));
        }
        Ok(limits) => {
            let tools =
                compaction::estimate_tokens(&serde_json::to_string(&super::chat_web::tools()).unwrap_or_default());
            let reservation = compaction::reply_reservation(backend, proposed_reply);
            // At this window the proposal is the ceiling; only a larger window raises it.
            let raise = compaction::WebRaise::for_budget(limits.total_tokens, limits.total_tokens, Some(window));
            if let Some(warning) = compaction::web_budget_warning(limits.total_tokens, reservation, tools, raise) {
                state = "not_viable";
                notes.push(format!("Web chat would not fit at this window: {warning}"));
            }
        }
    }
    json!({
        "state": state,
        "window": window,
        "tuned_window": TUNED_WINDOW,
        "values": values,
        "notes": notes,
        "detail": "Proposals. With models.local_llm.auto_tune on, /model use applies them (budgets only tighten); otherwise edit config.yaml (web keys reload; models and chat keys need a restart).",
    })
}

/// Web budgets a tuning lowers; never raised past the live snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebBudget {
    pub total_tokens: u64,
    pub evidence_tokens: u64,
    pub model_tokens: u64,
    pub chat_tool_calls: u64,
}

/// What `auto_tune` installed for one model. Budgets are ceilings combined with
/// the configured values by `min` where they are read, so a reload that lowers
/// a configured value still wins.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Tuning {
    pub model: String,
    pub window: u64,
    pub state: String,
    /// No tool support declared, or no web-chat prompt fits the window.
    pub tools_off: bool,
    pub max_tokens: u64,
    pub compact_prompt_tokens: u64,
    /// `None` when the window is too small for web chat: limits stay as configured.
    pub web: Option<WebBudget>,
    pub timeout_sec: Option<u64>,
    /// Measured `/web research` answer deadline; `None` without a speed sample.
    pub synthesis_seconds: Option<u64>,
    /// Set when the coding planner runs this model; agent runs then use it.
    pub planner_timeout_sec: Option<u64>,
    pub speed: Option<Value>,
    pub notes: Vec<String>,
}

impl Tuning {
    pub fn apply_web(&self, limits: &mut Limits) {
        if let Some(budget) = &self.web {
            limits.total_tokens = limits.total_tokens.min(budget.total_tokens);
            limits.evidence_tokens = limits.evidence_tokens.min(budget.evidence_tokens);
            limits.model_tokens = limits.model_tokens.min(budget.model_tokens);
            limits.chat_tool_calls = limits.chat_tool_calls.min(budget.chat_tool_calls as usize);
        }
        // A deadline, not a budget: the measured value replaces the configured
        // one. It is derived again from the budgets just applied, so a reload
        // that raises them also lengthens the deadline.
        if self.synthesis_seconds.is_some() {
            if let Some(seconds) = self
                .speed
                .as_ref()
                .and_then(|speed| derived_synthesis(speed, limits.evidence_tokens, limits.model_tokens))
            {
                limits.synthesis_seconds = seconds;
            }
        }
    }
}

/// The tuning a proposal implies, or `None` when the window is unknown.
pub fn tuning_from(model: &str, profile: &Value, proposal: &Value, speed: Option<Value>) -> Option<Tuning> {
    let window = proposal["window"].as_u64()?;
    let state = proposal["state"].as_str()?.to_string();
    let value = |key: &str| proposal["values"].as_array()?.iter().find(|row| row["key"] == key)?["proposed"].as_u64();
    let web = match state.as_str() {
        "not_viable" => None,
        _ => Some(WebBudget {
            total_tokens: value("web.total_tokens")?,
            evidence_tokens: value("web.evidence_tokens")?,
            model_tokens: value("web.model_tokens")?,
            chat_tool_calls: value("web.chat_tool_calls")?,
        }),
    };
    Some(Tuning {
        model: model.to_string(),
        window,
        tools_off: profile["declared"]["tools"] == false || state == "not_viable",
        state,
        max_tokens: value("models.local_llm.max_tokens")?,
        compact_prompt_tokens: value("chat.compact_prompt_tokens")?,
        web,
        timeout_sec: value("models.local_llm.timeout_sec"),
        synthesis_seconds: value("web.synthesis_seconds"),
        planner_timeout_sec: value("agentic.deepagent_github.planner_timeout_sec"),
        speed,
        notes: proposal["notes"]
            .as_array()
            .map(|notes| notes.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
    })
}

/// `models.local_llm.auto_tune`: sample the model's speed while holding the
/// generation gate (this loads it, like warmup), read its loaded window, derive
/// limits, and install them if `model` is still the selection. Every failure
/// leaves the configured limits in force and is audited, never surfaced as an error.
pub async fn tune(state: Arc<AppState>, model: String) {
    if !state.cfg.flag_is_true("models.local_llm.auto_tune") {
        return;
    }
    if state.cloud_chat.is_cloud_selection(&model) || state.backend.provider != "ollama" {
        state.set_tuning(None);
        return;
    }
    let skipped = |reason: &str| {
        state
            .audit
            .log(json!({"event": "model_tune_skipped", "model": model, "reason": reason}));
    };
    let Ok(limits) = InventoryLimits::from_config(&state.cfg) else {
        return skipped("inventory_limits");
    };
    let keep_alive = crate::llm::ollama::clamped(&state.cfg, "models.local_llm.warmup.keep_alive_sec", 300, 1, 3600);
    let mut speed = None;
    for _ in 0..GATE_ATTEMPTS {
        // A newer selection starts its own tune; this one stops competing for the gate.
        if state.current_model() != model {
            return skipped("selection_changed");
        }
        if let Some(_gate) = state.generation_gate.claim(TUNE_OWNER) {
            speed = profile::measure(
                &state.backend.base_url,
                &model,
                keep_alive,
                MEASURE_TIMEOUT,
                limits.max_bytes,
            )
            .await;
            break;
        }
        tokio::time::sleep(GATE_RETRY).await;
    }
    let probed = profile::probe(&state.backend.base_url, &model, limits).await;
    if probed["state"] != "profiled" {
        return skipped(probed["state"].as_str().unwrap_or("unavailable"));
    }
    let proposal = propose(
        &probed,
        speed.as_ref(),
        &state.cfg,
        &state.runtime_limits().web,
        &state.backend,
    );
    let Some(tuning) = tuning_from(&model, &probed, &proposal, speed) else {
        return skipped("unknown_window");
    };
    if state.current_model() != model {
        return skipped("selection_changed");
    }
    state.audit.log(json!({
        "event": "model_tuned",
        "model": model,
        "window": tuning.window,
        "state": tuning.state,
        "tools_off": tuning.tools_off,
        "max_tokens": tuning.max_tokens,
        "timeout_sec": tuning.timeout_sec,
        "synthesis_seconds": tuning.synthesis_seconds,
        "decode_tps": tuning.speed.as_ref().map(|s| s["decode_tps"].clone()),
    }));
    state.set_tuning(Some(tuning));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(reasoning: Option<&str>) -> ResolvedLocalBackend {
        ResolvedLocalBackend {
            provider: "ollama".into(),
            base_url: "http://127.0.0.1:11434/v1".into(), // DevSkim: ignore DS162092 because this fixture names the loopback Ollama endpoint.
            model: "fixture".into(),
            source: "primary".into(),
            api_key: String::new(),
            degraded: false,
            reasoning_effort: reasoning.map(str::to_string),
        }
    }

    fn run(profile: Value) -> Value {
        let cfg = shipped().expect("embedded default parses");
        let web = Limits::load(&cfg).expect("shipped web limits are valid");
        propose(&profile, None, &cfg, &web, &backend(Some("none")))
    }

    fn loaded(window: u64) -> Value {
        json!({"declared": {"tools": true, "thinking": false}, "loaded": {"context_length": window, "gpu_fraction": 1.0}})
    }

    fn value(proposal: &Value, key: &str) -> u64 {
        proposal["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .unwrap_or_else(|| panic!("{key} missing: {proposal}"))["proposed"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn the_tuned_window_proposes_the_shipped_values() {
        let proposal = run(loaded(TUNED_WINDOW));
        assert_eq!(proposal["state"], "shipped", "{proposal}");
        for row in proposal["values"].as_array().unwrap() {
            assert_eq!(row["current"], row["proposed"], "{row}");
        }
        assert_eq!(value(&proposal, "web.total_tokens"), 28_000);
    }

    #[test]
    fn a_half_window_halves_the_budgets_and_passes_validation() {
        let proposal = run(loaded(16_384));
        assert_eq!(proposal["state"], "scaled", "{proposal}");
        assert_eq!(value(&proposal, "models.local_llm.max_tokens"), 2_048);
        assert_eq!(value(&proposal, "chat.compact_prompt_tokens"), 12_000);
        assert_eq!(value(&proposal, "web.total_tokens"), 14_000);
        assert_eq!(value(&proposal, "web.evidence_tokens"), 3_000);
        assert_eq!(value(&proposal, "web.model_tokens"), 512);
        assert_eq!(value(&proposal, "web.chat_tool_calls"), 5);
    }

    #[test]
    fn budgets_shrink_monotonically_and_tiny_windows_are_not_viable() {
        let mut previous: Option<Value> = None;
        for window in [2_048, 4_096, 8_192, 16_384, 24_576, 32_768] {
            let proposal = run(loaded(window));
            assert!(value(&proposal, "web.chat_tool_calls") >= 1, "{proposal}");
            if let Some(smaller) = &previous {
                for (key, _, _) in SCALED {
                    assert!(value(smaller, key) <= value(&proposal, key), "{key} at {window}");
                }
            }
            previous = Some(proposal);
        }
        assert_eq!(run(loaded(2_048))["state"], "not_viable");
    }

    #[test]
    fn larger_windows_scale_up_to_each_code_ceiling_and_say_how_to_apply() {
        let proposal = run(loaded(65_536));
        assert_eq!(proposal["state"], "scaled_up", "{proposal}");
        assert_eq!(value(&proposal, "models.local_llm.max_tokens"), 8_192);
        assert_eq!(value(&proposal, "chat.compact_prompt_tokens"), 48_000);
        assert_eq!(value(&proposal, "web.total_tokens"), 56_000);
        // Already at their own ceilings at 32k.
        assert_eq!(value(&proposal, "web.evidence_tokens"), 6_000);
        assert_eq!(value(&proposal, "web.model_tokens"), 2_048);
        let notes = proposal["notes"].to_string();
        assert!(notes.contains("set these larger values in config.yaml"), "{notes}");
        // Monotonic up to the window ceiling, then flat; never past a validator bound.
        let mut previous = run(loaded(TUNED_WINDOW));
        for window in [49_152, 65_536, 98_304, 131_072, 262_144] {
            let proposal = run(loaded(window));
            assert_eq!(proposal["state"], "scaled_up", "{window}: {proposal}");
            for (key, _, _) in SCALED {
                assert!(value(&previous, key) <= value(&proposal, key), "{key} at {window}");
            }
            previous = proposal;
        }
        assert_eq!(value(&previous, "web.total_tokens"), 112_000);
        assert_eq!(value(&previous, "chat.compact_prompt_tokens"), 96_000);
        assert!(previous["notes"].to_string().contains("stop growing"), "{previous}");
    }

    #[test]
    fn the_timeout_follows_the_budgets_in_force_not_larger_proposals() {
        let cfg = shipped().unwrap();
        let web = Limits::load(&cfg).unwrap();
        let speed = json!({"prefill_tps": 1500.0, "decode_tps": 50.0, "load_seconds": 0.0});
        let none = backend(Some("none"));
        let shipped_size = propose(&loaded(TUNED_WINDOW), Some(&speed), &cfg, &web, &none);
        let large = propose(&loaded(131_072), Some(&speed), &cfg, &web, &none);
        // The larger proposals are not applied over the shipped config (budgets
        // only tighten), so the timeout is sized for the same in-force budgets.
        assert!(value(&large, "chat.compact_prompt_tokens") > value(&shipped_size, "chat.compact_prompt_tokens"));
        assert_eq!(
            value(&large, "models.local_llm.timeout_sec"),
            value(&shipped_size, "models.local_llm.timeout_sec"),
            "{large}"
        );
    }

    #[test]
    fn the_planner_gets_a_timeout_only_when_it_runs_the_measured_model() {
        let cfg = shipped().expect("embedded default parses");
        let web = Limits::load(&cfg).expect("shipped web limits are valid");
        let speed = json!({"prefill_tps": 1500.0, "decode_tps": 50.0, "load_seconds": 0.0});
        let mut profile = loaded(TUNED_WINDOW);
        profile["model"] = json!("qwen3.8:27b-mlx");
        let proposal = propose(&profile, Some(&speed), &cfg, &web, &backend(Some("none")));
        // 6000/1500 + 3072/50 = 65.4 s; doubled and rounded up to 150.
        assert_eq!(value(&proposal, "agentic.deepagent_github.planner_timeout_sec"), 150);
        let tuning = tuning_from("qwen3.8:27b-mlx", &profile, &proposal, Some(speed.clone())).unwrap();
        assert_eq!(tuning.planner_timeout_sec, Some(150));
        // An untagged planner name is the same model as its :latest tag.
        let mut aliased = cfg.clone();
        aliased.raw["agentic"]["deepagent_github"]["model"] = "qwen3.8".into();
        let mut latest = loaded(TUNED_WINDOW);
        latest["model"] = json!("qwen3.8:latest");
        let proposal = propose(&latest, Some(&speed), &aliased, &web, &backend(Some("none")));
        assert_eq!(value(&proposal, "agentic.deepagent_github.planner_timeout_sec"), 150);
        // Another model, or no sample: the configured planner timeout stays.
        profile["model"] = json!("humanizer:q8");
        let other = propose(&profile, Some(&speed), &cfg, &web, &backend(Some("none")));
        assert!(!other.to_string().contains("planner_timeout_sec"), "{other}");
        profile["model"] = json!("qwen3.8:27b-mlx");
        let unsampled = propose(&profile, None, &cfg, &web, &backend(Some("none")));
        assert!(!unsampled.to_string().contains("planner_timeout_sec"), "{unsampled}");
    }

    #[test]
    fn unknown_windows_and_model_facts_become_notes_not_values() {
        let cold = run(json!({"declared": {"tools": false, "thinking": true}, "loaded": null}));
        assert_eq!(cold["state"], "unknown_window");
        assert!(cold["values"].as_array().unwrap().is_empty());
        let notes = cold["notes"].to_string();
        assert!(notes.contains("no tool support"), "{notes}");
        assert!(notes.contains("can reason"), "{notes}");
        let spilled = run(json!({"declared": {}, "loaded": {"context_length": 32_768, "gpu_fraction": 0.9}}));
        assert!(spilled["notes"].to_string().contains("90% of the model"), "{spilled}");
    }

    #[test]
    fn reasoning_backends_floor_compaction_at_a_doubled_reservation() {
        let cfg = shipped().unwrap();
        let web = Limits::load(&cfg).unwrap();
        let proposal = propose(&loaded(8_192), None, &cfg, &web, &backend(Some("high")));
        let reply = value(&proposal, "models.local_llm.max_tokens");
        assert!(
            value(&proposal, "chat.compact_prompt_tokens") >= reply * 2 + MIN_PROMPT_HEADROOM,
            "{proposal}"
        );
    }

    #[test]
    fn a_speed_sample_turns_into_a_bounded_timeout() {
        let speed = json!({"prefill_tps": 1500.0, "decode_tps": 50.0, "load_seconds": 0.0});
        // 24000/1500 = 16 s, (4096 + 10 x 128)/50 = 107.5 s; x2 = 247 s, rounded up to 270.
        assert_eq!(derived_timeout(&speed, 24_000, 4_096, 10), Some(270));
        let instant = json!({"prefill_tps": 1e5, "decode_tps": 1e4});
        assert_eq!(derived_timeout(&instant, 1_000, 256, 1), Some(MIN_TIMEOUT_SEC));
        let crawl = json!({"prefill_tps": 1.0, "decode_tps": 1.0});
        assert_eq!(derived_timeout(&crawl, 24_000, 4_096, 10), Some(MAX_TIMEOUT_SEC));
        assert!(derived_timeout(&json!({"decode_tps": 50.0}), 24_000, 4_096, 10).is_none());
        // (3000 + 1000)/50 = 80 s read and 1024/3 = 341 s written by a 27B on
        // CPU; x2 = 843 s, rounded up to 870, far past the shipped 300 s.
        let cpu = json!({"prefill_tps": 50.0, "decode_tps": 3.0, "load_seconds": 0.0});
        assert_eq!(derived_synthesis(&cpu, 3_000, 1_024), Some(870));
        let gpu = json!({"prefill_tps": 3000.0, "decode_tps": 120.0});
        assert_eq!(derived_synthesis(&gpu, 3_000, 1_024), Some(MIN_SYNTHESIS_SEC));
        assert!(derived_synthesis(&json!({"decode_tps": 50.0}), 3_000, 1_024).is_none());
    }

    #[test]
    fn a_measured_proposal_becomes_a_tuning_that_only_tightens() {
        let cfg = shipped().unwrap();
        let web = Limits::load(&cfg).unwrap();
        let speed = json!({"prefill_tps": 1500.0, "decode_tps": 50.0, "load_seconds": 0.0});
        let profile = loaded(16_384);
        let proposal = propose(&profile, Some(&speed), &cfg, &web, &backend(Some("none")));
        // 12000/1500 = 8 s, (2048 + 5 x 128)/50 = 53.8 s; x2 = 124 s, rounded up to 150.
        assert_eq!(value(&proposal, "models.local_llm.timeout_sec"), 150);
        let tuning = tuning_from("m", &profile, &proposal, Some(speed)).unwrap();
        assert_eq!(
            (tuning.max_tokens, tuning.timeout_sec, tuning.tools_off),
            (2_048, Some(150), false)
        );
        let mut live = web.clone();
        tuning.apply_web(&mut live);
        assert_eq!((live.total_tokens, live.chat_tool_calls), (14_000, 5));
        // (1500 + 1000)/1500 + 512/50 = 11.9 s; x2 rounded up is 30, floored at 60.
        assert_eq!((tuning.synthesis_seconds, live.synthesis_seconds), (Some(60), 60));
        // A configured value already below the tuning is kept.
        let mut low = web.clone();
        low.total_tokens = 9_000;
        tuning.apply_web(&mut low);
        assert_eq!(low.total_tokens, 9_000);
    }

    #[test]
    fn a_reload_that_raises_the_budgets_lengthens_the_tuned_deadline() {
        let cfg = shipped().unwrap();
        let mut web = Limits::load(&cfg).unwrap();
        web.evidence_tokens = 250;
        web.model_tokens = 256;
        let cpu = json!({"prefill_tps": 50.0, "decode_tps": 3.0, "load_seconds": 0.0});
        let profile = loaded(16_384);
        let proposal = propose(&profile, Some(&cpu), &cfg, &web, &backend(Some("none")));
        let tuning = tuning_from("m", &profile, &proposal, Some(cpu)).unwrap();
        // (250 + 1000)/50 = 25 s read, 256/3 = 85.3 s written; x2 rounded up is 240.
        assert_eq!(tuning.synthesis_seconds, Some(240));
        let mut live = web.clone();
        tuning.apply_web(&mut live);
        assert_eq!(live.synthesis_seconds, 240);
        // A reload raises both budgets; the tuned ceilings (3000/512) now bound them.
        let mut reloaded = Limits::load(&cfg).unwrap();
        tuning.apply_web(&mut reloaded);
        assert_eq!((reloaded.evidence_tokens, reloaded.model_tokens), (3_000, 512));
        // (3000 + 1000)/50 = 80 s read, 512/3 = 170.7 s written; x2 rounded up is 510.
        assert_eq!(reloaded.synthesis_seconds, 510);
    }

    #[test]
    fn tool_less_and_too_small_models_get_no_tools_and_cold_ones_no_tuning() {
        let cfg = shipped().unwrap();
        let web = Limits::load(&cfg).unwrap();
        let none = backend(Some("none"));
        let rewriter = json!({"declared": {"tools": false}, "loaded": {"context_length": 32_768, "gpu_fraction": 1.0}});
        let tuning = tuning_from(
            "humanizer",
            &rewriter,
            &propose(&rewriter, None, &cfg, &web, &none),
            None,
        )
        .unwrap();
        assert!(tuning.tools_off && tuning.web.is_some() && tuning.timeout_sec.is_none());
        // Without a speed sample the research deadline stays as configured.
        assert!(tuning.synthesis_seconds.is_none());
        let tiny = loaded(2_048);
        let tuning = tuning_from("tiny", &tiny, &propose(&tiny, None, &cfg, &web, &none), None).unwrap();
        assert!(tuning.tools_off && tuning.web.is_none());
        let cold = json!({"declared": {}, "loaded": null});
        assert!(tuning_from("cold", &cold, &propose(&cold, None, &cfg, &web, &none), None).is_none());
    }
}
