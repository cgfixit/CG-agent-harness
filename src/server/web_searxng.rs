//! Search provider choice and loopback SearXNG listings. The configured SearXNG
//! URL is one exact loopback socket used only for search listings; it never
//! becomes a fetch, discovery, research or webhook target.
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::{json, Value};

use super::web_google::{add_result, SearchResult, SEARCH_NOTICE};
use super::web_policy::{canonical_http_url, error};
use super::web_search::WebTool;
use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};

/// `web.search.provider`, restart-only. `Auto` is today's rule: an active SerpAPI
/// key selects SerpAPI, otherwise public Google.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchProvider {
    Auto,
    GooglePublic,
    Serpapi,
    Searxng { endpoint: url::Url, socket: SocketAddr },
}

impl SearchProvider {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let text = |key: &str| match cfg.get(key) {
            None => Ok(""),
            Some(serde_yaml_ng::Value::String(s)) => Ok(s.trim()),
            Some(_) => Err(HarnessError::config(format!("{key} must be a string"))),
        };
        let raw_url = text("web.search.searxng_url")?;
        // A URL is validated even while another provider is selected: fail closed.
        let searxng = if raw_url.is_empty() {
            None
        } else {
            Some(searxng_endpoint(raw_url)?)
        };
        match text("web.search.provider")? {
            "" | "auto" => Ok(Self::Auto),
            "google-public" => Ok(Self::GooglePublic),
            "serpapi" => Ok(Self::Serpapi),
            "searxng" => {
                let (endpoint, socket) = searxng.ok_or_else(|| {
                    HarnessError::config("web.search.searxng_url is required when web.search.provider is searxng")
                })?;
                Ok(Self::Searxng { endpoint, socket })
            }
            _ => Err(HarnessError::config(
                "web.search.provider must be auto, google-public, serpapi or searxng",
            )),
        }
    }

    /// The `/web status` wording.
    pub fn describe(&self, keyed: bool) -> &'static str {
        match self {
            Self::Auto if keyed => "Google via SerpAPI",
            Self::Auto | Self::GooglePublic => "public Google (may require JavaScript/CAPTCHA)",
            Self::Serpapi => "Google via SerpAPI (web.search.provider: serpapi)",
            Self::Searxng { .. } => "SearXNG on loopback",
        }
    }
}

/// One exact `http(s)://<loopback IP literal>:<port>[/ or /search]`. No hostname
/// (no DNS, so no rebinding), no public or LAN address, no default port.
fn searxng_endpoint(raw: &str) -> Result<(url::Url, SocketAddr)> {
    let bad = |why: &str| HarnessError::config(format!("web.search.searxng_url {why}"));
    let url = canonical_http_url(raw, true).map_err(|_| bad("must be an explicit http(s) URL without credentials"))?;
    let ip: IpAddr = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.into(),
        Some(url::Host::Ipv6(ip)) => ip.into(),
        _ => return Err(bad("must name a loopback IP literal such as 127.0.0.1, not a hostname")), // DevSkim: ignore DS162092 because this is operator guidance naming the required loopback literal, not a connection.
    };
    if !ip.is_loopback() {
        return Err(bad("must be a loopback address (127.0.0.0/8 or ::1)"));
    }
    // `url` drops a scheme's default port, so :80 and :443 are refused too.
    let port = url
        .port()
        .ok_or_else(|| bad("needs an explicit non-default port, such as :8888"))?;
    if !matches!(url.path(), "/" | "/search") || url.query().is_some() || url.fragment().is_some() {
        return Err(bad("must be the instance root or /search, without a query"));
    }
    let mut endpoint = url;
    endpoint.set_path("/search");
    Ok((endpoint, SocketAddr::new(ip, port)))
}

/// SearXNG forwards `!bang` and `:language` tokens to engine selection; the
/// model never needs them, so they are refused rather than rewritten.
pub(super) fn refuse_searxng_syntax(query: &str) -> Result<()> {
    if query
        .split_whitespace()
        .any(|token| token.starts_with('!') || token.starts_with(':'))
    {
        return Err(error(
            "WEB_BAD_QUERY",
            "search query tokens may not start with ! or : (SearXNG engine and language selectors)",
        ));
    }
    Ok(())
}

/// Rows and the names of engines that did not answer. Messages beside the
/// names are localized upstream text and never leave this function.
pub(super) fn searxng_results(body: &[u8], limit: usize) -> Result<(Vec<SearchResult>, Vec<String>)> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| error("WEB_SEARCH_RESPONSE", "invalid search response"))?;
    if value.get("error").is_some() {
        return Err(error("WEB_SEARCH_PROVIDER", "search provider returned an error"));
    }
    let items = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| error("WEB_SEARCH_RESPONSE", "search result listing is missing"))?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for item in items.iter().take(100) {
        add_result(
            &mut rows,
            &mut seen,
            item["title"].as_str().unwrap_or(""),
            item["url"].as_str().unwrap_or(""),
            item["content"].as_str().unwrap_or(""),
            limit,
        );
    }
    let mut degraded: Vec<String> = Vec::new();
    for pair in value
        .get("unresponsive_engines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(64)
    {
        let Some(name) = pair.get(0).and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        {
            continue;
        }
        if degraded.len() < 16 && !degraded.iter().any(|seen| seen == name) {
            degraded.push(name.to_string());
        }
    }
    Ok((rows, degraded))
}

