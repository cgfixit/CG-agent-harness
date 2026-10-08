//! Read-only per-model limit proposals for `/model profile`.
//!
//! The shipped budgets were sized for one verified 32,768-token Ollama window
//! (the `web.total_tokens` comment in `assets/config.default.yaml`). A model
//! loaded with a smaller window gets those shipped values scaled in proportion,
//! then checked by the validators startup and reload use. Shipped values, not
//! the home's current ones, are scaled, so a home already tuned for a small
//! window is not halved twice. Nothing here applies a value.
use serde_json::{json, Value};
use serde_yaml_ng::Value as Yaml;

use super::compaction::{self, MIN_PROMPT_HEADROOM};
use super::web_search::Limits;
use crate::common::config::AppConfig;
use crate::llm::backend::ResolvedLocalBackend;

/// The loaded window the shipped budgets were sized for.
pub const TUNED_WINDOW: u64 = 32_768;

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

/// Proposed limits for the profiled model. `profile` is a `profiled` probe
/// result; `web` is the live snapshot and `cfg` the running config.
pub fn propose(profile: &Value, cfg: &AppConfig, web: &Limits, backend: &ResolvedLocalBackend) -> Value {
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
    notes.push("Timeouts stay as configured until decode speed is measured.".to_string());
    let Some(window) = profile["loaded"]["context_length"].as_u64() else {
        return json!({"state": "unknown_window", "tuned_window": TUNED_WINDOW, "values": [], "notes": notes,
            "detail": "The model is not resident, so its window is unknown. Send one chat, then run /model profile again."});
    };
    let Some(shipped) = shipped() else {
        return json!({"state": "unknown_window", "tuned_window": TUNED_WINDOW, "values": [], "notes": notes,
            "detail": "The shipped defaults could not be read."});
    };
    let effective = window.min(TUNED_WINDOW);
    if window > TUNED_WINDOW {
        notes.push(format!(
            "The loaded window ({window}) is larger than the {TUNED_WINDOW} the budgets were tuned for. The code caps web.total_tokens at 32000 and prompts at {} today, so the shipped values are the ceiling.",
            compaction::MAX_PROMPT_TOKENS
        ));
    }
    let mut candidate = cfg.raw.clone();
    let mut values = Vec::new();
    let mut proposed_reply = compaction::DEFAULT_REPLY_TOKENS;
    for (key, step, min) in SCALED {
        let Some(base) = shipped.get(key).and_then(|v| v.as_u64()) else {
            continue;
        };
        let mut value = scale(key, base, effective, step, min);
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
    // The same validators startup and reload run; a failure means this window is
    // too small for the shipped shape, not that the proposal should be forced.
    let mut state = if window < TUNED_WINDOW { "scaled" } else { "shipped" };
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
            if let Some(warning) = compaction::web_budget_warning(limits.total_tokens, reservation, tools) {
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
        "detail": "Advisory only: nothing was applied. Edit config.yaml (web keys reload; models and chat keys need a restart).",
    })
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
        propose(&profile, &cfg, &web, &backend(Some("none")))
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
    fn larger_windows_stop_at_the_shipped_ceiling_and_say_why() {
        let proposal = run(loaded(65_536));
        assert_eq!(proposal["state"], "shipped", "{proposal}");
        assert_eq!(value(&proposal, "web.total_tokens"), 28_000);
        assert!(
            proposal["notes"].to_string().contains("larger than the 32768"),
            "{proposal}"
        );
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
        let proposal = propose(&loaded(8_192), &cfg, &web, &backend(Some("high")));
        let reply = value(&proposal, "models.local_llm.max_tokens");
        assert!(
            value(&proposal, "chat.compact_prompt_tokens") >= reply * 2 + MIN_PROMPT_HEADROOM,
            "{proposal}"
        );
    }
}
