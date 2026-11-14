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

use super::web_policy::{
    canonical_url, error, is_public_ip, Mode, Policy, Rule, DEFAULT_MAX_RULES, MAX_BATCH_URLS, MAX_RULES_CEILING,
};
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::tool_broker::assert_allowed;

pub const MAX_BYTES: usize = 262_144;
/// Largest `web.cache_pages`: the most documents the derived cache can hold
/// under any configuration, and so the bound on listing it.
pub(super) const MAX_CACHE_PAGES: usize = 256;
const MAX_CONTEXT: usize = 4000;

/// Smallest `web.evidence_tokens`, and (calibrated) the least room web chat keeps
/// for each tool result it may still receive.
pub const MIN_EVIDENCE_TOKENS: u64 = 256;
pub const MAX_EVIDENCE_TOKENS: u64 = 6000;
pub const MAX_MODEL_TOKENS: u64 = 2048;
/// Ceiling for `web.research_seconds` (discovery and planning) and
/// `web.synthesis_seconds` (the answer call): one hour each, the longest
/// deadline the harness gives one local-model call (`MAX_TIMEOUT_SEC`). A slow
/// CPU model then gets its whole measured answer deadline instead of half.
pub const MAX_RESEARCH_SECONDS: u64 = super::model_limits::MAX_TIMEOUT_SEC;
pub const MAX_SYNTHESIS_SECONDS: u64 = super::model_limits::MAX_TIMEOUT_SEC;

