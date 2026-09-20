//! Optional, bounded, best-effort completion webhooks. No job content leaves here.
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc;
use url::Url;

use super::web_policy::{canonical_http_url, is_public_ip};
use crate::common::{
    audit::Audit,
    config::AppConfig,
    errors::{HarnessError, Result},
    home::Home,
};

#[derive(Clone, Debug, Serialize)]
pub struct Completion {
    pub job_id: String,
    pub status: String,
    pub created_at: f64,
    pub finished_at: f64,
}

struct Settings {
    url: Url,
    private: bool,
    batch_size: usize,
    batch_interval: Duration,
    attempts: u64,
    retry_delay: Duration,
    timeout: Duration,
    token: Option<String>,
}

pub struct Notifier {
    sender: mpsc::Sender<Completion>,
    audit: Arc<Audit>,
}

fn invalid() -> HarnessError {
    HarnessError::config("invalid notifications configuration; check URL, limits and private destination allowlist")
}

fn bounded(cfg: &AppConfig, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let n = match cfg.get(key) {
        None => default,
        Some(v) => v.as_u64().ok_or_else(invalid)?,
    };
    if !(min..=max).contains(&n) {
        return Err(invalid());
    }
    Ok(n)
}

impl Notifier {
    pub fn start(home: &Home, cfg: &AppConfig) -> Result<Option<Self>> {
        if !cfg.flag_is_true("notifications.enabled") {
            return Ok(None);
        }
        let url = canonical_http_url(&cfg.str_or("notifications.webhook_url", ""), true).map_err(|_| invalid())?;
        if url.fragment().is_some() {
            return Err(invalid());
        }
        let mut private = false;
        if let Some(value) = cfg.get("notifications.private_url_allowlist") {
            let entries = value.as_sequence().ok_or_else(invalid)?;
            if entries.len() > 8 {
                return Err(invalid());
            }
            for entry in entries {
                let allowed = canonical_http_url(entry.as_str().ok_or_else(invalid)?, true).map_err(|_| invalid())?;
                if allowed == url {
                    private = true;
                }
            }
        }
        if url.scheme() != "https" && !private {
            return Err(invalid());
        }
        let token = match std::env::var("CGAGENTHARNESS_WEBHOOK_TOKEN") {
            Ok(token) => Some(token),
            Err(std::env::VarError::NotPresent) => super::env_keys::read_startup_keys(&home.env_path())
                .map_err(|_| invalid())?
                .remove("CGAGENTHARNESS_WEBHOOK_TOKEN"),
            Err(_) => return Err(invalid()),
        }
        .filter(|s| !s.trim().is_empty());
        if let Some(token) = &token {
            if token.len() > 4096 || token.bytes().any(|c| !(0x21..=0x7e).contains(&c)) {
                return Err(invalid());
            }
        }
        let settings = Settings {
            url,
            private,
            token,
            batch_size: bounded(cfg, "notifications.batch_size", 16, 1, 32)? as usize,
            batch_interval: Duration::from_secs(bounded(cfg, "notifications.batch_interval_sec", 30, 1, 3600)?),
            attempts: bounded(cfg, "notifications.max_attempts", 3, 1, 3)?,
            retry_delay: Duration::from_secs(bounded(cfg, "notifications.retry_delay_sec", 2, 1, 60)?),
            timeout: Duration::from_secs(bounded(cfg, "notifications.timeout_sec", 5, 1, 30)?),
        };
        let (sender, receiver) = mpsc::channel(bounded(cfg, "notifications.queue_capacity", 128, 1, 512)? as usize);
        let audit = Arc::new(Audit::from_home(&home.root, cfg));
        tokio::spawn(run(receiver, settings, audit.clone()));
        Ok(Some(Self { sender, audit }))
    }

    pub fn enqueue(&self, event: Completion) {
        let id = event.job_id.clone();
        if self.sender.try_send(event).is_err() {
            self.audit
                .log(json!({"event":"notification_dropped", "job_id":id, "reason":"queue_unavailable"}));
        }
    }
}

fn allowed_addresses(addresses: &[SocketAddr], private: bool) -> bool {
    !addresses.is_empty()
        && addresses.len() <= 32
        && addresses.iter().all(|a| {
            is_public_ip(a.ip())
                || private
                    && match a.ip() {
                        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
                        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
                    }
        })
}

async fn deliver(
    settings: &Settings,
    payload: &serde_json::Value,
    batch_id: &str,
) -> std::result::Result<(), &'static str> {
    let host = settings.url.host_str().ok_or("invalid_url")?.trim_matches(['[', ']']);
    let port = settings.url.port_or_known_default().ok_or("invalid_url")?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "dns_failed")?
        .take(33)
        .collect();
    if !allowed_addresses(&addresses, settings.private) {
        return Err("destination_refused");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(super::web_search::RefuseDns))
        .resolve_to_addrs(host, &addresses)
        .timeout(settings.timeout)
        .build()
        .map_err(|_| "client_failed")?;
    let mut request = client
        .post(settings.url.clone())
        .header("X-CGAgentHarness-Batch-ID", batch_id)
        .json(payload);
    if let Some(token) = &settings.token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|_| "transport_failed")?;
    let status = response.status();
    // Response bodies and URLs may contain secrets. Neither is read or logged.
    if status.is_success() {
        Ok(())
    } else if status.as_u16() == 429 || status.is_server_error() {
        Err("retryable_status")
    } else {
        Err("status_refused")
    }
}

async fn run(mut receiver: mpsc::Receiver<Completion>, settings: Settings, audit: Arc<Audit>) {
    while let Some(first) = receiver.recv().await {
        tokio::time::sleep(settings.batch_interval).await;
        let mut events = vec![first];
        while events.len() < settings.batch_size {
            match receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        let batch_id = crate::common::random_hex(16);
        let payload = json!({"version":1,"batch_id":batch_id,"events":events});
        for attempt in 1..=settings.attempts {
            let result = tokio::time::timeout(settings.timeout, deliver(&settings, &payload, &batch_id))
                .await
                .unwrap_or(Err("timeout"));
            let retry = matches!(
                result,
                Err("timeout" | "transport_failed" | "dns_failed" | "retryable_status")
            ) && attempt < settings.attempts;
            audit.log(
                json!({"event":"notification_delivery","batch_id":batch_id,"count":events.len(),"attempt":attempt,
                "ok":result.is_ok(),"code":result.err(),"retry":retry}),
            );
            if !retry {
                break;
            }
            tokio::time::sleep(settings.retry_delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_answers_fail_closed() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(!allowed_addresses(&[], true));
        assert!(!allowed_addresses(&[addr("127.0.0.1:80")], false));
        assert!(allowed_addresses(&[addr("127.0.0.1:80")], true));
        assert!(!allowed_addresses(&[addr("8.8.8.8:443"), addr("127.0.0.1:80")], false));
        assert!(!allowed_addresses(&[addr("169.254.169.254:80")], true));
        assert!(!allowed_addresses(&[addr("[::ffff:127.0.0.1]:80")], true));
    }
}
