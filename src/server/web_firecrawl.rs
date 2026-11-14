//! Firecrawl as an opt-in second reader for exact permitted URLs
//! (`web.firecrawl.enabled`, ships off). It runs only after the pinned fetch got
//! a server's 403, 429 or 503, a near-empty HTML extract (a script shell), a
//! body over `web.response_bytes`, or a PDF (operator decision, 2026-11-10: the
//! last two were honest failures before), and only for exact reads: `/web fetch`, the chat `web_fetch` tool and batch reads,
//! never discovery, crawl or search. The URL is authorized first and must come
//! back unchanged: a redirect anywhere is refused, as for the pinned fetch. The
//! URL leaves the machine, so each read is counted against
//! `web.firecrawl.max_per_hour`.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::web_google::{listing_body, unix_now};
use super::web_policy::{canonical_url, error};
use super::web_search::{Page, WebTool};
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::Result;

/// The fixed production endpoint; only tests substitute a loopback fixture.
const FIRECRAWL_ENDPOINT: &str = "https://api.firecrawl.dev/v2/scrape";
const PROVIDER: &str = "firecrawl";
/// An HTML page whose extract is shorter than this is treated as a script shell.
pub(super) const THIN_EXTRACT_CHARS: usize = 200;
/// Pages Firecrawl may parse from one PDF. It bills a PDF one credit per page,
/// so every read pins this cap; a longer document is read from its first pages.
pub(super) const PDF_MAX_PAGES: u64 = 10;
/// Longest a fallback read may take, whatever `web.request_seconds` is.
const MAX_FALLBACK_SECONDS: u64 = 60;

/// `web.firecrawl.*`, restart-only. `enabled` reads through `flag_is_true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirecrawlConfig {
    pub enabled: bool,
    pub max_per_hour: u64,
}

impl FirecrawlConfig {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        Ok(Self {
            enabled: cfg.flag_is_true("web.firecrawl.enabled"),
            max_per_hour: cfg.u64_in("web.firecrawl.max_per_hour", 20, 1..=1000)?,
        })
    }
}

/// Reads taken in the current clock hour; in memory, lost on restart.
#[derive(Debug, Default)]
pub(crate) struct HourlyCap(std::sync::Mutex<(u64, u64)>);

impl HourlyCap {
    /// Take one read when fewer than `max` were taken this clock hour.
    pub(super) fn take(&self, now_unix: u64, max: u64) -> bool {
        let hour = now_unix - now_unix % 3600;
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.0 != hour {
            *state = (hour, 0);
        }
        if state.1 >= max {
            return false;
        }
        state.1 += 1;
        true
    }
}

/// Whether a pinned read may be re-read by Firecrawl: a server answered 403, 429
/// or 503, or a fresh 200 HTML page that is a script shell: it carries a script,
/// extracted to almost nothing, and that text is under 5% of the markup. A short
/// page, even one with an analytics script (example.com), is an answer. A body
/// over `web.response_bytes` (`WEB_TOO_LARGE`) and a PDF (`WEB_NOT_TEXT` marked
/// `pdf`) escalate too: the pinned fetch reached a permitted public server and
/// only could not use what it sent. A transport failure, a redirect, a policy
/// or SSRF refusal, any other content type and every other code never escalate.
pub(super) fn escalates(outcome: &Result<(Page, String)>) -> bool {
    match outcome {
        Ok((page, raw)) => {
            page.status == 200
                && page.transfer_bytes > 0
                && page.content_type.contains("html")
                && page.chars < THIN_EXTRACT_CHARS
                && page.chars.saturating_mul(20) < raw.len()
                && raw.as_bytes().windows(7).any(|w| w.eq_ignore_ascii_case(b"<script"))
        }
        Err(failure) => match failure.code.as_str() {
            "WEB_FETCH_FAILED" => matches!(
                failure.details.get("status").and_then(Value::as_u64),
                Some(403 | 429 | 503)
            ),
            "WEB_TOO_LARGE" => true,
            "WEB_NOT_TEXT" => failure.details.get("pdf") == Some(&Value::Bool(true)),
            _ => false,
        },
    }
}

/// A rejected key stays loud; a spent plan (402) or rate limit (429) pauses
/// Firecrawl; anything else is an unusable provider.
fn refusal_code(status: u16) -> Option<&'static str> {
    match status {
        200 => None,
        401 => Some("WEB_FALLBACK_KEY_REJECTED"),
        402 | 429 => Some("WEB_FALLBACK_QUOTA"),
        _ => Some("WEB_FALLBACK_PROVIDER"),
    }
}

/// Title, markdown and the target's own status from a scrape answer. The answer
/// must name `target` as its final URL: a redirect, even to a permitted URL, is
/// refused. Only `markdown`, `metadata.url`, `metadata.statusCode` and
/// `metadata.title` are read.
pub(super) fn firecrawl_page(body: &[u8], target: &url::Url) -> Result<(String, String, u16)> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| error("WEB_FALLBACK_PROVIDER", "invalid fallback response"))?;
    if value.get("success") != Some(&Value::Bool(true)) {
        return Err(error("WEB_FALLBACK_PROVIDER", "fallback reader returned an error"));
    }
    let data = &value["data"];
    let final_url = data["metadata"]["url"]
        .as_str()
        .and_then(|raw| canonical_url(raw).ok())
        .ok_or_else(|| error("WEB_FALLBACK_PROVIDER", "fallback response names no final URL"))?;
    if final_url.as_str() != target.as_str() {
        return Err(error(
            "WEB_REDIRECT_REFUSED",
            "fallback reader followed a redirect; refused",
        ));
    }
    let status = data["metadata"]["statusCode"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .ok_or_else(|| error("WEB_FALLBACK_PROVIDER", "fallback response names no status"))?;
    if !(200..300).contains(&status) {
        return Err(error("WEB_FETCH_FAILED", "upstream error").detail("status", status));
    }
    let markdown = data["markdown"]
        .as_str()
        .ok_or_else(|| error("WEB_FALLBACK_PROVIDER", "fallback response carries no text"))?;
    let title = crate::common::clip_chars(data["metadata"]["title"].as_str().unwrap_or("").trim(), 300);
    Ok((title, markdown.to_string(), status))
}

impl WebTool {
    fn firecrawl_endpoint(&self) -> url::Url {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(fixture) = &self.test_firecrawl {
            return fixture.clone();
        }
        url::Url::parse(FIRECRAWL_ENDPOINT).expect("static URL")
    }

