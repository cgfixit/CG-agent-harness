//! Google result listings, distinct from permission to read linked pages.
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use scraper::Selector;
use serde::Serialize;
use serde_json::{json, Value};

use super::web_policy::{canonical_url, error};
use super::web_provider::{falls_through, hop_budget, plan_hops, timed_hops, Hop, SearchProvider};
use super::web_search::{parse_html, run_web_cpu, WebTool};
use crate::common::audit::Audit;
use crate::common::credential_store::CredentialStore;
use crate::common::errors::Result;

/// Public Google answers bursts with challenges: chat searches there are at
/// least this far apart whatever `web.pace_ms` says. SerpAPI is never paced.
pub(crate) const PUBLIC_SEARCH_PACE_FLOOR_MS: u64 = 3000;

/// When the next public Google request may leave, or `WEB_SEARCH_PACED` when
/// that is not before the call's own deadline (never block past it).
pub(super) fn public_pace(
    last: Option<tokio::time::Instant>,
    pace_ms: u64,
    deadline: tokio::time::Instant,
) -> Result<Option<tokio::time::Instant>> {
    let Some(last) = last else {
        return Ok(None);
    };
    let ready = last + Duration::from_millis(pace_ms.max(PUBLIC_SEARCH_PACE_FLOOR_MS));
    if ready >= deadline {
        return Err(error(
            "WEB_SEARCH_PACED",
            "public Google searches are paced and the next one cannot start within web.request_seconds; do not retry web_search this turn",
        ));
    }
    Ok(Some(ready))
}

/// The notice on every search listing.
pub(crate) const SEARCH_NOTICE: &str =
    "Search-provider listings only. Linked pages have not been fetched and require their own URL permission.";

fn blocked() -> crate::common::errors::HarnessError {
    error(
        "WEB_GOOGLE_BLOCKED",
        "Public Google refused or redirected the request; configure a SerpAPI key in API Keys",
    )
}

/// A provider paused after it refused us, and why.
#[derive(Debug, Clone)]
pub(super) struct Pause {
    ends: Instant,
    until_unix: u64,
    code: String,
}

