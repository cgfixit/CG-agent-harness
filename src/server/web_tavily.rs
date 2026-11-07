//! Tavily search listings: the keyed `web.search.provider: tavily` hop. The
//! request pins `include_answer`, `include_raw_content` and `include_images` off
//! and the parser retains bounded listing fields and ranking metadata, so neither a
//! provider-written answer nor a page body enters a listing. Linked pages still
//! need their own URL permission.
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::web_google::{add_result, cooldown_error, listing_body, unix_now, SearchResult, SEARCH_NOTICE};
use super::web_policy::error;
use super::web_search::WebTool;
use crate::common::audit::Audit;
use crate::common::errors::Result;

/// The fixed production endpoint; only tests substitute a loopback fixture.
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const PROVIDER: &str = "tavily";

/// Rows from a Tavily answer. Only `title`, `url` and `content` are read: an
/// `answer` or `raw_content` field is never copied, whatever the request said.
pub(super) fn tavily_results(body: &[u8], limit: usize) -> Result<Vec<SearchResult>> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| error("WEB_SEARCH_RESPONSE", "invalid search response"))?;
    let items = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| error("WEB_SEARCH_RESPONSE", "search result listing is missing"))?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, item) in items.iter().take(100).enumerate() {
        let before = rows.len();
        add_result(
            &mut rows,
            &mut seen,
            item["title"].as_str().unwrap_or(""),
            item["url"].as_str().unwrap_or(""),
            item["content"].as_str().unwrap_or(""),
            limit,
        );
        if rows.len() > before {
            let row = rows.last_mut().unwrap();
            row.provider_rank = Some(index as u64 + 1);
            row.provider_score = item["score"]
                .as_f64()
                .filter(|s| s.is_finite() && (0.0..=1.0).contains(s));
        }
    }
    Ok(rows)
}

/// A rejected key stays loud; a spent plan (432), pay-as-you-go cap (433) or
/// rate limit (429) is a quota, which pauses Tavily and may fall through.
fn refusal_code(status: u16) -> Option<&'static str> {
    match status {
        200 => None,
        401 | 403 => Some("WEB_SEARCH_KEY_REJECTED"),
        429 | 432 | 433 => Some("WEB_SEARCH_QUOTA"),
        _ => Some("WEB_SEARCH_PROVIDER"),
    }
}

impl WebTool {
    pub(super) fn tavily_endpoint(&self) -> url::Url {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(fixture) = &self.test_tavily_search {
            return fixture.clone();
        }
        url::Url::parse(TAVILY_ENDPOINT).expect("static URL")
    }

