//! Durable owner-bound completion delivery. Only bounded job metadata leaves here.
use super::notification_outbox::{digest, read_private, Delivery, Outbox};
use super::web_policy::{canonical_http_url, is_public_ip};
use crate::common::{
    audit::Audit,
    auth_store::AuthManager,
    config::AppConfig,
    errors::{HarnessError, Result},
    home::Home,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub owner: String,
    pub job_id: String,
    pub status: String,
    pub created_at: f64,
    pub finished_at: f64,
}
impl Completion {
    pub(crate) fn valid(&self) -> bool {
        super::structured_memory::valid_owner(&self.owner)
            && self.job_id.len() == 32
            && self
                .job_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && ["finished", "failed", "cancelled"].contains(&self.status.as_str())
            && self.created_at.is_finite()
            && self.created_at >= 0.0
            && self.finished_at.is_finite()
            && self.finished_at >= 0.0
    }
    pub(crate) fn event_id(&self) -> String {
        digest(&serde_json::to_vec(self).expect("finite validated completion"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationSpec {
    id: String,
    owner: String,
    #[serde(default)]
    enabled: bool,
    url: String,
    events: Vec<String>,
    #[serde(default)]
    use_bearer: bool,
    rate_per_minute: u64,
}
#[derive(Clone)]
struct Destination {
    spec: DestinationSpec,
    url: Url,
    private: bool,
    revision: String,
}
struct Settings {
    destinations: Vec<Destination>,
    capacity: usize,
    batch_size: usize,
    batch_interval: u64,
    poll_interval: u64,
    attempts: u64,
    retry_delay: u64,
    max_backoff: u64,
    jitter_percent: u64,
    retention: u64,
    max_replays: u64,
    timeout: Duration,
    token: Option<String>,
}
fn invalid() -> HarnessError {
    HarnessError::config("invalid notifications configuration; use version 1 owned destinations, explicit subscriptions and finite limits; legacy webhook_url needs deliberate migration")
}
fn bounded(cfg: &AppConfig, key: &str, min: u64, max: u64) -> Result<u64> {
    let defaults = AppConfig::from_str(AppConfig::embedded_default(), std::path::Path::new("defaults"))?;
    cfg.get(key)
        .or_else(|| defaults.get(key))
        .and_then(|v| v.as_u64())
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(invalid)
}
fn read_token(home: &Home) -> Result<Option<String>> {
    let token = match std::env::var("CGAGENTHARNESS_WEBHOOK_TOKEN") {
        Ok(token) => Some(token),
        Err(std::env::VarError::NotPresent) => super::env_keys::read_startup_keys(&home.env_path())
            .map_err(|_| invalid())?
            .remove("CGAGENTHARNESS_WEBHOOK_TOKEN"),
        Err(_) => return Err(invalid()),
    }
    .filter(|s| !s.trim().is_empty());
    if token
        .as_ref()
        .is_some_and(|t| t.len() > 4096 || t.bytes().any(|c| !(0x21..=0x7e).contains(&c)))
    {
        return Err(invalid());
    }
    Ok(token)
}
impl Settings {
    fn load(home: &Home, cfg: &AppConfig) -> Result<Self> {
        if cfg.get("notifications.schema_version").and_then(|v| v.as_u64()) != Some(1)
            || !cfg.str_or("notifications.webhook_url", "").is_empty()
        {
            return Err(invalid());
        }
        let specs: Vec<DestinationSpec> =
            serde_json::from_value(cfg.json("notifications.destinations")).map_err(|_| invalid())?;
        if specs.len() > 8 {
            return Err(invalid());
        }
        let grants = cfg
            .get("notifications.private_url_allowlist")
            .and_then(|v| v.as_sequence())
            .ok_or_else(invalid)?;
        if grants.len() > 8 {
            return Err(invalid());
        }
        let mut allowed = Vec::new();
        for grant in grants {
            allowed.push(canonical_http_url(grant.as_str().ok_or_else(invalid)?, true).map_err(|_| invalid())?);
        }
        let mut ids = BTreeSet::new();
        let mut destinations = Vec::new();
        for spec in specs {
            if spec.id.is_empty()
                || spec.id.len() > 64
                || !spec
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !ids.insert(spec.id.clone())
                || !super::structured_memory::valid_owner(&spec.owner)
                || spec.events.is_empty()
                || spec.events.len() > 3
                || spec
                    .events
                    .iter()
                    .any(|s| !["finished", "failed", "cancelled"].contains(&s.as_str()))
                || !(1..=120).contains(&spec.rate_per_minute)
            {
                return Err(invalid());
            }
            let url = canonical_http_url(&spec.url, true).map_err(|_| invalid())?;
            let private = allowed.contains(&url);
            if url.fragment().is_some() || (url.scheme() != "https" && !private) {
                return Err(invalid());
            }
            let revision = digest(&serde_json::to_vec(&json!({"spec":spec,"private":private}))?);
            destinations.push(Destination {
                spec,
                url,
                private,
                revision,
            });
        }
        let token = read_token(home)?;
        if destinations.iter().any(|d| d.spec.enabled && d.spec.use_bearer) && token.is_none() {
            return Err(invalid());
        }
        Ok(Self {
            destinations,
            token,
            capacity: bounded(cfg, "notifications.queue_capacity", 1, 512)? as usize,
            batch_size: bounded(cfg, "notifications.batch_size", 1, 32)? as usize,
            batch_interval: bounded(cfg, "notifications.batch_interval_sec", 1, 3600)?,
            poll_interval: bounded(cfg, "notifications.poll_interval_sec", 1, 60)?,
            attempts: bounded(cfg, "notifications.max_attempts", 1, 3)?,
            retry_delay: bounded(cfg, "notifications.retry_delay_sec", 1, 60)?,
            max_backoff: bounded(cfg, "notifications.max_backoff_sec", 1, 3600)?,
            jitter_percent: bounded(cfg, "notifications.jitter_percent", 0, 50)?,
            retention: bounded(cfg, "notifications.retention_sec", 60, 2592000)?,
            max_replays: bounded(cfg, "notifications.max_replays", 0, 10)?,
            timeout: Duration::from_secs(bounded(cfg, "notifications.timeout_sec", 1, 30)?),
        })
    }
}
struct Inner {
    outbox: Mutex<Outbox>,
    settings: Settings,
    home: Home,
    config_path: PathBuf,
    auth: Option<Arc<AuthManager>>,
    audit: Audit,
    stop: tokio_util::sync::CancellationToken,
}
#[derive(Clone)]
pub struct Notifier(Arc<Inner>);
impl Notifier {
    pub fn start(home: &Home, cfg: &AppConfig, auth: Option<Arc<AuthManager>>) -> Result<Option<Self>> {
        if !cfg.flag_is_true("notifications.enabled") {
            return Ok(None);
        }
        if cfg.flag_is_true("auth.enabled") != auth.is_some() {
            return Err(invalid());
        }
        let settings = Settings::load(home, cfg)?;
        let outbox = Outbox::open(&home.data_dir().join("notifications/outbox.json"))?;
        let inner = Arc::new(Inner {
            outbox: Mutex::new(outbox),
            settings,
            home: Home::at(home.root.clone()),
            config_path: cfg.path.clone(),
            auth,
            audit: Audit::from_home(&home.root, cfg),
            stop: tokio_util::sync::CancellationToken::new(),
        });
        let weak = Arc::downgrade(&inner);
        let interval = Duration::from_secs(inner.settings.poll_interval);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if inner.stop.is_cancelled() {
                    break;
                }
                tokio::select! { _ = inner.stop.cancelled() => break, _ = inner.tick() => {} }
            }
        });
        Ok(Some(Self(inner)))
    }
    pub fn stop(&self) {
        self.0.stop.cancel();
    }
    pub fn enqueue(&self, event: Completion) {
        if let Err(code) = self.0.enqueue(event.clone()) {
            self.0
                .audit
                .log(json!({"event":"notification_dropped","job_id":if event.valid(){Some(&event.job_id)}else{None},"code":code}));
        }
    }
    pub fn status(&self, owner: &str) -> Value {
        let g = self.0.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let destinations: Vec<_> = self.0.settings.destinations.iter().filter(|d| d.spec.owner == owner).map(|d|
            json!({"id":d.spec.id,"enabled":d.spec.enabled,"authorized_now":self.0.authorized(d),"events":d.spec.events,"rate_per_minute":d.spec.rate_per_minute,"bearer_configured":d.spec.use_bearer})).collect();
        let deliveries: Vec<_> = g.data.deliveries.values().filter(|d| d.owner == owner).map(|d|
            json!({"delivery_id":d.delivery_id,"event_id":d.event_id,"destination_id":d.destination_id,"job_id":d.completion.job_id,"job_status":d.completion.status,"finished_at":d.completion.finished_at,"state":d.state,"attempts":d.attempts,"replays":d.replays,"next_attempt_at":d.next_attempt_at,"last_code":d.last_code})).collect();
        json!({"enabled":true,"destinations":destinations,"deliveries":deliveries,"semantics":"at_least_once_with_receiver_deduplication"})
    }
    pub fn replay(&self, owner: &str, id: &str) -> Result<()> {
        let mut g = self.0.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = g.data.clone();
        let d = next
            .deliveries
            .get_mut(id)
            .filter(|d| d.owner == owner)
            .ok_or_else(|| HarnessError::new("DELIVERY_NOT_FOUND", "no such retained delivery"))?;
        let destination = self
            .0
            .settings
            .destinations
            .iter()
            .find(|x| x.spec.id == d.destination_id && x.revision == d.destination_revision)
            .ok_or_else(|| HarnessError::new("DELIVERY_REVOKED", "destination grant changed"))?;
        if !self.0.authorized(destination) {
            return Err(HarnessError::new(
                "DELIVERY_REVOKED",
                "current destination or owner authority refused delivery",
            ));
        }
        let now = crate::common::now_ts();
        if !["failed", "delivered"].contains(&d.state.as_str())
            || d.replays >= self.0.settings.max_replays
            || d.completion.finished_at + self.0.settings.retention as f64 <= now
        {
            return Err(HarnessError::new(
                "DELIVERY_REPLAY_REFUSED",
                "delivery is pending, revoked, expired or at its replay limit",
            ));
        }
        d.state = "pending".into();
        d.attempts = 0;
        d.replays += 1;
        d.next_attempt_at = now + self.0.settings.batch_interval as f64;
        d.last_code = None;
        g.commit(next)?;
        self.0
            .audit
            .log(json!({"event":"notification_replay","delivery_id":id}));
        Ok(())
    }
}
impl Inner {
    /// Fresh disk checks can remove startup authority, never expand it. New
    /// destinations/credentials require restart; ordinary config reload adds none.
    fn authorized(&self, destination: &Destination) -> bool {
        if !destination.spec.enabled {
            return false;
        }
        let owner_ok = match &self.auth {
            Some(auth) => auth.list_users().iter().any(|u| {
                u.user_id == destination.spec.owner
                    && !u.disabled
                    && !u.must_change_password
                    && matches!(u.role.as_str(), "admin" | "operator")
            }),
            None => destination.spec.owner == "local",
        };
        if !owner_ok {
            return false;
        }
        let current = (|| -> Result<Settings> {
            let bytes = read_private(&self.config_path, 1_048_576)?;
            let cfg = AppConfig::from_str(std::str::from_utf8(&bytes).map_err(|_| invalid())?, &self.config_path)?;
            if !cfg.flag_is_true("notifications.enabled") || cfg.flag_is_true("auth.enabled") != self.auth.is_some() {
                return Err(invalid());
            }
            Settings::load(&self.home, &cfg)
        })();
        current.is_ok_and(|s| {
            s.destinations
                .iter()
                .any(|d| d.spec.enabled && d.spec.id == destination.spec.id && d.revision == destination.revision)
                && (!destination.spec.use_bearer || s.token == self.settings.token)
        })
    }
    fn enqueue(&self, event: Completion) -> std::result::Result<(), &'static str> {
        if !event.valid() {
            return Err("invalid_event");
        }
        let now = crate::common::now_ts();
        if event.finished_at + self.settings.retention as f64 <= now {
            return Ok(());
        }
        let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = g.data.clone();
        next.deliveries
            .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
        let mut pressure = false;
        for destination in self
            .settings
            .destinations
            .iter()
            .filter(|d| d.spec.owner == event.owner && d.spec.events.contains(&event.status))
        {
            if !self.authorized(destination) {
                continue;
            }
            let event_id = event.event_id();
            let delivery_id = digest(format!("{event_id}:{}:{}", destination.spec.id, destination.revision).as_bytes());
            if next.deliveries.contains_key(&delivery_id) {
                continue;
            }
            if next.deliveries.len() >= self.settings.capacity {
                let oldest = next
                    .deliveries
                    .iter()
                    .filter(|(_, d)| d.state != "pending")
                    .min_by(|(_, a), (_, b)| a.completion.finished_at.total_cmp(&b.completion.finished_at))
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    next.deliveries.remove(&id);
                    self.audit
                        .log(json!({"event":"notification_evicted","delivery_id":id,"code":"retention_pressure"}));
                } else {
                    pressure = true;
                    continue;
                }
            }
            next.deliveries.insert(
                delivery_id.clone(),
                Delivery {
                    delivery_id,
                    event_id,
                    owner: event.owner.clone(),
                    destination_id: destination.spec.id.clone(),
                    destination_revision: destination.revision.clone(),
                    completion: event.clone(),
                    state: "pending".into(),
                    attempts: 0,
                    replays: 0,
                    next_attempt_at: now + self.settings.batch_interval as f64,
                    last_code: None,
                },
            );
        }
        g.commit(next).map_err(|_| "outbox_persist_failed")?;
        if pressure {
            Err("queue_full")
        } else {
            Ok(())
        }
    }
    async fn tick(&self) {
        // Sweep even with zero configured destinations: removing the last grant
        // must revoke retained work rather than leave it misleadingly pending.
        {
            let now = crate::common::now_ts();
            let allowed: BTreeSet<_> = self
                .settings
                .destinations
                .iter()
                .filter(|d| self.authorized(d))
                .map(|d| (d.spec.id.clone(), d.revision.clone()))
                .collect();
            let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
            let mut next = g.data.clone();
            next.deliveries
                .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
            for d in next.deliveries.values_mut().filter(|d| d.state == "pending") {
                if !allowed.contains(&(d.destination_id.clone(), d.destination_revision.clone())) {
                    d.state = "revoked".into();
                    d.last_code = Some("authority_revoked".into());
                } else if d.attempts >= self.settings.attempts {
                    d.state = "failed".into();
                    d.last_code = Some("attempts_exhausted".into());
                }
            }
            if g.commit(next).is_err() {
                self.audit.log(json!({"event":"notification_store_failed"}));
                return;
            }
        }
        for destination in &self.settings.destinations {
            if self.stop.is_cancelled() {
                return;
            }
            let authorized = self.authorized(destination);
            let now = crate::common::now_ts();
            let batch = {
                let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
                let mut next = g.data.clone();
                next.deliveries
                    .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
                for d in next.deliveries.values_mut().filter(|d| d.state == "pending") {
                    let exists = self
                        .settings
                        .destinations
                        .iter()
                        .any(|x| x.spec.id == d.destination_id && x.revision == d.destination_revision);
                    if !exists || (d.destination_id == destination.spec.id && !authorized) {
                        d.state = "revoked".into();
                        d.last_code = Some("authority_revoked".into());
                    } else if d.attempts >= self.settings.attempts {
                        d.state = "failed".into();
                        d.last_code = Some("attempts_exhausted".into());
                    }
                }
                let mut batch = Vec::new();
                let rate_ok = next.next_send_at.get(&destination.spec.id).is_none_or(|t| *t <= now);
                if authorized && rate_ok {
                    for d in next
                        .deliveries
                        .values_mut()
                        .filter(|d| {
                            d.state == "pending"
                                && d.destination_id == destination.spec.id
                                && d.destination_revision == destination.revision
                                && d.next_attempt_at <= now
                        })
                        .take(self.settings.batch_size)
                    {
                        d.attempts += 1;
                        let base = (self.settings.retry_delay.saturating_mul(1u64 << (d.attempts - 1)))
                            .min(self.settings.max_backoff);
                        use rand::Rng;
                        let jitter = rand::thread_rng()
                            .gen_range(0.0..=base as f64 * self.settings.jitter_percent as f64 / 100.0);
                        d.next_attempt_at = now + self.settings.timeout.as_secs_f64() + base as f64 + jitter;
                        batch.push(d.clone());
                    }
                    if !batch.is_empty() {
                        next.next_send_at.insert(
                            destination.spec.id.clone(),
                            now + 60.0 / destination.spec.rate_per_minute as f64,
                        );
                    }
                }
                next.next_send_at
                    .retain(|id, _| self.settings.destinations.iter().any(|d| &d.spec.id == id));
                if g.commit(next).is_err() {
                    self.audit.log(json!({"event":"notification_store_failed"}));
                    continue;
                }
                batch
            };
            if batch.is_empty() {
                continue;
            }
            let ids: Vec<_> = batch.iter().map(|d| d.delivery_id.as_str()).collect();
            let batch_id = digest(ids.join(":").as_bytes());
            let events:Vec<_>=batch.iter().map(|d|json!({"event_id":d.event_id,"delivery_id":d.delivery_id,"job_id":d.completion.job_id,"status":d.completion.status,"created_at":d.completion.created_at,"finished_at":d.completion.finished_at})).collect();
            let payload = json!({"version":2,"batch_id":batch_id,"events":events});
            let result = if serde_json::to_vec(&payload).map_or(true, |b| b.len() > 65536) {
                Err("payload_too_large")
            } else if !self.authorized(destination) {
                Err("authority_revoked")
            } else {
                tokio::time::timeout(
                    self.settings.timeout,
                    deliver(
                        destination,
                        self.settings.timeout,
                        if destination.spec.use_bearer {
                            self.settings.token.as_deref()
                        } else {
                            None
                        },
                        &payload,
                        &batch_id,
                    ),
                )
                .await
                .unwrap_or(Err("timeout"))
            };
            let retry = matches!(
                result,
                Err("timeout" | "transport_failed" | "dns_failed" | "retryable_status")
            );
            let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
            let mut next = g.data.clone();
            for sent in &batch {
                if let Some(d) = next.deliveries.get_mut(&sent.delivery_id) {
                    d.state = if result.is_ok() {
                        "delivered"
                    } else if result == Err("authority_revoked") {
                        "revoked"
                    } else if retry && d.attempts < self.settings.attempts {
                        "pending"
                    } else {
                        "failed"
                    }
                    .into();
                    d.last_code = result.err().map(str::to_string);
                    // Waiting time excludes an unused request timeout budget.
                    if d.state == "pending" {
                        d.next_attempt_at = crate::common::now_ts()
                            + (sent.next_attempt_at - now - self.settings.timeout.as_secs_f64()).max(0.0);
                    }
                }
            }
            let persisted = g.commit(next).is_ok();
            self.audit.log(json!({"event":"notification_delivery","batch_id":batch_id,"count":batch.len(),"ok":result.is_ok(),"code":result.err(),"result_persisted":persisted}));
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
    destination: &Destination,
    timeout: Duration,
    token: Option<&str>,
    payload: &serde_json::Value,
    batch_id: &str,
) -> std::result::Result<(), &'static str> {
    let host = destination
        .url
        .host_str()
        .ok_or("invalid_url")?
        .trim_matches(['[', ']']);
    let port = destination.url.port_or_known_default().ok_or("invalid_url")?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "dns_failed")?
        .take(33)
        .collect();
    send_pinned(destination, timeout, token, payload, batch_id, &addresses).await
}

