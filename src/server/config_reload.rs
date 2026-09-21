//! Only listed non-secret limits can change. One validated snapshot is published.
use std::io::Read;
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use serde_yaml_ng::Value;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::backend::ResolvedLocalBackend;

use super::state::AppState;
use super::web_search::Limits;

pub const RELOADABLE: [&str; 22] = [
    "api.rate_limit.max_requests",
    "api.rate_limit.window_seconds",
    "api.harness_loop_rate_limit.max_requests",
    "api.harness_loop_rate_limit.window_seconds",
    "api.harness_loop_rate_limit.max_tokens",
    "web.response_bytes",
    "web.request_seconds",
    "web.pages",
    "web.run_bytes",
    "web.run_seconds",
    "web.per_site_pages",
    "web.pace_ms",
    "web.cache_pages",
    "web.cache_bytes",
    "web.subqueries",
    "web.rounds",
    "web.evidence_tokens",
    "web.model_tokens",
    "web.total_tokens",
    "web.research_seconds",
    "web.stale_seconds",
    "web.chat_tool_calls",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RateBudget {
    pub max_requests: usize,
    pub window_seconds: f64,
}

impl RateBudget {
    fn load(cfg: &AppConfig, prefix: &str, requests: u64, seconds: f64) -> Result<Self> {
        let max_requests = integer(cfg, &format!("{prefix}.max_requests"), requests, 1, 100_000)? as usize;
        let key = format!("{prefix}.window_seconds");
        let window_seconds = match cfg.get(&key) {
            None => seconds,
            Some(value) => value.as_f64().ok_or_else(|| invalid_limit(&key))?,
        };
        if !window_seconds.is_finite() || !(0.001..=86400.0).contains(&window_seconds) {
            return Err(invalid_limit(&key));
        }
        Ok(Self {
            max_requests,
            window_seconds,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuntimeLimits {
    pub revision: u64,
    pub api: RateBudget,
    pub loop_rate: RateBudget,
    pub loop_max_tokens: u64,
    pub web: Limits,
}

impl RuntimeLimits {
    pub fn load(cfg: &AppConfig, backend: &ResolvedLocalBackend) -> Result<Self> {
        let key = "api.harness_loop_rate_limit.max_tokens";
        let cap = match integer(cfg, key, 2048, 0, u64::MAX)? {
            0 => 2048,
            n => n,
        };
        Ok(Self {
            revision: 0,
            api: RateBudget::load(cfg, "api.rate_limit", 60, 60.0)?,
            loop_rate: RateBudget::load(cfg, "api.harness_loop_rate_limit", 8, 300.0)?,
            loop_max_tokens: super::validate_reply_budget(key, cap, backend)?,
            web: Limits::load(cfg)?,
        })
    }
}

fn invalid_limit(key: &str) -> HarnessError {
    HarnessError::config(format!("invalid limit: {key}"))
}

fn integer(cfg: &AppConfig, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let value = match cfg.get(key) {
        None => default,
        Some(v) => v.as_u64().ok_or_else(|| invalid_limit(key))?,
    };
    if !(min..=max).contains(&value) {
        return Err(invalid_limit(key));
    }
    Ok(value)
}

fn remove_path(value: &mut Value, path: &[&str]) {
    let Some(map) = value.as_mapping_mut() else { return };
    let key = Value::String(path[0].into());
    if path.len() == 1 {
        map.remove(&key);
    } else if let Some(child) = map.get_mut(&key) {
        remove_path(child, &path[1..]);
        if child.as_mapping().is_some_and(|m| m.is_empty()) {
            map.remove(&key);
        }
    }
}

fn restart_fields(cfg: &AppConfig) -> Value {
    let mut raw = cfg.raw.clone();
    for key in RELOADABLE {
        remove_path(&mut raw, &key.split('.').collect::<Vec<_>>());
    }
    raw
}

fn candidate(state: &AppState) -> Result<RuntimeLimits> {
    // The file is local operator input, but a pipe or growing file must not
    // hold a worker indefinitely or allocate without a bound.
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&state.cfg.path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid_limit("config.yaml"));
    }
    let mut text = String::new();
    file.take(1_048_577).read_to_string(&mut text)?;
    if text.len() > 1_048_576 {
        return Err(invalid_limit("config.yaml"));
    }
    let cfg = AppConfig::from_str(&text, &state.cfg.path)?;
    if restart_fields(&cfg) != restart_fields(&state.cfg) {
        return Err(HarnessError::new(
            "CONFIG_RESTART_REQUIRED",
            "restart-only settings changed; no limits were reloaded",
        ));
    }
    RuntimeLimits::load(&cfg, &state.backend)
}

pub async fn reload(state: Arc<AppState>, source: &'static str) -> Result<serde_json::Value> {
    tokio::task::spawn_blocking(move || {
        // Serializes validation and publication, including concurrent SIGHUP/HTTP.
        let mut current = state
            .runtime
            .write()
            .map_err(|_| HarnessError::new("CONFIG_RELOAD_FAILED", "limits unavailable"))?;
        let mut next = match candidate(&state) {
            Ok(next) => next,
            Err(error) => {
                let code = if error.code == "CONFIG_RESTART_REQUIRED" {
                    "CONFIG_RESTART_REQUIRED"
                } else {
                    "CONFIG_RELOAD_INVALID"
                };
                state
                    .audit
                    .log(json!({"event":"config_reload_refused", "source":source, "code":code}));
                // YAML parser errors may contain file content. Never echo them.
                return Err(HarnessError::new(
                    code,
                    if code == "CONFIG_RESTART_REQUIRED" {
                        "restart-only or unknown settings changed; restore those settings or restart. All running limits were preserved"
                    } else {
                        "config.yaml contains invalid YAML or limits, or cannot be read; all running limits were preserved"
                    },
                ));
            }
        };
        next.revision = current.revision;
        let changed = next != **current;
        if changed {
            next.revision = current.revision.saturating_add(1);
        }
        let result = json!({"ok":true, "changed":changed, "limits":next, "reloadable":RELOADABLE});
        *current = Arc::new(next);
        state
            .audit
            .log(json!({"event":"config_reloaded", "source":source, "revision":current.revision, "changed":changed}));
        Ok(result)
    })
    .await
    .map_err(|_| HarnessError::new("CONFIG_RELOAD_FAILED", "reload worker failed"))?
}

/// Installed only by process entrypoints, never by in-process app fixtures.
#[cfg(unix)]
pub fn install_sighup(state: &Arc<AppState>) -> Result<tokio::task::JoinHandle<()>> {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let state = Arc::downgrade(state);
    Ok(tokio::spawn(async move {
        while signal.recv().await.is_some() {
            let Some(state) = state.upgrade() else { break };
            if let Err(error) = reload(state, "sighup").await {
                tracing::warn!(code = %error.code, "configuration reload refused; previous limits retained");
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_share_fetch_capacity_cancellation_and_mutation_locks() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", &dir.path().join("config.yaml")).unwrap();
        let first = super::super::web_search::WebTool::new(dir.path(), &cfg).unwrap();
        let mut second = first.clone();
        second.limits.response_bytes = 1024;
        assert_ne!(first.limits.response_bytes, second.limits.response_bytes);
        let _permits = first.permits.try_acquire_many(2).unwrap();
        assert!(
            second.permits.try_acquire().is_err(),
            "reload must not double the fetch capacity"
        );
        let _mutation = first.mutation.lock().unwrap();
        assert!(second.mutation.try_lock().is_err());
        let lease = first.research.start("local").unwrap();
        assert!(second.research.start("local").is_err());
        assert!(second.research.cancel("local").unwrap());
        assert!(lease.token.is_cancelled());
        drop(lease);
        assert!(second.research.start("local").is_ok());
    }

    #[test]
    fn reload_list_is_exact_and_rate_limits_reject_invalid_types() {
        let parse = |text: &str| AppConfig::from_str(text, std::path::Path::new("config.yaml")).unwrap();
        let initial = parse("web: {pages: 2, concurrency: 2}\nauth: {enabled: true}");
        let edited =
            parse("web: {pages: 3, concurrency: 2}\nauth: {enabled: true}\napi: {rate_limit: {max_requests: 10}}");
        assert_eq!(restart_fields(&initial), restart_fields(&edited));
        for text in [
            "web: {concurrency: 3}",
            "auth: {enabled: false}",
            "web: {pages: 3, arbitrary: 1}",
        ] {
            assert_ne!(restart_fields(&initial), restart_fields(&parse(text)));
        }
        for value in [".nan", ".inf", "0", "-1", "86401", "'60'", "null"] {
            let cfg = parse(&format!("api: {{rate_limit: {{window_seconds: {value}}}}}"));
            assert!(RateBudget::load(&cfg, "api.rate_limit", 60, 60.0).is_err());
        }
        for value in ["0", "-1", "100001", "'1'", "true", "null"] {
            let cfg = parse(&format!("api: {{rate_limit: {{max_requests: {value}}}}}"));
            assert!(RateBudget::load(&cfg, "api.rate_limit", 60, 60.0).is_err());
        }
    }
}