/// Per-provider pause after a refusal, so a `/loop` stops asking a provider that
/// will refuse again. In memory only: a restart clears it.
#[derive(Debug, Default)]
pub(crate) struct SearchCooldown(Mutex<HashMap<&'static str, Pause>>);

/// How long a refusal pauses its provider; `None` keeps the provider loud. A
/// rejected key is the operator's error and must fail on every search.
fn cooldown_seconds(provider: &str, code: &str, now_unix: u64, public_sec: u64, quota_sec: u64) -> Option<u64> {
    match (provider, code) {
        ("google-public", "WEB_GOOGLE_CHALLENGE" | "WEB_GOOGLE_BLOCKED") => Some(public_sec),
        // Keyed providers cap searches per window (SerpAPI per clock hour): 0 waits
        // for the next hour boundary.
        ("google-serpapi" | "tavily", "WEB_SEARCH_QUOTA") | ("firecrawl", "WEB_FALLBACK_QUOTA") if quota_sec == 0 => {
            Some(3600 - now_unix % 3600)
        }
        ("google-serpapi" | "tavily", "WEB_SEARCH_QUOTA") | ("firecrawl", "WEB_FALLBACK_QUOTA") => Some(quota_sec),
        _ => None,
    }
}

impl SearchCooldown {
    pub(super) fn holding(&self, provider: &str, now: Instant) -> Option<Pause> {
        let mut paused = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match paused.get(provider) {
            Some(pause) if pause.ends > now => Some(pause.clone()),
            Some(_) => {
                paused.remove(provider);
                None
            }
            None => None,
        }
    }

    /// Pause `provider` when `code` calls for it; returns the wall-clock end.
    pub(super) fn start(
        &self,
        provider: &'static str,
        code: &str,
        limits: &super::web_search::Limits,
        now: Instant,
        now_unix: u64,
    ) -> Option<u64> {
        let seconds = cooldown_seconds(
            provider,
            code,
            now_unix,
            limits.search_cooldown_public_sec,
            limits.search_cooldown_quota_sec,
        )?;
        let until_unix = now_unix + seconds;
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(
            provider,
            Pause {
                ends: now + Duration::from_secs(seconds),
                until_unix,
                code: code.into(),
            },
        );
        Some(until_unix)
    }
}

pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `HH:MM` in the server's local time zone.
#[cfg(unix)]
fn local_hhmm(unix: u64) -> String {
    let at = unix as libc::time_t;
    // SAFETY: an all-zero `tm` is valid, and localtime_r writes only into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&at, &mut tm) }.is_null() {
        return format!("{:02}:{:02} UTC", unix / 3600 % 24, unix / 60 % 60);
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

#[cfg(not(unix))]
fn local_hhmm(unix: u64) -> String {
    format!("{:02}:{:02} UTC", unix / 3600 % 24, unix / 60 % 60)
}

pub(super) fn cooldown_error(provider: &str, pause: &Pause) -> crate::common::errors::HarnessError {
    let (label, hint) = match provider {
        "google-public" => ("Public Google", "; a SerpAPI key in API Keys searches sooner"),
        "tavily" => ("Tavily", ""),
        _ => ("SerpAPI", ""),
    };
    error(
        "WEB_SEARCH_COOLDOWN",
        &format!(
            "{label} search is cooling down until {} after {}; do not retry web_search before then{hint}",
            local_hhmm(pause.until_unix),
            pause.code
        ),
    )
}

#[derive(Debug, Serialize)]
pub(super) struct SearchResult {
    pub(super) rank: usize,
    pub(super) title: String,
    pub(super) url: String,
    pub(super) snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provider_rank: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provider_score: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) highlights: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_date: Option<String>,
}

pub(super) fn add_result(
    rows: &mut Vec<SearchResult>,
    seen: &mut BTreeSet<String>,
    title: &str,
    raw: &str,
    snippet: &str,
    limit: usize,
) {
    if rows.len() >= limit || title.trim().is_empty() {
        return;
    }
    let Ok(url) = canonical_url(raw) else {
        return;
    };
    if !seen.insert(url.to_string()) {
        return;
    }
    rows.push(SearchResult {
        rank: rows.len() + 1,
        title: crate::common::clip_chars(title.trim(), 300),
        url: url.to_string(),
        snippet: crate::common::clip_chars(snippet.trim(), 1000),
        provider_rank: None,
        provider_score: None,
        highlights: Vec::new(),
        source_date: None,
    });
}

fn api_results(body: &[u8], limit: usize) -> Result<Vec<SearchResult>> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| error("WEB_SEARCH_RESPONSE", "invalid search response"))?;
    if value.get("error").is_some() {
        return Err(error("WEB_SEARCH_PROVIDER", "search provider returned an error"));
    }
    let items = value.get("organic_results").and_then(Value::as_array);
    if items.is_none() && value["search_information"]["organic_results_state"] != "Fully empty" {
        return Err(error("WEB_SEARCH_RESPONSE", "search result listing is missing"));
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for item in items.into_iter().flatten().take(100) {
        let before = rows.len();
        add_result(
            &mut rows,
            &mut seen,
            item["title"].as_str().unwrap_or(""),
            item["link"].as_str().unwrap_or(""),
            item["snippet"].as_str().unwrap_or(""),
            limit,
        );
        if rows.len() > before {
            let row = rows.last_mut().unwrap();
            row.provider_rank = item["position"].as_u64().filter(|n| *n > 0);
            row.highlights = item["snippet_highlighted_words"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(8)
                .map(|s| crate::common::clip_chars(s, 80))
                .collect();
            row.source_date = item["date"].as_str().map(|s| crate::common::clip_chars(s, 80));
        }
    }
    Ok(rows)
}

/// Provider `site:` echoes are hints; enforce recognizable host/path constraints locally.
fn filter_sites(query: &str, value: &mut Value) {
    let constraints: Vec<_> = query
        .split_whitespace()
        .filter_map(|word| {
            let (excluded, word) = (word.starts_with('-'), word.trim_start_matches('-'));
            let raw = word.strip_prefix("site:")?.trim_matches(['\"', '\'']);
            let raw = raw
                .strip_prefix("https://")
                .or_else(|| raw.strip_prefix("http://"))
                .unwrap_or(raw);
            let url = canonical_url(&format!("https://{raw}")).ok()?;
            Some((excluded, url))
        })
        .collect();
    if constraints.is_empty() {
        return;
    }
    if let Some(rows) = value["results"].as_array_mut() {
        let before = rows.len();
        rows.retain(|row| {
            let Some(url) = row["url"].as_str().and_then(|s| url::Url::parse(s).ok()) else {
                return false;
            };
            let matches = |site: &url::Url| {
                let host = url.host_str().unwrap_or("");
                let wanted = site.host_str().unwrap_or("");
                (host == wanted || host.ends_with(&format!(".{wanted}"))) && url.path().starts_with(site.path())
            };
            !constraints.iter().any(|(excluded, site)| *excluded && matches(site))
                && (!constraints.iter().any(|(excluded, _)| !excluded)
                    || constraints.iter().any(|(excluded, site)| !excluded && matches(site)))
        });
        let removed = before - rows.len();
        for (index, row) in rows.iter_mut().enumerate() {
            row["rank"] = json!(index + 1);
        }
        if removed > 0 {
            value["filtered_results"] = json!(removed);
        }
    }
}

fn public_results(
    body: &str,
    limit: usize,
    max_handles: usize,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Vec<SearchResult>> {
    checkpoint()?;
    let lower = body.to_ascii_lowercase();
    if [
        "/httpservice/retry/enablejs",
        "unusual traffic",
        "g-recaptcha",
        "consent.google.com",
        "enable javascript to continue",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Err(error(
            "WEB_GOOGLE_CHALLENGE",
            "Google requires browser interaction; configure a SerpAPI key in API Keys",
        ));
    }
    let html = parse_html(body, max_handles, checkpoint)?;
    let anchors = Selector::parse("a[href]").unwrap();
    let headings = Selector::parse("h3").unwrap();
    let base = url::Url::parse("https://www.google.com/").unwrap();
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in html.select(&anchors) {
        checkpoint()?;
        let Some(heading) = anchor.select(&headings).next() else {
            continue;
        };
        let Some(raw) = anchor.value().attr("href") else {
            continue;
        };
        let Ok(mut url) = base.join(raw) else {
            continue;
        };
        if url.host_str() == Some("www.google.com") && url.path() == "/url" {
            let target = url
                .query_pairs()
                .find(|(k, _)| matches!(k.as_ref(), "q" | "url"))
                .map(|(_, v)| v.into_owned());
            let Some(target) = target.and_then(|v| canonical_url(&v).ok()) else {
                continue;
            };
            url = target;
        }
        if matches!(url.host_str(), Some("www.google.com" | "google.com")) {
            continue;
        }
        let title = heading.text().collect::<Vec<_>>().join(" ");
        add_result(&mut rows, &mut seen, &title, url.as_str(), "", limit);
    }
    if rows.is_empty() && !lower.contains("did not match any documents") {
        return Err(error(
            "WEB_GOOGLE_UNREADABLE",
            "Google returned no recognizable listing; configure a SerpAPI key in API Keys",
        ));
    }
    checkpoint()?;
    Ok(rows)
}

impl WebTool {
    /// Read the saved SerpAPI key on demand. An explicit process environment still wins
    /// unless this process loaded the saved value itself (`search_key_from_file`).
    pub(super) fn search_key(&self) -> Result<String> {
        self.provider_key("SERPAPI_API_KEY", self.search_key_from_file)
    }

    /// Read the saved Tavily key on demand, under the same precedence as SerpAPI's.
    pub(super) fn tavily_key(&self) -> Result<String> {
        self.provider_key("TAVILY_API_KEY", self.tavily_key_from_file)
    }

    pub(super) fn provider_key(&self, name: &str, loaded_from_file: bool) -> Result<String> {
        if self.plaintext_key_file {
            if !loaded_from_file {
                if let Ok(key) = std::env::var(name) {
                    return Ok(key.trim().to_string());
                }
            }
            let path = self.tools_dir.parent().unwrap_or(&self.tools_dir).join(".env");
            return super::env_keys::read_startup_keys(&path)
                .map(|mut keys| keys.remove(name).unwrap_or_default())
                .map_err(|_| {
                    error(
                        "WEB_SEARCH_KEY_UNAVAILABLE",
                        "Cannot read the saved search key; check API Keys and credential-file permissions",
                    )
                });
        }
        if !loaded_from_file {
            if let Ok(key) = std::env::var(name) {
                return Ok(key.trim().to_string());
            }
        }
        let home = self.tools_dir.parent().unwrap_or(&self.tools_dir);
        match crate::common::credential_store::OsCredentialStore::for_home(home).get(name) {
            Ok(value) => Ok(value.unwrap_or_default()),
            Err(crate::common::credential_store::StoreError::Unavailable) => Ok(String::new()),
            Err(_) => Err(error(
                "WEB_SEARCH_KEY_UNAVAILABLE",
                "Cannot read the saved search key from the OS credential store",
            )),
        }
    }

    /// Time held back for a final public Google hop: the pacing wait still due
    /// before its request may leave (none when the last public search is older
    /// than the pace), plus one second for the request. Before, the whole pace
    /// (at least 3 s) was held back on every search, which halved a keyed
    /// provider's share of the shipped 8 s deadline.
    pub(super) fn public_search_reserve(&self) -> Duration {
        let pace = Duration::from_millis(self.limits.pace_ms.max(PUBLIC_SEARCH_PACE_FLOOR_MS));
        let last = *self.public_search_sent.lock().unwrap_or_else(|p| p.into_inner());
        let due = last.map_or(Duration::ZERO, |sent| {
            (sent + pace).saturating_duration_since(tokio::time::Instant::now())
        });
        due + Duration::from_secs(1)
    }

    /// `web.search.provider` picks the backend; `auto` lets a key choose the fixed
    /// SerpAPI Google backend. A key is sent only to its own provider's fixed
    /// endpoint, never to Google, another provider or a returned result
    /// URL. Fallback hops (`web.search.fallback_*`): public Google ships on,
    /// SerpAPI off; see `plan_hops`.
    pub async fn google_search(&self, query: &str, count: usize, enabled: bool, audit: &Audit) -> Result<Value> {
        self.google_search_at(
            query,
            count,
            enabled,
            audit,
            url::Url::parse("https://serpapi.com/search.json").unwrap(),
        )
        .await
    }

    // Only the fixed endpoint above is reachable in production; fixtures supply a loopback server.
    async fn google_search_at(
        &self,
        query: &str,
        count: usize,
        enabled: bool,
        audit: &Audit,
        endpoint: url::Url,
    ) -> Result<Value> {
        if query.trim().is_empty() || query.chars().count() > 200 || !(1..=10).contains(&count) {
            return Err(error(
                "WEB_BAD_QUERY",
                "search needs a query of 1–200 characters and 1–10 results",
            ));
        }
        if !enabled {
            return Err(error("WEB_DISABLED", "web access is disabled"));
        }
        // The saved key is read only when a SerpAPI hop could run.
        let reads_key = match &self.search_provider {
            SearchProvider::GooglePublic => false,
            SearchProvider::Tavily => self.search_fallback.to_serpapi,
            SearchProvider::Auto | SearchProvider::Serpapi => true,
        };
        let key = if reads_key { self.search_key()? } else { String::new() };
        let keyed = !key.trim().is_empty();
        if self.search_provider == SearchProvider::Serpapi && !keyed {
            return Err(error(
                "WEB_SEARCH_KEY_MISSING",
                "web.search.provider is serpapi but no SerpAPI key is active; add one in API Keys",
            ));
        }
        // A forced Tavily provider without its key is the operator's error: loud, no
        // fallback. `auto` reads the key too: when one is saved it is searched first.
        let tavily_key = match &self.search_provider {
            SearchProvider::Tavily => {
                let tavily = self.tavily_key()?;
                if tavily.trim().is_empty() {
                    return Err(error(
                        "WEB_SEARCH_KEY_MISSING",
                        "web.search.provider is tavily but no Tavily key is active; add one in API Keys",
                    ));
                }
                tavily
            }
            SearchProvider::Auto => self.tavily_key()?,
            SearchProvider::Serpapi | SearchProvider::GooglePublic => String::new(),
        };
        let tavily_keyed = !tavily_key.trim().is_empty();
        let hops = plan_hops(&self.search_provider, keyed, tavily_keyed, &self.search_fallback);
        // One broker decision, one search gate and one deadline for the whole chain:
        // fallback never makes a search longer or more concurrent than one search.
        self.gate_tool("web_search", &[query.into()], enabled, audit)?;
        let _gate = self
            .search_gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("WEB_BUSY", "a web search is already running"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.limits.request_seconds);
        let mut fallback_from: Vec<&str> = Vec::new();
        for (index, hop) in hops.iter().copied().enumerate() {
            let next = hops.get(index + 1).copied();
            let started = tokio::time::Instant::now();
            // A later hop that is cooling down answers without a request: it
            // takes no share. The public reserve is the pacing wait still due.
            let timed = timed_hops(&hops[index..], |hop| {
                self.search_cooldown.holding(hop.name(), Instant::now()).is_some()
            });
            let budget = hop_budget(
                deadline.saturating_duration_since(started),
                &timed,
                self.public_search_reserve(),
            );
            if budget.is_zero() {
                audit
                    .log(json!({"event":"web_search_attempt","provider":hop.name(),"code":"exhausted_before_attempt"}));
                return Err(error(
                    "WEB_TIMEOUT",
                    "search deadline exceeded before the next provider",
                ));
            }
            let hop_deadline = started + budget;
            if hop == Hop::Serpapi && !fallback_from.is_empty() {
                if let Some(count) = self
                    .search_fallback_burst
                    .note(unix_now(), self.search_fallback.alert_per_hour)
                {
                    audit.log(json!({"event":"web_search_fallback_burst","provider":hop.name(),
                        "count":count,"threshold":self.search_fallback.alert_per_hour}));
                }
            }
            let expected_key = match hop {
                Hop::Serpapi => Some(key.clone()),
                // Keyless auto: a key saved mid-search still discards public listings.
                Hop::GooglePublic if self.search_provider == SearchProvider::Auto && !keyed => Some(String::new()),
                _ => None,
            };
            let attempt = async {
                match (hop, &self.search_provider) {
                    (Hop::Tavily, _) => {
                        self.tavily_listing(query, count, audit, self.tavily_endpoint(), tavily_key.trim())
                            .await
                    }
                    (Hop::Serpapi, _) => {
                        self.google_hop(
                            query,
                            count,
                            enabled,
                            audit,
                            endpoint.clone(),
                            key.trim(),
                            expected_key,
                            hop_deadline,
                        )
                        .await
                    }
                    (Hop::GooglePublic, _) => {
                        let listing = self
                            .google_hop(
                                query,
                                count,
                                enabled,
                                audit,
                                endpoint.clone(),
                                "",
                                expected_key,
                                hop_deadline,
                            )
                            .await?;
                        // Keyless auto: a Tavily key saved mid-search discards the
                        // public listing too, as a SerpAPI key does (`expected_key`).
                        if self.search_provider == SearchProvider::Auto
                            && !tavily_keyed
                            && !self.tavily_key()?.trim().is_empty()
                        {
                            return Err(error("WEB_SEARCH_KEY_CHANGED", "Search key changed; retry the search"));
                        }
                        Ok(listing)
                    }
                }
            };
            let result = tokio::time::timeout_at(hop_deadline, attempt)
                .await
                .map_err(|_| error("WEB_TIMEOUT", "search deadline exceeded"))
                .and_then(|value| value);
            audit.log(json!({"event":"web_search_attempt","provider":hop.name(),
                "budget_ms":budget.as_millis(),"elapsed_ms":started.elapsed().as_millis(),
                "code":result.as_ref().map_or_else(|e| e.code.as_str(), |_| "OK")}));
            match result {
                Ok(mut value) => {
                    filter_sites(query, &mut value);
                    if !fallback_from.is_empty() {
                        value["fallback_from"] = json!(fallback_from);
                    }
                    return Ok(value);
                }
                Err(failure) => match next {
                    Some(next) if falls_through(hop, &failure.code) => {
                        // Codes only: never the query, a key or provider text.
                        audit.log(json!({"event":"web_search_fallback","from":hop.name(),
                            "to":next.name(),"code":failure.code}));
                        fallback_from.push(hop.name());
                    }
                    _ => return Err(failure),
                },
            }
        }
        Err(error("WEB_SEARCH_PROVIDER", "no search provider is available"))
    }

    /// One Google hop: SerpAPI when `key` is non-empty, else public Google (URL
    /// policy, cooldown and pacing apply). `expected_key` rechecks the saved key.
    #[allow(clippy::too_many_arguments)]
    async fn google_hop(
        &self,
        query: &str,
        count: usize,
        enabled: bool,
        audit: &Audit,
        endpoint: url::Url,
        key: &str,
        expected_key: Option<String>,
        deadline: tokio::time::Instant,
    ) -> Result<Value> {
        // Provider listings require web/account authority, not page-content permission.
        // Public Google is a page fetch and retains the original URL policy checks.
        let policy = if key.is_empty() {
            Some(self.require_enabled(enabled).map_err(|e| {
                if e.code == "WEB_ALLOWLIST_EMPTY" {
                    error(
                        "WEB_ALLOWLIST_EMPTY",
                        "no SerpAPI key is configured and no URL rule permits public Google; add a SerpAPI key in API Keys (no URL rule needed) or /web allow https://www.google.com/*",
                    )
                } else {
                    e
                }
            })?)
        } else {
            None
        };
        let bases = match self.public_search_fixture() {
            Some(fixture) => vec![fixture],
            None => ["https://www.google.com/search", "https://google.com/search"]
                .into_iter()
                .map(|base| url::Url::parse(base).unwrap())
                .collect(),
        };
        let target = bases
            .into_iter()
            .find_map(|mut url| {
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("num", &count.to_string());
                match &policy {
                    Some(policy) => policy.authorize(url.as_str(), None).ok(),
                    None => Some(url),
                }
            })
            .ok_or_else(|| {
                error(
                    "WEB_GOOGLE_PERMISSION",
                    "allow https://www.google.com/* or the exact Google search URL first",
                )
            })?;
        let provider = if key.trim().is_empty() {
            "google-public"
        } else {
            "google-serpapi"
        };
        // A paused provider answers before the broker, the gate and any request.
        if let Some(pause) = self.search_cooldown.holding(provider, Instant::now()) {
            return Err(cooldown_error(provider, &pause));
        }
        let fetched = if key.trim().is_empty() {
            tokio::time::timeout_at(deadline, async {
                let (_, body) = self
                    .get_raw_paced(
                        target.as_str(),
                        None,
                        None,
                        Some((self.limits.pace_ms, deadline, audit)),
                    )
                    .await
                    .map_err(|failure| match failure.code.as_str() {
                        // Google answered with a status or a redirect: its verdict, so it pauses.
                        "WEB_REDIRECT_REFUSED" => blocked(),
                        "WEB_FETCH_FAILED" if failure.details.get("status").is_some() => blocked(),
                        // No answer (connect, TLS, a dropped body, local setup) is a provider
                        // failure, not a refusal: it never starts a pause.
                        "WEB_FETCH_FAILED" => error("WEB_SEARCH_PROVIDER", "public Google request failed"),
                        _ => failure,
                    })?;
                // Listings reparse HTML too, including text/plain responses.
                // Keep that work off async workers under the original deadline.
                let permit = self
                    .permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| error("WEB_CANCELLED", "search cancelled"))?;
                let max_handles = self.limits.html_parser_handles;
                run_web_cpu(deadline, permit, move |checkpoint| {
                    public_results(&body, count, max_handles, checkpoint)
                })
                .await
            })
            .await
            .map_err(|_| error("WEB_TIMEOUT", "request deadline exceeded"))
            .and_then(|rows| rows)
        } else {
            self.api_search(endpoint, query, count, key.trim()).await
        };
        let rows = match fetched {
            Ok(rows) => rows,
            Err(failure) => {
                // One audit line when a pause starts; none while it holds.
                if let Some(until) =
                    self.search_cooldown
                        .start(provider, &failure.code, &self.limits, Instant::now(), unix_now())
                {
                    audit.log(json!({"event":"web_search_cooldown","provider":provider,
                        "code":failure.code,"until":until}));
                }
                return Err(failure);
            }
        };
        if let Some(policy) = policy {
            let current = self.policy()?;
            current.authorize(target.as_str(), None)?;
            if current.revision != policy.revision {
                return Err(error(
                    "WEB_POLICY_CHANGED",
                    "search permission changed; results discarded",
                ));
            }
        }
        if let Some(expected) = expected_key {
            if self.search_key()? != expected {
                return Err(error("WEB_SEARCH_KEY_CHANGED", "Search key changed; retry the search"));
            }
        }
        Ok(
            json!({"query":query,"provider":provider,"search_url":target.as_str(),"results":rows,
            "notice":SEARCH_NOTICE,"complete":false}),
        )
    }
    // Only the fixed HTTPS endpoint above calls this in production. Tests use a
    // local fixture to verify credential isolation without sending a real key.
    async fn api_search(&self, endpoint: url::Url, query: &str, count: usize, key: &str) -> Result<Vec<SearchResult>> {
        tokio::time::timeout(Duration::from_secs(self.limits.request_seconds), async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| error("WEB_CANCELLED", "search cancelled"))?;
            let client = self.pinned_client(&endpoint).await?;
            let response = client
                .get(endpoint)
                .query(&[
                    ("engine", "google"),
                    ("q", query),
                    ("num", &count.to_string()),
                    ("api_key", key.trim()),
                ])
                .send()
                .await
                .map_err(|_| error("WEB_SEARCH_PROVIDER", "search provider request failed"))?;
            let code = match response.status().as_u16() {
                200 => None,
                401 | 403 => Some("WEB_SEARCH_KEY_REJECTED"),
                429 => Some("WEB_SEARCH_QUOTA"),
                _ => Some("WEB_SEARCH_PROVIDER"),
            };
            if let Some(code) = code {
                return Err(error(
                    code,
                    "search provider refused the request; check API Keys and provider quota",
                ));
            }
            let body = listing_body(response, self.limits.response_bytes).await?;
            let rows = api_results(&body, count)?;
            if !key.is_empty() && serde_json::to_string(&rows)?.contains(key) {
                return Err(error(
                    "WEB_SEARCH_RESPONSE",
                    "credential reflected in provider results; response discarded",
                ));
            }
            Ok(rows)
        })
        .await
        .map_err(|_| error("WEB_TIMEOUT", "search deadline exceeded"))?
    }
}