async fn send_pinned(
    destination: &Destination,
    timeout: Duration,
    token: Option<&str>,
    payload: &Value,
    batch_id: &str,
    addresses: &[SocketAddr],
) -> std::result::Result<(), &'static str> {
    let host = destination
        .url
        .host_str()
        .ok_or("invalid_url")?
        .trim_matches(['[', ']']);
    if !allowed_addresses(addresses, destination.private)
        || (destination.url.scheme() != "https" && addresses.iter().any(|a| is_public_ip(a.ip())))
    {
        return Err("destination_refused");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(super::web_search::RefuseDns))
        .resolve_to_addrs(host, addresses)
        .timeout(timeout)
        .build()
        .map_err(|_| "client_failed")?;
    let mut request = client
        .post(destination.url.clone())
        .header("X-CGAgentHarness-Batch-ID", batch_id)
        .json(payload);
    if let Some(token) = token {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_answers_fail_closed() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(!allowed_addresses(&[], true));
        assert!(!allowed_addresses(&[addr("127.0.0.1:80")], false)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(allowed_addresses(&[addr("127.0.0.1:80")], true)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(!allowed_addresses(&[addr("8.8.8.8:443"), addr("127.0.0.1:80")], false)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(!allowed_addresses(&[addr("169.254.169.254:80")], true));
        assert!(!allowed_addresses(&[addr("[::ffff:127.0.0.1]:80")], true)); // DevSkim: ignore DS162092 because this unit test pins IPv4-mapped loopback for the webhook destination grant.
    }
    #[tokio::test]
    async fn pinned_transport_does_not_reresolve_and_refuses_rebound_answers() {
        use axum::{
            http::{HeaderMap, StatusCode},
            routing::post,
            Router,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this owned test receiver must bind only to loopback.
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = hits.clone();
        let router = Router::new().route(
            "/hook",
            post(move |headers: HeaderMap| {
                let count = count.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        "Bearer isolated-transport-fixture"
                    );
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        // .invalid never resolves. Only the validated pinned loopback address
        // can reach this owned fixture; RefuseDns forbids any fallback lookup.
        let url = Url::parse(&format!("http://receiver.invalid:{}/hook", address.port())).unwrap(); // DevSkim: ignore DS137138 because the test pins this nonresolving hostname to its owned loopback HTTP receiver.
        let mut destination = Destination {
            spec: DestinationSpec {
                id: "fixture".into(),
                owner: "local".into(),
                enabled: true,
                url: url.to_string(),
                events: vec!["finished".into()],
                use_bearer: true,
                rate_per_minute: 60,
            },
            url,
            private: true,
            revision: "fixture".into(),
        };
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                Some("isolated-transport-fixture"),
                &json!({}),
                "fixture",
                &[address]
            )
            .await,
            Ok(())
        );
        destination.private = false;
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                None,
                &json!({}),
                "fixture",
                &[address]
            )
            .await,
            Err("destination_refused")
        );
        destination.private = true;
        let public: SocketAddr = "8.8.8.8:80".parse().unwrap();
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                None,
                &json!({}),
                "fixture",
                &[address, public]
            )
            .await,
            Err("destination_refused"),
            "public HTTP cannot be authorized by a private URL grant"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        task.abort();
    }
}
