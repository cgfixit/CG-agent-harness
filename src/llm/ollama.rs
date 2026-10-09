//! Native Ollama HTTP: tags inventory, abortable pull, keep_alive warmup.
//!
//! Talks only to a loopback origin derived from the configured OpenAI-compatible
//! `/v1` base. Never sends `num_ctx`. Provider error bodies are not echoed.

use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::backend::{is_loopback_url, ResolvedLocalBackend};
use crate::llm::inventory::{local_endpoint, model_readiness, InventoryLimits};

pub const OLLAMA_ERROR: &str = "OLLAMA_UNAVAILABLE";
const MAX_MODEL_NAME: usize = 128;
const MAX_TAG_ROWS: usize = 256;

#[derive(Clone, Copy)]
pub struct PullLimits {
    pub timeout: Duration,
    pub max_bytes: usize,
}

impl PullLimits {
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            timeout: Duration::from_secs(clamped(cfg, "models.local_llm.pull.timeout_sec", 600, 1, 3600)),
            max_bytes: clamped(
                cfg,
                "models.local_llm.pull.max_response_bytes",
                1_048_576,
                1024,
                4_194_304,
            ) as usize,
        }
    }
}

pub fn clamped(cfg: &AppConfig, path: &str, default: u64, min: u64, max: u64) -> u64 {
    cfg.u64_or(path, default).clamp(min, max)
}

/// Origin used for `/api/tags`, `/api/pull`, `/api/generate`. Strips a trailing `/v1`.
pub fn native_base_url(endpoint: &str) -> Option<String> {
    if !local_endpoint(endpoint) && !native_endpoint(endpoint) {
        return None;
    }
    let url = url::Url::parse(endpoint).ok()?;
    // Rebuilt from literals (the scheme and the loopback allowlist's own host
    // string) plus the port number, so no configured text reaches a request URL.
    let scheme = match url.scheme() {
        "http" => "http",
        "https" => "https",
        _ => return None,
    };
    let configured = url.host_str()?.trim_matches(['[', ']']);
    let host = crate::llm::backend::LOOPBACK_HOSTS
        .iter()
        .copied()
        .find(|allowed| *allowed == configured)?;
    let mut origin = if host.contains(':') {
        format!("{scheme}://[{host}]")
    } else {
        format!("{scheme}://{host}")
    };
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    let path = url.path().trim_end_matches('/');
    if path.is_empty() || path == "/v1" {
        Some(origin)
    } else {
        None
    }
}

fn native_endpoint(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && is_loopback_url(raw)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && {
                let path = url.path().trim_end_matches('/');
                path.is_empty() || path == "/" || path == "/v1"
            }
    })
}

/// Ollama library names: `name`, `ns/name`, optional `:tag`. No URLs or paths.
pub fn model_name_ok(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_MODEL_NAME {
        return false;
    }
    let (body, tag) = match name.split_once(':') {
        Some((body, tag)) => (body, Some(tag)),
        None => (name, None),
    };
    if let Some(tag) = tag {
        if tag.is_empty() || !tag_token(tag) {
            return false;
        }
    }
    let mut parts = body.split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    if !tag_token(first) {
        return false;
    }
    parts.all(tag_token)
}

/// A name `/api/pull` may fetch: [`model_name_ok`] with at most `ns/name`.
/// Ollama reads a third segment as `host/ns/name` and dials that host, so a
/// pull of such a name reaches any registry, loopback or LAN address
/// included. Inventory and warmup keep host-qualified names an operator
/// pulled outside the harness; neither contacts a registry.
pub fn pull_name_ok(name: &str) -> bool {
    model_name_ok(name)
        && name
            .trim()
            .split(':')
            .next()
            .is_some_and(|body| body.split('/').count() <= 2)
}

fn tag_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s != "."
        && s != ".."
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn warmup_payload(model: &str, keep_alive_sec: u64) -> Value {
    json!({
        "model": model,
        "prompt": "",
        "stream": false,
        "keep_alive": keep_alive_sec,
    })
}

pub fn pull_payload(model: &str) -> Value {
    json!({
        "name": model,
        "model": model,
        "stream": true,
    })
}

pub(crate) fn http_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|e| HarnessError::new(OLLAMA_ERROR, format!("cannot build http client: {e}")))
}