    /// One Tavily hop: web/account authority, no URL grant, one basic-depth
    /// request (one credit) inside `web.request_seconds`. The caller
    /// (`google_search_at`) holds the broker decision and the single search gate.
    /// The key travels only in the Authorization header to `endpoint`.
    pub(super) async fn tavily_listing(
        &self,
        query: &str,
        count: usize,
        audit: &Audit,
        endpoint: url::Url,
        key: &str,
    ) -> Result<Value> {
        // A paused provider answers before any request.
        if let Some(pause) = self.search_cooldown.holding(PROVIDER, Instant::now()) {
            return Err(cooldown_error(PROVIDER, &pause));
        }
        let fetched = tokio::time::timeout(Duration::from_secs(self.limits.request_seconds), async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| error("WEB_CANCELLED", "search cancelled"))?;
            let client = self.pinned_client(&endpoint).await?;
            let response = client
                .post(endpoint)
                .bearer_auth(key)
                .json(
                    &json!({"query":query,"search_depth":"basic","max_results":count,"topic":"general",
                    "include_answer":false,"include_raw_content":false,"include_images":false,
                    "auto_parameters":false}),
                )
                .send()
                .await
                .map_err(|_| error("WEB_SEARCH_PROVIDER", "search provider request failed"))?;
            if let Some(code) = refusal_code(response.status().as_u16()) {
                return Err(error(
                    code,
                    "search provider refused the request; check API Keys and provider quota",
                ));
            }
            let body = listing_body(response, self.limits.response_bytes).await?;
            let rows = tavily_results(&body, count)?;
            if serde_json::to_string(&rows)?.contains(key) {
                return Err(error(
                    "WEB_SEARCH_RESPONSE",
                    "credential reflected in provider results; response discarded",
                ));
            }
            Ok(rows)
        })
        .await
        .map_err(|_| error("WEB_TIMEOUT", "search deadline exceeded"))
        .and_then(|rows| rows);
        let rows = match fetched {
            Ok(rows) => rows,
            Err(failure) => {
                // One audit line when a pause starts; none while it holds.
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
        if self.tavily_key()?.trim() != key {
            return Err(error("WEB_SEARCH_KEY_CHANGED", "Search key changed; retry the search"));
        }
        Ok(json!({"query":query,"provider":PROVIDER,"results":rows,"notice":SEARCH_NOTICE,"complete":false}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tavily_rows_keep_title_url_and_snippet_and_never_the_answer_or_page_body() {
        let body = json!({"query":"q","answer":"PROVIDER ANSWER","images":[],"response_time":0.4,
            "results":[
                {"title":"First","url":"https://veeam.com/kb1","content":"Evidence","score":0.91,
                 "raw_content":"FULL PAGE BODY"},
                {"title":"Loop","url":"http://127.0.0.1:8080/admin","content":"private"}, // DevSkim: ignore DS137138,DS162092 because this asserts a loopback result is dropped.
                {"title":"","url":"https://example.org/untitled","content":"x"},
                {"title":"Dup","url":"https://veeam.com/kb1","content":"again"},
                {"title":"Second","url":"https://nvd.nist.gov/vuln","content":"More"}]});
        let rows = tavily_results(body.to_string().as_bytes(), 5).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://veeam.com/kb1");
        assert_eq!(rows[0].snippet, "Evidence");
        assert_eq!(rows[1].rank, 2);
        let rendered = serde_json::to_string(&rows).unwrap();
        assert_eq!(rows[0].provider_score, Some(0.91));
        assert_eq!(rows[1].provider_rank, Some(5));
        for absent in ["PROVIDER ANSWER", "FULL PAGE BODY", "127.0.0.1"] {
            assert!(!rendered.contains(absent), "{absent}: {rendered}");
        }
        let long = "y".repeat(5000);
        let rows = tavily_results(
            json!({"results":[{"title":"Long","url":"https://example.org/l","content":long}]})
                .to_string()
                .as_bytes(),
            5,
        )
        .unwrap();
        assert_eq!(
            rows[0].snippet.chars().count(),
            1000,
            "snippets are clipped like every listing"
        );
        assert!(tavily_results(br#"{"results":[]}"#, 5).unwrap().is_empty());
        for bad in [
            &br#"{"detail":{"error":"secret upstream text"}}"#[..],
            b"<html>",
            b"\xff\xfe{",
        ] {
            let failure = tavily_results(bad, 5).unwrap_err();
            assert_eq!(failure.code, "WEB_SEARCH_RESPONSE");
            assert!(!failure.to_string().contains("secret"));
        }
    }

    #[test]
    fn tavily_statuses_map_to_loud_keys_quota_or_an_unusable_provider() {
        assert_eq!(refusal_code(200), None);
        for status in [401, 403] {
            assert_eq!(refusal_code(status), Some("WEB_SEARCH_KEY_REJECTED"), "{status}");
        }
        for status in [429, 432, 433] {
            assert_eq!(refusal_code(status), Some("WEB_SEARCH_QUOTA"), "{status}");
        }
        for status in [400, 422, 434, 500, 502] {
            assert_eq!(refusal_code(status), Some("WEB_SEARCH_PROVIDER"), "{status}");
        }
    }
}
