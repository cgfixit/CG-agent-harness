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
/// A listed id must equal `model` exactly: what an OpenAI-compatible server
/// lists is what a request must send.
pub async fn model_readiness(endpoint: &str, model: &str, key: &str, limits: InventoryLimits) -> Value {
    readiness(endpoint, model, key, limits, false).await
}

/// [`model_readiness`] for a configured `provider`. Only "ollama" lists names
/// its own way (`name` as `name:latest`, `library/` dropped), and resolves the
/// configured spelling to the same model when called, so only there is the
/// name compared as Ollama lists it. Every other provider stays exact: a
/// near-miss id reported as installed would keep an unusable primary selected
/// over a working fallback.
pub async fn model_readiness_for(
    provider: &str,
    endpoint: &str,
    model: &str,
    key: &str,
    limits: InventoryLimits,
) -> Value {
    readiness(endpoint, model, key, limits, provider == "ollama").await
}

async fn readiness(endpoint: &str, model: &str, key: &str, limits: InventoryLimits, ollama_names: bool) -> Value {
    if !local_endpoint(endpoint) {
        return json!({"model":model,"state":"not_probed","detail":"Only a configured loopback model endpoint can be checked here."});
    }
    let result = match super::ollama::http_client(limits.timeout) {
        Ok(client) => {
            let mut request = client.get(format!("{}/models", endpoint.trim_end_matches('/')));
            if !key.is_empty() {
                request = request.bearer_auth(key);
            }
            super::ollama::bounded_json(request, limits.max_bytes).await
        }
        Err(_) => super::ollama::Bounded::Failed,
    };
    match result {
        super::ollama::Bounded::Json(value) if value["data"].is_array() => {
            let found = value["data"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|row| row["id"].as_str())
                .any(|listed| listed == model || (ollama_names && super::profile::same_model(listed, model)));
            json!({"endpoint":endpoint,"model":model,"state":if found {"installed"} else {"tag_missing"},
                "detail":"Inventory only; use an explicit console chat to test inference. No model was downloaded."})
        }
        _ => json!({"endpoint":endpoint,"model":model,"state":"unavailable",
            "detail":"Start the configured local model service and retry. No runtime was switched."}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ollama lists `llama3.2` as `llama3.2:latest`; both name the same model.
    #[tokio::test]
    async fn an_untagged_model_is_installed_when_ollama_lists_its_latest_tag() {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async {
                axum::Json(json!({"data": [{"id": "llama3.2:latest"}, {"id": "qwen3.8:27b-mlx-cg"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only ephemeral loopback.
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this fixture uses no credentials and binds only to loopback.
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let limits = InventoryLimits {
            timeout: Duration::from_secs(5),
            max_bytes: 1 << 16,
        };
        for (model, state) in [
            ("llama3.2", "installed"),
            ("llama3.2:latest", "installed"),
            ("library/llama3.2", "installed"),
            ("qwen3.8:27b-mlx-cg", "installed"),
            ("llama3.2:3b", "tag_missing"),
            ("qwen3.8", "tag_missing"),
            ("", "tag_missing"),
        ] {
            let readiness = model_readiness_for("ollama", &endpoint, model, "", limits).await;
            assert_eq!(readiness["state"], state, "{model}");
        }
        // Another provider lists the ids a request must send: only an exact id
        // is installed, with or without a provider named.
        for (model, state) in [
            ("llama3.2:latest", "installed"),
            ("qwen3.8:27b-mlx-cg", "installed"),
            ("llama3.2", "tag_missing"),
            ("library/llama3.2", "tag_missing"),
            ("library/qwen3.8:27b-mlx-cg", "tag_missing"),
        ] {
            let named = model_readiness_for("lmstudio", &endpoint, model, "", limits).await;
            assert_eq!(named["state"], state, "lmstudio {model}");
            let unnamed = model_readiness(&endpoint, model, "", limits).await;
            assert_eq!(unnamed["state"], state, "exact {model}");
        }
        server.abort();
    }
}
