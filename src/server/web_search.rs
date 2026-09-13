//! Permission-checked content reads. Every network/evidence path reloads URL policy.
use std::collections::BTreeSet;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::web_policy::{canonical_url, error, is_public_ip, Policy, Rule, MAX_RULES};
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::tool_broker::assert_allowed;

pub const MAX_ALLOW: usize = MAX_RULES;
pub const MAX_BYTES: usize = 262_144;
const MAX_CONTEXT: usize = 4000;

#[derive(Debug, Clone)]
pub struct Limits {
    pub response_bytes: usize,
    pub request_seconds: u64,
    pub concurrency: usize,
    pub pages: usize,
    pub run_bytes: usize,
    pub run_seconds: u64,
    pub per_site_pages: usize,
    pub pace_ms: u64,
    pub cache_pages: usize,
    pub cache_bytes: usize,
    pub subqueries: usize,
    pub rounds: usize,
    pub evidence_tokens: u64,
    pub model_tokens: u64,
    pub total_tokens: u64,
    pub research_seconds: u64,
    pub stale_seconds: u64,
    pub chat_tool_calls: usize,
}

impl Limits {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let bound = |name: &str, default: u64, min: u64, max: u64| -> Result<u64> {
            let key = format!("web.{name}");
            let value = match cfg.get(&key) {
                None => default,
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| HarnessError::config(format!("{key} must be an integer")))?,
            };
            if !(min..=max).contains(&value) {
                return Err(HarnessError::config(format!("{key} must be {min}..={max}")));
            }
            Ok(value)
        };
        Ok(Self {
            response_bytes: bound("response_bytes", MAX_BYTES as u64, 1024, 1_048_576)? as usize,
            request_seconds: bound("request_seconds", 8, 1, 30)?,
            concurrency: bound("concurrency", 2, 1, 4)? as usize,
            pages: bound("pages", 20, 1, 40)? as usize,
            run_bytes: bound("run_bytes", 2_097_152, 1024, 8_388_608)? as usize,
            run_seconds: bound("run_seconds", 60, 1, 180)?,
            per_site_pages: bound("per_site_pages", 10, 1, 20)? as usize,
            pace_ms: bound("pace_ms", 250, 100, 5000)?,
            cache_pages: bound("cache_pages", 128, 1, 256)? as usize,
            cache_bytes: bound("cache_bytes", 16_777_216, 1_048_576, 33_554_432)? as usize,
            subqueries: bound("subqueries", 3, 1, 5)? as usize,
            rounds: bound("rounds", 2, 1, 2)? as usize,
            evidence_tokens: bound("evidence_tokens", 3000, 256, 6000)?,
            model_tokens: bound("model_tokens", 1024, 256, 2048)?,
            total_tokens: bound("total_tokens", 16000, 2048, 32000)?,
            research_seconds: bound("research_seconds", 300, 10, 1800)?,
            chat_tool_calls: bound("chat_tool_calls", 3, 1, 5)? as usize,
            stale_seconds: bound("stale_seconds", 604800, 60, 31_536_000)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
    pub links: Vec<String>,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    pub transfer_bytes: usize,
    pub chars: usize,
    pub fetched_at: f64,
    pub content_hash: String,
    pub extraction_version: u32,
    pub policy_revision: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl Page {
    pub fn validate(&self, policy: &Policy, group: Option<&str>) -> Result<()> {
        policy.authorize(&self.url, group)?;
        if self.extraction_version != 1
            || self.policy_revision.len() != 64
            || self.content_hash != crate::common::sha256_hex(&self.text)
            || !self.fetched_at.is_finite()
            || self.links.len() > 128
        {
            return Err(error(
                "WEB_EVIDENCE_INVALID",
                "unproven or corrupt saved evidence refused",
            ));
        }
        Ok(())
    }
}

/// A fresh client has exactly one DNS override and no fallback resolver.
struct RefuseDns;
impl reqwest::dns::Resolve for RefuseDns {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async { Err(std::io::Error::other("unvalidated DNS refused").into()) })
    }
}

pub fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
    if addresses.is_empty() {
        return Err(error("WEB_DNS", "DNS returned no addresses"));
    }
    if addresses.len() > 32 || addresses.iter().any(|a| !is_public_ip(a.ip())) {
        return Err(error("WEB_SSRF_DENIED", "DNS answer includes prohibited addresses"));
    }
    Ok(())
}