    /// The pinned read, then, when `web.firecrawl.enabled` and the read qualifies
    /// (`escalates`), one Firecrawl read. A Firecrawl failure keeps the pinned
    /// outcome: the thin page, or the original error with a `fallback` code.
    ///
    /// Both reads are charged: a returned page's `transfer_bytes` covers the pinned
    /// read and the fallback, and an error carries `charged_bytes` once a fallback
    /// was sent. A failed pinned read counts as a full `web.response_bytes`, as a
    /// batch already charges failures. `budget`, the bytes a batch has left, skips a
    /// fallback that could not fit a second full response; single reads pass `None`.
    pub(super) async fn get_or_fallback(
        &self,
        raw: &str,
        group: Option<&str>,
        cached: Option<&Page>,
        audit: &Audit,
        budget: Option<usize>,
    ) -> Result<Page> {
        let pinned = self.get_raw(raw, group, cached, self.pdf_cap(budget)).await;
        self.check_enabled()?;
        if !self.firecrawl.enabled || !escalates(&pinned) {
            return pinned.map(|(page, _)| page);
        }
        let pinned_charge = match &pinned {
            Ok((page, _)) => page.transfer_bytes,
            // A PDF read past `response_bytes` reports what it actually transferred.
            Err(e) => e
                .details
                .get("charged_bytes")
                .and_then(Value::as_u64)
                .map_or(self.limits.response_bytes, |n| n as usize),
        };
        // Codes only: never the URL, a key or provider text.
        let from = match &pinned {
            Ok(_) => "WEB_THIN_EXTRACT".to_string(),
            Err(failure) => failure.code.clone(),
        };
        let fallback_charge = AtomicUsize::new(0);
        let read = if budget.is_some_and(|left| pinned_charge + self.limits.response_bytes > left) {
            Err(error(
                "WEB_FALLBACK_BUDGET",
                "web.run_bytes cannot cover a fallback read",
            ))
        } else {
            self.firecrawl_read(raw, group, audit, &fallback_charge).await
        };
        let fallback_charge = fallback_charge.load(Ordering::SeqCst);
        self.check_enabled()?;
        match read {
            Ok(mut page) => {
                audit.log(json!({"event":"web_fetch_fallback","provider":PROVIDER,"from":from,"ok":true}));
                page.transfer_bytes = pinned_charge + fallback_charge;
                Ok(page)
            }
            Err(failure) => {
                audit.log(json!({"event":"web_fetch_fallback","provider":PROVIDER,"from":from,
                    "ok":false,"code":failure.code}));
                match pinned {
                    Ok((mut page, _)) => {
                        page.transfer_bytes += fallback_charge;
                        Ok(page)
                    }
                    Err(original) => {
                        let original = original.detail("fallback", failure.code);
                        Err(if fallback_charge > 0 {
                            original.detail("charged_bytes", pinned_charge + fallback_charge)
                        } else {
                            original
                        })
                    }
                }
            }
        }
    }