impl WebTool {
    /// A SearXNG listing: web/account authority, no URL grant, one attempt
    /// inside `web.request_seconds`. Zero rows and unresponsive engines are answers.
    pub(super) async fn searxng_listing(
        &self,
        query: &str,
        count: usize,
        enabled: bool,
        audit: &Audit,
        endpoint: &url::Url,
        socket: SocketAddr,
    ) -> Result<Value> {
        refuse_searxng_syntax(query)?;
        self.gate_tool("web_search", &[query.into()], enabled, audit)?;
        let _gate = self
            .search_gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("WEB_BUSY", "a web search is already running"))?;
        let (rows, degraded) = tokio::time::timeout(Duration::from_secs(self.limits.request_seconds), async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| error("WEB_CANCELLED", "search cancelled"))?;
            let client = self.searxng_client(endpoint, socket)?;
            let mut response = client
                .get(endpoint.clone())
                .query(&[
                    ("q", query),
                    ("format", "json"),
                    ("pageno", "1"),
                    ("language", "en"),
                    ("safesearch", "0"),
                    ("categories", "general"),
                ])
                .send()
                .await
                .map_err(|_| {
                    error(
                        "WEB_SEARCH_PROVIDER",
                        "SearXNG request failed; is it running on the configured address?",
                    )
                })?;
            if response.status().as_u16() != 200 {
                return Err(error(
                    "WEB_SEARCH_PROVIDER",
                    "SearXNG refused the request; check that search.formats includes json",
                ));
            }
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
                if body.len() + chunk.len() > self.limits.response_bytes {
                    return Err(error("WEB_TOO_LARGE", "search response exceeds limit"));
                }
                body.extend_from_slice(&chunk);
            }
            searxng_results(&body, count)
        })
        .await
        .map_err(|_| error("WEB_TIMEOUT", "search deadline exceeded"))??;
        let notice = if degraded.is_empty() {
            SEARCH_NOTICE.to_string()
        } else {
            audit.log(json!({"event":"web_search_degraded","provider":"searxng","degraded":degraded}));
            format!(
                "{SEARCH_NOTICE} SearXNG engines that did not answer: {}.",
                degraded.join(", ")
            )
        };
        Ok(
            json!({"query":query,"provider":"searxng","results":rows,"degraded":degraded,
            "notice":notice,"complete":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(yaml: &str) -> Result<SearchProvider> {
        SearchProvider::load(&AppConfig::from_str(yaml, std::path::Path::new("config.yaml")).unwrap())
    }

    #[test]
    fn provider_defaults_to_today_and_searxng_needs_an_exact_loopback_socket() {
        assert_eq!(load("{}").unwrap(), SearchProvider::Auto);
        assert_eq!(
            load("web: {search: {provider: auto, searxng_url: ''}}").unwrap(),
            SearchProvider::Auto
        );
        assert_eq!(
            load("web: {search: {provider: google-public}}").unwrap(),
            SearchProvider::GooglePublic
        );
        assert_eq!(
            load("web: {search: {provider: serpapi}}").unwrap(),
            SearchProvider::Serpapi
        );
        for (raw, socket) in [
            ("http://127.0.0.1:8888", "127.0.0.1:8888"), // DevSkim: ignore DS137138,DS162092 because this tests the loopback-only SearXNG grant.
            ("http://127.0.0.1:8888/search", "127.0.0.1:8888"), // DevSkim: ignore DS137138,DS162092 because this tests the loopback-only SearXNG grant.
            ("https://[::1]:8443/", "[::1]:8443"),
        ] {
            let SearchProvider::Searxng { endpoint, socket: got } =
                load(&format!("web: {{search: {{provider: searxng, searxng_url: '{raw}'}}}}")).unwrap()
            else {
                panic!("{raw}");
            };
            assert_eq!(got.to_string(), socket, "{raw}");
            assert_eq!(endpoint.path(), "/search", "{raw}");
            assert!(endpoint.query().is_none(), "{raw}");
        }
        for bad in [
            "web: {search: {provider: searxng}}",
            "web: {search: {provider: searxng, searxng_url: ''}}",
            "web: {search: {provider: bing}}",
            "web: {search: {provider: true}}",
            "web: {search: {provider: searxng, searxng_url: 'http://localhost:8888'}}", // DevSkim: ignore DS137138,DS162092 because this asserts a loopback hostname is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://searx.example:8888'}}", // DevSkim: ignore DS137138 because this asserts a hostname is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://93.184.216.34:8888'}}", // DevSkim: ignore DS137138 because this asserts a public IP is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://192.168.1.20:8888'}}", // DevSkim: ignore DS137138 because this asserts a LAN IP is refused in Phase 1.
            "web: {search: {provider: searxng, searxng_url: 'http://[::ffff:127.0.0.1]:8888'}}", // DevSkim: ignore DS137138,DS162092 because this asserts a mapped address is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://127.0.0.1'}}", // DevSkim: ignore DS137138,DS162092 because this asserts a missing port is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://127.0.0.1:80'}}", // DevSkim: ignore DS137138,DS162092 because this asserts a default port is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://u:p@127.0.0.1:8888'}}", // DevSkim: ignore DS137138,DS162092 because this asserts credentials are refused.
            "web: {search: {provider: searxng, searxng_url: 'http://127.0.0.1:8888/other'}}", // DevSkim: ignore DS137138,DS162092 because this asserts another path is refused.
            "web: {search: {provider: searxng, searxng_url: 'http://127.0.0.1:8888/search?q=x'}}", // DevSkim: ignore DS137138,DS162092 because this asserts a query is refused.
            "web: {search: {provider: searxng, searxng_url: 'ftp://127.0.0.1:8888'}}", // DevSkim: ignore DS162092 because this asserts a non-HTTP loopback URL is refused.
            // A malformed URL fails boot even while another provider is selected.
            "web: {search: {provider: auto, searxng_url: 'http://searx.example:8888'}}", // DevSkim: ignore DS137138 because this asserts a hostname is refused.
        ] {
            let err = load(bad).unwrap_err();
            assert_eq!(err.code, "CONFIG_ERROR", "{bad}: {err}");
        }
    }

    #[test]
    fn searxng_results_map_rows_drop_private_links_and_report_degraded_engines() {
        let body = json!({"query":"q","results":[
            {"title":"First","url":"https://veeam.com/kb1","content":"Evidence","engine":"duckduckgo","score":3.0},
            {"title":"Loop","url":"http://127.0.0.1:8888/admin","content":"private"}, // DevSkim: ignore DS137138,DS162092 because this asserts a loopback result is dropped.
            {"title":"","url":"https://example.org/untitled"},
            {"title":"Dup","url":"https://veeam.com/kb1"},
            {"title":"Second","url":"https://nvd.nist.gov/vuln","content":"More"}],
            "unresponsive_engines":[["google","Suspended: CAPTCHA"],["google","again"],["bing","timeout"],["<script>","x"],[7,"x"],"bad"]});
        let (rows, degraded) = searxng_results(body.to_string().as_bytes(), 5).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://veeam.com/kb1");
        assert_eq!(rows[0].snippet, "Evidence");
        assert_eq!(rows[1].rank, 2);
        assert_eq!(degraded, ["google", "bing"]);
        let rendered = serde_json::to_string(&rows).unwrap();
        assert!(
            !rendered.contains("duckduckgo") && !rendered.contains("score"),
            "{rendered}"
        );
        let (rows, degraded) = searxng_results(br#"{"results":[],"unresponsive_engines":[]}"#, 5).unwrap();
        assert!(rows.is_empty() && degraded.is_empty(), "zero results is an answer");
        assert_eq!(
            searxng_results(br#"{"error":"secret upstream text"}"#, 5)
                .unwrap_err()
                .code,
            "WEB_SEARCH_PROVIDER"
        );
        assert!(!searxng_results(br#"{"error":"secret upstream text"}"#, 5)
            .unwrap_err()
            .to_string()
            .contains("secret"));
        assert_eq!(
            searxng_results(br#"{"query":"q"}"#, 5).unwrap_err().code,
            "WEB_SEARCH_RESPONSE"
        );
        assert_eq!(
            searxng_results(b"\xff\xfe{", 5).unwrap_err().code,
            "WEB_SEARCH_RESPONSE"
        );
        assert_eq!(searxng_results(b"<html>", 5).unwrap_err().code, "WEB_SEARCH_RESPONSE");
        let many: Vec<Value> = (0..10_000)
            .map(|i| json!({"title":format!("Row {i}"),"url":format!("https://example.org/{i}")}))
            .collect();
        let (rows, _) = searxng_results(json!({"results":many}).to_string().as_bytes(), 10).unwrap();
        assert_eq!(rows.len(), 10, "rows are cut to count after dedupe");
    }

    #[test]
    fn searxng_syntax_tokens_are_refused() {
        for query in ["!g rust", "rust !ddg", ":de rust", "rust :fr", "!!g x"] {
            assert_eq!(
                refuse_searxng_syntax(query).unwrap_err().code,
                "WEB_BAD_QUERY",
                "{query}"
            );
        }
        for query in ["rust async", "c++ 'hello!'", "time 10:30", "\"exact phrase\""] {
            assert!(refuse_searxng_syntax(query).is_ok(), "{query}");
        }
    }
}