fn static_err(message: &str) -> HarnessError {
    HarnessError::new(OLLAMA_ERROR, message)
}

/// Outcome of one bounded native-API read. Provider text is never kept.
pub(crate) enum Bounded {
    Json(Value),
    Status(u16),
    Failed,
}

/// Sends `request` and parses at most `max_bytes` of JSON body. Oversized,
/// malformed or unreachable responses are `Failed`; non-2xx is `Status`.
pub(crate) async fn bounded_json(request: reqwest::RequestBuilder, max_bytes: usize) -> Bounded {
    let Ok(mut response) = request.send().await else {
        return Bounded::Failed;
    };
    if !response.status().is_success() {
        return Bounded::Status(response.status().as_u16());
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= max_bytes.saturating_sub(bytes.len()) => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => return Bounded::Failed,
        }
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => Bounded::Json(value),
        Err(_) => Bounded::Failed,
    }
}

/// Installed tags from `GET /api/tags`; never echoes provider text. `None`
/// when the query fails, times out, or returns an oversized or malformed body,
/// so an outage is not reported as an empty install.
pub async fn list_tags(native: &str, limits: InventoryLimits) -> Option<Vec<Value>> {
    let client = http_client(limits.timeout).ok()?;
    let Bounded::Json(value) = bounded_json(client.get(format!("{native}/api/tags")), limits.max_bytes).await else {
        return None;
    };
    let rows = value.get("models")?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let name = row.get("name").and_then(Value::as_str)?.to_string();
                if !model_name_ok(&name) {
                    return None;
                }
                Some(json!({
                    "name": name,
                    "size": row.get("size").and_then(Value::as_u64).unwrap_or(0),
                }))
            })
            .take(MAX_TAG_ROWS)
            .collect(),
    )
}

/// What `/api/ps` says about `model`'s loaded window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadedWindow {
    Loaded(u64),
    /// Ollama answered with a valid list that has no row for `model`.
    NotLoaded,
    /// The read failed, timed out, was oversized or malformed, or the row has no window.
    Unknown,
}

pub async fn loaded_window_state(native: &str, model: &str, limits: InventoryLimits) -> LoadedWindow {
    let Ok(client) = http_client(limits.timeout) else {
        return LoadedWindow::Unknown;
    };
    let Bounded::Json(body) = bounded_json(client.get(format!("{native}/api/ps")), limits.max_bytes).await else {
        return LoadedWindow::Unknown;
    };
    if !body["models"].is_array() {
        return LoadedWindow::Unknown;
    }
    // One parser for `/api/ps` rows: the one `/model profile` uses.
    match crate::llm::profile::parse_ps(&body, model) {
        None => LoadedWindow::NotLoaded,
        Some(row) => row["context_length"]
            .as_u64()
            .map_or(LoadedWindow::Unknown, LoadedWindow::Loaded),
    }
}

