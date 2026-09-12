//! Bounded discovery and a rebuildable Tantivy passage index over authorized cache.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing, TextOptions, Value as _, STORED};
use tantivy::{doc, Index};
use tokio::time::Instant;

use super::web_policy::{canonical_url, error, valid_group, Policy};
use super::web_search::{Page, WebTool};
use crate::common::audit::Audit;
use crate::common::errors::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Passage {
    pub id: String,
    pub source_id: String,
    pub url: String,
    pub title: String,
    pub heading: String,
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub fetched_at: f64,
    pub content_hash: String,
    pub groups: Vec<String>,
    pub score: f32,
}

pub fn passages(page: &Page, policy: &Policy) -> Vec<Passage> {
    let Ok(url) = canonical_url(&page.url) else {
        return Vec::new();
    };
    let groups: Vec<_> = policy
        .rules
        .iter()
        .filter(|r| r.permits(&url))
        .map(|r| r.group.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let source_id = crate::common::sha256_hex(&page.url)[..16].to_string();
    let mut output = Vec::new();
    let mut start = 0;
    let mut heading = String::new();
    while start < page.text.len() {
        let tail = &page.text[start..];
        let mut len = tail.char_indices().nth(1200).map(|(i, _)| i).unwrap_or(tail.len());
        if len < tail.len() {
            if let Some(end) = tail[..len].rfind(['\n', ' ']).filter(|n| *n > len / 2) {
                len = end + 1;
            }
        }
        let end = start + len;
        let text = &page.text[start..end];
        if let Some(line) = text.lines().find(|l| l.starts_with("# ")) {
            heading = line.trim_start_matches("# ").trim().to_string();
        }
        if !text.trim().is_empty() {
            output.push(Passage {
                id: crate::common::sha256_hex(&format!("{}:{}:{start}:{end}", page.url, page.content_hash))[..20]
                    .into(),
                source_id: source_id.clone(),
                url: page.url.clone(),
                title: page.title.clone(),
                heading: heading.clone(),
                text: text.into(),
                start,
                end,
                fetched_at: page.fetched_at,
                content_hash: page.content_hash.clone(),
                groups: groups.clone(),
                score: 0.0,
            });
        }
        start = end;
    }
    output
}

pub fn retrieve(
    pages: &[Page],
    policy: &Policy,
    query: &str,
    group: Option<&str>,
    limit: usize,
) -> Result<Vec<Passage>> {
    let failed = |_| error("WEB_INDEX_FAILED", "derived passage index failed; cache can be rebuilt");
    let mut schema = Schema::builder();
    let text_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let title = schema.add_text_field("title", text_options.clone());
    let heading = schema.add_text_field("heading", text_options.clone());
    let body = schema.add_text_field("text", text_options);
    let row = schema.add_u64_field("row", STORED);
    // ponytail: rebuild from the bounded cache each query. A persistent reader
    // is justified only if measured rebuild latency dominates interactive use.
    let index = Index::create_in_ram(schema.build());
    let mut writer = index
        .writer_with_num_threads::<TantivyDocument>(1, 32_000_000)
        .map_err(failed)?;
    let evidence: Vec<_> = pages
        .iter()
        .filter(|p| p.validate(policy, group).is_ok())
        .flat_map(|p| passages(p, policy))
        .collect();
    for (i, p) in evidence.iter().enumerate() {
        writer
            .add_document(
                doc!(title => p.title.clone(), heading => p.heading.clone(), body => p.text.clone(), row => i as u64),
            )
            .map_err(failed)?;
    }
    writer.commit().map_err(failed)?;
    let reader = index.reader().map_err(failed)?;
    let searcher = reader.searcher();
    let mut parser = QueryParser::for_index(&index, vec![title, heading, body]);
    parser.set_field_boost(title, 1.6);
    parser.set_field_boost(heading, 1.3);
    // User input is data, never Tantivy query syntax. Split terms support BM25;
    // the quoted full query adds a phrase boost without an operator escape.
    let terms: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(32)
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let quoted = query.replace('\\', "\\\\").replace('"', "\\\"");
    let expression = format!(
        "\"{quoted}\"^2 {}",
        terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")
    );
    let query_expr = parser
        .parse_query(&expression)
        .map_err(|_| error("WEB_BAD_QUERY", "query cannot be indexed"))?;
    let identifier = if query.contains('_') || query.contains("::") {
        Some(
            regex::RegexBuilder::new(&format!(
                r"(?:^|[^\p{{L}}\p{{N}}_]){}(?:$|[^\p{{L}}\p{{N}}_])",
                regex::escape(query)
            ))
            .case_insensitive(true)
            .build()
            .map_err(|_| error("WEB_BAD_QUERY", "invalid identifier"))?,
        )
    } else {
        None
    };
    let mut candidates = Vec::new();
    for (score, address) in searcher
        .search(&query_expr, &TopDocs::with_limit(64).order_by_score())
        .map_err(failed)?
    {
        let found: TantivyDocument = searcher.doc(address).map_err(failed)?;
        let i = found
            .get_first(row)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| error("WEB_INDEX_FAILED", "invalid passage index row"))? as usize;
        let mut p = evidence
            .get(i)
            .ok_or_else(|| error("WEB_INDEX_FAILED", "invalid passage index row"))?
            .clone();
        if identifier
            .as_ref()
            .is_some_and(|re| !re.is_match(&p.text) && !re.is_match(&p.title) && !re.is_match(&p.heading))
        {
            continue;
        }
        // Exact identifiers/phrases get a modest deterministic bonus.
        p.score = score
            + if p.text.to_lowercase().contains(&query.to_lowercase()) {
                2.0
            } else {
                0.0
            };
        candidates.push(p);
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    let mut output = Vec::new();
    let mut seen_text = BTreeSet::new();
    let mut source_counts: BTreeMap<String, usize> = BTreeMap::new();
    // First pass keeps source diversity; second allows a neighboring passage.
    for cap in [1, 2] {
        for p in &candidates {
            if output.len() >= limit.min(16) {
                return Ok(output);
            }
            let normalized = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let hash = crate::common::sha256_hex(&normalized);
            if source_counts.get(&p.url).copied().unwrap_or(0) >= cap || seen_text.contains(&hash) {
                continue;
            }
            // Substantially overlapping boilerplate: compare word sets for this
            // small top-64 candidate set, never the whole corpus pairwise.
            let words: BTreeSet<_> = normalized.split_whitespace().collect();
            if output.iter().any(|prior: &Passage| {
                let old: BTreeSet<_> = prior.text.split_whitespace().collect();
                !words.is_empty() && words.intersection(&old).count() * 10 > words.union(&old).count() * 9
            }) {
                continue;
            }
            seen_text.insert(hash);
            *source_counts.entry(p.url.clone()).or_default() += 1;
            output.push(p.clone());
        }
    }
    Ok(output)
}

#[derive(Debug, Default, Serialize)]
pub struct Coverage {
    pub searched: Vec<String>,
    pub skipped: Vec<Value>,
    pub refused: Vec<Value>,
    pub failed: Vec<Value>,
    pub unvisited: Vec<String>,
    pub requests: usize,
    pub transferred_bytes: usize,
    pub charged_bytes: usize,
    pub budget_exhausted: Option<String>,
    pub elapsed_ms: u128,
}

impl WebTool {
    fn cache_dir(&self) -> PathBuf {
        self.tools_dir.join("web_cache")
    }
    pub(super) fn cache_path(&self, url: &str) -> PathBuf {
        self.cache_dir()
            .join(format!("{}.json", crate::common::sha256_hex(url)))
    }

    fn cache_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.cache_dir();
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Ok(m) if m.is_dir() => {}
            _ => return Err(error("WEB_CACHE_INVALID", "cache must be a directory")),
        }
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir)?.take(self.limits.cache_pages + 1) {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() != 69
                || !name.ends_with(".json")
                || !name[..64].bytes().all(|c| c.is_ascii_hexdigit())
                || !entry.file_type()?.is_file()
            {
                return Err(error(
                    "WEB_CACHE_INVALID",
                    "unexpected cache entry; repair the derived cache",
                ));
            }
            files.push(entry.path());
        }
        if files.len() > self.limits.cache_pages {
            return Err(error("WEB_CACHE_LIMIT", "cache exceeds configured page limit"));
        }
        Ok(files)
    }

    pub(super) fn cache_page(&self, page: &Page, group: Option<&str>) -> Result<()> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| error("WEB_CACHE_INVALID", "cache lock failed"))?;
        page.validate(&self.policy()?, group)?;
        let path = self.cache_path(&page.url);
        let payload = serde_json::to_vec(page)?;
        if payload.len() > self.limits.cache_bytes {
            return Err(error("WEB_CACHE_LIMIT", "document exceeds cache budget"));
        }
        let mut files: Vec<_> = self
            .cache_files()?
            .into_iter()
            .filter(|p| *p != path)
            .map(|p| std::fs::metadata(&p).map(|m| (m.modified().ok(), p, m.len() as usize)))
            .collect::<std::io::Result<_>>()?;
        files.sort_by_key(|(time, _, _)| *time);
        let mut size = files.iter().map(|(_, _, len)| len).sum::<usize>() + payload.len();
        while !files.is_empty() && (files.len() >= self.limits.cache_pages || size > self.limits.cache_bytes) {
            let (_, old, len) = files.remove(0);
            std::fs::remove_file(old)?;
            size -= len;
        }
        std::fs::create_dir_all(self.cache_dir())?;
        crate::common::atomic::write_atomic(&path, &payload, Some(0o600))
    }

    pub(super) fn cached_pages(&self, group: Option<&str>) -> Result<(Vec<Page>, Vec<Value>)> {
        let mut pages = Vec::new();
        let mut errors = Vec::new();
        let mut bytes = 0u64;
        for file in self.cache_files()? {
            bytes = bytes.saturating_add(std::fs::metadata(&file)?.len());
            if bytes > self.limits.cache_bytes as u64 {
                return Err(error("WEB_CACHE_LIMIT", "cache exceeds configured byte limit"));
            }
            match self.read_page(&file, group) {
                Ok(p) if self.cache_path(&p.url) == file => pages.push(p),
                Ok(_) => errors.push(json!({"code":"WEB_EVIDENCE_INVALID"})),
                Err(e) => errors.push(json!({"code":e.code})),
            }
        }
        pages.sort_by(|a, b| a.url.cmp(&b.url));
        Ok((pages, errors))
    }

    pub(super) async fn discover(&self, group: Option<&str>) -> Result<Coverage> {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(self.limits.run_seconds);
        let policy = self.policy()?;
        let mut queue = VecDeque::new();
        let mut coverage = Coverage::default();
        for rule in policy.rules.iter().filter(|r| group.is_none_or(|g| r.group == g)) {
            let seeds = rule.start_urls();
            if seeds.is_empty() {
                coverage
                    .skipped
                    .push(json!({"rule_id":rule.id,"code":"WEB_SEED_REQUIRED"}));
            }
            queue.extend(seeds.into_iter().map(|u| (u, true)));
        }
        let mut seen = BTreeSet::new();
        let mut site_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut site_last: BTreeMap<String, Instant> = BTreeMap::new();
        let mut robots: BTreeMap<String, Option<String>> = BTreeMap::new();
        while let Some((raw, explicit)) = queue.pop_front() {
            if !seen.insert(raw.clone()) {
                continue;
            }
            let url = match self.policy()?.authorize(&raw, group) {
                Ok(u) => u,
                Err(e) => {
                    coverage.refused.push(json!({"url":raw,"code":e.code}));
                    continue;
                }
            };
            let origin = url.origin().ascii_serialization();
            // Robots is an ordinary content request: same policy, DNS and budget.
            // A path-restricted rule does not implicitly authorize /robots.txt.
            if !robots.contains_key(&origin) {
                let robot_url = format!("{origin}/robots.txt");
                let body = if self.policy()?.authorize(&robot_url, group).is_ok() {
                    self.crawl_get(
                        &robot_url,
                        group,
                        deadline,
                        &mut coverage,
                        &mut site_counts,
                        &mut site_last,
                    )
                    .await
                    .map(|p| p.text)
                } else {
                    coverage
                        .skipped
                        .push(json!({"url":robot_url,"code":"WEB_ROBOTS_NOT_PERMITTED"}));
                    None
                };
                robots.insert(origin.clone(), body);
            }
            if url.path() == "/robots.txt" && url.query().is_none() {
                continue;
            }
            match robots.get(&origin).and_then(Option::as_ref) {
                Some(body)
                    if !robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
                        body,
                        "CGagentHarness-web",
                        url.as_str(),
                    ) =>
                {
                    coverage
                        .refused
                        .push(json!({"url":url.as_str(),"code":"WEB_ROBOTS_DENIED"}));
                    continue;
                }
                None if !explicit => {
                    coverage
                        .skipped
                        .push(json!({"url":url.as_str(),"code":"WEB_ROBOTS_UNAVAILABLE"}));
                    continue;
                }
                _ => {}
            }
            if let Some(page) = self
                .crawl_get(
                    url.as_str(),
                    group,
                    deadline,
                    &mut coverage,
                    &mut site_counts,
                    &mut site_last,
                )
                .await
            {
                self.cache_page(&page, group)?;
                coverage.searched.push(page.url.clone());
                for link in &page.links {
                    if queue.len() >= 256 {
                        coverage.budget_exhausted.get_or_insert("candidate_limit".into());
                        break;
                    }
                    if !seen.contains(link) {
                        match self.policy()?.authorize(link, group) {
                            Ok(_) => queue.push_back((link.clone(), false)),
                            Err(e) => {
                                if coverage.refused.len() < 256 {
                                    coverage.refused.push(json!({"url":link,"code":e.code}));
                                }
                            }
                        }
                    }
                }
            }
            if coverage.budget_exhausted.is_some() {
                coverage.unvisited.push(raw);
                break;
            }
        }
        coverage.unvisited.extend(queue.into_iter().map(|(u, _)| u).take(256));
        coverage.elapsed_ms = start.elapsed().as_millis();
        Ok(coverage)
    }

    async fn crawl_get(
        &self,
        url: &str,
        group: Option<&str>,
        deadline: Instant,
        coverage: &mut Coverage,
        counts: &mut BTreeMap<String, usize>,
        last: &mut BTreeMap<String, Instant>,
    ) -> Option<Page> {
        let origin = canonical_url(url).ok()?.origin().ascii_serialization();
        let budget = if coverage.requests >= self.limits.pages {
            Some("page_limit")
        } else if coverage.charged_bytes + self.limits.response_bytes > self.limits.run_bytes {
            Some("byte_limit")
        } else if Instant::now() >= deadline {
            Some("deadline")
        } else {
            None
        };
        if let Some(reason) = budget {
            coverage.budget_exhausted = Some(reason.into());
            return None;
        }
        let count = counts.entry(origin.clone()).or_default();
        if *count >= self.limits.per_site_pages {
            coverage.skipped.push(json!({"url":url,"code":"WEB_SITE_LIMIT"}));
            return None;
        }
        if let Some(previous) = last.get(&origin) {
            let ready = *previous + Duration::from_millis(self.limits.pace_ms);
            if ready >= deadline {
                coverage.budget_exhausted = Some("deadline".into());
                return None;
            }
            tokio::time::sleep_until(ready).await;
        }
        *count += 1;
        coverage.requests += 1;
        last.insert(origin, Instant::now());
        let cached = self.read_page(&self.cache_path(url), group).ok();
        match tokio::time::timeout_at(deadline, self.get(url, group, cached.as_ref())).await {
            Ok(Ok(page)) => {
                coverage.transferred_bytes += page.transfer_bytes;
                coverage.charged_bytes += page.transfer_bytes;
                Some(page)
            }
            failed => {
                // Failed/aborted streams may have transferred bytes. Charge the
                // full reserved response allowance rather than undercount them.
                coverage.charged_bytes += self.limits.response_bytes;
                let code = match failed {
                    Ok(Err(e)) => e.code,
                    _ => "WEB_TIMEOUT".into(),
                };
                coverage.failed.push(json!({"url":url,"code":code}));
                None
            }
        }
    }

    pub async fn search(
        &self,
        query: &str,
        group: Option<&str>,
        enabled: bool,
        audit: &Audit,
        owner: &str,
    ) -> Result<Value> {
        if query.trim().is_empty() || query.chars().count() > 200 || group.is_some_and(|g| !valid_group(g)) {
            return Err(error("WEB_BAD_QUERY", "invalid query or source group"));
        }
        let policy = self.require_enabled(enabled)?;
        if group.is_some_and(|g| !policy.rules.iter().any(|r| r.group == g)) {
            return Err(error("WEB_GROUP_UNKNOWN", "source group does not exist"));
        }
        self.gate_tool("web_search", &[query.into()], enabled, audit)?;
        let guard = self
            .search_gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("WEB_BUSY", "a web search is already running"))?;
        let coverage = self.discover(group).await?;
        let (pages, cache_errors) = self.cached_pages(group)?;
        let policy = self.policy()?;
        let q = query.to_string();
        let selected = group.map(str::to_string);
        let found = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            retrieve(&pages, &policy, &q, selected.as_deref(), 12)
        })
        .await
        .map_err(|_| error("WEB_INDEX_FAILED", "passage search failed"))??;
        let current = self.policy()?;
        let found: Vec<_> = found
            .into_iter()
            .filter(|p| current.authorize(&p.url, group).is_ok())
            .collect();
        if let Some(first) = found.first() {
            self.store_last(&self.read_page(&self.cache_path(&first.url), group)?, group, owner)?;
        } else {
            self.forget(enabled, owner)?;
        }
        let hits: Vec<_> = found
            .iter()
            .map(|p| json!({"url":p.url,"snippets":[p.text],"passage_id":p.id}))
            .collect();
        Ok(
            json!({"query":query,"group":group,"passages":found,"hits":hits,"coverage":coverage,
            "errors":coverage.failed,"cache_errors":cache_errors,"scanned":coverage.searched.len(),
            "index":"tantivy-bm25","complete":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::web_policy::Rule;
    use super::*;
    use crate::common::config::AppConfig;
    use axum::{http::HeaderMap, routing::get, Router};
    use std::path::Path;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn discovery_obeys_robots_groups_limits_validators_and_cache_revocation() {
        let requests = Arc::new(AtomicUsize::new(0));
        let child_requests = requests.clone();
        let fixture = Router::new()
            .route("/robots.txt", get(|| async { ([("content-type","text/plain")], "User-agent: *\nDisallow: /docs/private\n") }))
            .route("/docs/", get(|| async { ([("content-type","text/html")], "<h1>Widget docs</h1><a href='nested'>Guide</a><a href='private'>Private</a><a href='/outside'>Outside</a>") }))
            .route("/docs/nested", get(move |headers: HeaderMap| { let count = child_requests.clone(); async move {
                count.fetch_add(1, Ordering::SeqCst);
                if headers.get("if-none-match").is_some_and(|h| h == "fixture-v1") {
                    (axum::http::StatusCode::NOT_MODIFIED, [("content-type","text/html"), ("etag","fixture-v1")], "")
                } else {
                    (axum::http::StatusCode::OK, [("content-type","text/html"), ("etag","fixture-v1")], "<title>Widget retries</title><h2>Retries</h2><p>Set WIDGET_RETRY_COUNT to 3 when a connection fails.</p>")
                }
            }}))
            .route("/docs/private", get(|| async { panic!("robots-disallowed page fetched"); #[allow(unreachable_code)] "" }))
            .route("/outside", get(|| async { panic!("policy-disallowed page fetched"); #[allow(unreachable_code)] "" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::from_str("web: {pace_ms: 100}", Path::new("config.yaml")).unwrap();
        crate::common::atomic::write_json_atomic(
            &dir.path().join("web_allowlist.json"),
            &json!({"version":1,"rules":[]}),
        )
        .unwrap();
        let mut web = WebTool::new(dir.path(), &cfg).unwrap();
        web.test_resolve = Some(("corpus.invalid".into(), address));
        let root = format!("http://corpus.invalid:{}/", address.port());
        let scope = format!("{root}docs/*");
        web.allow_rule(&scope, "manuals", &[format!("{root}docs/")], true)
            .unwrap();
        web.allow_rule(&format!("{root}robots.txt"), "manuals", &[], true)
            .unwrap();
        // Another group grants outside scope, but selected group must still refuse it.
        web.allow_rule(&format!("{root}outside"), "other", &[], true).unwrap();
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let found = web
            .search("WIDGET_RETRY_COUNT", Some("manuals"), true, &audit, "local")
            .await
            .unwrap();
        assert_eq!(found["hits"].as_array().unwrap().len(), 1, "{found}");
        assert_eq!(found["hits"][0]["url"], format!("{root}docs/nested"));
        assert!(found["coverage"]["refused"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "WEB_ROBOTS_DENIED"));
        assert!(found["coverage"]["refused"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "WEB_HOST_DENIED"));
        let first_bytes = found["coverage"]["transferred_bytes"].as_u64().unwrap();
        let again = web
            .search("connection retry", Some("manuals"), true, &audit, "local")
            .await
            .unwrap();
        assert!(
            again["coverage"]["transferred_bytes"].as_u64().unwrap() < first_bytes,
            "304 should avoid the unchanged body"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(
            web.search("x", Some("absent"), true, &audit, "local")
                .await
                .unwrap_err()
                .code,
            "WEB_GROUP_UNKNOWN"
        );
        web.deny(&scope, true).unwrap();
        let (cached, _) = web.cached_pages(Some("manuals")).unwrap();
        assert!(cached.iter().all(|p| !p.text.contains("WIDGET_RETRY_COUNT")));
        assert!(web.context_text(true, "local").is_empty());
        web.allow_rule(&scope, "manuals", &[format!("{root}docs/")], true)
            .unwrap();
        web.limits.pages = 1;
        let limited = web.discover(Some("manuals")).await.unwrap();
        assert!(limited.requests <= 1);
        assert_eq!(limited.budget_exhausted.as_deref(), Some("page_limit"));
        assert!(!limited.unvisited.is_empty());
        // No cache/derived-index corruption can modify authoritative rules.
        let revision = web.policy().unwrap().revision;
        std::fs::write(web.cache_path(&format!("{root}docs/nested")), b"broken derived cache").unwrap();
        let (_, errors) = web.cached_pages(None).unwrap();
        assert!(errors.iter().any(|e| e["code"] == "WEB_EVIDENCE_INVALID"));
        assert_eq!(web.policy().unwrap().revision, revision);
        server.abort();
    }

    #[test]
    fn passages_keep_original_offsets_stable_ids_and_literal_query_boundaries() {
        let policy = Policy {
            version: 1,
            rules: vec![Rule::new("https://docs.example/*", "docs", &[]).unwrap()],
            revision: "a".repeat(64),
        };
        let text = format!(
            "# Heading\n{}\nIdentifier WIDGET_RETRY_COUNT restarts connections.",
            "İK text ".repeat(250)
        );
        let page = Page {
            url: "https://docs.example/nested".into(),
            title: "Widget guide".into(),
            content_hash: crate::common::sha256_hex(&text),
            chars: text.chars().count(),
            text,
            links: vec![],
            status: 200,
            content_type: "text/plain".into(),
            bytes: 0,
            transfer_bytes: 0,
            fetched_at: crate::common::now_ts(),
            extraction_version: 1,
            policy_revision: policy.revision.clone(),
            etag: None,
            last_modified: None,
        };
        let parts = passages(&page, &policy);
        for part in &parts {
            assert_eq!(part.text, page.text[part.start..part.end]);
        }
        assert_eq!(
            parts.iter().map(|p| &p.id).collect::<Vec<_>>(),
            passages(&page, &policy).iter().map(|p| &p.id).collect::<Vec<_>>()
        );
        let hit = retrieve(
            std::slice::from_ref(&page),
            &policy,
            "WIDGET_RETRY_COUNT",
            Some("docs"),
            3,
        )
        .unwrap();
        assert_eq!(hit.len(), 1);
        assert!(hit[0].text.contains("WIDGET_RETRY_COUNT"));
        assert!(retrieve(std::slice::from_ref(&page), &policy, ".*", None, 3)
            .unwrap()
            .is_empty());
        assert!(retrieve(&[page], &policy, "Widget", Some("other"), 3)
            .unwrap()
            .is_empty());
    }
}