/// HTML5 parsing decodes entities and repairs malformed markup. Never execute it.
pub fn extract(body: &str, content_type: &str, base: &url::Url) -> (String, String, Vec<String>) {
    if !content_type.contains("html") {
        return (String::new(), body.to_string(), Vec::new());
    }
    let html = scraper::Html::parse_document(body);
    let mut title = String::new();
    let mut text = String::new();
    let mut links = BTreeSet::new();
    let mut previous_block = None;
    let block = |name: &str| {
        matches!(
            name,
            "body"
                | "main"
                | "article"
                | "section"
                | "p"
                | "div"
                | "li"
                | "tr"
                | "td"
                | "pre"
                | "blockquote"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        )
    };
    for node in html.tree.root().descendants() {
        if node.ancestors().any(|a| {
            a.value().as_element().is_some_and(|e| {
                matches!(
                    e.name(),
                    "script" | "style" | "noscript" | "template" | "nav" | "header" | "footer" | "form" | "svg"
                ) || e.attr("hidden").is_some()
                    || e.attr("aria-hidden") == Some("true")
            })
        }) {
            continue;
        }
        if let Some(element) = node.value().as_element() {
            if block(element.name()) {
                previous_block = Some(node.id());
            }
            if matches!(
                element.name(),
                "p" | "div" | "br" | "li" | "tr" | "pre" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                text.push('\n');
            }
            if matches!(element.name(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                text.push_str("# ");
            }
            if element.name() == "a" && links.len() < 128 {
                if let Some(url) = element
                    .attr("href")
                    .and_then(|v| base.join(v).ok())
                    .and_then(|u| canonical_url(u.as_str()).ok())
                {
                    links.insert(url.to_string());
                }
            }
        }
        if let Some(value) = node.value().as_text() {
            if node
                .ancestors()
                .any(|a| a.value().as_element().is_some_and(|e| e.name() == "title"))
            {
                title.push_str(value);
            } else if !node
                .ancestors()
                .any(|a| a.value().as_element().is_some_and(|e| e.name() == "head"))
            {
                let current = node
                    .ancestors()
                    .find(|n| n.value().as_element().is_some_and(|e| block(e.name())))
                    .map(|n| n.id());
                if current != previous_block {
                    text.push('\n');
                    previous_block = current;
                }
                if node
                    .ancestors()
                    .any(|a| a.value().as_element().is_some_and(|e| e.name() == "pre"))
                {
                    text.push_str(value);
                } else {
                    text.push_str(&value.split_whitespace().collect::<Vec<_>>().join(" "));
                    text.push(' ');
                }
            }
        }
    }
    (
        title.trim().to_string(),
        text.trim().to_string(),
        links.into_iter().collect(),
    )
}

#[derive(Debug)]
pub struct WebTool {
    pub(super) tools_dir: PathBuf,
    /// Programmatic fixture hook; no configuration, CLI or HTTP path sets it.
    pub test_resolve: Option<(String, SocketAddr)>,
    /// Additional exact hosts for bounded multi-site fixtures, never runtime configuration.
    pub test_resolve_extra: Vec<(String, SocketAddr)>,
    pub limits: Limits,
    pub(super) permits: Semaphore,
    pub(super) mutation: Mutex<()>,
    pub(super) search_gate: Arc<Semaphore>,
    pub research: super::web_research::ResearchState,
    pub chat_turn: super::web_research::ResearchState,
}

impl WebTool {
    pub fn new(tools_dir: &Path, cfg: &AppConfig) -> Result<Self> {
        let limits = Limits::load(cfg)?;
        Ok(Self {
            tools_dir: tools_dir.into(),
            test_resolve: None,
            test_resolve_extra: Vec::new(),
            permits: Semaphore::new(limits.concurrency),
            limits,
            mutation: Mutex::new(()),
            search_gate: Arc::new(Semaphore::new(1)),
            research: super::web_research::ResearchState::default(),
            chat_turn: super::web_research::ResearchState::default(),
        })
    }
    fn allow_path(&self) -> PathBuf {
        self.tools_dir.join("web_allowlist.json")
    }
    fn last_path(&self, owner: &str) -> PathBuf {
        self.tools_dir
            .join(format!("web_{}_last.json", crate::common::sha256_hex(owner)))
    }
    fn context_path(&self, owner: &str) -> PathBuf {
        self.tools_dir
            .join(format!("web_{}_context.json", crate::common::sha256_hex(owner)))
    }
    pub fn policy(&self) -> Result<Policy> {
        Policy::load(&self.allow_path())
    }

    pub(super) fn read_page(&self, path: &Path, group: Option<&str>) -> Result<Page> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = opts
            .open(path)
            .map_err(|_| error("WEB_NO_LAST", "no valid saved extract"))?;
        if !file.metadata()?.is_file() {
            return Err(error("WEB_EVIDENCE_INVALID", "saved extract must be a file"));
        }
        let mut bytes = Vec::new();
        let cap = self.limits.response_bytes * 8 + 65_536;
        file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > cap {
            return Err(error("WEB_EVIDENCE_INVALID", "saved extract exceeds limit"));
        }
        let page: Page = serde_json::from_slice(&bytes)
            .map_err(|_| error("WEB_EVIDENCE_INVALID", "unproven saved extract refused"))?;
        page.validate(&self.policy()?, group)?;
        Ok(page)
    }

    pub fn status(&self, enabled: bool, owner: &str) -> Result<Value> {
        let policy = self.policy()?;
        let stored = self.read_page(&self.context_path(owner), None).is_ok();
        Ok(
            json!({"enabled": enabled, "allowlist": policy.rules.iter().map(|r| &r.pattern).collect::<Vec<_>>(),
            "rules": policy.rules, "policy_revision": policy.revision,
            "search_provider": if std::env::var("SERPAPI_API_KEY").unwrap_or_default().trim().is_empty() { "public Google (may require JavaScript/CAPTCHA)" } else { "Google via SerpAPI" },
            "injected": enabled && stored, "context_stored": stored,
            "has_last": self.read_page(&self.last_path(owner), None).is_ok(), "max_allow": MAX_ALLOW}),
        )
    }

    pub fn allow(&self, raw: &str, enabled: bool) -> Result<Value> {
        self.allow_rule(raw, "default", &[], enabled)
    }
    pub fn allow_rule(&self, raw: &str, group: &str, seeds: &[String], enabled: bool) -> Result<Value> {
        let rule = Rule::new(raw, group, seeds)?;
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        // An explicit validated admin grant can initialize a missing policy.
        // Reads still fail closed, and corrupt/unsafe existing files never reset.
        let mut policy = match std::fs::symlink_metadata(self.allow_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Policy::empty(),
            _ => self.policy()?,
        };
        if let Some(existing) = policy.rules.iter_mut().find(|r| r.id == rule.id) {
            *existing = rule;
        } else {
            if policy.rules.len() >= MAX_ALLOW {
                return Err(error("WEB_ALLOWLIST_FULL", "rule cap reached"));
            }
            policy.rules.push(rule);
        }
        self.save_policy(&policy)?;
        self.status(enabled, "")
    }

    fn save_policy(&self, policy: &Policy) -> Result<()> {
        let bytes = serde_json::to_vec(policy)?;
        Policy::parse(&bytes)?;
        crate::common::atomic::write_atomic(&self.allow_path(), &bytes, Some(0o600))
    }

    pub fn deny(&self, raw: &str, enabled: bool) -> Result<Value> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let mut policy = self.policy()?;
        let pattern = Rule::new(raw, "default", &[]).ok().map(|r| r.pattern);
        if pattern.is_none() && !policy.rules.iter().any(|r| r.id == raw) {
            return Err(error("WEB_BAD_URL", "use a rule ID or pattern"));
        }
        policy
            .rules
            .retain(|r| r.id != raw && pattern.as_ref().is_none_or(|p| r.pattern != *p));
        self.save_policy(&policy)?;
        self.status(enabled, "")
    }

    pub fn context_text(&self, enabled: bool, owner: &str) -> String {
        if !enabled {
            return String::new();
        }
        self.read_page(&self.context_path(owner), None)
            .map(|p| crate::common::clip_chars(&format!("Source: {}\n\n{}", p.url, p.text), MAX_CONTEXT))
            .unwrap_or_default()
    }

    pub fn forget(&self, enabled: bool, owner: &str) -> Result<Value> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        for path in [self.context_path(owner), self.last_path(owner)] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(error("WEB_CLEAR_FAILED", "could not clear saved web evidence")),
            }
        }
        self.status(enabled, owner)
    }

    pub fn inject(&self, enabled: bool, owner: &str) -> Result<Value> {
        self.require_enabled(enabled)?;
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let page = self.read_page(&self.last_path(owner), None)?;
        if page.text.is_empty() {
            return Err(error("WEB_NO_LAST", "last extract is empty"));
        }
        page.validate(&self.policy()?, None)?;
        crate::common::atomic::write_json_atomic_mode(&self.context_path(owner), &serde_json::to_value(&page)?, 0o600)?;
        self.status(enabled, owner)
    }

    pub fn require_enabled(&self, enabled: bool) -> Result<Policy> {
        if !enabled {
            return Err(error("WEB_DISABLED", "web access is disabled"));
        }
        let policy = self.policy()?;
        if policy.rules.is_empty() {
            return Err(error("WEB_ALLOWLIST_EMPTY", "empty URL policy refuses all web access"));
        }
        Ok(policy)
    }

    pub(super) fn gate_tool(&self, name: &str, argv: &[String], enabled: bool, audit: &Audit) -> Result<()> {
        let allow = if enabled {
            ["web_fetch".to_string(), "web_search".to_string()]
                .into_iter()
                .collect()
        } else {
            BTreeSet::new()
        };
        assert_allowed(name, argv, &allow, audit)
            .map(|_| ())
            .map_err(|e| HarnessError::new("WEB_TOOL_DENIED", e.message).with_details(e.details))
    }

    pub(super) async fn pinned_client(&self, target: &url::Url) -> Result<reqwest::Client> {
        let host = target.host_str().unwrap().trim_matches(['[', ']']);
        let port = target.port_or_known_default().unwrap();
        let addresses = match self
            .test_resolve
            .iter()
            .chain(self.test_resolve_extra.iter())
            .find(|(test_host, _)| test_host == host)
        {
            Some((_, addr)) => vec![*addr],
            _ => {
                let addresses: Vec<_> = tokio::net::lookup_host((host, port))
                    .await
                    .map_err(|_| error("WEB_DNS", "DNS lookup failed"))?
                    .take(33)
                    .collect();
                validate_addresses(&addresses)?;
                addresses
            }
        };
        // New client, no pooling/retries/proxies/redirects, only validated DNS.
        // HTTP/1's finite Hyper header-count/buffer caps apply before our 16KiB header check.
        reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(RefuseDns))
            .resolve_to_addrs(host, &addresses)
            .timeout(Duration::from_secs(self.limits.request_seconds))
            .user_agent("CGagentHarness-web/1.0")
            .build()
            .map_err(|_| error("WEB_FETCH_FAILED", "HTTP setup failed"))
    }

    pub(super) async fn get(&self, raw: &str, group: Option<&str>, cached: Option<&Page>) -> Result<Page> {
        self.get_raw(raw, group, cached).await.map(|(page, _)| page)
    }

    pub(super) async fn get_raw(
        &self,
        raw: &str,
        group: Option<&str>,
        cached: Option<&Page>,
    ) -> Result<(Page, String)> {
        tokio::time::timeout(Duration::from_secs(self.limits.request_seconds), async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| error("WEB_CANCELLED", "fetch cancelled"))?;
            let policy = self.policy()?;
            let target = policy.authorize(raw, group)?;
            let client = self.pinned_client(&target).await?;
            let current = self.policy()?;
            current.authorize(target.as_str(), group)?;
            if current.revision != policy.revision {
                return Err(error("WEB_POLICY_CHANGED", "policy changed during request"));
            }
            let mut request = client.get(target.clone());
            if let Some(page) = cached {
                page.validate(&current, group)?;
                if page.url != target.as_str() {
                    return Err(error("WEB_EVIDENCE_INVALID", "validator source mismatch"));
                }
                if let Some(etag) = &page.etag {
                    request = request.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                if let Some(modified) = &page.last_modified {
                    request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
                }
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| error("WEB_FETCH_FAILED", "fetch failed"))?;
            let status = response.status();
            if response
                .headers()
                .iter()
                .map(|(k, v)| k.as_str().len() + v.len())
                .sum::<usize>()
                > 16_384
            {
                return Err(error("WEB_HEADERS_TOO_LARGE", "response headers exceed limit"));
            }
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            let mut raw_body = String::new();
            let mut page = if status == reqwest::StatusCode::NOT_MODIFIED {
                let mut page = cached
                    .cloned()
                    .ok_or_else(|| error("WEB_FETCH_FAILED", "304 without cached evidence"))?;
                page.transfer_bytes = 0;
                page
            } else {
                if status.is_redirection() {
                    return Err(error("WEB_REDIRECT_REFUSED", "redirect refused"));
                }
                if !status.is_success() {
                    return Err(error("WEB_FETCH_FAILED", "upstream error"));
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > self.limits.response_bytes as u64)
                {
                    return Err(error("WEB_TOO_LARGE", "response exceeds limit"));
                }
                if header(reqwest::header::CONTENT_ENCODING).is_some_and(|v| v != "identity") {
                    return Err(error("WEB_ENCODING_REFUSED", "compressed responses are not enabled"));
                }
                let content_type = header(reqwest::header::CONTENT_TYPE)
                    .unwrap_or_default()
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                if !matches!(
                    content_type.as_str(),
                    "text/plain" | "text/html" | "application/xhtml+xml" | "text/markdown"
                ) {
                    return Err(error("WEB_NOT_TEXT", "unsupported content type"));
                }
                let etag = header(reqwest::header::ETAG);
                let last_modified = header(reqwest::header::LAST_MODIFIED);
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| error("WEB_FETCH_FAILED", "response read failed"))?
                {
                    if bytes.len() + chunk.len() > self.limits.response_bytes {
                        return Err(error("WEB_TOO_LARGE", "response exceeds limit"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                raw_body = String::from_utf8_lossy(&bytes).into_owned();
                let (title, text, links) = extract(&raw_body, &content_type, &target);
                Page {
                    url: target.to_string(),
                    title,
                    content_hash: crate::common::sha256_hex(&text),
                    chars: text.chars().count(),
                    text,
                    links,
                    status: status.as_u16(),
                    content_type,
                    bytes: bytes.len(),
                    transfer_bytes: bytes.len(),
                    fetched_at: 0.0,
                    extraction_version: 1,
                    policy_revision: policy.revision.clone(),
                    etag,
                    last_modified,
                }
            };
            let current = self.policy()?;
            current.authorize(&page.url, group)?;
            if current.revision != policy.revision {
                return Err(error(
                    "WEB_POLICY_CHANGED",
                    "policy changed during request; result discarded",
                ));
            }
            page.fetched_at = crate::common::now_ts();
            page.policy_revision = current.revision;
            Ok((page, raw_body))
        })
        .await
        .map_err(|_| error("WEB_TIMEOUT", "request deadline exceeded"))?
    }

    pub(super) fn store_last(&self, page: &Page, group: Option<&str>, owner: &str) -> Result<()> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        page.validate(&self.policy()?, group)?;
        crate::common::atomic::write_json_atomic_mode(&self.last_path(owner), &serde_json::to_value(page)?, 0o600)
    }

    pub async fn fetch(&self, raw: &str, enabled: bool, audit: &Audit, owner: &str) -> Result<Value> {
        self.require_enabled(enabled)?;
        self.gate_tool("web_fetch", &[raw.into()], enabled, audit)?;
        let page = self.get(raw, None, None).await?;
        self.cache_page(&page, None)?;
        self.store_last(&page, None, owner)?;
        page.validate(&self.policy()?, None)?;
        Ok(serde_json::to_value(page)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::atomic::write_json_atomic;
    use axum::{extract::State, routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Clone, Default)]
    struct Fixture {
        started: Arc<Notify>,
        release: Arc<Notify>,
        requests: Arc<AtomicUsize>,
    }

    #[tokio::test]
    async fn pinned_fetch_refuses_redirects_and_discards_revoked_or_cancelled_work() {
        let fixture = Fixture::default();
        let app = Router::new()
            .route(
                "/docs/child",
                get(|| async { ([("content-type", "text/plain")], "actual child Target after İK") }),
            )
            .route(
                "/redirect",
                get(|| async {
                    (
                        axum::http::StatusCode::FOUND,
                        [("location", "http://127.0.0.1/private")],
                        "untrusted redirect",
                    )
                }),
            )
            .route(
                "/slow",
                get(|State(f): State<Fixture>| async move {
                    f.requests.fetch_add(1, Ordering::SeqCst);
                    f.started.notify_one();
                    f.release.notified().await;
                    ([("content-type", "text/plain")], "REVOKED_MARKER")
                }),
            )
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        write_json_atomic(&dir.path().join("web_allowlist.json"), &json!({"version":1,"rules":[]})).unwrap();
        let mut web = WebTool::new(dir.path(), &cfg).unwrap();
        web.test_resolve = Some(("fixture.invalid".into(), address));
        let web = Arc::new(web);
        let root = format!("http://fixture.invalid:{}/", address.port());
        let all = format!("{root}*");
        let child = format!("{root}docs/child");
        web.allow(&all, true).unwrap();
        let audit = Arc::new(Audit::new(dir.path().join("audit.jsonl"), &cfg));
        let page = web.fetch(&child, true, &audit, "local").await.unwrap();
        assert_eq!(page["url"], child);
        assert!(page["text"].as_str().unwrap().starts_with("actual child"));
        // fixture.invalid cannot resolve via system DNS. Success proves the
        // checked connection override works with the refusing fallback resolver.
        assert_eq!(
            web.fetch(&format!("{root}redirect"), true, &audit, "local")
                .await
                .unwrap_err()
                .code,
            "WEB_REDIRECT_REFUSED"
        );
        web.inject(true, "local").unwrap();
        assert!(web.context_text(true, "local").contains("actual child"));
        assert!(web.context_text(false, "local").is_empty());
        let slow = format!("{root}slow");
        let pending = {
            let web = web.clone();
            let audit = audit.clone();
            let slow = slow.clone();
            tokio::spawn(async move { web.fetch(&slow, true, &audit, "local").await })
        };
        fixture.started.notified().await;
        web.deny(&all, true).unwrap();
        assert!(web.context_text(true, "local").is_empty());
        assert!(!web.status(true, "local").unwrap()["has_last"].as_bool().unwrap());
        fixture.release.notify_one();
        assert_eq!(pending.await.unwrap().unwrap_err().code, "WEB_HOST_DENIED");
        assert!(!std::fs::read_to_string(web.last_path("local"))
            .unwrap()
            .contains("REVOKED_MARKER"));
        web.allow(&all, true).unwrap();
        let pending = {
            let web = web.clone();
            let audit = audit.clone();
            tokio::spawn(async move { web.fetch(&slow, true, &audit, "local").await })
        };
        fixture.started.notified().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        fixture.release.notify_one();
        assert_eq!(web.permits.available_permits(), web.limits.concurrency);
        std::fs::write(web.allow_path(), "{invalid").unwrap();
        assert_eq!(
            web.fetch(&child, true, &audit, "local").await.unwrap_err().code,
            "WEB_POLICY_INVALID"
        );
        assert!(web.context_text(true, "local").is_empty());
        assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[test]
    fn mixed_dns_special_ranges_and_unproven_context_are_refused() {
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        assert!(validate_addresses(&[public]).is_ok());
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.0.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "224.1.1.1",
            "::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001:db8::1",
            "fc00::1",
            "fe80::1",
            "3fff::1",
        ] {
            let private = SocketAddr::new(ip.parse().unwrap(), 443);
            assert!(validate_addresses(&[public, private]).is_err(), "{ip}");
        }
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("{}", Path::new("config.yaml")).unwrap();
        let web = WebTool::new(dir.path(), &cfg).unwrap();
        write_json_atomic(&web.allow_path(), &json!({"version":1,"rules":[]})).unwrap();
        web.allow("https://example.com/", true).unwrap();
        std::fs::write(web.context_path("local"), "legacy shared context").unwrap();
        assert!(web.context_text(true, "local").is_empty());
        let base = canonical_url("https://example.com/").unwrap();
        let (title, text, links) = extract("<title>Docs &amp; tests</title><script>evil()</script><nav>noise</nav><h1>Heading</h1><p>A &amp; B</p><pre>x = 1\n  y = 2</pre><a href='/nested'>Next</a>", "text/html", &base);
        assert_eq!(title, "Docs & tests");
        assert!(text.contains("Heading") && text.contains("A & B") && text.contains("x = 1\n  y = 2"));
        assert!(!text.contains("evil()") && !text.contains("noise"));
        assert_eq!(links, vec!["https://example.com/nested"]);
        assert!(
            Limits::load(&AppConfig::from_str("web: {concurrency: 0}", Path::new("config.yaml")).unwrap()).is_err()
        );
    }
}
