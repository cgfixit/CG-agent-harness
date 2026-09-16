//! Google result listings, distinct from permission to read linked pages.
use std::collections::BTreeSet;
use std::time::Duration;

use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::{json, Value};

use super::web_policy::{canonical_url, error};
use super::web_search::WebTool;
use crate::common::audit::Audit;
use crate::common::errors::Result;

#[derive(Debug, Serialize)]
struct SearchResult {
    rank: usize,
    title: String,
    url: String,
    snippet: String,
}

fn add_result(
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
        add_result(
            &mut rows,
            &mut seen,
            item["title"].as_str().unwrap_or(""),
            item["link"].as_str().unwrap_or(""),
            item["snippet"].as_str().unwrap_or(""),
            limit,
        );
    }
    Ok(rows)
}

fn public_results(body: &str, limit: usize) -> Result<Vec<SearchResult>> {
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
    let html = Html::parse_document(body);
    let anchors = Selector::parse("a[href]").unwrap();
    let headings = Selector::parse("h3").unwrap();
    let base = url::Url::parse("https://www.google.com/").unwrap();
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in html.select(&anchors) {
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
    Ok(rows)
}

impl WebTool {
    /// Read the private saved key on demand; an explicit process environment still wins.
    pub(super) fn search_key(&self) -> Result<String> {
        if !self.search_key_from_file {
            if let Ok(key) = std::env::var("SERPAPI_API_KEY") {
                return Ok(key.trim().to_string());
            }
        }
        let path = self.tools_dir.parent().unwrap_or(&self.tools_dir).join(".env");
        super::env_keys::read_startup_keys(&path)
            .map(|mut keys| keys.remove("SERPAPI_API_KEY").unwrap_or_default())
            .map_err(|_| {
                error(
                    "WEB_SEARCH_KEY_UNAVAILABLE",
                    "Cannot read the saved search key; check API Keys and credential-file permissions",
                )
            })
    }

    /// A key chooses the fixed SerpAPI Google backend. It is never sent to Google
    /// or a returned result URL. No fallback on a configured provider's failure.
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
        let key = self.search_key()?;
        // Provider listings require web/account authority, not page-content permission.
        // Public Google is a page fetch and retains the original URL policy checks.
        let policy = if key.is_empty() {
            Some(self.require_enabled(enabled)?)
        } else {
            None
        };
        let target = ["https://www.google.com/search", "https://google.com/search"]
            .into_iter()
            .find_map(|base| {
                let mut url = url::Url::parse(base).unwrap();
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
        self.gate_tool("web_search", &[query.into()], enabled, audit)?;
        let _gate = self
            .search_gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("WEB_BUSY", "a web search is already running"))?;
        let provider = if key.trim().is_empty() {
            "google-public"
        } else {
            "google-serpapi"
        };
        let rows = if key.trim().is_empty() {
            let (_, body) = self.get_raw(target.as_str(), None, None).await.map_err(|failure| {
                if matches!(failure.code.as_str(), "WEB_FETCH_FAILED" | "WEB_REDIRECT_REFUSED") {
                    error(
                        "WEB_GOOGLE_BLOCKED",
                        "Public Google refused or redirected the request; configure a SerpAPI key in API Keys",
                    )
                } else {
                    failure
                }
            })?;
            public_results(&body, count)?
        } else {
            self.api_search(endpoint, query, count, key.trim()).await?
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
        if self.search_key()? != key {
            return Err(error("WEB_SEARCH_KEY_CHANGED", "Search key changed; retry the search"));
        }
        Ok(
            json!({"query":query,"provider":provider,"search_url":target.as_str(),"results":rows,
            "notice":"Search-provider listings only. Linked pages have not been fetched and require their own URL permission.","complete":false}),
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
            let mut response = client
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

#[cfg(test)]
mod tests {
    use super::*;
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
        crate::common::home::Home::at(dir.path().to_path_buf())
            .ensure_layout()
            .unwrap();
        let cfg = crate::common::config::AppConfig::from_str("{}", std::path::Path::new("config.yaml")).unwrap();
        let mut web = WebTool::new(&dir.path().join("tools"), &cfg).unwrap();
        web.search_key_from_file = true;
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
        let result = web
            .google_search_at("veeam", 5, true, &audit, endpoint.clone())
            .await
            .unwrap();
        assert_eq!(result["provider"], "google-serpapi");
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