/// How long the CLI may wait on synchronous `/api/web/research`. Auto-tune may
/// raise either deadline to its ceiling, so that path waits for both. Otherwise
/// the wait is the configured pair, each clamped to its ceiling. Ten extra
/// seconds let the server's typed timeout finish first. The console does not
/// use this: it starts a job and polls, so a tab close does not end the run.
pub(crate) fn research_request_timeout_sec(research_seconds: u64, synthesis_seconds: u64, auto_tune: bool) -> u64 {
    let spanned = if auto_tune {
        MAX_RESEARCH_SECONDS + MAX_SYNTHESIS_SECONDS
    } else {
        research_seconds.clamp(10, MAX_RESEARCH_SECONDS) + synthesis_seconds.clamp(10, MAX_SYNTHESIS_SECONDS)
    };
    spanned + 10
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Limits {
    pub response_bytes: usize,
    pub request_seconds: u64,
    pub html_parser_handles: usize,
    pub concurrency: usize,
    /// Total allowlist rules (`web.max_rules`, restart-only); one grant still names at most 32.
    pub max_rules: usize,
    pub pages: usize,
    pub run_bytes: usize,
    pub run_seconds: u64,
    pub per_site_pages: usize,
    pub pace_ms: u64,
    /// Restart-only pause after a public Google challenge or block (`web.search_cooldown_public_sec`).
    pub search_cooldown_public_sec: u64,
    /// Restart-only pause after a SerpAPI quota refusal; 0 waits for the next clock hour.
    pub search_cooldown_quota_sec: u64,
    pub cache_pages: usize,
    pub cache_bytes: usize,
    pub subqueries: usize,
    pub rounds: usize,
    pub evidence_tokens: u64,
    pub model_tokens: u64,
    pub total_tokens: u64,
    pub research_seconds: u64,
    /// The synthesis call's own deadline, started after discovery and planning.
    pub synthesis_seconds: u64,
    pub stale_seconds: u64,
    pub chat_tool_calls: usize,
    /// Earlier web reads listed in a web turn's prompt (`web_ledger::carry_forward`);
    /// 0, the shipped value, lists none. Reloadable.
    pub carry_forward_sources: usize,
    /// The mode a `/goal` takes when the request names none (`web.goal_web_default`,
    /// restart-only). Ships `all`: search is a read-only listing that grants no URL
    /// permission, so unattended loop turns may search; fetches stay policy-gated.
    pub goal_web_default: super::sessions::GoalWebMode,
}

impl Limits {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let bound =
            |name: &str, default: u64, min: u64, max: u64| cfg.u64_in(&format!("web.{name}"), default, min..=max);
        Ok(Self {
            response_bytes: bound("response_bytes", MAX_BYTES as u64, 1024, 1_048_576)? as usize,
            request_seconds: bound("request_seconds", 8, 1, 30)?,
            html_parser_handles: bound("html_parser_handles", 512, 64, 4096)? as usize,
            concurrency: bound("concurrency", 2, 1, 4)? as usize,
            max_rules: bound("max_rules", DEFAULT_MAX_RULES as u64, 1, MAX_RULES_CEILING as u64)? as usize,
            pages: bound("pages", 20, 1, 40)? as usize,
            run_bytes: bound("run_bytes", 2_097_152, 1024, 8_388_608)? as usize,
            run_seconds: bound("run_seconds", 60, 1, 180)?,
            per_site_pages: bound("per_site_pages", 10, 1, 20)? as usize,
            pace_ms: bound("pace_ms", 250, 100, 5000)?,
            search_cooldown_public_sec: bound("search_cooldown_public_sec", 600, 60, 3600)?,
            search_cooldown_quota_sec: bound("search_cooldown_quota_sec", 0, 0, 3600)?,
            cache_pages: bound("cache_pages", 128, 1, MAX_CACHE_PAGES as u64)? as usize,
            cache_bytes: bound("cache_bytes", 16_777_216, 1_048_576, 33_554_432)? as usize,
            subqueries: bound("subqueries", 3, 1, 5)? as usize,
            rounds: bound("rounds", 2, 1, 2)? as usize,
            evidence_tokens: bound("evidence_tokens", 3000, MIN_EVIDENCE_TOKENS, MAX_EVIDENCE_TOKENS)?,
            model_tokens: bound("model_tokens", 1024, 256, MAX_MODEL_TOKENS)?,
            // Held at the model's window cap where it is read (`AppState::model_web_limits`).
            total_tokens: bound("total_tokens", 16000, 2048, super::compaction::MAX_WEB_TOTAL_TOKENS)?,
            research_seconds: bound("research_seconds", 300, 10, MAX_RESEARCH_SECONDS)?,
            synthesis_seconds: bound("synthesis_seconds", 300, 10, MAX_SYNTHESIS_SECONDS)?,
            chat_tool_calls: bound("chat_tool_calls", 30, 1, crate::llm::openai_stream::MAX_TOOL_CALLS)? as usize,
            stale_seconds: bound("stale_seconds", 604800, 60, 31_536_000)?,
            carry_forward_sources: bound("carry_forward_sources", 0, 0, 50)? as usize,
            goal_web_default: match cfg.get("web.goal_web_default") {
                None => super::sessions::GoalWebMode::All,
                Some(serde_yaml_ng::Value::String(s)) => s
                    .parse()
                    .map_err(|_| HarnessError::config("web.goal_web_default must be off, fetch or all"))?,
                Some(_) => return Err(HarnessError::config("web.goal_web_default must be off, fetch or all")),
            },
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
    /// `firecrawl` when the opt-in Firecrawl fallback read the page; absent for the
    /// pinned fetch, so saved pages from earlier builds still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
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

/// Callers validate the complete address set against their own egress policy first.
pub(super) fn pinned_client_builder(host: &str, addresses: &[SocketAddr], timeout: Duration) -> reqwest::ClientBuilder {
    // HTTP/1's finite Hyper header-count/buffer caps precede application header checks.
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(RefuseDns))
        .resolve_to_addrs(host, addresses)
        .timeout(timeout)
}

pub fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
    if addresses.is_empty() {
        return Err(error("WEB_DNS", "DNS returned no addresses"));
    }
    if addresses.len() > crate::common::dns::MAX_PINNED_ADDRESSES || addresses.iter().any(|a| !is_public_ip(a.ip())) {
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

/// Local text-only PDF extraction (`common::pdf_text`) under the fetch permit
/// and request deadline. Returns the extracted text as both body and text.
async fn pdf_in_worker(
    data: Vec<u8>,
    target: &url::Url,
    deadline: tokio::time::Instant,
    permit: OwnedSemaphorePermit,
) -> Result<(String, (String, String, Vec<String>))> {
    let title = target
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .map(|name| crate::common::clip_chars(name, 120))
        .unwrap_or_else(|| "PDF document".to_string());
    let hard = deadline.into_std();
    run_web_cpu(deadline, permit, move |_| {
        let text = crate::common::pdf_text::extract(&data, hard).map_err(|e| {
            error("WEB_NOT_TEXT", "PDF text could not be extracted locally")
                .detail("pdf", true)
                .detail("pdf_code", e.code)
        })?;
        Ok((text.to_string(), (title, text.to_string(), Vec::new())))
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
    /// Test hook: pin a fixture host to an address, skipping the lookup and
    /// `validate_addresses`. Compiled only with `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub test_resolve: Option<(String, SocketAddr)>,
    /// Test hook: more exact fixture hosts. Compiled only with `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub test_resolve_extra: Vec<(String, SocketAddr)>,
    pub(super) search_key_from_file: bool,
    /// Whether this process loaded `TAVILY_API_KEY` from the saved store itself.
    pub(super) tavily_key_from_file: bool,
    /// Whether this process loaded `FIRECRAWL_API_KEY` from the saved store itself.
    pub(super) firecrawl_key_from_file: bool,
    /// `web.firecrawl.*`, restart-only: the opt-in fallback reader for exact reads.
    pub(super) firecrawl: super::web_firecrawl::FirecrawlConfig,
    /// Firecrawl reads this clock hour, shared by every snapshot; lost on restart.
    pub(super) firecrawl_spent: Arc<super::web_firecrawl::HourlyCap>,
    /// Literal `security.allow_plaintext_key_file`. When false, saved search keys come from the OS store.
    pub(super) plaintext_key_file: bool,
    pub limits: Limits,
    pub(super) permits: Arc<Semaphore>,
    pub(super) mutation: Arc<Mutex<()>>,
    pub(super) search_gate: Arc<Semaphore>,
    /// Opt-in fallback hops after the provider; restart-only.
    pub(super) search_fallback: super::web_provider::Fallback,
    /// SerpAPI fallbacks this clock hour, shared by every snapshot.
    pub(super) search_fallback_burst: Arc<super::web_provider::FallbackBurst>,
    /// When the last public Google search request left, shared by every snapshot.
    pub(super) public_search_sent: Arc<std::sync::Mutex<Option<tokio::time::Instant>>>,
    /// `web.search.provider`; restart-only.
    pub(super) search_provider: super::web_provider::SearchProvider,
    /// Per-provider pause after a refusal, shared by every snapshot; lost on restart.
    pub(super) search_cooldown: Arc<super::web_google::SearchCooldown>,
    /// `web.search.result_reads`; restart-only.
    pub(super) result_reads: super::web_provider::ResultReads,
    /// Exact URLs this snapshot's own search listings returned. `None` on the
    /// shared tool and on every snapshot unless `result_reads` is on and one
    /// chat turn asked for it (`for_search_grants`); `policy` then lays them
    /// over the saved policy, in memory, for that turn's reads alone.
    pub(super) search_grants: Option<Arc<std::sync::Mutex<Arc<std::collections::BTreeSet<String>>>>>,
    /// Test hook replacing the public Google search URL with a fixture.
    /// Compiled only with `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub test_public_search: Option<url::Url>,
    /// Test hook replacing the fixed Tavily endpoint with a fixture.
    /// Compiled only with `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub test_tavily_search: Option<url::Url>,
    /// Test hook replacing the fixed Firecrawl endpoint with a fixture.
    /// Compiled only with `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub test_firecrawl: Option<url::Url>,
    pub research: Arc<super::web_research::ResearchState>,
    pub chat_turn: Arc<super::web_research::ResearchState>,
    /// Detached `/web research` jobs, shared by every snapshot; lost on restart.
    pub research_jobs: Arc<super::web_research::ResearchJobs>,
}

impl WebTool {
    pub fn new(tools_dir: &Path, cfg: &AppConfig) -> Result<Self> {
        let limits = Limits::load(cfg)?;
        Ok(Self {
            tools_dir: tools_dir.into(),
            #[cfg(any(test, feature = "test-support"))]
            test_resolve: None,
            #[cfg(any(test, feature = "test-support"))]
            test_resolve_extra: Vec::new(),
            permits: Arc::new(Semaphore::new(limits.concurrency)),
            limits,
            search_key_from_file: false,
            tavily_key_from_file: false,
            firecrawl_key_from_file: false,
            firecrawl: super::web_firecrawl::FirecrawlConfig::load(cfg)?,
            firecrawl_spent: Arc::default(),
            plaintext_key_file: cfg.flag_is_true(crate::common::credential_store::PLAINTEXT_KEY_FILE),
            mutation: Arc::new(Mutex::new(())),
            search_gate: Arc::new(Semaphore::new(1)),
            search_fallback: super::web_provider::Fallback::load(cfg)?,
            search_fallback_burst: Arc::default(),
            public_search_sent: Arc::default(),
            search_provider: super::web_provider::SearchProvider::load(cfg)?,
            search_cooldown: Arc::default(),
            result_reads: super::web_provider::ResultReads::load(cfg)?,
            search_grants: None,
            #[cfg(any(test, feature = "test-support"))]
            test_public_search: None,
            #[cfg(any(test, feature = "test-support"))]
            test_tavily_search: None,
            #[cfg(any(test, feature = "test-support"))]
            test_firecrawl: None,
            research: Arc::default(),
            chat_turn: Arc::default(),
            research_jobs: Arc::default(),
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
        let mut policy = Policy::load(&self.allow_path(), self.limits.max_rules)?;
        if let Some(grants) = &self.search_grants {
            policy.granted = grants.lock().unwrap_or_else(|p| p.into_inner()).clone();
        }
        Ok(policy)
    }

    /// A snapshot for one chat turn whose reads may also use the exact URLs
    /// its own searches list, starting from `seed` (a loop run's earlier
    /// grants). `None` when `web.search.result_reads.enabled` is literal false.
    pub(super) fn for_search_grants(&self, seed: &[String]) -> Option<Self> {
        self.result_reads.enabled.then(|| {
            let held = seed.iter().take(self.result_reads.max_urls).cloned().collect();
            Self {
                search_grants: Some(Arc::new(std::sync::Mutex::new(Arc::new(held)))),
                ..self.clone()
            }
        })
    }

    /// Whether this snapshot grants reads from its search listings.
    pub(super) fn grants_search_results(&self) -> bool {
        self.search_grants.is_some()
    }

    /// Grant the URLs of one delivered listing, in rank order, up to
    /// `max_urls` held. Returns how many were added. A URL `canonical_url`
    /// refuses (credentials, a private or ambiguous spelling) is skipped.
    pub(super) fn grant_search_results(&self, listing: &Value) -> usize {
        let Some(grants) = &self.search_grants else {
            return 0;
        };
        let mut held = grants.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = (**held).clone();
        let before = next.len();
        for row in listing["results"].as_array().into_iter().flatten() {
            if next.len() >= self.result_reads.max_urls {
                break;
            }
            if let Some(url) = row["url"]
                .as_str()
                .and_then(|raw| super::web_policy::canonical_url(raw).ok())
            {
                next.insert(url.to_string());
            }
        }
        let added = next.len() - before;
        *held = Arc::new(next);
        added
    }

    /// The URLs this snapshot holds, for a loop run to carry to its next turn.
    pub(super) fn search_grants(&self) -> Vec<String> {
        self.search_grants
            .as_ref()
            .map(|grants| {
                grants
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `raw` is readable only because a search listed it: the saved
    /// policy alone would refuse it. Used to mark and audit such a read.
    pub(super) fn read_by_search_grant(&self, raw: &str) -> bool {
        let Some(grants) = &self.search_grants else {
            return false;
        };
        let Ok(target) = super::web_policy::canonical_url(raw) else {
            return false;
        };
        grants
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(target.as_str())
            && Policy::load(&self.allow_path(), self.limits.max_rules)
                .is_ok_and(|saved| !saved.blocks(&target) && !saved.permits(&target, None))
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

    /// A home with no policy file yet has no rules: reads still refuse (as an
    /// empty allowlist) and only a validated admin grant creates the file. An
    /// unreadable or invalid file is still an error and is never reset.
    fn policy_or_empty(&self) -> Result<(Policy, bool)> {
        match std::fs::symlink_metadata(self.allow_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Policy::empty(), true)),
            _ => Ok((self.policy()?, false)),
        }
    }

    pub fn status(&self, enabled: bool, owner: &str) -> Result<Value> {
        let (policy, missing) = self.policy_or_empty()?;
        let stored = self.read_page(&self.context_path(owner), None).is_ok();
        // Only `auto` follows the saved keys: a Tavily key first, then SerpAPI.
        let auto = self.search_provider == super::web_provider::SearchProvider::Auto;
        let search_provider = self.search_provider.describe(
            auto && !self.search_key()?.trim().is_empty(),
            auto && !self.tavily_key()?.trim().is_empty(),
        );
        Ok(
            json!({"enabled": enabled, "allowlist": policy.rules.iter().map(|r| &r.pattern).collect::<Vec<_>>(),
            "rules": policy.rules, "policy_revision": policy.revision,
            "mode": policy.mode, "blocked": policy.blocked,
            "policy_scope":"shared_home", "context_scope":"account", "limits":self.limits,
            "search_provider": search_provider,
            // Whether a chat turn may read the exact URLs its own search listed.
            "search_result_reads": self.result_reads.enabled,
            "injected": enabled && stored, "context_stored": stored,
            "has_last": self.read_page(&self.last_path(owner), None).is_ok(), "max_allow": self.limits.max_rules,
            "policy_missing": missing}),
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
        if patterns.is_empty() || patterns.len() > MAX_BATCH_URLS || seeds.len() > 16 {
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
                if policy.rules.len() >= self.limits.max_rules {
                    return Err(error(
                        "WEB_ALLOWLIST_FULL",
                        &format!(
                            "rule cap reached (web.max_rules = {}); /web deny unused rules, or raise it in config.yaml and restart",
                            self.limits.max_rules
                        ),
                    ));
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
        Policy::parse(&bytes, self.limits.max_rules)?;
        crate::common::atomic::write_atomic(&self.allow_path(), &bytes, Some(0o600))
    }

    pub fn deny(&self, raw: &str, enabled: bool) -> Result<Value> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let mut policy = self.policy()?;
        let pattern = Rule::new(raw, "default", &[]).ok().map(|r| r.pattern);
        let before = policy.rules.len();
        policy
            .rules
            .retain(|r| r.id != raw && pattern.as_ref().is_none_or(|p| r.pattern != *p));
        // Removing nothing is not a success: a stale ID or a near-miss pattern
        // (www vs apex, missing /*) would otherwise look revoked.
        if policy.rules.len() == before {
            return Err(error("WEB_RULE_UNKNOWN", "no rule has that ID or pattern"));
        }
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
        let (policy, _) = self.policy_or_empty()?;
        if policy.refuses_all() {
            return Err(error("WEB_ALLOWLIST_EMPTY", "empty URL policy refuses all web access"));
        }
        Ok(policy)
    }

    /// The policy for a mutation: a missing file starts empty, an unreadable or
    /// invalid one is still an error and is never reset.
    fn policy_for_change(&self) -> Result<Policy> {
        match std::fs::symlink_metadata(self.allow_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::empty()),
            _ => self.policy(),
        }
    }

    /// Switch between the allowlist and the denylist. Allow and blocked rules
    /// are both kept, so switching back restores the earlier allowlist exactly.
    /// Opening the denylist needs `confirm: true`; returning to the allowlist
    /// only narrows access and needs none.
    pub fn set_mode(&self, mode: Mode, confirm: bool, enabled: bool) -> Result<Value> {
        if mode == Mode::Denylist && !confirm {
            return Err(error(
                "WEB_MODE_CONFIRM_REQUIRED",
                "switching to the denylist permits every public URL that is not blocked; resend with confirm: true",
            ));
        }
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let mut policy = self.policy_for_change()?;
        policy.mode = mode;
        self.save_policy(&policy)?;
        self.status(enabled, "")
    }

    /// Add blocked rules, validated together before one write. A pattern with
    /// no path (`https://example.org`) blocks the whole site: the host and
    /// every subdomain, each with all paths.
    pub fn block_rules(&self, patterns: &[String], enabled: bool) -> Result<Value> {
        if patterns.is_empty() || patterns.len() > MAX_BATCH_URLS {
            return Err(error("WEB_POLICY_INVALID", "invalid rule count"));
        }
        let mut rules: Vec<Rule> = Vec::new();
        for raw in patterns {
            for (i, pattern) in site_patterns(raw).iter().enumerate() {
                // The derived subdomain pattern does not exist for an IP host.
                let rule = match Policy::block_rule(pattern) {
                    Ok(rule) => rule,
                    Err(_) if i > 0 => continue,
                    Err(e) => return Err(e),
                };
                if !rules.iter().any(|r| r.id == rule.id) {
                    rules.push(rule);
                }
            }
        }
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let mut policy = self.policy_for_change()?;
        for rule in rules {
            if policy.blocked.iter().any(|r| r.id == rule.id) {
                continue;
            }
            if policy.blocked.len() >= self.limits.max_rules {
                return Err(error(
                    "WEB_BLOCKLIST_FULL",
                    &format!(
                        "blocked rule cap reached (web.max_rules = {}); /web unblock unused rules, or raise it in config.yaml and restart",
                        self.limits.max_rules
                    ),
                ));
            }
            policy.blocked.push(rule);
        }
        self.save_policy(&policy)?;
        self.status(enabled, "")
    }

    /// Remove one blocked rule by ID or exact pattern. A bare host removes the
    /// pair `block` added for it.
    pub fn unblock(&self, raw: &str, enabled: bool) -> Result<Value> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_POLICY_UNAVAILABLE", "policy lock failed"))?;
        let mut policy = self.policy()?;
        let patterns: Vec<String> = site_patterns(raw)
            .iter()
            .filter_map(|p| Policy::block_rule(p).ok().map(|r| r.pattern))
            .collect();
        let before = policy.blocked.len();
        policy.blocked.retain(|r| r.id != raw && !patterns.contains(&r.pattern));
        if policy.blocked.len() == before {
            return Err(error("WEB_RULE_UNKNOWN", "no blocked rule has that ID or pattern"));
        }
        self.save_policy(&policy)?;
        self.status(enabled, "")
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

    /// The address a test pinned for `host`. Release builds have no fixtures,
    /// so every host is resolved and passes `validate_addresses`.
    #[cfg(any(test, feature = "test-support"))]
    fn fixture_address(&self, host: &str) -> Option<SocketAddr> {
        self.test_resolve
            .iter()
            .chain(self.test_resolve_extra.iter())
            .find(|(test_host, _)| test_host == host)
            .map(|(_, addr)| *addr)
    }

    #[cfg(not(any(test, feature = "test-support")))]
    fn fixture_address(&self, _host: &str) -> Option<SocketAddr> {
        None
    }

    /// The fixture replacing the public Google search URL, when a test set one.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn public_search_fixture(&self) -> Option<url::Url> {
        self.test_public_search.clone()
    }

    #[cfg(not(any(test, feature = "test-support")))]
    pub(super) fn public_search_fixture(&self) -> Option<url::Url> {
        None
    }

    pub(super) async fn pinned_client(&self, target: &url::Url) -> Result<reqwest::Client> {
        let host = target.host_str().unwrap().trim_matches(['[', ']']);
        let port = target.port_or_known_default().unwrap();
        let addresses = match self.fixture_address(host) {
            Some(addr) => vec![addr],
            None => {
                let addresses = crate::common::dns::lookup_bounded(host, port)
                    .await
                    .map_err(|_| error("WEB_DNS", "DNS lookup failed"))?;
                validate_addresses(&addresses)?;
                addresses
            }
        };
        pinned_client_builder(host, &addresses, Duration::from_secs(self.limits.request_seconds))
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
        self.get_raw_paced(raw, group, cached, None).await
    }

    /// `pace` (`pace_ms`, the caller's deadline) spaces public Google searches.
    /// It is checked and stamped just before the send, after the permit wait and
    /// DNS, so queueing behind other fetches cannot shorten the gap.
    pub(super) async fn get_raw_paced(
        &self,
        raw: &str,
        group: Option<&str>,
        cached: Option<&Page>,
        pace: Option<(u64, tokio::time::Instant, &Audit)>,
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
            let target = policy.authorize_read(raw, group)?;
            let client = self.pinned_client(&target).await?;
            let current = self.policy()?;
            current.authorize_read(target.as_str(), group)?;
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
            if let Some((pace_ms, search_deadline, audit)) = pace {
                let last = *self.public_search_sent.lock().unwrap_or_else(|p| p.into_inner());
                let waiting = tokio::time::Instant::now();
                if let Some(ready) = super::web_google::public_pace(last, pace_ms, search_deadline)? {
                    tokio::time::sleep_until(ready).await;
                }
                let sent = tokio::time::Instant::now();
                *self.public_search_sent.lock().unwrap_or_else(|p| p.into_inner()) = Some(sent);
                // The broker decision is written before this wait; this line's timestamp is the send.
                audit.log(json!({"event":"web_search_sent","provider":"google-public",
                    "waited_ms":(sent - waiting).as_millis() as u64,
                    "gap_ms":last.map(|at| (sent - at).as_millis() as u64)}));
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
                    // `status` tells a server's answer from a connection that never produced one.
                    return Err(error("WEB_FETCH_FAILED", "upstream error").detail("status", status.as_u16()));
                }
                // Encoding and type are refused before size: the opt-in fallback
                // re-reads an oversized text page, never an image or a compressed
                // body that also happens to be large.
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
                let pdf = content_type == crate::common::pdf_text::MIME;
                if !pdf
                    && !matches!(
                        content_type.as_str(),
                        "text/plain" | "text/html" | "application/xhtml+xml" | "text/markdown"
                    )
                {
                    // Never the header itself. A PDF is read below; only a PDF whose
                    // local extraction fails is marked `pdf` for the opt-in fallback.
                    return Err(error("WEB_NOT_TEXT", "unsupported content type").detail("pdf", false));
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > self.limits.response_bytes as u64)
                {
                    return Err(error("WEB_TOO_LARGE", "response exceeds limit"));
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
                // A PDF keeps `web.response_bytes`, so batch byte budgets still hold;
                // a failed local extraction stays `WEB_NOT_TEXT` marked `pdf` for the
                // opt-in fallback. Its link annotations are never collected.
                let (body, (title, text, links)) = if pdf {
                    pdf_in_worker(bytes.clone(), &target, deadline, permit).await?
                } else {
                    extract_in_worker(body, kind, base, self.limits.html_parser_handles, deadline, permit).await?
                };
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
                    via: None,
                }
            };
            let current = self.policy()?;
            current.authorize_read(&page.url, group)?;
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
        Ok(serde_json::to_value(
            self.fetch_page(raw, enabled, audit, owner).await?,
        )?)
    }

    pub(super) async fn fetch_page(&self, raw: &str, enabled: bool, audit: &Audit, owner: &str) -> Result<Page> {
        let policy = self.require_enabled(enabled)?;
        self.gate_tool("web_fetch", &[raw.into()], enabled, audit)?;
        // A content read, like every batch path: the pinned read below also admits
        // an origin's robots.txt, which is discovery's exception and not a grant.
        policy.authorize(raw, None)?;
        let page = self.get_or_fallback(raw, None, None, audit, None).await?;
        self.cache_page(&page, None)?;
        self.store_last(&page, None, owner)?;
        page.validate(&self.policy()?, None)?;
        Ok(page)
    }
}

/// `https://example.org` (no path) names a whole site for blocking: the host
/// and its subdomains, all paths. Any other pattern is used as written.
fn site_patterns(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let Some((scheme, rest)) = raw.split_once("://") else {
        return vec![raw.to_string()];
    };
    if rest.contains(['/', '?', '#']) || rest.is_empty() {
        return vec![raw.to_string()];
    }
    match rest.strip_prefix("*.") {
        Some(host) => vec![format!("{scheme}://*.{host}/*")],
        None => vec![format!("{scheme}://{rest}/*"), format!("{scheme}://*.{rest}/*")],
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_pathless_block_names_the_site_and_its_subdomains() {
        assert_eq!(
            super::site_patterns("https://example.org"),
            ["https://example.org/*", "https://*.example.org/*"]
        );
        assert_eq!(
            super::site_patterns("https://*.example.org"),
            ["https://*.example.org/*"]
        );
        assert_eq!(
            super::site_patterns("https://example.org/news/*"),
            ["https://example.org/news/*"]
        );
        assert_eq!(super::site_patterns("https://example.org/"), ["https://example.org/"]);
    }

    /// The console polls a detached research job, so it does not hold one fetch
    /// across both deadlines. The CLI client of the synchronous route still
    /// waits for both ceilings plus 10s: 7_210_000 ms at the one-hour maxima.
    #[test]
    fn research_client_wait_covers_both_deadline_ceilings() {
        let sec = super::research_request_timeout_sec(MAX_RESEARCH_SECONDS, MAX_SYNTHESIS_SECONDS, false);
        assert_eq!(sec, MAX_RESEARCH_SECONDS + MAX_SYNTHESIS_SECONDS + 10);
        assert_eq!(sec * 1000, 7_210_000);
        assert_eq!(super::research_request_timeout_sec(1200, 1800, false), 3010);
        assert_eq!(
            super::research_request_timeout_sec(10, 10, true),
            MAX_RESEARCH_SECONDS + MAX_SYNTHESIS_SECONDS + 10
        );
        let console = include_str!("../../assets/static/harness.html");
        assert!(
            console.contains("api('/api/web/research/jobs', 'POST', body)"),
            "the console starts research as a job"
        );
        assert!(
            !console.contains("api('/api/web/research', 'POST'"),
            "the console must not hold a research request for {sec}s"
        );
        let cfg = AppConfig::from_str(
            "web:\n  research_seconds: 3600\n  synthesis_seconds: 3600\n",
            Path::new("t.yaml"),
        )
        .unwrap();
        let limits = Limits::load(&cfg).unwrap();
        assert_eq!((limits.research_seconds, limits.synthesis_seconds), (3600, 3600));
        for key in ["research_seconds", "synthesis_seconds"] {
            let cfg = AppConfig::from_str(&format!("web:\n  {key}: 3601\n"), Path::new("t.yaml")).unwrap();
            let err = Limits::load(&cfg).unwrap_err().to_string();
            assert!(err.contains(&format!("web.{key}")) && !err.contains("3601"), "{err}");
        }
    }
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
    fn chat_tool_call_budget_defaults_to_thirty_and_rejects_out_of_range_values() {
        for yaml in ["{}", include_str!("../../assets/config.default.yaml")] {
            let cfg = AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap();
            assert_eq!(Limits::load(&cfg).unwrap().chat_tool_calls, 30);
        }
        for value in ["1", "3", "10", "30", "0", "31", "-1", "1.5", "\"10\"", "null"] {
            let cfg =
                AppConfig::from_str(&format!("web: {{chat_tool_calls: {value}}}"), Path::new("config.yaml")).unwrap();
            let result = Limits::load(&cfg);
            if let Ok(expected @ 1..=30) = value.parse::<usize>() {
                assert_eq!(result.unwrap().chat_tool_calls, expected);
            } else {
                assert!(result.is_err(), "invalid limit accepted: {value}");
            }
        }
    }

    #[test]
    fn goal_web_default_ships_all_and_refuses_unknown_modes() {
        use crate::server::sessions::GoalWebMode;
        for yaml in ["{}", include_str!("../../assets/config.default.yaml")] {
            let cfg = AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap();
            assert_eq!(Limits::load(&cfg).unwrap().goal_web_default, GoalWebMode::All);
        }
        for (value, expected) in [
            ("off", Some(GoalWebMode::Off)),
            ("fetch", Some(GoalWebMode::Fetch)),
            ("all", Some(GoalWebMode::All)),
            ("All", None),
            ("search", None),
            ("true", None),
            ("1", None),
            ("null", None),
        ] {
            let cfg =
                AppConfig::from_str(&format!("web: {{goal_web_default: {value}}}"), Path::new("config.yaml")).unwrap();
            match expected {
                Some(mode) => assert_eq!(Limits::load(&cfg).unwrap().goal_web_default, mode, "{value}"),
                None => assert!(Limits::load(&cfg).is_err(), "invalid mode accepted: {value}"),
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
        for (yaml, expected) in [("{}", 512), (include_str!("../../assets/config.default.yaml"), 4096)] {
            assert_eq!(
                Limits::load(&AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap())
                    .unwrap()
                    .html_parser_handles,
                expected
            );
        }
    }

    #[test]
    fn rule_cap_is_bounded_and_defaults_to_the_shipped_64() {
        for value in ["1", "32", "64", "100", "256", "0", "257", "-1", "1.5", "\"32\"", "null"] {
            let cfg = AppConfig::from_str(&format!("web: {{max_rules: {value}}}"), Path::new("config.yaml")).unwrap();
            let parsed = Limits::load(&cfg);
            if let Ok(n @ 1..=256) = value.parse::<usize>() {
                assert_eq!(parsed.unwrap().max_rules, n);
            } else {
                assert!(parsed.is_err(), "{value}");
            }
        }
        for yaml in ["{}", include_str!("../../assets/config.default.yaml")] {
            assert_eq!(
                Limits::load(&AppConfig::from_str(yaml, Path::new("config.yaml")).unwrap())
                    .unwrap()
                    .max_rules,
                DEFAULT_MAX_RULES
            );
        }
    }

    /// `web.search.result_reads`: on when the key is missing or literal `true`,
    /// off at literal `false`, refused at load for anything else. When on the
    /// grants live on one turn's snapshot only. The shared tool, the saved file
    /// and a rule change made through that snapshot never carry them.
    #[test]
    fn search_result_grants_stay_on_one_snapshot_and_out_of_the_saved_policy() {
        let listing = json!({"results":[
            {"url":"https://news.example.net/story?id=1"},
            {"url":"https://user:pw@creds.example.net/"},
            {"url":"http://127.0.0.1/admin"},
            {"url":"https://docs.example.org/guide"},
            {"url":"https://third.example.net/"},
            {"url":"https://fourth.example.net/"}]}); // DevSkim: ignore DS162092 because the loopback URL must be skipped, never fetched.
        let dir = tempfile::tempdir().unwrap();
        let off = AppConfig::from_str(
            "web: {max_rules: 8, search: {result_reads: {enabled: false}}}",
            Path::new("config.yaml"),
        )
        .unwrap();
        let shipped = WebTool::new(dir.path(), &off).unwrap();
        shipped
            .allow_rules(&["https://docs.example.org/*".to_string()], "docs", &[], true)
            .unwrap();
        assert!(shipped.for_search_grants(&[]).is_none(), "off at literal false");
        assert_eq!(shipped.grant_search_results(&listing), 0);
        // A home written before the key existed has no key: it reads as on.
        let absent = AppConfig::from_str("web: {max_rules: 8}", Path::new("config.yaml")).unwrap();
        let older_home = WebTool::new(dir.path(), &absent).unwrap();
        assert!(older_home.for_search_grants(&[]).is_some(), "a missing key is on");
        assert_eq!(older_home.status(true, "").unwrap()["search_result_reads"], true);
        // The shared tool holds no grant in either case.
        assert_eq!(older_home.grant_search_results(&listing), 0);
        for quoted in ["'true'", "'false'", "1"] {
            let cfg = AppConfig::from_str(
                &format!("web: {{search: {{result_reads: {{enabled: {quoted}}}}}}}"),
                Path::new("config.yaml"),
            )
            .unwrap();
            let failure = WebTool::new(dir.path(), &cfg).unwrap_err();
            assert!(
                failure.message.contains("web.search.result_reads.enabled"),
                "{quoted} is refused by key"
            );
        }
        for bad in ["0", "101", "'5'"] {
            let cfg = AppConfig::from_str(
                &format!("web: {{search: {{result_reads: {{max_urls: {bad}}}}}}}"),
                Path::new("config.yaml"),
            )
            .unwrap();
            let failure = WebTool::new(dir.path(), &cfg).unwrap_err();
            assert!(failure.message.contains("web.search.result_reads.max_urls"), "{bad}");
        }
        let on = AppConfig::from_str(
            "web: {max_rules: 8, search: {result_reads: {enabled: true, max_urls: 3}}}",
            Path::new("config.yaml"),
        )
        .unwrap();
        let shared = WebTool::new(dir.path(), &on).unwrap();
        let saved = std::fs::read(shared.allow_path()).unwrap();
        let turn = shared.for_search_grants(&[]).unwrap();
        // Rank order, canonical URLs only, at most max_urls held.
        assert_eq!(turn.grant_search_results(&listing), 3);
        assert_eq!(
            turn.search_grants(),
            [
                "https://docs.example.org/guide",
                "https://news.example.net/story?id=1",
                "https://third.example.net/"
            ]
        );
        assert_eq!(
            turn.grant_search_results(&listing),
            0,
            "the limit is held, not refilled"
        );
        let granted = turn.policy().unwrap();
        assert!(granted.authorize("https://news.example.net/story?id=1", None).is_ok());
        assert!(granted.authorize("https://news.example.net/story?id=2", None).is_err());
        assert!(granted.authorize("https://fourth.example.net/", None).is_err());
        // Marked only when the saved policy alone would refuse the read.
        assert!(turn.read_by_search_grant("https://news.example.net/story?id=1"));
        assert!(!turn.read_by_search_grant("https://docs.example.org/guide"));
        assert!(!turn.read_by_search_grant("https://fourth.example.net/"));
        // A clone of the turn's snapshot (a blocking worker) sees the same grants.
        assert!(turn
            .clone()
            .policy()
            .unwrap()
            .authorize("https://third.example.net/", None)
            .is_ok());
        // The shared tool and another turn's snapshot see none of them.
        assert!(shared
            .policy()
            .unwrap()
            .authorize("https://news.example.net/story?id=1", None)
            .is_err());
        assert!(shared
            .for_search_grants(&[])
            .unwrap()
            .policy()
            .unwrap()
            .authorize("https://news.example.net/story?id=1", None)
            .is_err());
        // A loop run seeds its next turn, within the limit.
        let seeded = shared.for_search_grants(&turn.search_grants()).unwrap();
        assert!(seeded
            .policy()
            .unwrap()
            .authorize("https://third.example.net/", None)
            .is_ok());
        // A blocked rule wins, and saving through the granting snapshot writes no grant.
        turn.block_rules(&["https://news.example.net/*".to_string()], true)
            .unwrap();
        assert_eq!(
            turn.policy()
                .unwrap()
                .authorize("https://news.example.net/story?id=1", None)
                .unwrap_err()
                .code,
            "WEB_BLOCKED"
        );
        assert!(!turn.read_by_search_grant("https://news.example.net/story?id=1"));
        let written = std::fs::read_to_string(shared.allow_path()).unwrap();
        assert!(
            !written.contains("third.example.net") && !written.contains("granted"),
            "{written}"
        );
        turn.unblock("https://news.example.net/*", true).unwrap();
        assert_eq!(
            std::fs::read(shared.allow_path()).unwrap(),
            saved,
            "the saved policy is unchanged"
        );
    }

    #[test]
    fn the_configured_rule_cap_limits_the_policy_but_not_one_grant() {
        let site = |i: usize| format!("https://site{i}.example/");
        let batch = |range: std::ops::Range<usize>| range.map(site).collect::<Vec<_>>();
        let three = AppConfig::from_str("web: {max_rules: 3}", Path::new("config.yaml")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let web = WebTool::new(dir.path(), &three).unwrap();
        assert_eq!(
            web.allow_rules(&batch(0..3), "docs", &[], true).unwrap()["max_allow"],
            3
        );
        // Re-granting an existing rule replaces it, even at the cap.
        web.allow_rules(&batch(0..1), "docs", &[], true).unwrap();
        let full = web.allow_rules(&batch(3..4), "docs", &[], true).unwrap_err();
        assert_eq!(full.code, "WEB_ALLOWLIST_FULL");
        assert!(full.message.contains("web.max_rules = 3"), "{}", full.message);
        // A raised cap admits more rules in total; one grant still names at most 32.
        let hundred = AppConfig::from_str("web: {max_rules: 100}", Path::new("config.yaml")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let web = WebTool::new(dir.path(), &hundred).unwrap();
        assert_eq!(
            web.allow_rules(&batch(0..33), "docs", &[], true).unwrap_err().code,
            "WEB_POLICY_INVALID"
        );
        web.allow_rules(&batch(0..32), "docs", &[], true).unwrap();
        let status = web.allow_rules(&batch(32..64), "docs", &[], true).unwrap();
        assert_eq!(status["rules"].as_array().unwrap().len(), 64);
        // A lowered cap refuses the whole policy and names the key; it never truncates.
        let refused = WebTool::new(dir.path(), &three).unwrap().policy().unwrap_err();
        assert_eq!(refused.code, "WEB_POLICY_INVALID");
        assert!(refused.message.contains("web.max_rules is 3"), "{}", refused.message);
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