/// Load `model` with an empty generate (the `keep_alive` warmup request; never
/// `num_ctx`), so Ollama picks the window it will serve it with. True on a 2xx.
pub async fn load_model(native: &str, model: &str, keep_alive_sec: u64, timeout: Duration) -> bool {
    if !model_name_ok(model) {
        return false;
    }
    let Ok(client) = http_client(timeout) else {
        return false;
    };
    client
        .post(format!("{native}/api/generate"))
        .json(&warmup_payload(model, keep_alive_sec))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

pub async fn snapshot(endpoint: &str, model: &str, key: &str, cfg: &AppConfig) -> Result<Value> {
    let Some(native) = native_base_url(endpoint) else {
        return Ok(json!({
            "endpoint": Value::Null,
            "configured_model": model,
            "configured_state": "not_probed",
            "models": [],
            "detail": "Only a configured loopback Ollama endpoint can be inventoried.",
        }));
    };
    let limits = InventoryLimits::from_config(cfg)?;
    let readiness = model_readiness(endpoint, model, key, limits).await;
    let models = list_tags(&native, limits).await;
    Ok(json!({
        "endpoint": native,
        "configured_model": model,
        "configured_state": readiness.get("state").cloned().unwrap_or(json!("unavailable")),
        "tags_state": if models.is_some() { "listed" } else { "unavailable" },
        "models": models.unwrap_or_default(),
        "detail": "Inventory only; use POST /api/ollama/pull to download. No num_ctx.",
    }))
}

pub async fn run_warmup(backend: &ResolvedLocalBackend, cfg: &AppConfig, audit: &Audit) {
    if !cfg.flag_is_true("models.local_llm.warmup.enabled") {
        return;
    }
    if backend.provider != "ollama" {
        return;
    }
    let Some(native) = native_base_url(&backend.base_url) else {
        tracing::warn!("ollama warmup skipped: endpoint is not a loopback Ollama origin");
        audit.log(json!({"event":"ollama_warmup_degraded","reason":"endpoint"}));
        return;
    };
    if !model_name_ok(&backend.model) {
        audit.log(json!({"event":"ollama_warmup_degraded","reason":"model"}));
        return;
    }
    let keep_alive = clamped(cfg, "models.local_llm.warmup.keep_alive_sec", 300, 1, 3600);
    let timeout = Duration::from_secs(clamped(cfg, "models.local_llm.warmup.timeout_sec", 30, 1, 120));
    let payload = warmup_payload(&backend.model, keep_alive);
    debug_assert!(!payload.to_string().contains("num_ctx"));
    let result = async {
        let client = http_client(timeout)?;
        let response = client
            .post(format!("{native}/api/generate"))
            .json(&payload)
            .send()
            .await
            .map_err(|_| static_err("warmup request failed"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(static_err("warmup refused"))
        }
    }
    .await;
    match result {
        Ok(()) => {
            tracing::info!(model = backend.model.as_str(), "ollama keep_alive warmup ok");
            audit.log(json!({"event":"ollama_warmup_ok","model":backend.model}));
        }
        Err(_) => {
            tracing::warn!(model = backend.model.as_str(), "ollama keep_alive warmup degraded");
            audit.log(json!({"event":"ollama_warmup_degraded","model":backend.model}));
        }
    }
}

/// Stream NDJSON pull progress. `on_event` receives sanitized status objects.
pub async fn pull_model(
    native: &str,
    model: &str,
    limits: PullLimits,
    cancel: CancellationToken,
    mut on_event: impl FnMut(Value),
) -> Result<()> {
    if !pull_name_ok(model) {
        return Err(static_err("model name rejected"));
    }
    let payload = pull_payload(model);
    debug_assert!(!payload.to_string().contains("num_ctx"));
    let client = http_client(limits.timeout)?;
    let request = client.post(format!("{native}/api/pull")).json(&payload);
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(static_err("pull aborted")),
        response = request.send() => response.map_err(|_| static_err("pull request failed"))?,
    };
    if !response.status().is_success() {
        return Err(static_err("pull refused"));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    let mut saw_success = false;
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(static_err("pull aborted")),
            chunk = response.chunk() => chunk.map_err(|_| static_err("pull stream failed"))?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if chunk.len() > limits.max_bytes.saturating_sub(bytes.len()) {
            return Err(static_err("pull stream exceeded the configured byte bound"));
        }
        bytes.extend_from_slice(&chunk);
        while let Some(idx) = bytes.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = bytes.drain(..=idx).collect();
            if let Some(event) = sanitize_pull_line(&line) {
                if event.get("status").and_then(Value::as_str) == Some("success") {
                    saw_success = true;
                }
                on_event(event);
            }
        }
    }
    if !bytes.is_empty() {
        if let Some(event) = sanitize_pull_line(&bytes) {
            if event.get("status").and_then(Value::as_str) == Some("success") {
                saw_success = true;
            }
            on_event(event);
        }
    }
    if saw_success {
        Ok(())
    } else if cancel.is_cancelled() {
        Err(static_err("pull aborted"))
    } else {
        Err(static_err("pull finished without success"))
    }
}