/// A provider listing body: bounded headers, no compression, at most `max` bytes.
pub(super) async fn listing_body(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if response
        .headers()
        .iter()
        .map(|(k, v)| k.as_str().len() + v.len())
        .sum::<usize>()
        > 16_384
    {
        return Err(error("WEB_HEADERS_TOO_LARGE", "search headers exceed limit"));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return Err(error("WEB_ENCODING_REFUSED", "compressed search response refused"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("WEB_SEARCH_PROVIDER", "search response read failed"))?
    {
        if body.len() + chunk.len() > max {
            return Err(error("WEB_TOO_LARGE", "search response exceeds limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_metadata_is_bounded_and_site_filters_check_actual_hosts() {
        let rows = api_results(json!({"organic_results":[{"title":"Guide","link":"https://docs.example.com/path","snippet":"matching result","position":7,"snippet_highlighted_words":["matching",null,7],"date":"2026-11-07"}]}).to_string().as_bytes(), 5).unwrap();
        assert_eq!(rows[0].provider_rank, Some(7));
        assert_eq!(rows[0].highlights, vec!["matching"]);
        assert_eq!(rows[0].source_date.as_deref(), Some("2026-11-07"));
        let mut value = json!({"results":[{"url":"https://other.test/","rank":1},{"url":"https://docs.example.com/path","rank":2,"provider_rank":7},{"url":"https://example.com.attacker.test/path","rank":3}]});
        filter_sites("manual site:http://example.com -site:other.test", &mut value);
        assert_eq!(value["results"].as_array().unwrap().len(), 1);
        assert_eq!(value["results"][0]["rank"], 1);
        assert_eq!(value["results"][0]["provider_rank"], 7);
        assert_eq!(value["filtered_results"], 2);
        filter_sites("manual site:other.test", &mut value);
        assert!(value["results"].as_array().unwrap().is_empty());
    }

    fn public_results(body: &str, limit: usize) -> Result<Vec<SearchResult>> {
        super::public_results(body, limit, 512, &|| Ok(()))
    }

    #[tokio::test]
    async fn public_pace_is_measured_at_the_send_not_before_the_permit_wait() {
        use axum::{routing::get, Router};
        use std::sync::{Arc, Mutex};
        let sent = Arc::new(Mutex::new(Vec::<std::time::Instant>::new()));
        let log = sent.clone();
        let router = Router::new().route(
            "/search",
            get(move || {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(std::time::Instant::now());
                    "<a href='/url?q=https%3A%2F%2Fveeam.com%2Fkb1'><h3>Paced</h3></a>"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        crate::common::home::Home::at(dir.path().to_path_buf())
            .ensure_layout()
            .unwrap();
        let cfg =
            crate::common::config::AppConfig::from_str("web: {pace_ms: 100}", std::path::Path::new("config.yaml"))
                .unwrap();
        let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
        web.search_key_from_file = true;
        web.plaintext_key_file = true;
        web.test_resolve = Some(("google.fixture".into(), address));
        web.test_public_search = Some(url::Url::parse("http://google.fixture/search").unwrap()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        web.allow("http://google.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let audit = crate::common::audit::Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let endpoint = url::Url::parse("http://unused.example/search").unwrap();
        // Another web operation holds every fetch permit for 2 s: the first search
        // queues, then sends; the second must still leave >= 3 s after that send.
        let held = web
            .permits
            .clone()
            .acquire_many_owned(web.limits.concurrency as u32)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(2000)).await;
            drop(held);
        });
        web.google_search_at("veeam", 3, true, &audit, endpoint.clone())
            .await
            .unwrap();
        web.google_search_at("veeam backup", 3, true, &audit, endpoint)
            .await
            .unwrap();
        release.await.unwrap();
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        // The fixture sees arrivals, which trail each send stamp by the audit write
        // and connect, and the first request's lag can exceed the second's by
        // hundreds of ms on a loaded runner (2.67 s was seen on macOS CI). So the
        // exact floor is asserted on the send stamp (`gap_ms`) below, and this
        // arrival check only has to tell the regression apart: pacing before the
        // permit wait lands the second send about 1 s after the first, correct
        // pacing about 3 s. A 2 s bound keeps a full second of margin each way.
        assert!(
            sent[1] - sent[0] >= Duration::from_millis(PUBLIC_SEARCH_PACE_FLOOR_MS - 1000),
            "sends {:?} apart; permit queueing must not shorten the floor",
            sent[1] - sent[0]
        );
        // The audit line is written at the send, not at the broker decision before the wait.
        let lines: Vec<Value> = std::fs::read_to_string(dir.path().join("audit.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["event"] == "web_search_sent")
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["provider"], "google-public");
        assert!(
            lines[0]["gap_ms"].is_null(),
            "the first send has no earlier send: {lines:?}"
        );
        assert!(
            lines[1]["gap_ms"].as_u64().unwrap() >= PUBLIC_SEARCH_PACE_FLOOR_MS,
            "{lines:?}"
        );
        assert!(lines[1]["waited_ms"].as_u64().unwrap() > 0, "{lines:?}");
        assert!(
            !lines.iter().any(|line| line.to_string().contains("veeam")),
            "no query text"
        );
        server.abort();
    }

    /// A public-Google fixture tool: `google.fixture` resolves to `address`.
    fn public_tool(address: std::net::SocketAddr) -> (WebTool, Audit, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        crate::common::home::Home::at(dir.path().to_path_buf())
            .ensure_layout()
            .unwrap();
        let cfg = crate::common::config::AppConfig::from_str("web: {}", std::path::Path::new("config.yaml")).unwrap();
        let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
        web.search_key_from_file = true;
        web.plaintext_key_file = true;
        web.test_resolve = Some(("google.fixture".into(), address));
        web.test_public_search = Some(url::Url::parse("http://google.fixture/search").unwrap()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        web.allow("http://google.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        (web, audit, dir)
    }

    async fn public_search(web: &WebTool, audit: &Audit, query: &str) -> Result<Value> {
        let endpoint = url::Url::parse("http://unused.example/search").unwrap(); // DevSkim: ignore DS137138 because public Google never reads the endpoint.
        web.google_search_at(query, 3, true, audit, endpoint).await
    }

    fn cooldown_lines(dir: &tempfile::TempDir) -> usize {
        std::fs::read_to_string(dir.path().join("audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("\"web_search_cooldown\""))
            .count()
    }

    #[tokio::test]
    async fn a_failed_connection_to_public_google_is_a_provider_failure_not_a_refusal() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
        let address = listener.local_addr().unwrap();
        drop(listener); // nothing listens: connection refused, no status ever arrives
        let (web, audit, dir) = public_tool(address);
        for query in ["veeam first", "veeam second"] {
            let err = public_search(&web, &audit, query).await.unwrap_err();
            assert_eq!(err.code, "WEB_SEARCH_PROVIDER", "{query}: {err:?}");
        }
        assert_eq!(cooldown_lines(&dir), 0, "a blip must not start a pause");
        assert!(web.search_cooldown.holding("google-public", Instant::now()).is_none());
    }

    #[tokio::test]
    async fn transient_public_google_failures_allow_the_next_search_to_recover() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for fault in ["headers-timeout", "body-timeout", "truncated-body"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                for attempt in 0..2 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                    }
                    if attempt == 0 {
                        if fault != "headers-timeout" {
                            // Even challenge text in an incomplete 200 body is not a verdict.
                            socket
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nunusual traffic")
                                .await
                                .unwrap();
                        }
                        if fault != "truncated-body" {
                            // Wait for the client's real deadline to close the connection.
                            let mut rest = Vec::new();
                            let _ = socket.read_to_end(&mut rest).await;
                        }
                    } else {
                        let body = "<a href='https://www.veeam.com/'><h3>Recovered result</h3></a>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        socket.write_all(response.as_bytes()).await.unwrap();
                    }
                }
            });
            let (web, audit, dir) = public_tool(address);
            let err = public_search(&web, &audit, "veeam first").await.unwrap_err();
            if fault == "truncated-body" {
                assert_eq!(err.code, "WEB_SEARCH_PROVIDER", "{fault}: {err:?}");
            } else {
                // Reqwest and the outer deadline race; neither outcome is a refusal.
                assert!(
                    matches!(err.code.as_str(), "WEB_TIMEOUT" | "WEB_SEARCH_PROVIDER"),
                    "{fault}: {err:?}"
                );
            }
            assert!(
                web.search_cooldown.holding("google-public", Instant::now()).is_none(),
                "{fault}"
            );
            let recovered = public_search(&web, &audit, "veeam second").await.unwrap();
            assert_eq!(recovered["results"][0]["url"], "https://www.veeam.com/", "{fault}");
            assert_eq!(cooldown_lines(&dir), 0, "{fault}: a blip must not start a pause");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_public_google_tls_failure_never_starts_a_pause() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut hello = [0; 4096];
                assert!(socket.read(&mut hello).await.unwrap() > 0);
                // Plain HTTP cannot satisfy a TLS handshake; no Google answer exists.
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            }
        });
        let (mut web, audit, dir) = public_tool(address);
        web.test_public_search = Some(url::Url::parse("https://google.fixture/search").unwrap());
        web.allow("https://google.fixture/*", true).unwrap();
        for query in ["veeam first", "veeam second"] {
            assert_eq!(
                public_search(&web, &audit, query).await.unwrap_err().code,
                "WEB_SEARCH_PROVIDER"
            );
        }
        assert!(web.search_cooldown.holding("google-public", Instant::now()).is_none());
        assert_eq!(cooldown_lines(&dir), 0);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_status_redirect_or_challenge_from_public_google_still_pauses() {
        use axum::{
            http::StatusCode,
            response::{Html, Redirect},
            routing::get,
            Router,
        };
        let status = Router::new().route("/search", get(|| async { StatusCode::TOO_MANY_REQUESTS }));
        let redirect = Router::new().route("/search", get(|| async { Redirect::temporary("/sorry") }));
        let challenge = Router::new().route(
            "/search",
            get(|| async {
                Html("<html>Our systems have detected unusual traffic. <div class='g-recaptcha'></div></html>")
            }),
        );
        for (label, router, code) in [
            ("429", status, "WEB_GOOGLE_BLOCKED"),
            ("307", redirect, "WEB_GOOGLE_BLOCKED"),
            ("challenge", challenge, "WEB_GOOGLE_CHALLENGE"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let (web, audit, dir) = public_tool(address);
            assert_eq!(
                public_search(&web, &audit, "veeam first").await.unwrap_err().code,
                code,
                "{label}"
            );
            assert_eq!(
                public_search(&web, &audit, "veeam second").await.unwrap_err().code,
                "WEB_SEARCH_COOLDOWN",
                "{label}: a refusal pauses the provider"
            );
            assert_eq!(cooldown_lines(&dir), 1, "{label}");
            server.abort();
        }
    }

    /// Loopback fixtures for the fallback chain: Tavily (via `test_tavily_search`),
    /// SerpAPI (via the endpoint seam) and public Google (via `test_public_search`).
    mod chain {
        use super::*;
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;
        use std::net::SocketAddr;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        pub(super) struct Fixture {
            pub hits: Arc<AtomicUsize>,
            pub addr: SocketAddr,
            task: tokio::task::JoinHandle<()>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                self.task.abort();
            }
        }

        async fn serve(router: Router, hits: Arc<AtomicUsize>) -> Fixture {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Fixture { hits, addr, task }
        }

        /// SerpAPI: "slow" delays ten seconds, "bad" is 401, "quota" is 429.
        pub(super) async fn serpapi() -> Fixture {
            let hits = Arc::new(AtomicUsize::new(0));
            let h = hits.clone();
            let router = Router::new().route(
                "/search",
                get(move |Query(q): Query<HashMap<String, String>>| {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, Ordering::SeqCst);
                        if q.get("api_key").is_some_and(|key| key == "slow") {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                        if q.get("api_key").is_some_and(|key| key == "late") {
                            tokio::time::sleep(Duration::from_millis(2500)).await;
                        }
                        let status = match q.get("api_key").map(String::as_str) {
                            Some("bad") => axum::http::StatusCode::UNAUTHORIZED,
                            Some("quota") => axum::http::StatusCode::TOO_MANY_REQUESTS,
                            _ => axum::http::StatusCode::OK,
                        };
                        (
                            status,
                            Json(json!({"organic_results":[{"title":"SerpAPI row","link":"https://veeam.com/kb1","snippet":"s"}]})),
                        )
                    }
                }),
            );
            serve(router, hits).await
        }

        /// Tavily: Bearer "good" answers, "zero" answers with no rows, "slow" answers
        /// after 10 s, "quota" is 432 (plan spent) and anything else is 401.
        /// A request that asks for an answer, page bodies, images or auto parameters,
        /// or carries a key anywhere but the header, is refused with 400.
        pub(super) async fn tavily() -> Fixture {
            use axum::{http::HeaderMap, routing::post};
            let hits = Arc::new(AtomicUsize::new(0));
            let h = hits.clone();
            let router = Router::new().route(
                "/search",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, Ordering::SeqCst);
                        let pinned = [
                            "include_answer",
                            "include_raw_content",
                            "include_images",
                            "auto_parameters",
                        ]
                        .iter()
                        .all(|flag| body[*flag] == json!(false));
                        if !pinned || body["search_depth"] != "basic" || body.get("api_key").is_some() {
                            return (axum::http::StatusCode::BAD_REQUEST, Json(json!({})));
                        }
                        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
                        if auth == "Bearer slow" {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                        if auth == "Bearer zero" {
                            return (
                                axum::http::StatusCode::OK,
                                Json(json!({"query":body["query"],"results":[]})),
                            );
                        }
                        let status = match auth {
                            "Bearer good" | "Bearer slow" => axum::http::StatusCode::OK,
                            "Bearer quota" => axum::http::StatusCode::from_u16(432).unwrap(),
                            _ => axum::http::StatusCode::UNAUTHORIZED,
                        };
                        (
                            status,
                            Json(json!({"query":body["query"],"answer":"PROVIDER ANSWER","results":[
                                {"title":"Tavily row","url":"https://veeam.com/tv","content":"highlight",
                                 "score":0.9,"raw_content":"FULL PAGE BODY"}]})),
                        )
                    }
                }),
            );
            serve(router, hits).await
        }

        pub(super) async fn google() -> Fixture {
            let hits = Arc::new(AtomicUsize::new(0));
            let h = hits.clone();
            let router = Router::new().route(
                "/search",
                get(move || {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, Ordering::SeqCst);
                        "<a href='/url?q=https%3A%2F%2Fveeam.com%2Fpublic'><h3>Public row</h3></a>"
                    }
                }),
            );
            serve(router, hits).await
        }

        pub(super) struct Chain {
            pub web: WebTool,
            pub audit: Audit,
            pub endpoint: url::Url,
            pub dir: tempfile::TempDir,
        }

        impl Chain {
            pub(super) async fn new(yaml: &str, key: &str, serp: &Fixture, google: Option<&Fixture>) -> Self {
                let dir = tempfile::tempdir().unwrap();
                crate::common::home::Home::at(dir.path().to_path_buf())
                    .ensure_layout()
                    .unwrap();
                let cfg =
                    crate::common::config::AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap();
                let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
                web.search_key_from_file = true;
                web.plaintext_key_file = true;
                web.test_resolve = Some(("provider.example".into(), serp.addr));
                if let Some(google) = google {
                    web.test_resolve_extra = vec![("google.fixture".into(), google.addr)];
                    web.test_public_search = Some(url::Url::parse("http://google.fixture/search").unwrap()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                    web.allow("http://google.fixture/*", true).unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                }
                if !key.is_empty() {
                    super::super::super::env_keys::write_keys(
                        &dir.path().join(".env"),
                        &std::collections::BTreeMap::from([("SERPAPI_API_KEY".into(), key.into())]),
                    )
                    .unwrap();
                }
                let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
                let endpoint = url::Url::parse("http://provider.example/search").unwrap(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                Self {
                    web,
                    audit,
                    endpoint,
                    dir,
                }
            }

            /// Point the Tavily hop at `fixture` and save `key` (empty saves none).
            pub(super) fn with_tavily(self, key: &str, fixture: &Fixture) -> Self {
                self.with_tavily_at(key, fixture.addr)
            }

            /// Point the Tavily hop at `addr`, such as a refused port (Tavily down).
            pub(super) fn with_tavily_at(mut self, key: &str, addr: SocketAddr) -> Self {
                self.web.tavily_key_from_file = true;
                self.web.test_resolve_extra.push(("tavily.fixture".into(), addr));
                self.web.test_tavily_search = Some(url::Url::parse("http://tavily.fixture/search").unwrap()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                if !key.is_empty() {
                    super::super::super::env_keys::update_keys(
                        &self.dir.path().join(".env"),
                        &std::collections::BTreeMap::from([("TAVILY_API_KEY".into(), key.into())]),
                        &[],
                    )
                    .unwrap();
                }
                self
            }

            pub(super) async fn search(&self, query: &str) -> Result<Value> {
                self.web
                    .google_search_at(query, 3, true, &self.audit, self.endpoint.clone())
                    .await
            }

            pub(super) fn audit_events(&self, event: &str) -> Vec<Value> {
                std::fs::read_to_string(self.dir.path().join("audit.jsonl"))
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter(|line| line["event"] == event)
                    .collect()
            }
        }

        /// A refused loopback address: Tavily is down.
        pub(super) async fn refused_addr() -> SocketAddr {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because the fixture binds only an ephemeral loopback port.
            listener.local_addr().unwrap()
        }

        /// `request_seconds: 10` gives a Tavily first hop a 6 s budget: Windows retries a
        /// refused loopback connect for about 2 s before reporting it, which must
        /// finish as `WEB_SEARCH_PROVIDER`, not race the budget into `WEB_TIMEOUT`.
        pub(super) fn yaml(extra: &str) -> String {
            yaml_with(10, extra)
        }

        pub(super) fn yaml_with(request_seconds: u64, extra: &str) -> String {
            format!("web: {{request_seconds: {request_seconds}, search: {{provider: tavily{extra}}}}}")
        }
    }

    const TAVILY_CHAIN: &str = "web: {request_seconds: 10, search: {provider: tavily, fallback_to_serpapi: true, fallback_to_public_google: true}}";

    #[tokio::test]
    async fn tavily_answers_with_snippets_only_and_spends_no_later_hop() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new(TAVILY_CHAIN, "good", &serp, Some(&google))
            .await
            .with_tavily("good", &tv);
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "tavily");
        assert_eq!(value["results"][0]["title"], "Tavily row");
        assert_eq!(value["results"][0]["snippet"], "highlight");
        assert!(value.get("fallback_from").is_none());
        // Fixed failure messages: neither the listing nor the audit text is echoed, since a
        // failure here means either may hold provider or credential data.
        let text = value.to_string();
        assert!(
            !text.contains("PROVIDER ANSWER") && !text.contains("FULL PAGE BODY"),
            "listing carried the provider answer or a page body"
        );
        assert_eq!(tv.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(serp.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        let audit = std::fs::read_to_string(chain.dir.path().join("audit.jsonl")).unwrap_or_default();
        assert!(
            !audit.contains("veeam cve") && !audit.contains("Bearer"),
            "audit log recorded the query or an Authorization header"
        );
    }

    #[tokio::test]
    async fn a_spent_tavily_plan_falls_back_to_serpapi_then_cools_down() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new(TAVILY_CHAIN, "good", &serp, None)
            .await
            .with_tavily("quota", &tv);
        let first = chain.search("veeam cve").await.unwrap();
        assert_eq!(first["provider"], "google-serpapi");
        assert_eq!(first["fallback_from"], json!(["tavily"]));
        let second = chain.search("veeam backup").await.unwrap();
        assert_eq!(second["provider"], "google-serpapi");
        assert_eq!(
            tv.hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the cooling Tavily was asked once"
        );
        let codes: Vec<Value> = chain
            .audit_events("web_search_fallback")
            .iter()
            .map(|line| line["code"].clone())
            .collect();
        assert_eq!(codes, [json!("WEB_SEARCH_QUOTA"), json!("WEB_SEARCH_COOLDOWN")]);
        let cooldowns = chain.audit_events("web_search_cooldown");
        assert_eq!(cooldowns.len(), 1);
        assert_eq!(cooldowns[0]["provider"], "tavily");
    }

    #[tokio::test]
    async fn a_rejected_or_missing_tavily_key_stays_loud_and_spends_nothing_else() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let rejected = chain::Chain::new(TAVILY_CHAIN, "good", &serp, Some(&google))
            .await
            .with_tavily("bad", &tv);
        assert_eq!(
            rejected.search("veeam cve").await.unwrap_err().code,
            "WEB_SEARCH_KEY_REJECTED"
        );
        assert!(
            rejected.audit_events("web_search_cooldown").is_empty(),
            "a bad key never pauses"
        );
        let missing = chain::Chain::new(TAVILY_CHAIN, "good", &serp, Some(&google))
            .await
            .with_tavily("", &tv);
        assert_eq!(
            missing.search("veeam cve").await.unwrap_err().code,
            "WEB_SEARCH_KEY_MISSING"
        );
        assert_eq!(tv.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(serp.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn auto_searches_a_saved_tavily_key_first_then_serpapi_then_public_google() {
        use std::sync::atomic::Ordering::SeqCst;
        // Both keys saved: Tavily wins, and SerpAPI is not asked.
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("{}", "good", &serp, None)
            .await
            .with_tavily("good", &tv);
        assert_eq!(chain.search("veeam cve").await.unwrap()["provider"], "tavily");
        assert_eq!((tv.hits.load(SeqCst), serp.hits.load(SeqCst)), (1, 0));

        // Only a Tavily key: auto is Tavily.
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("{}", "", &serp, None).await.with_tavily("good", &tv);
        assert_eq!(chain.search("veeam cve").await.unwrap()["provider"], "tavily");
        assert_eq!((tv.hits.load(SeqCst), serp.hits.load(SeqCst)), (1, 0));

        // Only a SerpAPI key: auto is SerpAPI, as before, and Tavily is never asked.
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("{}", "good", &serp, None).await.with_tavily("", &tv);
        assert_eq!(chain.search("veeam cve").await.unwrap()["provider"], "google-serpapi");
        assert_eq!((tv.hits.load(SeqCst), serp.hits.load(SeqCst)), (0, 1));

        // No key: public Google, and neither keyed provider is asked.
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new("{}", "", &serp, Some(&google))
            .await
            .with_tavily("", &tv);
        assert_eq!(chain.search("veeam cve").await.unwrap()["provider"], "google-public");
        assert_eq!((tv.hits.load(SeqCst), serp.hits.load(SeqCst)), (0, 0));

        // A forced SerpAPI is not reordered by a saved Tavily key.
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("web: {search: {provider: serpapi}}", "good", &serp, None)
            .await
            .with_tavily("good", &tv);
        assert_eq!(chain.search("veeam cve").await.unwrap()["provider"], "google-serpapi");
        assert_eq!(tv.hits.load(SeqCst), 0);

        // Auto with both keys follows the same fallback flags as a forced Tavily:
        // a down Tavily reaches SerpAPI only when fallback_to_serpapi is on.
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("web: {search: {fallback_to_serpapi: true}}", "good", &serp, None)
            .await
            .with_tavily_at("good", chain::refused_addr().await);
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-serpapi");
        assert_eq!(value["fallback_from"], json!(["tavily"]));
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("{}", "good", &serp, None)
            .await
            .with_tavily_at("good", chain::refused_addr().await);
        assert_eq!(chain.search("veeam cve").await.unwrap_err().code, "WEB_SEARCH_PROVIDER");
        assert_eq!(serp.hits.load(SeqCst), 0, "no SerpAPI hop without its flag");
    }

    #[tokio::test]
    async fn a_tavily_key_cleared_while_the_search_waits_is_never_sent() {
        use std::sync::atomic::Ordering::SeqCst;
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new("{}", "", &serp, None).await.with_tavily("good", &tv);
        // Every permit is taken, so the search reads its key and then waits.
        let held = chain
            .web
            .permits
            .clone()
            .acquire_many_owned(chain.web.limits.concurrency as u32)
            .await
            .unwrap();
        let clear = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            super::super::env_keys::update_keys(
                &chain.dir.path().join(".env"),
                &std::collections::BTreeMap::new(),
                &["TAVILY_API_KEY".to_string()],
            )
            .unwrap();
            drop(held);
        };
        let (result, ()) = tokio::join!(chain.search("veeam cve"), clear);
        assert_eq!(result.unwrap_err().code, "WEB_SEARCH_KEY_CHANGED");
        assert_eq!(
            (tv.hits.load(SeqCst), serp.hits.load(SeqCst)),
            (0, 0),
            "a cleared key reaches no provider"
        );
    }

    #[tokio::test]
    async fn fallback_is_off_by_default_and_a_down_tavily_stays_explicit() {
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new(&chain::yaml(""), "good", &serp, None)
            .await
            .with_tavily_at("good", chain::refused_addr().await);
        let err = chain.search("veeam cve").await.unwrap_err();
        assert_eq!(err.code, "WEB_SEARCH_PROVIDER");
        assert_eq!(
            serp.hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no hop was enabled"
        );
        assert!(chain.audit_events("web_search_fallback").is_empty());
    }

    #[tokio::test]
    async fn a_down_tavily_falls_back_to_serpapi_and_says_so() {
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new(&chain::yaml(", fallback_to_serpapi: true"), "good", &serp, None)
            .await
            .with_tavily_at("good", chain::refused_addr().await);
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-serpapi");
        assert_eq!(value["fallback_from"], json!(["tavily"]));
        assert_eq!(value["results"][0]["title"], "SerpAPI row");
        let lines = chain.audit_events("web_search_fallback");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["from"], "tavily");
        assert_eq!(lines[0]["to"], "google-serpapi");
        assert_eq!(lines[0]["code"], "WEB_SEARCH_PROVIDER");
        let text = std::fs::read_to_string(chain.dir.path().join("audit.jsonl")).unwrap();
        assert!(
            !text.contains("veeam cve") && !text.contains("\"good\""),
            "audit lines carry codes, never the query or a key"
        );
    }

    #[tokio::test]
    async fn zero_tavily_results_are_an_answer_not_a_fallback() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new(
            &chain::yaml(", fallback_to_serpapi: true, fallback_to_public_google: true"),
            "good",
            &serp,
            Some(&google),
        )
        .await
        .with_tavily("zero", &tv);
        let value = chain.search("nothing matches").await.unwrap();
        assert_eq!(value["provider"], "tavily");
        assert_eq!(value["results"], json!([]));
        assert!(value.get("fallback_from").is_none());
        assert_eq!(serp.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_rejected_key_never_reaches_public_google() {
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new(
            &chain::yaml(", fallback_to_serpapi: true, fallback_to_public_google: true"),
            "bad",
            &serp,
            Some(&google),
        )
        .await
        .with_tavily_at("good", chain::refused_addr().await);
        assert_eq!(
            chain.search("veeam cve").await.unwrap_err().code,
            "WEB_SEARCH_KEY_REJECTED",
            "the operator's error stays loud"
        );
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_spent_quota_moves_on_to_public_google_on_the_first_search_and_then_skips_serpapi() {
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let down = chain::refused_addr().await;
        let yaml = chain::yaml(", fallback_to_serpapi: true, fallback_to_public_google: true");
        let chain = chain::Chain::new(&yaml, "quota", &serp, Some(&google))
            .await
            .with_tavily_at("good", down);
        let first = chain.search("veeam cve").await.unwrap();
        assert_eq!(first["provider"], "google-public");
        assert_eq!(first["fallback_from"], json!(["tavily", "google-serpapi"]));
        let second = chain.search("veeam backup").await.unwrap();
        assert_eq!(second["provider"], "google-public");
        assert_eq!(
            serp.hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the cooling SerpAPI was asked once"
        );
        let codes: Vec<Value> = chain
            .audit_events("web_search_fallback")
            .iter()
            .map(|line| line["code"].clone())
            .collect();
        assert_eq!(
            codes,
            [
                json!("WEB_SEARCH_PROVIDER"),
                json!("WEB_SEARCH_QUOTA"),
                json!("WEB_SEARCH_PROVIDER"),
                json!("WEB_SEARCH_COOLDOWN")
            ]
        );
        // Without the public hop nobody is spent beyond the operator's opt-ins: still an error.
        let alone = chain::Chain::new(&chain::yaml(", fallback_to_serpapi: true"), "quota", &serp, None)
            .await
            .with_tavily_at("good", down);
        assert_eq!(alone.search("veeam cve").await.unwrap_err().code, "WEB_SEARCH_QUOTA");
    }

    #[tokio::test]
    async fn a_serpapi_cooldown_is_skipped_to_public_google_without_a_request() {
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        // Keyed auto: SerpAPI first, public Google as the opt-in last hop.
        let chain = chain::Chain::new(
            "web: {search: {fallback_to_public_google: true}}",
            "quota",
            &serp,
            Some(&google),
        )
        .await;
        let first = chain.search("first").await.unwrap();
        assert_eq!(first["provider"], "google-public");
        assert_eq!(first["fallback_from"], json!(["google-serpapi"]));
        let value = chain.search("second").await.unwrap();
        assert_eq!(value["provider"], "google-public");
        assert_eq!(value["fallback_from"], json!(["google-serpapi"]));
        assert_eq!(
            serp.hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cooling SerpAPI was not asked again"
        );
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        let lines = chain.audit_events("web_search_fallback");
        assert_eq!(lines[0]["code"], "WEB_SEARCH_QUOTA");
        assert_eq!(lines[1]["code"], "WEB_SEARCH_COOLDOWN");
    }

    #[tokio::test]
    async fn a_slow_serpapi_leaves_time_for_the_public_google_fallback() {
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new(
            "web: {request_seconds: 8, search: {fallback_to_public_google: true}}",
            "slow",
            &serp,
            Some(&google),
        )
        .await;
        let started = tokio::time::Instant::now();
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-public");
        assert_eq!(value["fallback_from"], json!(["google-serpapi"]));
        assert_eq!(serp.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test]
    async fn a_keyed_provider_keeps_the_time_public_google_cannot_use() {
        // SerpAPI answers after 2.5 s of a 4 s deadline. Holding back the whole
        // 3 s pace plus 1 s left it 2 s, so a healthy answer timed out and the
        // search fell to public Google.
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let yaml = "web: {request_seconds: 4, search: {fallback_to_public_google: true}}";
        let chain = chain::Chain::new(yaml, "late", &serp, Some(&google)).await;
        assert_eq!(
            chain.web.public_search_reserve(),
            Duration::from_secs(1),
            "no pacing wait is due"
        );
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-serpapi");
        assert!(value.get("fallback_from").is_none());
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        let attempt = &chain.audit_events("web_search_attempt")[0];
        assert_eq!(attempt["code"], "OK");
        assert!(attempt["budget_ms"].as_u64().unwrap() >= 2900, "{attempt}");

        // A public search just left: its pacing wait is due, so the reserve is
        // the old one and public Google still gets its turn after a slow hop.
        *chain.web.public_search_sent.lock().unwrap() = Some(tokio::time::Instant::now());
        let reserve = chain.web.public_search_reserve();
        assert!(
            reserve > Duration::from_millis(3900) && reserve <= Duration::from_secs(4),
            "{reserve:?}"
        );

        // Public Google cooling down takes no share at all: a 3 s deadline is
        // SerpAPI's alone, and its 2.5 s answer arrives.
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let yaml = "web: {request_seconds: 3, search: {fallback_to_public_google: true}}";
        let chain = chain::Chain::new(yaml, "late", &serp, Some(&google)).await;
        *chain.web.public_search_sent.lock().unwrap() = Some(tokio::time::Instant::now());
        chain
            .web
            .search_cooldown
            .start(
                "google-public",
                "WEB_GOOGLE_CHALLENGE",
                &chain.web.limits,
                Instant::now(),
                unix_now(),
            )
            .unwrap();
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-serpapi");
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn two_slow_keyed_providers_still_reach_public_google() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let google = chain::google().await;
        let chain = chain::Chain::new(
            "web: {request_seconds: 8, search: {provider: tavily, fallback_to_serpapi: true, fallback_to_public_google: true}}",
            "slow", &serp, Some(&google),
        ).await.with_tavily("slow", &tv);
        let started = tokio::time::Instant::now();
        let value = chain.search("veeam cve").await.unwrap();
        assert_eq!(value["provider"], "google-public");
        assert_eq!(value["fallback_from"], json!(["tavily", "google-serpapi"]));
        assert_eq!(tv.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(serp.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(google.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test]
    async fn the_whole_chain_shares_one_deadline() {
        let tv = chain::tavily().await;
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new(&chain::yaml_with(4, ", fallback_to_serpapi: true"), "good", &serp, None)
            .await
            .with_tavily("slow", &tv);
        let started = tokio::time::Instant::now();
        let value = chain.search("veeam cve").await.unwrap();
        let elapsed = started.elapsed();
        assert_eq!(value["provider"], "google-serpapi");
        assert_eq!(chain.audit_events("web_search_fallback")[0]["code"], "WEB_TIMEOUT");
        // request_seconds 4: Tavily gets min(4 - 2, 6) = 2 s, the chain ends inside 4 s.
        assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    }

    #[tokio::test]
    async fn serpapi_fallbacks_past_the_hourly_threshold_log_one_burst_line() {
        let serp = chain::serpapi().await;
        let chain = chain::Chain::new(
            &chain::yaml(", fallback_to_serpapi: true, fallback_alert_per_hour: 1"),
            "good",
            &serp,
            None,
        )
        .await
        .with_tavily_at("good", chain::refused_addr().await);
        for query in ["a one", "b two", "c three"] {
            chain.search(query).await.unwrap();
        }
        let bursts = chain.audit_events("web_search_fallback_burst");
        assert_eq!(bursts.len(), 1, "{bursts:?}");
        assert_eq!(bursts[0]["count"], 2);
        assert_eq!(bursts[0]["threshold"], 1);
    }

    #[test]
    fn public_pace_has_a_floor_and_refuses_past_the_deadline() {
        let now = tokio::time::Instant::now();
        let deadline = now + Duration::from_secs(8);
        assert_eq!(
            public_pace(None, 400, deadline).unwrap(),
            None,
            "the first search waits for nothing"
        );
        assert_eq!(
            public_pace(Some(now), 400, deadline).unwrap(),
            Some(now + Duration::from_millis(PUBLIC_SEARCH_PACE_FLOOR_MS)),
            "a configured pace below the floor is raised to it"
        );
        assert_eq!(
            public_pace(Some(now), 5000, deadline).unwrap(),
            Some(now + Duration::from_millis(5000))
        );
        let short = now + Duration::from_secs(3);
        assert_eq!(public_pace(Some(now), 400, short).unwrap_err().code, "WEB_SEARCH_PACED");
        let later = now + Duration::from_secs(10);
        assert!(
            public_pace(Some(now), 400, later + Duration::from_secs(8))
                .unwrap()
                .unwrap()
                <= later,
            "an old search leaves nothing to wait for"
        );
    }

    #[test]
    fn cooldown_starts_holds_expires_and_never_pauses_a_rejected_key() {
        let cfg = crate::common::config::AppConfig::from_str(
            "web: {search_cooldown_public_sec: 600, search_cooldown_quota_sec: 120}",
            std::path::Path::new("config.yaml"),
        )
        .unwrap();
        let limits = super::super::web_search::Limits::load(&cfg).unwrap();
        let pauses = SearchCooldown::default();
        let now = Instant::now();
        let wall = 1_800_000_000;
        assert!(pauses.holding("google-public", now).is_none());
        assert_eq!(
            pauses.start("google-public", "WEB_GOOGLE_CHALLENGE", &limits, now, wall),
            Some(wall + 600)
        );
        let held = pauses.holding("google-public", now + Duration::from_secs(599)).unwrap();
        assert_eq!(held.code, "WEB_GOOGLE_CHALLENGE");
        let message = cooldown_error("google-public", &held).message;
        assert!(message.contains("search is cooling down until "), "{message}");
        assert!(message.contains("WEB_GOOGLE_CHALLENGE"), "{message}");
        assert!(
            pauses.holding("google-serpapi", now).is_none(),
            "a pause is per provider"
        );
        assert!(pauses
            .holding("google-public", now + Duration::from_secs(600))
            .is_none());
        assert!(
            pauses.holding("google-public", now).is_none(),
            "an expired pause is removed"
        );
        assert_eq!(
            pauses.start("google-public", "WEB_GOOGLE_BLOCKED", &limits, now, wall),
            Some(wall + 600)
        );
        assert_eq!(
            pauses.start("google-serpapi", "WEB_SEARCH_QUOTA", &limits, now, wall),
            Some(wall + 120)
        );
        for (provider, code) in [
            ("google-serpapi", "WEB_SEARCH_KEY_REJECTED"),
            ("google-serpapi", "WEB_SEARCH_PROVIDER"),
            ("google-public", "WEB_GOOGLE_UNREADABLE"),
            ("google-public", "WEB_TIMEOUT"),
            ("google-public", "WEB_SEARCH_QUOTA"),
            ("google-serpapi", "WEB_GOOGLE_CHALLENGE"),
        ] {
            let fresh = SearchCooldown::default();
            assert_eq!(
                fresh.start(provider, code, &limits, now, wall),
                None,
                "{provider} {code}"
            );
            assert!(fresh.holding(provider, now).is_none(), "{provider} {code}");
        }
    }

    #[test]
    fn quota_cooldown_zero_waits_for_the_next_clock_hour() {
        let hour = 500_001 * 3600;
        for (now, seconds) in [(hour, 3600), (hour + 1, 3599), (hour + 3599, 1), (hour + 1800, 1800)] {
            assert_eq!(
                cooldown_seconds("google-serpapi", "WEB_SEARCH_QUOTA", now, 600, 0),
                Some(seconds),
                "{now}"
            );
            assert_eq!((now + seconds) % 3600, 0, "ends on the hour");
        }
        assert_eq!(
            cooldown_seconds("google-serpapi", "WEB_SEARCH_KEY_REJECTED", hour, 600, 0),
            None
        );
    }

    #[test]
    fn public_listing_parser_obeys_complexity_and_cancellation() {
        let body = "<div/>".repeat(40_000);
        assert_eq!(public_results(&body, 5).unwrap_err().code, "WEB_HTML_COMPLEXITY");
        let calls = std::cell::Cell::new(0);
        let err = super::public_results(&body, 5, 512, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                Err(error("WEB_CANCELLED", "stopped"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(err.code, "WEB_CANCELLED");
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn keyed_transport_bounds_results_and_never_echoes_provider_errors_or_credentials() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;
        let router = Router::new().route("/search", get(|Query(q): Query<HashMap<String,String>>| async move {
            assert_eq!(q.get("api_key").map(String::as_str),Some("fixture-secret-key"));
            assert_eq!(q.get("engine").map(String::as_str),Some("google"));
            let query=q.get("q").unwrap();
            if query=="quota" { return (axum::http::StatusCode::TOO_MANY_REQUESTS,Json(json!({"error":"fixture-secret-key"}))); }
            (axum::http::StatusCode::OK,Json(json!({"organic_results":[{"title":if query=="reflect" {"fixture-secret-key"} else {"Actual API result"},"link":"https://veeam.com/kb1","snippet":"API snippet"}]})))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let home = crate::common::home::Home::at(dir.path().to_path_buf());
        home.ensure_layout().unwrap();
        // A fresh home seeds the Google search rule. This test asserts that a keyed
        // listing grants nothing and that a keyless search refuses without reaching
        // google.com, so it starts from the explicit empty allowlist.
        crate::common::atomic::write_json_atomic(
            &home.tools_dir().join("web_allowlist.json"),
            &json!({"version": 1, "rules": []}),
        )
        .unwrap();
        let cfg = crate::common::config::AppConfig::from_str("{}", std::path::Path::new("config.yaml")).unwrap();
        let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
        web.search_key_from_file = true;
        web.plaintext_key_file = true;
        web.test_resolve = Some(("provider.example".into(), address));
        let endpoint = url::Url::parse("http://provider.example/search").unwrap();
        let audit = crate::common::audit::Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let path = dir.path().join(".env");
        let save = super::super::env_keys::write_keys(
            &path,
            &std::collections::BTreeMap::from([("SERPAPI_API_KEY".into(), "fixture-secret-key".into())]),
        )
        .unwrap();
        assert_eq!(save["restart_required"], false);
        let status = super::super::env_keys::read_status(&path, &BTreeSet::from(["SERPAPI_API_KEY".into()])).unwrap();
        let row = status.iter().find(|row| row["name"] == "SERPAPI_API_KEY").unwrap();
        assert_eq!(row["active_configured"], true);
        assert_eq!(row["pending_restart"], false);
        let started = tokio::time::Instant::now();
        let result = web
            .google_search_at("veeam", 5, true, &audit, endpoint.clone())
            .await
            .unwrap();
        assert_eq!(result["provider"], "google-serpapi");
        web.google_search_at("veeam backup", 5, true, &audit, endpoint.clone())
            .await
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(PUBLIC_SEARCH_PACE_FLOOR_MS),
            "SerpAPI searches are never paced"
        );
        assert!(web.public_search_sent.lock().unwrap().is_none());
        assert_eq!(result["results"][0]["title"], "Actual API result");
        assert_eq!(
            web.require_enabled(true).unwrap_err().code,
            "WEB_ALLOWLIST_EMPTY",
            "search must not grant page permissions"
        );
        assert_eq!(
            web.google_search_at("veeam", 5, false, &audit, endpoint.clone())
                .await
                .unwrap_err()
                .code,
            "WEB_DISABLED"
        );
        super::super::env_keys::update_keys(&path, &Default::default(), &["SERPAPI_API_KEY".into()]).unwrap();
        assert!(web.search_key().unwrap().is_empty());
        assert_eq!(
            web.google_search_at("veeam", 5, true, &audit, endpoint.clone())
                .await
                .unwrap_err()
                .code,
            "WEB_ALLOWLIST_EMPTY"
        );
        let rows = web
            .api_search(endpoint.clone(), "veeam", 5, "fixture-secret-key")
            .await
            .unwrap();
        assert_eq!(rows[0].title, "Actual API result");
        let serialized = serde_json::to_string(&rows).unwrap();
        assert!(
            !serialized.contains("fixture-secret-key"),
            "HTTP-shaped listing JSON must not echo the provider key: {serialized}"
        );
        let audit = crate::common::audit::Audit::new(dir.path().join("audit.jsonl"), &cfg);
        crate::common::tool_broker::assert_allowed(
            "web_search",
            &["veeam".into()],
            &std::collections::BTreeSet::from(["web_search".to_string()]),
            &audit,
        )
        .unwrap();
        let audit_text = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        assert!(audit_text.contains("tool_broker_decision"), "{audit_text}");
        assert!(audit_text.contains("argv_digest"), "{audit_text}");
        assert!(!audit_text.contains("fixture-secret-key"), "{audit_text}");
        assert!(!audit_text.contains("api_key"), "{audit_text}");
        for (query, code) in [("quota", "WEB_SEARCH_QUOTA"), ("reflect", "WEB_SEARCH_RESPONSE")] {
            let failure = web
                .api_search(endpoint.clone(), query, 5, "fixture-secret-key")
                .await
                .unwrap_err();
            assert_eq!(failure.code, code);
            assert!(!failure.to_string().contains("fixture-secret-key"));
            let http_shaped = json!({"detail":{"code":failure.code,"message":failure.message}});
            assert!(
                !http_shaped.to_string().contains("fixture-secret-key"),
                "HTTP error envelope echoed the provider key: {http_shaped}"
            );
        }
        server.abort();
    }

    #[test]
    fn listings_preserve_order_and_refuse_challenges_or_private_links() {
        let rows = api_results(br#"{"organic_results":[{"title":"First","link":"https://veeam.com/kb1","snippet":"Evidence"},{"title":"Private","link":"http://127.0.0.1/"},{"title":"Second","link":"https://nvd.nist.gov/vuln"}]}"#, 5).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].rank, 2);
        assert_eq!(rows[0].snippet, "Evidence");
        let rows = public_results(
            "<a href='/url?q=https%3A%2F%2Fveeam.com%2Fkb1'><h3>First result</h3></a>",
            5,
        )
        .unwrap();
        assert_eq!(rows[0].url, "https://veeam.com/kb1");
        assert_eq!(
            public_results("<a href='/httpservice/retry/enablejs'>Enable JavaScript</a>", 5)
                .unwrap_err()
                .code,
            "WEB_GOOGLE_CHALLENGE"
        );
        assert!(public_results("<html>unrecognized</html>", 5).is_err());
        assert!(!api_results(br#"{"error":"secret echoed by upstream"}"#, 5)
            .unwrap_err()
            .to_string()
            .contains("secret"));
    }
}