    /// `charged` records what this read spent: nothing before the request leaves,
    /// a full `web.response_bytes` once it is sent, then the answer's actual size.
    async fn firecrawl_read(
        &self,
        raw: &str,
        group: Option<&str>,
        audit: &Audit,
        charged: &AtomicUsize,
    ) -> Result<Page> {
        if self.search_cooldown.holding(PROVIDER, Instant::now()).is_some() {
            return Err(error(
                "WEB_FALLBACK_PAUSED",
                "Firecrawl is paused after a quota refusal",
            ));
        }
        let key = self.provider_key("FIRECRAWL_API_KEY", self.firecrawl_key_from_file)?;
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(error(
                "WEB_FALLBACK_KEY_MISSING",
                "web.firecrawl.enabled is true but no Firecrawl key is active; add one in API Keys",
            ));
        }
        let policy = self.policy()?;
        let target = policy.authorize_read(raw, group)?;
        if !self.firecrawl_spent.take(unix_now(), self.firecrawl.max_per_hour) {
            return Err(error(
                "WEB_FALLBACK_CAP",
                "web.firecrawl.max_per_hour is spent for this hour",
            ));
        }
        let endpoint = self.firecrawl_endpoint();
        // A rendered or parsed read takes longer than a direct fetch: the fallback
        // gets twice `web.request_seconds` (at most a minute; before, the same 8 s
        // less one cut most rendered pages off). Firecrawl's own page budget sits
        // inside ours, so its answer can still arrive.
        let fallback_seconds = (self.limits.request_seconds * 2).min(MAX_FALLBACK_SECONDS);
        let remote_ms = (fallback_seconds * 1000).saturating_sub(1000).max(1000);
        let fetched = tokio::time::timeout(Duration::from_secs(fallback_seconds + 1), async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| error("WEB_CANCELLED", "fetch cancelled"))?;
            self.check_enabled()?;
            let client = self.pinned_client(&endpoint).await?;
            self.check_enabled()?;
            // Permission may change while this waits for a permit or DNS: recheck it
            // immediately before the URL leaves the machine, as the pinned fetch does.
            let current = self.policy()?;
            current.authorize_read(target.as_str(), group)?;
            if current.revision != policy.revision {
                return Err(error(
                    "WEB_POLICY_CHANGED",
                    "policy changed before the fallback read; nothing was sent",
                ));
            }
            // So may the key: one cleared or replaced since this read began is never sent.
            if self
                .provider_key("FIRECRAWL_API_KEY", self.firecrawl_key_from_file)?
                .trim()
                != key
            {
                return Err(error(
                    "WEB_FALLBACK_KEY_CHANGED",
                    "Firecrawl key changed before the fallback read; nothing was sent",
                ));
            }
            charged.store(self.limits.response_bytes, Ordering::SeqCst);
            let response = client
                .post(endpoint)
                // The pinned client stops at `web.request_seconds`; this request
                // has the longer fallback budget, so it carries its own.
                .timeout(Duration::from_secs(fallback_seconds))
                .bearer_auth(&key)
                .json(
                    &json!({"url":target.as_str(),"formats":["markdown"],"onlyMainContent":true,
                    "timeout":remote_ms,"waitFor":0,"mobile":false,"skipTlsVerification":false,
                    "removeBase64Images":true,"blockAds":true,"proxy":"basic","storeInCache":false,"maxAge":0,
                    // PDFs only, a bounded number of pages (one credit each); image OCR stays off.
                    "parsers":[{"type":"pdf","maxPages":PDF_MAX_PAGES}]}),
                )
                .send()
                .await
                .map_err(|_| error("WEB_FALLBACK_PROVIDER", "fallback request failed"))?;
            if let Some(code) = refusal_code(response.status().as_u16()) {
                return Err(error(
                    code,
                    "fallback reader refused the request; check API Keys and plan",
                ));
            }
            let body = listing_body(response, self.limits.response_bytes).await?;
            charged.store(body.len(), Ordering::SeqCst);
            let (title, text, status) = firecrawl_page(&body, &target)?;
            if text.contains(&key) || title.contains(&key) {
                return Err(error(
                    "WEB_FALLBACK_PROVIDER",
                    "credential reflected in fallback text; response discarded",
                ));
            }
            Ok((title, text, status, body.len()))
        })
        .await
        .map_err(|_| error("WEB_TIMEOUT", "fallback deadline exceeded"))
        .and_then(|read| read);
        let (title, text, status, transfer_bytes) = match fetched {
            Ok(read) => read,
            Err(failure) => {
                if let Some(until) =
                    self.search_cooldown
                        .start(PROVIDER, &failure.code, &self.limits, Instant::now(), unix_now())
                {
                    audit.log(json!({"event":"web_search_cooldown","provider":PROVIDER,
                        "code":failure.code,"until":until}));
                }
                return Err(failure);
            }
        };
        self.check_enabled()?;
        let current = self.policy()?;
        current.authorize_read(target.as_str(), group)?;
        if current.revision != policy.revision {
            return Err(error(
                "WEB_POLICY_CHANGED",
                "policy changed during request; result discarded",
            ));
        }
        if self
            .provider_key("FIRECRAWL_API_KEY", self.firecrawl_key_from_file)?
            .trim()
            != key
        {
            return Err(error(
                "WEB_FALLBACK_KEY_CHANGED",
                "Firecrawl key changed; retry the read",
            ));
        }
        Ok(Page {
            url: target.to_string(),
            title,
            content_hash: crate::common::sha256_hex(&text),
            chars: text.chars().count(),
            bytes: text.len(),
            text,
            // Links are never taken from a fallback read, so it cannot widen a crawl.
            links: Vec::new(),
            status,
            content_type: "text/markdown".into(),
            transfer_bytes,
            fetched_at: crate::common::now_ts(),
            extraction_version: 1,
            policy_revision: current.revision,
            etag: None,
            last_modified: None,
            via: Some(PROVIDER.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(status: u16, content_type: &str, chars: usize, transfer_bytes: usize) -> Page {
        let text = "x".repeat(chars);
        Page {
            url: "https://example.org/".into(),
            title: String::new(),
            content_hash: crate::common::sha256_hex(&text),
            chars,
            bytes: chars,
            text,
            links: Vec::new(),
            status,
            content_type: content_type.into(),
            transfer_bytes,
            fetched_at: 0.0,
            extraction_version: 1,
            policy_revision: "a".repeat(64),
            etag: None,
            last_modified: None,
            via: None,
        }
    }

    #[test]
    fn only_bot_walls_and_script_shells_escalate() {
        for status in [403, 429, 503] {
            let failure = error("WEB_FETCH_FAILED", "upstream error").detail("status", status);
            assert!(escalates(&Err(failure)), "{status}");
        }
        for status in [401, 404, 410, 500, 502] {
            let failure = error("WEB_FETCH_FAILED", "upstream error").detail("status", status);
            assert!(!escalates(&Err(failure)), "{status}");
        }
        for code in [
            "WEB_FETCH_FAILED",
            "WEB_REDIRECT_REFUSED",
            "WEB_SSRF_DENIED",
            "WEB_HOST_DENIED",
            "WEB_DNS",
            "WEB_TIMEOUT",
            "WEB_NOT_TEXT",
            "WEB_ENCODING_REFUSED",
            "WEB_HEADERS_TOO_LARGE",
            "WEB_POLICY_CHANGED",
        ] {
            assert!(!escalates(&Err(error(code, "x"))), "{code} without a server status");
        }
        // Operator decision, 2026-11-10: an oversized body and a PDF also go to
        // the fallback. Any other non-text type (an image, a zip) does not.
        assert!(escalates(&Err(error("WEB_TOO_LARGE", "response exceeds limit"))));
        assert!(escalates(&Err(
            error("WEB_NOT_TEXT", "unsupported content type").detail("pdf", true)
        )));
        assert!(!escalates(&Err(
            error("WEB_NOT_TEXT", "unsupported content type").detail("pdf", false)
        )));
        let shell = format!(
            "<html><body><div id=app></div><SCRIPT>{}</script></body></html>",
            "x".repeat(2000)
        );
        let shell = shell.as_str();
        let short = "<html><body><p>Example Domain. This domain is for examples.</p></body></html>";
        // example.com today: 156 characters of text in 577 bytes, with one analytics script.
        let analytics = format!(
            "<html><head><script src=/s.js></script></head><body>{}</body></html>",
            "t".repeat(500)
        );
        let read =
            |status, kind: &str, chars, bytes, raw: &str| Ok((page(status, kind, chars, bytes), raw.to_string()));
        assert!(escalates(&read(200, "text/html", 40, 9000, shell)), "script shell");
        assert!(
            !escalates(&read(200, "text/html", 40, 9000, short)),
            "a short page without a script is an answer"
        );
        assert!(
            !escalates(&read(200, "text/html", 156, 9000, &analytics)),
            "a short page whose text is not dwarfed by script is an answer"
        );
        assert!(!escalates(&read(200, "text/html", THIN_EXTRACT_CHARS, 9000, shell)));
        assert!(
            !escalates(&read(200, "text/plain", 40, 9000, shell)),
            "short plain text is an answer"
        );
        assert!(
            !escalates(&read(200, "text/html", 40, 0, shell)),
            "a 304 reuse is not re-read"
        );
        assert!(!escalates(&read(203, "text/html", 40, 9000, shell)));
    }

    #[test]
    fn a_firecrawl_answer_must_name_the_exact_target_and_a_success_status() {
        let target = canonical_url("https://example.org/docs/a").unwrap();
        let answer = |url: &str, status: u64| {
            json!({"success":true,"data":{"markdown":"# Doc\nbody","rawHtml":"<script>x</script>",
                "metadata":{"title":"Doc","url":url,"sourceURL":url,"statusCode":status}}})
            .to_string()
        };
        let (title, text, status) =
            firecrawl_page(answer("https://example.org/docs/a", 200).as_bytes(), &target).unwrap();
        assert_eq!((title.as_str(), text.as_str(), status), ("Doc", "# Doc\nbody", 200));
        for moved in [
            "https://example.org/docs/b",
            "https://www.example.org/docs/a",
            "http://example.org/docs/a", // DevSkim: ignore DS137138 because this asserts a scheme downgrade of the target is refused.
            "https://example.org/docs/a?x=1",
        ] {
            assert_eq!(
                firecrawl_page(answer(moved, 200).as_bytes(), &target).unwrap_err().code,
                "WEB_REDIRECT_REFUSED",
                "{moved}"
            );
        }
        let failed = firecrawl_page(answer("https://example.org/docs/a", 404).as_bytes(), &target).unwrap_err();
        assert_eq!(failed.code, "WEB_FETCH_FAILED");
        assert_eq!(failed.details["status"], 404);
        for bad in [
            json!({"success":false,"error":"secret upstream text"}).to_string(),
            json!({"success":true,"data":{"markdown":"x","metadata":{"statusCode":200}}}).to_string(),
            json!({"success":true,"data":{"metadata":{"url":"https://example.org/docs/a","statusCode":200}}})
                .to_string(),
            json!({"success":true,"data":{"markdown":"x","metadata":{"url":"https://example.org/docs/a"}}}).to_string(),
            "<html>".to_string(),
        ] {
            let failure = firecrawl_page(bad.as_bytes(), &target).unwrap_err();
            assert_eq!(failure.code, "WEB_FALLBACK_PROVIDER", "{bad}");
            assert!(!failure.to_string().contains("secret"));
        }
    }

    #[test]
    fn statuses_map_to_loud_keys_quota_or_an_unusable_provider() {
        assert_eq!(refusal_code(200), None);
        assert_eq!(refusal_code(401), Some("WEB_FALLBACK_KEY_REJECTED"));
        for status in [402, 429] {
            assert_eq!(refusal_code(status), Some("WEB_FALLBACK_QUOTA"), "{status}");
        }
        for status in [400, 403, 404, 408, 500] {
            assert_eq!(refusal_code(status), Some("WEB_FALLBACK_PROVIDER"), "{status}");
        }
    }

    mod fixture {
        use super::*;
        use axum::{http::HeaderMap, http::StatusCode, routing::get, routing::post, Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        pub(super) struct Rig {
            pub web: WebTool,
            pub audit: Audit,
            pub dir: tempfile::TempDir,
            pub hits: Arc<AtomicUsize>,
            /// Pinned reads of the site's `/robots.txt`, which answers 403.
            pub robots: Arc<AtomicUsize>,
            /// `/gate` notifies `gate_hit` from inside the pinned read, then answers
            /// 403 only after `gate_go`.
            pub gate_hit: Arc<tokio::sync::Notify>,
            pub gate_go: Arc<tokio::sync::Notify>,
            tasks: Vec<tokio::task::JoinHandle<()>>,
        }
        impl Drop for Rig {
            fn drop(&mut self) {
                self.tasks.iter().for_each(|task| task.abort());
            }
        }
        impl Rig {
            pub(super) fn hits(&self) -> usize {
                self.hits.load(Ordering::SeqCst)
            }
            pub(super) async fn fetch(&self, path: &str) -> Result<Value> {
                self.web
                    .fetch(&format!("http://site.fixture/{path}"), true, &self.audit, "local") // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                    .await
            }
            pub(super) fn audit_text(&self) -> String {
                std::fs::read_to_string(self.dir.path().join("audit.jsonl")).unwrap_or_default()
            }
        }

        async fn serve(router: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
            let addr = listener.local_addr().unwrap();
            (
                addr,
                tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
            )
        }

        /// A site with a bot wall, a script shell, a plain page and a 404, and a
        /// Firecrawl that answers "Bearer good", refuses "quota" with 402, and
        /// reports a redirect for /moves. A request that asks for more than
        /// markdown, keeps the page in Firecrawl's cache or uses a costly proxy is 400.
        pub(super) async fn rig(yaml: &str, key: &str) -> Rig {
            let gate_hit = Arc::new(tokio::sync::Notify::new());
            let gate_go = Arc::new(tokio::sync::Notify::new());
            let (hit, go) = (gate_hit.clone(), gate_go.clone());
            let robots = Arc::new(AtomicUsize::new(0));
            let robots_seen = robots.clone();
            let site = Router::new()
                .route(
                    "/gate",
                    get(move || {
                        let (hit, go) = (hit.clone(), go.clone());
                        async move {
                            hit.notify_one();
                            go.notified().await;
                            (StatusCode::FORBIDDEN, "blocked")
                        }
                    }),
                )
                .route("/wall", get(|| async { (StatusCode::FORBIDDEN, "blocked") }))
                .route(
                    "/robots.txt",
                    get(move || {
                        robots_seen.fetch_add(1, Ordering::SeqCst);
                        async { (StatusCode::FORBIDDEN, "blocked") }
                    }),
                )
                .route("/moves", get(|| async { (StatusCode::FORBIDDEN, "blocked") }))
                .route("/missing", get(|| async { (StatusCode::NOT_FOUND, "gone") }))
                .route(
                    "/shell",
                    get(|| async {
                        (
                            [("content-type", "text/html")],
                            format!(
                                "<html><body><div id=app></div><script>{}</script></body></html>",
                                "x".repeat(4000)
                            ),
                        )
                    }),
                )
                .route(
                    "/ok",
                    get(|| async { ([("content-type", "text/plain")], "native ".repeat(80)) }),
                )
                .route(
                    "/paper.pdf",
                    get(|| async { ([("content-type", "application/pdf")], "%PDF-1.7 fixture") }),
                )
                .route(
                    "/real.pdf",
                    get(|| async {
                        let pdf = crate::common::pdf_text::fixture_pdf(&[&["Local PDF_WEB_FACT body"]], false);
                        ([("content-type", "application/pdf")], pdf)
                    }),
                )
                .route(
                    "/big.pdf",
                    get(|| async {
                        let lines: Vec<String> = (0..40)
                            .map(|n| format!("BIG_PDF_FACT line {n} of a long paper body"))
                            .collect();
                        let page: Vec<&str> = lines.iter().map(String::as_str).collect();
                        let pdf = crate::common::pdf_text::fixture_pdf(&[&page, &page, &page], false);
                        ([("content-type", "application/pdf")], pdf)
                    }),
                )
                .route(
                    "/junk.pdf",
                    get(|| async {
                        (
                            [("content-type", "application/pdf")],
                            format!("%PDF-1.7\n{}", "junk ".repeat(2000)),
                        )
                    }),
                )
                .route(
                    "/image.png",
                    get(|| async { ([("content-type", "image/png")], "not really a png") }),
                )
                .route(
                    "/huge",
                    get(|| async {
                        (
                            [("content-type", "text/html")],
                            format!("<p>{}</p>", "long ".repeat(4000)),
                        )
                    }),
                )
                .route(
                    "/huge.png",
                    get(|| async { ([("content-type", "image/png")], "pixel ".repeat(4000)) }),
                )
                .route(
                    "/huge.gz",
                    get(|| async {
                        (
                            [("content-type", "text/html"), ("content-encoding", "gzip")],
                            "packed ".repeat(4000),
                        )
                    }),
                )
                .route(
                    "/slow.pdf",
                    get(|| async { ([("content-type", "application/pdf")], "%PDF-1.7 fixture") }),
                );
            let hits = Arc::new(AtomicUsize::new(0));
            let counter = hits.clone();
            let firecrawl = Router::new().route(
                "/v2/scrape",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let pinned = body["formats"] == json!(["markdown"])
                            && body["storeInCache"] == json!(false)
                            && body["maxAge"] == json!(0)
                            && body["parsers"] == json!([{"type":"pdf","maxPages":PDF_MAX_PAGES}])
                            && body["proxy"] == "basic"
                            && body["skipTlsVerification"] == json!(false)
                            && body.get("actions").is_none();
                        if !pinned {
                            return (StatusCode::BAD_REQUEST, Json(json!({"success":false})));
                        }
                        match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                            Some("Bearer good") => {}
                            Some("Bearer quota") => {
                                return (StatusCode::PAYMENT_REQUIRED, Json(json!({"success":false})))
                            }
                            _ => return (StatusCode::UNAUTHORIZED, Json(json!({"success":false}))),
                        }
                        let url = body["url"].as_str().unwrap_or("").to_string();
                        if url.ends_with("/slow.pdf") {
                            // Longer than a 2 s `web.request_seconds`, inside its 4 s fallback budget.
                            tokio::time::sleep(Duration::from_millis(2300)).await;
                        }
                        let final_url = if url.ends_with("/moves") {
                            "http://site.fixture/elsewhere".to_string() // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        } else {
                            url
                        };
                        (
                            StatusCode::OK,
                            Json(json!({"success":true,"data":{"markdown":"# Rendered\nFallback body",
                                "metadata":{"title":"Rendered","url":final_url,"statusCode":200}}})),
                        )
                    }
                }),
            );
            let (site_addr, site_task) = serve(site).await;
            let (fc_addr, fc_task) = serve(firecrawl).await;
            let dir = tempfile::tempdir().unwrap();
            crate::common::home::Home::at(dir.path().to_path_buf())
                .ensure_layout()
                .unwrap();
            let cfg = AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap();
            let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
            web.plaintext_key_file = true;
            web.search_key_from_file = true;
            web.firecrawl_key_from_file = true;
            web.test_resolve = Some(("site.fixture".into(), site_addr));
            web.test_resolve_extra = vec![("firecrawl.fixture".into(), fc_addr)];
            web.test_firecrawl = Some(url::Url::parse("http://firecrawl.fixture/v2/scrape").unwrap()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            web.allow("http://site.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            if !key.is_empty() {
                super::super::super::env_keys::write_keys(
                    &dir.path().join(".env"),
                    &std::collections::BTreeMap::from([("FIRECRAWL_API_KEY".into(), key.into())]),
                )
                .unwrap();
            }
            let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
            Rig {
                web,
                audit,
                dir,
                hits,
                robots,
                gate_hit,
                gate_go,
                tasks: vec![site_task, fc_task],
            }
        }
    }

    const ON: &str = "web: {firecrawl: {enabled: true, max_per_hour: 4}}";

    /// Operator decision, 2026-11-10: a permitted URL whose body is over
    /// `web.response_bytes`, or is a PDF that local extraction (issue 813)
    /// cannot read, is re-read by the fallback when it is enabled. Any other type, and both cases with the fallback off, stay the
    /// failures they were, and nothing leaves the machine for them.
    #[tokio::test]
    async fn an_oversized_page_and_a_pdf_are_reread_and_nothing_else_is() {
        const SMALL: &str = "web: {response_bytes: 4096, firecrawl: {enabled: true, max_per_hour: 4}}";
        let rig = fixture::rig(SMALL, "good").await;
        for path in ["huge", "paper.pdf"] {
            let read = rig.fetch(path).await.unwrap();
            assert_eq!(read["via"], "firecrawl", "{path}: {read}");
            assert!(read["text"].as_str().unwrap().contains("Fallback body"), "{path}");
        }
        assert_eq!(rig.hits(), 2);
        assert_eq!(rig.fetch("image.png").await.unwrap_err().code, "WEB_NOT_TEXT");
        assert_eq!(rig.hits(), 2, "an image is never sent to the fallback");
        let audit = rig.audit_text();
        for from in ["WEB_TOO_LARGE", "WEB_NOT_TEXT"] {
            assert!(audit.contains(&format!("\"from\":\"{from}\"")), "{from}: {audit}");
        }

        let off = fixture::rig("web: {response_bytes: 4096}", "good").await;
        assert_eq!(off.fetch("huge").await.unwrap_err().code, "WEB_TOO_LARGE");
        let unreadable = off.fetch("paper.pdf").await.unwrap_err();
        assert_eq!(unreadable.code, "WEB_NOT_TEXT");
        assert_eq!(unreadable.details["pdf_code"], "PDF_MALFORMED");
        assert_eq!(off.hits(), 0);
    }

    /// A readable PDF is extracted locally and never reaches the fallback.
    #[tokio::test]
    async fn a_readable_pdf_is_read_locally_without_the_fallback() {
        let rig = fixture::rig(ON, "good").await;
        let read = rig.fetch("real.pdf").await.unwrap();
        assert!(read["via"].is_null(), "{read}");
        assert_eq!(read["content_type"], "application/pdf");
        assert_eq!(read["title"], "real.pdf");
        assert!(read["links"].as_array().unwrap().is_empty());
        assert!(
            read["text"]
                .as_str()
                .unwrap()
                .contains("# Page 1\nLocal PDF_WEB_FACT body"),
            "{read}"
        );
        assert_eq!(rig.hits(), 0);
    }

    /// Size is checked after encoding and type, so an oversized image or a large
    /// compressed body is refused for what it is and never reaches the fallback.
    #[tokio::test]
    async fn an_oversized_image_or_compressed_body_never_reaches_the_fallback() {
        const SMALL: &str = "web: {response_bytes: 4096, firecrawl: {enabled: true, max_per_hour: 4}}";
        let rig = fixture::rig(SMALL, "good").await;
        assert_eq!(rig.fetch("huge.png").await.unwrap_err().code, "WEB_NOT_TEXT");
        assert_eq!(rig.fetch("huge.gz").await.unwrap_err().code, "WEB_ENCODING_REFUSED");
        assert_eq!(rig.hits(), 0, "neither URL leaves the machine");
    }

    /// The fallback budget is twice `web.request_seconds`, and the request itself
    /// gets that budget: a 2.3 s read succeeds under a 2 s direct-fetch limit.
    #[tokio::test]
    async fn a_fallback_read_longer_than_request_seconds_fits_its_own_budget() {
        const SHORT: &str = "web: {request_seconds: 2, firecrawl: {enabled: true, max_per_hour: 4}}";
        let rig = fixture::rig(SHORT, "good").await;
        let read = rig.fetch("slow.pdf").await.unwrap();
        assert_eq!(read["via"], "firecrawl", "{read}");
        assert_eq!(rig.hits(), 1);
    }

    /// Reading an origin's robots.txt under a path rule is discovery's politeness
    /// exception. A single fetch is a content read, so it needs a content grant
    /// before the site or Firecrawl sees the URL.
    #[tokio::test]
    async fn a_single_fetch_of_robots_txt_is_refused_before_any_request() {
        let rig = fixture::rig(ON, "good").await;
        rig.web.allow("http://site.fixture/docs/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        rig.web.deny("http://site.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let robots = "http://site.fixture/robots.txt"; // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let policy = rig.web.policy().unwrap();
        assert!(
            policy.authorize_robots(robots, None).is_ok(),
            "discovery may still read it"
        );
        assert_eq!(policy.authorize(robots, None).unwrap_err().code, "WEB_HOST_DENIED");
        assert_eq!(rig.fetch("robots.txt").await.unwrap_err().code, "WEB_HOST_DENIED");
        let pinned_reads = rig.robots.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!((pinned_reads, rig.hits()), (0, 0), "(pinned reads, Firecrawl reads)");
    }

    #[tokio::test]
    async fn the_fallback_is_off_by_default_and_never_reads_by_itself() {
        let rig = fixture::rig("{}", "good").await;
        let failure = rig.fetch("wall").await.unwrap_err();
        assert_eq!(failure.code, "WEB_FETCH_FAILED");
        assert!(failure.details.get("fallback").is_none());
        assert_eq!(rig.fetch("shell").await.unwrap().get("via"), None);
        assert_eq!(rig.hits(), 0);
    }

    #[tokio::test]
    async fn a_bot_wall_or_script_shell_is_re_read_once_and_labelled() {
        let rig = fixture::rig(ON, "good").await;
        let page = rig.fetch("wall").await.unwrap();
        assert_eq!(page["via"], "firecrawl");
        assert_eq!(page["url"], "http://site.fixture/wall"); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        assert_eq!(page["text"], "# Rendered\nFallback body");
        assert_eq!(page["links"], json!([]), "a fallback read never widens a crawl");
        assert_eq!(rig.fetch("shell").await.unwrap()["via"], "firecrawl");
        assert_eq!(rig.hits(), 2);
        // A readable page and a real 404 stay with the pinned fetch.
        assert!(rig.fetch("ok").await.unwrap().get("via").is_none());
        let missing = rig.fetch("missing").await.unwrap_err();
        assert_eq!(missing.code, "WEB_FETCH_FAILED");
        assert!(missing.details.get("fallback").is_none());
        assert_eq!(rig.hits(), 2);
        // A batch of exact reads uses the same fallback.
        let batch = rig
            .web
            .fetch_many(&["http://site.fixture/wall".into()], None, true, &rig.audit, "local") // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .unwrap();
        assert_eq!(batch["pages"][0]["via"], "firecrawl");
        let audit = rig.audit_text();
        assert_eq!(audit.matches("\"event\":\"web_fetch_fallback\"").count(), 3);
        assert!(
            !audit.contains("site.fixture") && !audit.contains("good"),
            "audit lines carry codes, never the URL or key"
        );
    }

    #[tokio::test]
    async fn a_redirect_the_cap_a_quota_or_a_missing_key_keep_the_original_failure() {
        let rig = fixture::rig(ON, "good").await;
        let moved = rig.fetch("moves").await.unwrap_err();
        assert_eq!(moved.code, "WEB_FETCH_FAILED");
        assert_eq!(moved.details["fallback"], "WEB_REDIRECT_REFUSED");
        for _ in 0..3 {
            rig.fetch("wall").await.unwrap();
        }
        let capped = rig.fetch("wall").await.unwrap_err();
        assert_eq!(capped.details["fallback"], "WEB_FALLBACK_CAP");
        assert_eq!(rig.hits(), 4, "the capped read sent nothing");

        let spent = fixture::rig(ON, "quota").await;
        assert_eq!(
            spent.fetch("wall").await.unwrap_err().details["fallback"],
            "WEB_FALLBACK_QUOTA"
        );
        assert_eq!(
            spent.fetch("wall").await.unwrap_err().details["fallback"],
            "WEB_FALLBACK_PAUSED"
        );
        assert_eq!(spent.hits(), 1, "a paused Firecrawl is not asked again");

        let rejected = fixture::rig(ON, "wrong").await;
        assert_eq!(
            rejected.fetch("wall").await.unwrap_err().details["fallback"],
            "WEB_FALLBACK_KEY_REJECTED"
        );
        assert!(
            !rejected.audit_text().contains("web_search_cooldown"),
            "a bad key never pauses"
        );

        let keyless = fixture::rig(ON, "").await;
        assert_eq!(
            keyless.fetch("wall").await.unwrap_err().details["fallback"],
            "WEB_FALLBACK_KEY_MISSING"
        );
        assert_eq!(keyless.hits(), 0);
        // A thin page whose fallback fails is still returned as the pinned read.
        assert!(keyless.fetch("shell").await.unwrap().get("via").is_none());
    }

    #[tokio::test]
    async fn a_rule_revoked_while_the_fallback_waits_is_never_sent() {
        let rig = fixture::rig(
            "web: {concurrency: 1, firecrawl: {enabled: true, max_per_hour: 4}}",
            "good",
        )
        .await;
        let web = rig.web.clone();
        let cfg = AppConfig::from_str("{}", std::path::Path::new("config.yaml")).unwrap();
        let audit = Audit::new(rig.dir.path().join("audit-race.jsonl"), &cfg);
        let read = tokio::spawn(async move {
            web.fetch("http://site.fixture/gate", true, &audit, "local") // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                .await
        });
        // The pinned read holds the only permit inside the site handler. Queue for
        // that permit first (a first poll registers the waiter), then let it finish.
        rig.gate_hit.notified().await;
        let held = rig.web.permits.clone().acquire_owned();
        tokio::pin!(held);
        tokio::select! {
            biased;
            _ = &mut held => unreachable!("the pinned read holds the only permit"),
            _ = tokio::task::yield_now() => {}
        }
        rig.gate_go.notify_one();
        let held = held.await.unwrap();
        // The fallback has authorized the URL and taken its hourly slot; it now waits
        // for the permit. Revoke the rule, then let it run.
        while rig.web.firecrawl_spent.0.lock().unwrap().1 == 0 {
            tokio::task::yield_now().await;
        }
        rig.web.deny("http://site.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        drop(held);
        let failure = read.await.unwrap().unwrap_err();
        assert_eq!(failure.code, "WEB_FETCH_FAILED");
        assert!(
            matches!(
                failure.details["fallback"].as_str(),
                Some("WEB_HOST_DENIED" | "WEB_ALLOWLIST_EMPTY" | "WEB_POLICY_CHANGED")
            ),
            "the fallback stopped at the permission recheck"
        );
        assert!(failure.details.get("charged_bytes").is_none(), "nothing was sent");
        assert_eq!(rig.hits(), 0, "a revoked URL never reaches Firecrawl");
    }

    #[tokio::test]
    async fn web_disable_stops_queued_direct_and_fallback_reads() {
        for fallback in [false, true] {
            let mut rig = fixture::rig(
                "web: {concurrency: 1, firecrawl: {enabled: true, max_per_hour: 4}}",
                "good",
            )
            .await;
            let settings = std::sync::Arc::new(std::sync::Mutex::new(crate::common::home::HarnessSettings::default()));
            rig.web.settings = Some(settings.clone());
            let web = rig.web.clone();
            let cfg = AppConfig::from_str("{}", std::path::Path::new("config.yaml")).unwrap();
            let audit = Audit::new(rig.dir.path().join("audit-disable.jsonl"), &cfg);
            let held = if fallback {
                None
            } else {
                Some(rig.web.permits.clone().acquire_owned().await.unwrap())
            };
            let read = web.fetch("http://site.fixture/gate", true, &audit, "local"); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            tokio::pin!(read);
            // Polling registers the direct read on the held permit, or starts the pinned request.
            tokio::select! {
                biased;
                _ = &mut read => panic!("read must wait"),
                _ = tokio::task::yield_now() => {}
            }
            if fallback {
                let task = async {
                    rig.gate_hit.notified().await;
                    let held = rig.web.permits.clone().acquire_owned();
                    tokio::pin!(held);
                    tokio::select! {
                        biased;
                        _ = &mut held => unreachable!("pinned read holds permit"),
                        _ = tokio::task::yield_now() => {}
                    }
                    rig.gate_go.notify_one();
                    let held = held.await.unwrap();
                    while rig.web.firecrawl_spent.0.lock().unwrap().1 == 0 {
                        tokio::task::yield_now().await;
                    }
                    settings.lock().unwrap().web_enabled = false;
                    drop(held);
                };
                let (result, ()) = tokio::join!(read, task);
                assert_eq!(result.unwrap_err().code, "WEB_DISABLED");
            } else {
                settings.lock().unwrap().web_enabled = false;
                drop(held);
                assert_eq!(read.await.unwrap_err().code, "WEB_DISABLED");
                assert!(tokio::time::timeout(Duration::from_millis(10), rig.gate_hit.notified())
                    .await
                    .is_err());
            }
            assert_eq!(rig.hits(), 0, "disabled reads must never reach the provider");
            assert!(!rig.dir.path().join("tools/web_cache").exists());
        }
    }

    #[tokio::test]
    async fn a_key_cleared_while_the_fallback_waits_is_never_sent() {
        let rig = fixture::rig(
            "web: {concurrency: 1, firecrawl: {enabled: true, max_per_hour: 4}}",
            "good",
        )
        .await;
        let web = rig.web.clone();
        let cfg = AppConfig::from_str("{}", std::path::Path::new("config.yaml")).unwrap();
        let audit = Audit::new(rig.dir.path().join("audit-race.jsonl"), &cfg);
        let read = tokio::spawn(async move {
            web.fetch("http://site.fixture/gate", true, &audit, "local") // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                .await
        });
        // Same queueing as the revoked-rule case: the fallback reads its key, then
        // waits for the only permit while the key is cleared.
        rig.gate_hit.notified().await;
        let held = rig.web.permits.clone().acquire_owned();
        tokio::pin!(held);
        tokio::select! {
            biased;
            _ = &mut held => unreachable!("the pinned read holds the only permit"),
            _ = tokio::task::yield_now() => {}
        }
        rig.gate_go.notify_one();
        let held = held.await.unwrap();
        while rig.web.firecrawl_spent.0.lock().unwrap().1 == 0 {
            tokio::task::yield_now().await;
        }
        super::super::env_keys::update_keys(
            &rig.dir.path().join(".env"),
            &std::collections::BTreeMap::new(),
            &["FIRECRAWL_API_KEY".to_string()],
        )
        .unwrap();
        drop(held);
        let failure = read.await.unwrap().unwrap_err();
        assert_eq!(failure.code, "WEB_FETCH_FAILED");
        assert_eq!(failure.details["fallback"], "WEB_FALLBACK_KEY_CHANGED");
        assert!(failure.details.get("charged_bytes").is_none(), "nothing was sent");
        assert_eq!(rig.hits(), 0, "a cleared key never reaches Firecrawl");
    }

    /// Operator decision, 2026-11-14: a PDF answer may use `web.pdf_response_bytes`
    /// (arXiv papers); HTML and text keep `web.response_bytes`.
    #[tokio::test]
    async fn a_pdf_has_its_own_body_cap_and_html_does_not() {
        let rig = fixture::rig("web: {response_bytes: 4096}", "").await;
        let read = rig.fetch("big.pdf").await.unwrap();
        assert!(read["bytes"].as_u64().unwrap() > 4096, "{read}");
        assert!(read["text"].as_str().unwrap().contains("BIG_PDF_FACT line 39"));
        assert_eq!(rig.fetch("huge").await.unwrap_err().code, "WEB_TOO_LARGE");

        let tight = fixture::rig("web: {response_bytes: 4096, pdf_response_bytes: 4096}", "").await;
        let refused = tight.fetch("big.pdf").await.unwrap_err();
        assert_eq!(refused.code, "WEB_TOO_LARGE");
        assert_eq!(refused.details["pdf"], true);
    }

    /// In a budgeted batch a PDF is read only up to what `web.run_bytes` has left,
    /// and a failure past `web.response_bytes` charges what actually arrived.
    #[tokio::test]
    async fn a_batch_reads_a_pdf_within_the_run_budget_and_charges_what_arrived() {
        let url = |p: &str| vec![format!("http://site.fixture/{p}")]; // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let fitted = fixture::rig("web: {response_bytes: 4096, run_bytes: 4096}", "").await;
        let batch = fitted
            .web
            .fetch_many(&url("big.pdf"), None, true, &fitted.audit, "local")
            .await
            .unwrap();
        assert_eq!(batch["coverage"]["failed"][0]["code"], "WEB_TOO_LARGE", "{batch}");
        assert!(batch["coverage"]["charged_bytes"].as_u64().unwrap() <= 4096);

        let roomy = fixture::rig("web: {response_bytes: 4096, run_bytes: 1048576}", "").await;
        let batch = roomy
            .web
            .fetch_many(&url("big.pdf"), None, true, &roomy.audit, "local")
            .await
            .unwrap();
        let page = &batch["pages"][0];
        assert!(page["text"].as_str().unwrap().contains("BIG_PDF_FACT"), "{batch}");
        assert_eq!(batch["coverage"]["charged_bytes"], page["transfer_bytes"]);
        assert!(page["transfer_bytes"].as_u64().unwrap() > 4096);

        let junk = "%PDF-1.7\n".len() + "junk ".len() * 2000;
        let batch = roomy
            .web
            .fetch_many(&url("junk.pdf"), None, true, &roomy.audit, "local")
            .await
            .unwrap();
        assert_eq!(batch["coverage"]["failed"][0]["code"], "WEB_NOT_TEXT", "{batch}");
        assert_eq!(batch["coverage"]["charged_bytes"], junk, "not the 4096-byte allowance");
    }

    #[tokio::test]
    async fn a_batch_charges_both_reads_and_skips_a_fallback_its_budget_cannot_cover() {
        let shell_bytes = "<html><body><div id=app></div><script></script></body></html>".len() + 4000;
        let shell = vec!["http://site.fixture/shell".to_string()]; // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let wall = vec!["http://site.fixture/wall".to_string()]; // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let roomy = "web: {response_bytes: 65536, run_bytes: 1048576, firecrawl: {enabled: true, max_per_hour: 4}}";

        let rig = fixture::rig(roomy, "good").await;
        let batch = rig
            .web
            .fetch_many(&shell, None, true, &rig.audit, "local")
            .await
            .unwrap();
        assert_eq!(batch["pages"][0]["via"], "firecrawl");
        let charged = batch["coverage"]["charged_bytes"].as_u64().unwrap() as usize;
        assert_eq!(charged, batch["pages"][0]["transfer_bytes"].as_u64().unwrap() as usize);
        assert!(
            charged > shell_bytes && charged < shell_bytes + 1024,
            "the pinned shell and the Firecrawl answer are both charged"
        );

        // A fallback refused after it was sent charges a full allowance on top of the
        // failed pinned read.
        let spent = fixture::rig(roomy, "quota").await;
        let batch = spent
            .web
            .fetch_many(&wall, None, true, &spent.audit, "local")
            .await
            .unwrap();
        assert_eq!(batch["coverage"]["failed"][0]["code"], "WEB_FETCH_FAILED");
        assert_eq!(batch["coverage"]["charged_bytes"], 2 * 65536);

        // One allowance plus the shell: room for the pinned read, not for a second one.
        let tight = format!(
            "web: {{response_bytes: 65536, run_bytes: {}, firecrawl: {{enabled: true, max_per_hour: 4}}}}",
            65536 + 2048
        );
        let rig = fixture::rig(&tight, "good").await;
        let batch = rig
            .web
            .fetch_many(&shell, None, true, &rig.audit, "local")
            .await
            .unwrap();
        assert!(batch["pages"][0].get("via").is_none(), "the pinned page is kept");
        assert_eq!(batch["coverage"]["charged_bytes"], shell_bytes);
        assert_eq!(rig.hits(), 0, "a fallback the budget cannot cover is never sent");
        assert!(rig.audit_text().contains("WEB_FALLBACK_BUDGET"));
    }

    #[test]
    fn the_hourly_cap_counts_per_clock_hour() {
        let cap = HourlyCap::default();
        let hour = 500_000 * 3600;
        assert!(cap.take(hour + 1, 2));
        assert!(cap.take(hour + 2, 2));
        assert!(!cap.take(hour + 3, 2), "the third read in the hour is refused");
        assert!(cap.take(hour + 3600, 2), "a new hour starts empty");
    }

    #[test]
    fn config_ships_off_and_refuses_a_bad_cap() {
        let cfg = |yaml: &str| AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap();
        assert_eq!(
            FirecrawlConfig::load(&cfg("{}")).unwrap(),
            FirecrawlConfig {
                enabled: false,
                max_per_hour: 20
            }
        );
        let shipped = FirecrawlConfig::load(&cfg(include_str!("../../assets/config.default.yaml"))).unwrap();
        assert!(!shipped.enabled, "the shipped fallback is off");
        assert!(
            !FirecrawlConfig::load(&cfg("web: {firecrawl: {enabled: 'true'}}"))
                .unwrap()
                .enabled,
            "quoted true stays off"
        );
        for bad in ["0", "1001", "-1"] {
            assert_eq!(
                FirecrawlConfig::load(&cfg(&format!("web: {{firecrawl: {{max_per_hour: {bad}}}}}")))
                    .unwrap_err()
                    .code,
                "CONFIG_ERROR",
                "{bad}"
            );
        }
    }
}