fn sanitize_pull_line(line: &[u8]) -> Option<Value> {
    let line = std::str::from_utf8(line).ok()?.trim();
    if line.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(line).ok()?;
    if parsed.get("error").is_some() {
        return Some(json!({"status":"error"}));
    }
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("progress");
    let status: String = status.chars().take(64).collect();
    let mut out = json!({"status": status});
    if let Some(total) = parsed.get("total").and_then(Value::as_u64) {
        out["total"] = json!(total);
    }
    if let Some(completed) = parsed.get("completed").and_then(Value::as_u64) {
        out["completed"] = json!(completed);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failed_tag_query_is_none_not_an_empty_install() {
        // Bind then drop a listener so the port refuses connections.
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap(); // DevSkim: ignore DS162092 because this unit test needs a refused loopback port.
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let limits = InventoryLimits {
            timeout: Duration::from_secs(2),
            max_bytes: 1 << 16,
        };
        let native = format!("http://127.0.0.1:{port}"); // DevSkim: ignore DS162092 because this unit test needs a refused loopback origin.
        assert!(list_tags(&native, limits).await.is_none());
    }

    #[test]
    fn native_url_strips_v1_and_refuses_unsafe_endpoints() {
        assert_eq!(
            native_base_url("http://127.0.0.1:11434/v1").as_deref(), // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
            Some("http://127.0.0.1:11434") // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
        );
        assert_eq!(
            native_base_url("http://localhost:11434/v1/").as_deref(), // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
            Some("http://localhost:11434") // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
        );
        assert_eq!(
            native_base_url("http://127.0.0.1:11434").as_deref(), // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
            Some("http://127.0.0.1:11434") // DevSkim: ignore DS162092 because this unit test pins the loopback Ollama origin.
        );
        assert!(native_base_url("http://user:secret@127.0.0.1:11434/v1").is_none()); // DevSkim: ignore DS137138 because this unit test must reject userinfo on loopback HTTP.
        assert!(native_base_url("http://127.0.0.1:11434/v1?token=secret").is_none()); // DevSkim: ignore DS137138 because this unit test must reject query tokens on loopback HTTP.
        assert!(native_base_url("https://example.invalid/v1").is_none());
        assert!(native_base_url("ftp://127.0.0.1/v1").is_none()); // DevSkim: ignore DS162092 because this unit test must reject non-HTTP loopback schemes.
    }

    #[test]
    fn model_names_reject_urls_and_paths() {
        assert!(model_name_ok("tinyllama"));
        assert!(model_name_ok("library/tinyllama:latest"));
        assert!(model_name_ok("qwen3.8:27b-mlx"));
        assert!(!model_name_ok(""));
        assert!(!model_name_ok("../etc/passwd"));
        assert!(!model_name_ok("http://evil.example/model")); // DevSkim: ignore DS137138 because this unit test rejects URL-shaped model names.
        assert!(!model_name_ok("name with space"));
        assert!(!model_name_ok("a:"));
    }

    #[test]
    fn pull_names_never_select_a_registry_host() {
        assert!(pull_name_ok("tinyllama"));
        assert!(pull_name_ok("library/tinyllama:latest"));
        assert!(model_name_ok("hf.co/user/repo:Q4_K_M"));
        assert!(!pull_name_ok("hf.co/user/repo:Q4_K_M"));
        assert!(!pull_name_ok("registry.example.invalid/ns/model"));
        assert!(!pull_name_ok("127.0.0.1/ns/model:tag")); // DevSkim: ignore DS162092 because this unit test must refuse a loopback registry host in a pull name.
        assert!(!pull_name_ok("a/b/c/d"));
    }

    #[test]
    fn warmup_and_pull_payloads_omit_num_ctx() {
        let warm = warmup_payload("qwen3.8:27b-mlx", 300);
        assert_eq!(warm["keep_alive"], 300);
        assert_eq!(warm["prompt"], "");
        assert_eq!(warm["stream"], false);
        assert!(warm.get("num_ctx").is_none());
        assert!(!warm.to_string().contains("num_ctx"));
        let pull = pull_payload("tinyllama");
        assert_eq!(pull["name"], "tinyllama");
        assert_eq!(pull["model"], "tinyllama");
        assert_eq!(pull["stream"], true);
        assert!(pull.get("num_ctx").is_none());
        assert!(!pull.to_string().contains("num_ctx"));
    }

    #[test]
    fn pull_progress_drops_provider_error_text() {
        let event = sanitize_pull_line(br#"{"error":"fixture-private-provider-error"}"#).unwrap();
        assert_eq!(event["status"], "error");
        assert!(!event.to_string().contains("fixture-private"));
        let ok =
            sanitize_pull_line(br#"{"status":"downloading","digest":"sha256:abc","total":10,"completed":3}"#).unwrap();
        assert_eq!(ok["status"], "downloading");
        assert_eq!(ok["total"], 10);
        assert_eq!(ok["completed"], 3);
        assert!(ok.get("digest").is_none());
    }
}
