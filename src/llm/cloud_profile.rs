//! Config-owned cloud limits; absent profiles retain the shared chat settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

#[derive(Clone, Debug, Serialize)]
pub struct CloudProfile {
    pub context_window: Option<u64>,
    pub max_output_tokens: u64,
    pub temperature: Option<f64>,
    pub timeout_sec: f64,
    pub count_timeout_sec: f64,
    pub tool_support: bool,
    pub rpm: Option<u32>,
    pub sources: BTreeMap<&'static str, &'static str>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overrides {
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
    temperature: Option<f64>,
    timeout_sec: Option<f64>,
    count_timeout_sec: Option<f64>,
    tool_support: Option<bool>,
    rpm: Option<u32>,
}

impl CloudProfile {
    pub fn from_config(cfg: &AppConfig, provider: &str) -> Result<Self> {
        let root = format!("models.cloud_chat.{provider}.profile");
        let invalid = || HarnessError::config(format!("{root} contains an invalid cloud profile"));
        let p: Overrides = match cfg.get(&root) {
            None => Overrides::default(),
            Some(value) => serde_yaml_ng::from_value(value.clone()).map_err(|_| invalid())?,
        };
        if p.context_window == Some(0)
            || p.max_output_tokens == Some(0)
            || p.rpm == Some(0)
            || p.rpm.is_some_and(|n| n > 100_000)
            || p.temperature
                .is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
            || p.timeout_sec.is_some_and(|n| !n.is_finite() || n <= 0.0)
            || p.count_timeout_sec.is_some_and(|n| !n.is_finite() || n <= 0.0)
        {
            return Err(invalid());
        }
        let mut sources = BTreeMap::new();
        for (name, present) in [
            ("context_window", p.context_window.is_some()),
            ("temperature", p.temperature.is_some()),
            ("tool_support", p.tool_support.is_some()),
            ("rpm", p.rpm.is_some()),
        ] {
            sources.insert(name, if present { "profile" } else { "default" });
        }
        let shared_output = cfg.u64_or("models.cloud_chat.max_tokens", 4096);
        let shared_timeout = cfg.f64_or("models.cloud_chat.timeout_sec", 90.0);
        let shared_count = cfg.f64_or("models.cloud_chat.count_timeout_sec", 2.0);
        let output = p.max_output_tokens.map(|n| n.clamp(1, 32768));
        let timeout = p.timeout_sec.map(|n| n.clamp(1.0, 720.0));
        let count = p.count_timeout_sec.map(|n| n.clamp(0.1, 10.0));
        // Report the source that actually wins the tightening, not just an override's presence.
        for (field, shared, uses_profile) in [
            (
                "max_output_tokens",
                "max_tokens",
                output.is_some_and(|n| n <= shared_output),
            ),
            (
                "timeout_sec",
                "timeout_sec",
                timeout.is_some_and(|n| n <= shared_timeout),
            ),
            (
                "count_timeout_sec",
                "count_timeout_sec",
                count.is_some_and(|n| n <= shared_count),
            ),
        ] {
            sources.insert(
                field,
                if uses_profile {
                    "profile"
                } else if cfg.get(&format!("models.cloud_chat.{shared}")).is_some_and(|v| {
                    if field == "max_output_tokens" {
                        v.as_u64().is_some()
                    } else {
                        v.as_f64().is_some()
                    }
                }) {
                    "config"
                } else {
                    "default"
                },
            );
        }
        sources.insert(
            "model",
            if cfg
                .get(&format!("models.cloud_chat.{provider}.model"))
                .is_some_and(|v| v.is_string() || v.is_number())
            {
                "config"
            } else {
                "default"
            },
        );
        let max_output_tokens = output.unwrap_or(shared_output).min(shared_output);
        if p.context_window.is_some_and(|n| n <= max_output_tokens) {
            return Err(invalid());
        }
        Ok(Self {
            context_window: p.context_window,
            max_output_tokens,
            temperature: p.temperature,
            timeout_sec: timeout.unwrap_or(shared_timeout).min(shared_timeout),
            count_timeout_sec: count.unwrap_or(shared_count).min(shared_count),
            tool_support: p.tool_support.unwrap_or(false),
            rpm: p.rpm,
            sources,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_tighten_and_report_the_effective_source_without_echoing_invalid_values() {
        let cfg = |text: &str| AppConfig::from_str(text, std::path::Path::new("config.yaml")).unwrap();
        let fallback = CloudProfile::from_config(&cfg("{}"), "grok").unwrap();
        assert_eq!(fallback.max_output_tokens, 4096);
        assert_eq!(fallback.sources["max_output_tokens"], "default");
        let fallback = CloudProfile::from_config(&cfg("models: {cloud_chat: {max_tokens: '4096'}}"), "grok").unwrap();
        assert_eq!(fallback.sources["max_output_tokens"], "default");
        let p = CloudProfile::from_config(&cfg("models: {cloud_chat: {max_tokens: 100, timeout_sec: 90, grok: {profile: {max_output_tokens: 99999, timeout_sec: 0.2, count_timeout_sec: 100, temperature: 0.4, rpm: 2}}}}"), "grok").unwrap();
        assert_eq!(p.max_output_tokens, 100);
        assert_eq!(p.timeout_sec, 1.0);
        assert_eq!(p.count_timeout_sec, 2.0);
        assert_eq!(p.sources["max_output_tokens"], "config");
        assert_eq!(p.sources["timeout_sec"], "profile");
        for bad in [
            "temperature: .nan",
            "rpm: 0",
            "max_output_tokens: -1",
            "context_window: 1",
            "tool_support: 'true'",
            "unknown: synthetic-private-canary",
        ] {
            let error = CloudProfile::from_config(
                &cfg(&format!("models: {{cloud_chat: {{grok: {{profile: {{{bad}}}}}}}}}")),
                "grok",
            )
            .unwrap_err();
            assert!(!format!("{error:?}").contains("synthetic-private-canary"));
        }
    }
}
