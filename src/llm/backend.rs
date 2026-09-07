//! Local backend resolution (primary Ollama -> optional loopback fallback).
//! Port of `llm/client.py::resolve_local_backend` + `resolve_reasoning_effort`.

use std::time::Duration;

use serde::Serialize;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

pub const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];
const DEFAULT_PROBE_TIMEOUT_SEC: f64 = 1.5;
const VALID_EFFORTS: [&str; 5] = ["none", "low", "medium", "high", "max"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedLocalBackend {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    /// "primary" | "fallback"
    pub source: String,
    pub api_key: String,
    /// True when neither backend answered and this is the boot-must-not-die
    /// fallback to primary rather than a positive selection.
    pub degraded: bool,
    /// Ollama-only; `None` means "omit the field".
    pub reasoning_effort: Option<String>,
}

/// Whether `url`'s host is one of the supported loopback names.
pub fn is_loopback_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => u
            .host_str()
            .map(|h| LOOPBACK_HOSTS.contains(&h.to_lowercase().as_str()))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// `models.local_llm.reasoning_effort`: absent/empty -> None; invalid -> CONFIG_ERROR.
pub fn resolve_reasoning_effort(cfg: &AppConfig) -> Result<Option<String>> {
    match cfg.get("models.local_llm.reasoning_effort") {
        None | Some(serde_yaml_ng::Value::Null) => Ok(None),
        Some(serde_yaml_ng::Value::String(s)) => {
            let v = s.trim().to_lowercase();
            if v.is_empty() {
                return Ok(None);
            }
            if VALID_EFFORTS.contains(&v.as_str()) {
                Ok(Some(v))
            } else {
                Err(HarnessError::config(
                    "models.local_llm.reasoning_effort must be one of none|low|medium|high|max",
                ))
            }
        }
        Some(_) => Err(HarnessError::config(
            "models.local_llm.reasoning_effort must be a string",
        )),
    }
}

async fn probe_models(base_url: &str, timeout_sec: f64, api_key: &str) -> bool {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs_f64(timeout_sec.max(0.1)))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut req = client.get(format!("{}/models", base_url.trim_end_matches('/')));
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    matches!(req.send().await, Ok(resp) if resp.status().is_success())
}

/// Pick the primary or fallback local backend. With `fallback.enabled` false
/// (the shipped default) the primary is returned with no network probe.
pub async fn resolve_local_backend(cfg: &AppConfig) -> Result<ResolvedLocalBackend> {
    let primary_url = cfg.str_or("models.local_llm.base_url", "").trim().to_string();
    let primary_model = cfg.str_or("models.local_llm.model", "").trim().to_string();
    let mut primary_provider = cfg.str_or("models.local_llm.provider", "ollama").trim().to_string();
    if primary_provider.is_empty() {
        primary_provider = "ollama".to_string();
    }
    let primary_key = cfg.str_or("models.local_llm.api_key", "").trim().to_string();
    if primary_url.is_empty() {
        // Empty/unreadable config degrades to the Ollama default rather than failing app build.
        return Ok(ResolvedLocalBackend {
            provider: "ollama".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "qwen3.8:27b-mlx".into(),
            source: "primary".into(),
            api_key: String::new(),
            degraded: false,
            reasoning_effort: None,
        });
    }
    let configured_effort = resolve_reasoning_effort(cfg)?;
    let effort_for = |provider: &str| {
        if provider == "ollama" {
            configured_effort.clone()
        } else {
            None
        }
    };
    let primary = ResolvedLocalBackend {
        provider: primary_provider.clone(),
        base_url: primary_url.trim_end_matches('/').to_string(),
        model: primary_model,
        source: "primary".into(),
        api_key: primary_key.clone(),
        degraded: false,
        reasoning_effort: effort_for(&primary_provider),
    };
    if !cfg.flag_is_true("models.local_llm.fallback.enabled") {
        return Ok(primary);
    }
    let fb_url = cfg.str_or("models.local_llm.fallback.base_url", "").trim().to_string();
    let fb_model = cfg.str_or("models.local_llm.fallback.model", "").trim().to_string();
    let mut fb_provider = cfg
        .str_or("models.local_llm.fallback.provider", "lmstudio")
        .trim()
        .to_string();
    if fb_provider.is_empty() {
        fb_provider = "lmstudio".into();
    }
    let mut probe_timeout = cfg.f64_or("models.local_llm.fallback.probe_timeout_sec", DEFAULT_PROBE_TIMEOUT_SEC);
    if probe_timeout <= 0.0 {
        probe_timeout = DEFAULT_PROBE_TIMEOUT_SEC;
    }
    if fb_url.is_empty() || fb_model.is_empty() {
        return Err(HarnessError::new(
            "LLM_SERVICE_ERROR",
            "models.local_llm.fallback requires base_url and model when enabled",
        ));
    }
    if !is_loopback_url(&fb_url) {
        return Err(HarnessError::new(
            "LLM_SERVICE_ERROR",
            "models.local_llm.fallback.base_url must be loopback (127.0.0.1 / localhost / ::1)",
        ));
    }
    if probe_models(&primary.base_url, probe_timeout, &primary_key).await {
        tracing::info!("local LLM backend: primary ({primary_provider}) probe ok");
        return Ok(primary);
    }
    let secondary = ResolvedLocalBackend {
        provider: fb_provider.clone(),
        base_url: fb_url.trim_end_matches('/').to_string(),
        model: fb_model,
        source: "fallback".into(),
        api_key: cfg.str_or("models.local_llm.fallback.api_key", "").trim().to_string(),
        degraded: false,
        reasoning_effort: effort_for(&fb_provider),
    };
    if probe_models(&secondary.base_url, probe_timeout, &secondary.api_key).await {
        tracing::warn!("local LLM backend: primary unreachable; using fallback ({fb_provider})");
        return Ok(secondary);
    }
    tracing::warn!("local LLM backend: neither primary nor fallback answered; keeping primary (degraded)");
    Ok(ResolvedLocalBackend {
        degraded: true,
        ..primary
    })
}
