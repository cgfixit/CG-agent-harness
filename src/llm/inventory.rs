//! Bounded, explicit local model inventory shared by readiness and failover.
use std::time::Duration;

use serde_json::{json, Value};

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

#[derive(Clone, Copy)]
pub struct InventoryLimits {
    pub timeout: Duration,
    pub max_bytes: usize,
}

impl InventoryLimits {
    /// Missing fields retain the desktop's existing limits for older homes.
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        let timeout = read_timeout(cfg, "models.local_llm.inventory.timeout_sec", 2.0)?;
        let max_bytes = match cfg.get("models.local_llm.inventory.max_response_bytes") {
            None => 262144,
            Some(value) => value.as_u64().filter(|n| (1024..=1048576).contains(n)).ok_or_else(|| {
                HarnessError::config(
                    "models.local_llm.inventory.max_response_bytes must be an integer from 1024 to 1048576",
                )
            })? as usize,
        };
        Ok(Self { timeout, max_bytes })
    }

    pub fn for_fallback(cfg: &AppConfig) -> Result<Self> {
        let mut limits = Self::from_config(cfg)?;
        limits.timeout = read_timeout(cfg, "models.local_llm.fallback.probe_timeout_sec", 1.5)?;
        Ok(limits)
    }
}

fn read_timeout(cfg: &AppConfig, path: &str, default: f64) -> Result<Duration> {
    let value = match cfg.get(path) {
        None => default,
        Some(value) => value.as_f64().unwrap_or(f64::NAN),
    };
    if !value.is_finite() || !(0.1..=30.0).contains(&value) {
        return Err(HarnessError::config(format!(
            "{path} must be a finite number from 0.1 to 30"
        )));
    }
    Ok(Duration::from_secs_f64(value))
}

pub fn local_endpoint(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && super::backend::is_loopback_url(raw)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

/// Never probes external endpoints or downloads models. Provider bodies are not
/// diagnostics; errors return a static category without echoing response text.
pub async fn model_readiness(endpoint: &str, model: &str, key: &str, limits: InventoryLimits) -> Value {
    if !local_endpoint(endpoint) {
        return json!({"model":model,"state":"not_probed","detail":"Only a configured loopback model endpoint can be checked here."});
    }
    let result = async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(limits.timeout)
            .build()?;
        let mut request = client.get(format!("{}/models", endpoint.trim_end_matches('/')));
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        let mut response = request.send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > limits.max_bytes.saturating_sub(bytes.len()) {
                return Ok::<_, reqwest::Error>(None);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice::<Value>(&bytes).ok())
    }
    .await;
    match result {
        Ok(Some(value)) if value["data"].is_array() => {
            let found = value["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"].as_str() == Some(model));
            json!({"endpoint":endpoint,"model":model,"state":if found {"installed"} else {"tag_missing"},
                "detail":"Inventory only; use an explicit console chat to test inference. No model was downloaded."})
        }
        _ => json!({"endpoint":endpoint,"model":model,"state":"unavailable",
            "detail":"Start the configured local model service and retry. No runtime was switched."}),
    }
}
