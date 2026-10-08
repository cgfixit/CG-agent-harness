//! Permission-checked content reads. Every network/evidence path reloads URL policy.
use std::cell::Cell;
use std::collections::BTreeSet;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::{Tracer, TreeSink};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::web_policy::{canonical_url, error, is_public_ip, Policy, Rule, MAX_RULES};
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::tool_broker::assert_allowed;

pub const MAX_ALLOW: usize = MAX_RULES;
pub const MAX_BYTES: usize = 262_144;
const MAX_CONTEXT: usize = 4000;

/// Smallest `web.evidence_tokens`, and (calibrated) the least room web chat keeps
/// for each tool result it may still receive.
pub const MIN_EVIDENCE_TOKENS: u64 = 256;
pub const MAX_EVIDENCE_TOKENS: u64 = 6000;
pub const MAX_MODEL_TOKENS: u64 = 2048;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Limits {
    pub response_bytes: usize,
    pub request_seconds: u64,
    pub html_parser_handles: usize,
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
            html_parser_handles: bound("html_parser_handles", 512, 64, 4096)? as usize,
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
            evidence_tokens: bound("evidence_tokens", 3000, MIN_EVIDENCE_TOKENS, MAX_EVIDENCE_TOKENS)?,
            model_tokens: bound("model_tokens", 1024, 256, MAX_MODEL_TOKENS)?,
            // Held at the model's window cap where it is read (`AppState::model_web_limits`).
            total_tokens: bound("total_tokens", 16000, 2048, super::compaction::MAX_WEB_TOTAL_TOKENS)?,
            research_seconds: bound("research_seconds", 300, 10, 1800)?,
            chat_tool_calls: bound("chat_tool_calls", 10, 1, crate::llm::openai_stream::MAX_TOOL_CALLS)? as usize,
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
pub(super) struct RefuseDns;
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

/// Properties a node inherits from its ancestors during [`extract`].
#[derive(Clone, Copy)]
struct Inherited<Id> {
    in_title: bool,
    in_head: bool,
    in_pre: bool,
    /// Nearest enclosing block element.
    block: Option<Id>,
}

impl<Id> Default for Inherited<Id> {
    fn default() -> Self {
        Self {
            in_title: false,
            in_head: false,
            in_pre: false,
            block: None,
        }
    }
}

/// Count the actual HTML5 parser state, including active formatting elements.
/// Source-tag counting is unsafe: repair, ignored end tags and self-closing
/// HTML tags can leave more elements open than a lexical depth counter sees.
#[derive(Default)]
struct ParserHandles(Cell<usize>);
impl Tracer for ParserHandles {
    type Handle = <scraper::HtmlTreeSink as TreeSink>::Handle;
    fn trace_handle(&self, _: &Self::Handle) {
        self.0.set(self.0.get() + 1);
    }
}

pub(super) fn parse_html(body: &str, max_handles: usize, checkpoint: &dyn Fn() -> Result<()>) -> Result<scraper::Html> {
    let mut parser = html5ever::parse_document(
        scraper::HtmlTreeSink::new(scraper::Html::new_document()),
        Default::default(),
    );
    let mut remaining = body;
    while !remaining.is_empty() {
        checkpoint()?;
        // A scheduling quantum, not a content truncation. Keep UTF-8 intact.
        // Inspect repaired parser state between small batches, not source tags.
        let mut end = remaining.len().min(1024);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        parser.process(remaining[..end].into());
        let count = ParserHandles::default();
        parser.tokenizer.sink.trace_handles(&count);
        if count.0.get() > max_handles {
            return Err(error("WEB_HTML_COMPLEXITY", "HTML parser state exceeds limit"));
        }
        remaining = &remaining[end..];
    }
    checkpoint()?;
    // EOF repair is bounded by the same parser-state ceiling.
    let html = parser.finish();
    checkpoint()?;
    Ok(html)
}

/// HTML5 parsing decodes entities and repairs malformed markup. Never execute it.
/// The caller supplies its existing deadline/cancellation check; failure never
/// returns a partial extract. CPU-bound callers must run this off async workers.
pub fn extract(
    body: &str,
    content_type: &str,
    base: &url::Url,
    max_handles: usize,
    checkpoint: impl Fn() -> Result<()>,
) -> Result<(String, String, Vec<String>)> {
    checkpoint()?;
    if !content_type.contains("html") {
        return Ok((String::new(), body.to_string(), Vec::new()));
    }
    let html = parse_html(body, max_handles, &checkpoint)?;
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
    // Pre-order walk with each node's inherited properties carried down an
    // explicit stack. Recomputing them from an ancestor walk per node costs
    // O(nodes x depth), and html5ever does not bound nesting depth, so a
    // single hostile page within `response_bytes` could hold a worker for
    // minutes. A hidden element's subtree is never pushed.
    let mut pending = vec![(html.tree.root(), Inherited::default())];
    while let Some((node, inherited)) = pending.pop() {
        checkpoint()?;
        let Inherited {
            in_title,
            in_head,
            in_pre,
            block: current_block,
        } = inherited;
        let mut children = inherited;
        let mut hidden = false;
        if let Some(element) = node.value().as_element() {
            let name = element.name();
            hidden = matches!(
                name,
                "script" | "style" | "noscript" | "template" | "nav" | "header" | "footer" | "form" | "svg"
            ) || element.attr("hidden").is_some()
                || element.attr("aria-hidden") == Some("true");
            children.in_title |= name == "title";
            children.in_head |= name == "head";
            children.in_pre |= name == "pre";
            if block(name) {
                children.block = Some(node.id());
            }
        }
        if !hidden {
            pending.extend(node.children().rev().map(|child| (child, children)));
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
            if in_title {
                title.push_str(value);
            } else if !in_head {
                if current_block != previous_block {
                    text.push('\n');
                    previous_block = current_block;
                }
                if in_pre {
                    text.push_str(value);
                } else {
                    text.push_str(&value.split_whitespace().collect::<Vec<_>>().join(" "));
                    text.push(' ');
                }
            }
        }
    }
    checkpoint()?;
    Ok((
        title.trim().to_string(),
        text.trim().to_string(),
        links.into_iter().collect(),
    ))
}

// Dropping this future (timeout, /web cancel or caller abort) also signals
// queued/running blocking work. The worker, never the caller, owns the permit.
async fn extract_in_worker(
    body: String,
    kind: String,
    base: url::Url,
    max_handles: usize,
    deadline: tokio::time::Instant,
    permit: OwnedSemaphorePermit,
) -> Result<(String, (String, String, Vec<String>))> {
    run_web_cpu(deadline, permit, move |checkpoint| {
        let extracted = extract(&body, &kind, &base, max_handles, checkpoint)?;
        Ok((body, extracted))
    })
    .await
}

/// All HTML consumers share cancellation, the request deadline and fetch capacity.
pub(super) async fn run_web_cpu<T: Send + 'static>(
    deadline: tokio::time::Instant,
    permit: OwnedSemaphorePermit,
    work: impl FnOnce(&dyn Fn() -> Result<()>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let cancelled = CancellationToken::new();
    let _cancel_on_drop = cancelled.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(&|| {
            if cancelled.is_cancelled() {
                return Err(error("WEB_CANCELLED", "extraction cancelled"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(error("WEB_TIMEOUT", "request deadline exceeded"));
            }
            Ok(())
        })
    })
    .await
    .map_err(|_| error("WEB_FETCH_FAILED", "extraction failed"))?
}

#[derive(Debug, Clone)]
pub struct WebTool {
    pub(super) tools_dir: PathBuf,
    /// Programmatic fixture hook; no configuration, CLI or HTTP path sets it.
    pub test_resolve: Option<(String, SocketAddr)>,
    /// Additional exact hosts for bounded multi-site fixtures, never runtime configuration.
    pub test_resolve_extra: Vec<(String, SocketAddr)>,
    pub(super) search_key_from_file: bool,
    /// Literal `security.allow_plaintext_key_file`. When false, saved search keys come from the OS store.
    pub(super) plaintext_key_file: bool,
    pub limits: Limits,
    pub(super) permits: Arc<Semaphore>,
    pub(super) mutation: Arc<Mutex<()>>,
    pub(super) search_gate: Arc<Semaphore>,
    pub research: Arc<super::web_research::ResearchState>,
    pub chat_turn: Arc<super::web_research::ResearchState>,
}

impl WebTool {
    pub fn new(tools_dir: &Path, cfg: &AppConfig) -> Result<Self> {
        let limits = Limits::load(cfg)?;
        Ok(Self {
            tools_dir: tools_dir.into(),
            test_resolve: None,
            test_resolve_extra: Vec::new(),
            permits: Arc::new(Semaphore::new(limits.concurrency)),
            limits,
            search_key_from_file: false,
            plaintext_key_file: cfg.flag_is_true(crate::common::credential_store::PLAINTEXT_KEY_FILE),
            mutation: Arc::new(Mutex::new(())),
            search_gate: Arc::new(Semaphore::new(1)),
            research: Arc::default(),
            chat_turn: Arc::default(),
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
            "policy_scope":"shared_home", "context_scope":"account", "limits":self.limits,
            "search_provider": if self.search_key()?.is_empty() { "public Google (may require JavaScript/CAPTCHA)" } else { "Google via SerpAPI" },
            "injected": enabled && stored, "context_stored": stored,
            "has_last": self.read_page(&self.last_path(owner), None).is_ok(), "max_allow": MAX_ALLOW}),
        )
    }

    pub fn allow(&self, raw: &str, enabled: bool) -> Result<Value> {
        self.allow_rule(raw, "default", &[], enabled)
    }
    pub fn allow_rule(&self, raw: &str, group: &str, seeds: &[String], enabled: bool) -> Result<Value> {
        self.allow_rules(&[raw.into()], group, seeds, enabled)
    }

    /// Validate the entire grant before writing once; a bad tail never grants a prefix.
    pub fn allow_rules(&self, patterns: &[String], group: &str, seeds: &[String], enabled: bool) -> Result<Value> {
        if patterns.is_empty() || patterns.len() > MAX_ALLOW || seeds.len() > 16 {
            return Err(error("WEB_POLICY_INVALID", "invalid rule or seed count"));
        }
        let mut rules = patterns
            .iter()
            .map(|p| Rule::new(p, group, &[]))
            .collect::<Result<Vec<_>>>()?;
        for seed in seeds {
            let target = super::web_policy::canonical_url(seed)?;
            let mut matched = false;
            for rule in &mut rules {
                if rule.permits(&target) {
                    matched = true;
                    if !rule.seeds.contains(seed) {
                        rule.seeds.push(seed.clone());
                    }
                }
            }
            if !matched {
                return Err(error("WEB_POLICY_INVALID", "seed is outside every requested rule"));
            }
        }
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
        for rule in rules {
            if let Some(existing) = policy.rules.iter_mut().find(|r| r.id == rule.id) {
                *existing = rule;
            } else {
                if policy.rules.len() >= MAX_ALLOW {
                    return Err(error("WEB_ALLOWLIST_FULL", "rule cap reached"));
                }
                policy.rules.push(rule);
            }
        }
        self.save_policy(&policy)?;
        self.status(enabled, "")
    }

    /// A local policy diagnostic, not a DNS lookup, fetch or permission grant.
    pub fn check_urls(&self, urls: &[String], group: Option<&str>, enabled: bool) -> Result<Value> {
        let policy = self.policy()?;
        let checks: Vec<_> = urls.iter().map(|raw| {
            match policy.authorize(raw,group) {
                Ok(url) => json!({"url":raw,"canonical_url":url.as_str(),"permitted":true,"enabled":enabled,
                    "rule_ids":policy.rules.iter().filter(|r| group.is_none_or(|g| r.group==g) && r.permits(&url)).map(|r| &r.id).collect::<Vec<_>>() }),
                Err(e) => json!({"url":raw,"permitted":false,"code":e.code,"enabled":enabled}),
            }
        }).collect();
        Ok(
            json!({"checks":checks,"policy_scope":"shared_home","policy_revision":policy.revision,
            "notice":"Policy check only. Fetch still checks current permission, public DNS, redirects and resource limits. www and apex hosts are distinct; no host is inferred."}),
        )
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.limits.request_seconds);
        tokio::time::timeout_at(deadline, async {
            // Keep the permit until the worker actually exits, including its
            // bounded cleanup after cancellation or the original request deadline.
            let permit = self
                .permits
                .clone()
                .acquire_owned()
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
                let (body, kind, base) = (
                    String::from_utf8_lossy(&bytes).into_owned(),
                    content_type.clone(),
                    target.clone(),
                );
                let (body, (title, text, links)) =
                    extract_in_worker(body, kind, base, self.limits.html_parser_handles, deadline, permit).await?;
                raw_body = body;
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

    fn extract(body: &str, kind: &str, base: &url::Url) -> (String, String, Vec<String>) {
        super::extract(body, kind, base, 512, || Ok(())).unwrap()
    }

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
    fn chat_tool_call_budget_defaults_to_ten_and_rejects_out_of_range_values() {
        for yaml in ["{}", include_str!("../../assets/config.default.yaml")] {
            let cfg = AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap();
            assert_eq!(Limits::load(&cfg).unwrap().chat_tool_calls, 10);
        }
        for value in ["1", "3", "10", "0", "11", "-1", "1.5", "\"10\"", "null"] {
            let cfg =
                AppConfig::from_str(&format!("web: {{chat_tool_calls: {value}}}"), Path::new("config.yaml")).unwrap();
            let result = Limits::load(&cfg);
            if let Ok(expected @ 1..=10) = value.parse::<usize>() {
                assert_eq!(result.unwrap().chat_tool_calls, expected);
            } else {
                assert!(result.is_err(), "invalid limit accepted: {value}");
            }
        }
    }

    /// The pre-fix extractor, kept verbatim as the oracle for the stack walk:
    /// it recomputes inherited state from a full ancestor walk per node.
    fn extract_by_ancestor_walk(body: &str, base: &url::Url) -> (String, String, Vec<String>) {
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
            let mut hidden = false;
            let mut in_title = false;
            let mut in_head = false;
            let mut in_pre = false;
            let mut current_block = None;
            for ancestor in node.ancestors() {
                if let Some(element) = ancestor.value().as_element() {
                    let name = element.name();
                    if matches!(
                        name,
                        "script" | "style" | "noscript" | "template" | "nav" | "header" | "footer" | "form" | "svg"
                    ) || element.attr("hidden").is_some()
                        || element.attr("aria-hidden") == Some("true")
                    {
                        hidden = true;
                        break;
                    }
                    in_title |= name == "title";
                    in_head |= name == "head";
                    in_pre |= name == "pre";
                    if current_block.is_none() && block(name) {
                        current_block = Some(ancestor.id());
                    }
                }
            }
            if hidden {
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
                if in_title {
                    title.push_str(value);
                } else if !in_head {
                    if current_block != previous_block {
                        text.push('\n');
                        previous_block = current_block;
                    }
                    if in_pre {
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

    #[test]
    fn stack_walk_extraction_matches_the_ancestor_walk() {
        let base = canonical_url("https://example.com/dir/").unwrap();
        let fixtures = [
            "",
            "plain text with no markup",
            "<title>T</title><head><meta name=x><style>s{}</style></head><body>b</body>",
            "<p>one<p>two<div>three<span>four</span><p>five</div>six",
            "<div hidden><p>gone<pre>gone</pre></p></div><p>kept</p>",
            "<div aria-hidden='true'>x</div><div aria-hidden='false'>y</div>",
            "<pre> a\n  b <b>c\n d</b></pre><p>after   pre</p>",
            "<table><tr><td>1</td><td>2<p>3</p></td></tr><tr><td>4</td></tr></table>",
            "<svg><title>svg title</title><text>t</text></svg><template><p>t</p></template>",
            "<nav><a href='/n'>n</a></nav><header>h</header><footer>f</footer><form><p>x</p></form>",
            "<body><title>late title</title><h3>h <a href='x?y=1'>l</a></h3><br>br<li>li</body>",
            "<blockquote>q<section>s<article>a<main>m</main></article></section></blockquote>",
            "<b><i><u>misnested</b></i></u> <p><b>p<div>split</b>tail</div>",
            "<pre><title>in pre</title>x</pre><h1><pre>  h </pre></h1>",
        ];
        for fixture in fixtures {
            assert_eq!(
                extract(fixture, "text/html", &base),
                extract_by_ancestor_walk(fixture, &base),
                "{fixture}"
            );
        }
        // Seeded random markup reaches nestings the fixtures do not name.
        let tags = [
            "p", "div", "pre", "title", "head", "li", "td", "tr", "h2", "a", "b", "span", "script", "nav", "br",
        ];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % bound
        };
        for _ in 0..300 {
            let mut doc = String::new();
            for _ in 0..next(60) {
                let tag = tags[next(tags.len())];
                match next(5) {
                    0 => doc.push_str(&format!("</{tag}>")),
                    1 => doc.push_str(&format!(" w{}  x\n", next(9))),
                    2 => doc.push_str(&format!("<{tag} hidden>")),
                    3 => doc.push_str(&format!("<{tag} href='/l{}'>", next(4))),
                    _ => doc.push_str(&format!("<{tag}>")),
                }
            }
            assert_eq!(
                extract(&doc, "text/html", &base),
                extract_by_ancestor_walk(&doc, &base),
                "{doc}"
            );
        }
    }

    #[test]
    fn hostile_parser_shapes_are_refused_at_both_response_byte_limits() {
        let base = canonical_url("https://example.com/").unwrap();
        for cap in [262_144, 1_048_576] {
            for unit in [
                "<div>",
                "<span>",
                "<b>",
                "<i>x",
                "<div/>",
                "<div></ignored>",
                "<template>",
            ] {
                let body = unit.repeat(cap / unit.len());
                let started = std::time::Instant::now();
                let err = super::extract(&body, "text/html", &base, 512, || Ok(())).unwrap_err();
                assert_eq!(err.code, "WEB_HTML_COMPLEXITY", "{unit}");
                assert!(started.elapsed() < Duration::from_secs(2), "{unit}");
            }
        }
    }

    #[test]
    fn batched_parser_preserves_unicode_raw_text_and_wide_documents() {
        let base = canonical_url("https://example.com/").unwrap();
        let doc =
            format!(
            "<title>é 🦀 &amp; title</title><style>{}</style><script>{}</script><p data-x='{}'>{}</p><pre>{}</pre>{}",
            "<div>".repeat(1200), "<div>".repeat(1200), "é🦀".repeat(700),
            "é 🦀 &amp; &#x1f980; ".repeat(700), " x\n  y 🦀".repeat(300),
            "<p><a href='/next'>wide sibling</a></p>".repeat(6000),
        );
        assert_eq!(extract(&doc, "text/html", &base), extract_by_ancestor_walk(&doc, &base));
        let links = (0..200).map(|i| format!("<a href='/{i}'>link</a>")).collect::<String>();
        assert_eq!(extract(&links, "text/html", &base).2.len(), 128);
    }

    #[test]
    fn extraction_checkpoints_stop_parsing_and_tree_walk_without_partial_results() {
        let base = canonical_url("https://example.com/").unwrap();
        for (body, stop) in [("<p>normal</p>".repeat(1000), 4), ("<p>normal</p>".into(), 7)] {
            for code in ["WEB_CANCELLED", "WEB_TIMEOUT"] {
                let calls = Cell::new(0);
                let err = super::extract(&body, "text/html", &base, 512, || {
                    calls.set(calls.get() + 1);
                    if calls.get() == stop {
                        Err(error(code, "stopped"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
                assert_eq!(err.code, code);
                assert_eq!(calls.get(), stop);
            }
        }
    }

    #[test]
    fn parser_limit_is_bounded_and_fail_closed() {
        for value in ["64", "512", "4096", "63", "4097", "-1", "1.5", "\"512\"", "null"] {
            let cfg = AppConfig::from_str(
                &format!("web: {{html_parser_handles: {value}}}"),
                Path::new("config.yaml"),
            )
            .unwrap();
            let parsed = Limits::load(&cfg);
            if let Ok(n @ 64..=4096) = value.parse::<usize>() {
                assert_eq!(parsed.unwrap().html_parser_handles, n);
            } else {
                assert!(parsed.is_err(), "{value}");
            }
        }
        for yaml in ["{}", include_str!("../../assets/config.default.yaml")] {
            assert_eq!(
                Limits::load(&AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap())
                    .unwrap()
                    .html_parser_handles,
                512
            );
        }
    }

    #[test]
    fn dropped_or_expired_queued_extraction_releases_the_worker_permit() {
        use std::future::Future;
        use std::task::Poll;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            for cancel in [true, false] {
                let (release, hold) = std::sync::mpsc::channel();
                let (started, ready) = tokio::sync::oneshot::channel();
                let blocker = tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    hold.recv().unwrap();
                });
                ready.await.unwrap();
                let permits = Arc::new(Semaphore::new(1));
                let mut work = Box::pin(extract_in_worker(
                    "<div>".repeat(50_000),
                    "text/html".into(),
                    canonical_url("https://example.com/").unwrap(),
                    512,
                    tokio::time::Instant::now(),
                    permits.clone().acquire_owned().await.unwrap(),
                ));
                std::future::poll_fn(|cx| {
                    assert!(work.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                if cancel {
                    drop(work);
                    assert_eq!(permits.available_permits(), 0);
                    release.send(()).unwrap();
                } else {
                    release.send(()).unwrap();
                    let err = work.await.unwrap_err();
                    assert_eq!(err.code, "WEB_TIMEOUT");
                }
                blocker.await.unwrap();
                let permit = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
                    .await
                    .unwrap()
                    .unwrap();
                drop(permit);
                assert_eq!(permits.available_permits(), 1);
            }
        });
    }

    #[test]
    fn extraction_preserves_nested_blocks_hidden_ancestors_and_preformatted_text() {
        let base = canonical_url("https://example.com/").unwrap();
        let (title, text, links) = extract(
            "<title>A &amp; B</title><main><h2>Heading</h2><p>one <b>two</b></p><div hidden><p>hidden</p><a href='/secret'>bad</a></div><div aria-hidden='true'><span>invisible</span></div><pre> x\n  y</pre><p>three <a href='/ok'>link</a></p></main>",
            "text/html", &base);
        assert_eq!(title, "A & B");
        assert_eq!(text, "# Heading \none two \n\n\n x\n  y\nthree link");
        assert_eq!(links, vec!["https://example.com/ok"]);
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
