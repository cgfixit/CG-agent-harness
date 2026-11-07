//! Active-goal references reuse the existing authorized cache, never a second corpus.
use serde_json::{json, Value};

use crate::common::{audit::Audit, errors::Result};
use crate::server::{
    web_index::{passages, retrieve, Passage},
    web_policy::{canonical_url, error},
    web_research::Answer,
    web_search::{run_web_cpu, WebTool},
};

#[derive(Clone, Debug)]
pub(crate) struct Receipt {
    pub url: String,
    pub content_hash: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RunEvidence {
    pub receipts: Vec<Receipt>,
    pub queries: Vec<String>,
    pub packet: Option<Value>,
}

impl RunEvidence {
    pub fn remember(&mut self, evidence: &[Passage]) {
        for passage in evidence {
            if let Some(receipt) = self.receipts.iter_mut().find(|r| r.url == passage.url) {
                if receipt.content_hash != passage.content_hash {
                    receipt.content_hash.clone_from(&passage.content_hash);
                    receipt.ids.clear();
                }
                if !receipt.ids.contains(&passage.id) {
                    receipt.ids.push(passage.id.clone());
                    if receipt.ids.len() > 3 {
                        receipt.ids.remove(0);
                    }
                }
            } else {
                self.receipts.push(Receipt {
                    url: passage.url.clone(),
                    content_hash: passage.content_hash.clone(),
                    ids: vec![passage.id.clone()],
                });
                if self.receipts.len() > 9 {
                    self.receipts.remove(0);
                }
            }
        }
    }

    pub fn searched(&mut self, query: &str) {
        if !self.queries.iter().any(|q| q == query) {
            self.queries.push(query.into());
            if self.queries.len() > 16 {
                self.queries.remove(0);
            }
        }
    }

    pub fn restore(&self, web: &WebTool) -> Result<(Vec<Passage>, Option<Value>)> {
        let policy = web.require_enabled(true)?;
        let mut evidence = Vec::new();
        for receipt in &self.receipts {
            if let Ok(page) = web.read_page(&web.cache_path(&receipt.url), None) {
                if page.content_hash == receipt.content_hash && fresh(page.fetched_at, web.limits.stale_seconds) {
                    evidence.extend(
                        passages(&page, &policy)
                            .into_iter()
                            .filter(|p| receipt.ids.contains(&p.id)),
                    );
                }
            }
        }
        let packet = self
            .packet
            .as_ref()
            .filter(|_| !evidence.is_empty())
            .and_then(|packet| Answer::parse(&packet.to_string(), &evidence).ok())
            .and_then(|(answer, _)| serde_json::to_value(answer).ok());
        Ok((evidence, packet))
    }
}

fn fresh(fetched_at: f64, stale_seconds: u64) -> bool {
    let age = crate::common::now_ts() - fetched_at;
    age >= 0.0 && age <= stale_seconds as f64
}

pub(super) async fn read(
    web: &WebTool,
    url: &str,
    query: &str,
    refresh: bool,
    audit: &Audit,
    owner: &str,
) -> Result<(Value, Vec<Passage>)> {
    let url = canonical_url(url)?.to_string();
    web.require_enabled(true)?.authorize(&url, None)?;
    let cached = if !refresh {
        let reader = web.clone();
        let target = url.clone();
        tokio::task::spawn_blocking(move || reader.read_page(&reader.cache_path(&target), None))
            .await
            .map_err(|_| error("WEB_CACHE_INVALID", "cache reader stopped"))?
            .ok()
            .filter(|page| fresh(page.fetched_at, web.limits.stale_seconds))
    } else {
        None
    };
    let reused = cached.is_some();
    let page = match cached {
        Some(page) => {
            web.gate_tool("web_fetch", std::slice::from_ref(&url), true, audit)?;
            web.store_last(&page, None, owner)?;
            page
        }
        None => web.fetch_page(&url, true, audit, owner).await?,
    };
    let policy = web.policy()?;
    page.validate(&policy, None)?;
    let selected_page = page.clone();
    let question = query.to_string();
    let permit = web
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| error("WEB_CANCELLED", "passage selection cancelled"))?;
    let selected = run_web_cpu(
        tokio::time::Instant::now() + std::time::Duration::from_secs(web.limits.request_seconds),
        permit,
        move |checkpoint| {
            checkpoint()?;
            let mut selected = retrieve(std::slice::from_ref(&selected_page), &policy, &question, None, 3)?;
            if selected.is_empty() {
                selected = passages(&selected_page, &policy).into_iter().take(3).collect();
            }
            checkpoint()?;
            Ok(selected)
        },
    )
    .await?;
    page.validate(&web.policy()?, None)?;
    let text = selected
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut value = json!({"url":page.url,"title":page.title,"text":crate::common::clip_chars(&text, (web.limits.evidence_tokens * 4) as usize),
        "source_chars":page.chars,"fetched_at":page.fetched_at,"content_hash":page.content_hash,"cache_reused":reused,
        "notice":"Untrusted selected page passages; incomplete coverage. Fetch time is not publication time. Only the listed receipts were delivered."});
    if let Some(via) = page.via {
        value["via"] = json!(via);
    }
    delivered(&mut value, selected.clone());
    Ok((value, selected))
}

/// A cut tool result cannot attest to a passage the model did not receive whole.
pub(super) fn delivered(result: &mut Value, selected: Vec<Passage>) -> Vec<Passage> {
    let text = result["text"].as_str().unwrap_or("");
    let selected: Vec<_> = selected.into_iter().filter(|p| text.contains(&p.text)).collect();
    result["receipts"] = json!(selected
        .iter()
        .map(|p| json!({"id":p.id,"start":p.start,"end":p.end}))
        .collect::<Vec<_>>());
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn late_passages_survive_and_cache_reuse_respects_refresh_hash_and_revocation() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = axum::Router::new().route("/manual", axum::routing::get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], format!("{}\n\n## Recovery\nThe RECOVERY_ANCHOR requires two independent replicas before failback.\n", "Background material about unrelated widgets.\n\n".repeat(1600)))
            }
        }));
        let app = app.route("/other", axum::routing::get(|| async { "OTHER_PAGE" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let cfg =
            crate::common::config::AppConfig::from_str("web: {pace_ms: 100}", std::path::Path::new("config.yaml"))
                .unwrap();
        let mut web = WebTool::new(dir.path(), &cfg).unwrap();
        web.test_resolve = Some(("passages.invalid".into(), address));
        let url = format!("http://passages.invalid:{}/manual", address.port()); // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
        web.allow_rule(&url, "manuals", &[], true).unwrap();
        web.allow_rule("https://example.com/", "other", &[], true).unwrap();
        let audit = Audit::new(dir.path().join("audit.jsonl"), &cfg);
        let (first, selected) = read(&web, &url, "RECOVERY_ANCHOR", true, &audit, "owner")
            .await
            .unwrap();
        assert!(first["text"].as_str().unwrap().contains("two independent replicas"));
        assert!(selected[0].start > 24_000, "the anchor must be beyond the old prefix");
        let other = format!("http://passages.invalid:{}/other", address.port()); // DevSkim: ignore DS137138 because this synthetic host resolves only to the loopback fixture.
        web.allow_rule(&other, "other", &[], true).unwrap();
        read(&web, &other, "OTHER_PAGE", true, &audit, "owner").await.unwrap();
        let (cached, _) = read(&web, &url, "RECOVERY_ANCHOR", false, &audit, "owner")
            .await
            .unwrap();
        assert_eq!(cached["cache_reused"], true);
        web.inject(true, "owner").unwrap();
        assert!(web
            .context_text(true, "owner")
            .contains("Background material about unrelated widgets"));
        assert!(!web.context_text(true, "owner").contains("OTHER_PAGE"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        read(&web, &url, "RECOVERY_ANCHOR", true, &audit, "owner")
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let mut run = RunEvidence::default();
        run.remember(&selected);
        run.packet = Some(
            json!({"supported":[{"text":"Two replicas are required.","citations":[{"id":selected[0].id,"quote":"two independent replicas"}]}],"conflicts":[],"inferences":[],"missing":[]}),
        );
        assert!(run.restore(&web).unwrap().1.is_some());
        web.deny(&url, true).unwrap();
        let (restored, packet) = run.restore(&web).unwrap();
        assert!(restored.is_empty() && packet.is_none());
        web.allow_rule(&url, "manuals", &[], true).unwrap();
        let path = web.cache_path(&url);
        let mut page: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        page["text"] = json!("tampered");
        std::fs::write(&path, page.to_string()).unwrap();
        assert!(run.restore(&web).unwrap().0.is_empty());
        std::fs::remove_file(path).unwrap();
        assert!(run.restore(&web).unwrap().0.is_empty());
        server.abort();
    }

    #[test]
    fn cut_results_cannot_keep_undelivered_receipts_and_queries_are_bounded() {
        let passage = Passage {
            id: "id".into(),
            source_id: "source".into(),
            url: "https://example.com/".into(),
            title: String::new(),
            heading: String::new(),
            text: "whole passage".into(),
            start: 0,
            end: 13,
            fetched_at: 0.0,
            content_hash: String::new(),
            groups: vec![],
            score: 1.0,
        };
        let mut result = json!({"text":"whole pass"});
        assert!(delivered(&mut result, vec![passage.clone()]).is_empty());
        result["text"] = json!("whole passage");
        assert_eq!(delivered(&mut result, vec![passage]).len(), 1);
        let mut run = RunEvidence::default();
        for i in 0..20 {
            run.searched(&format!("query {i}"));
        }
        run.searched("query 19");
        assert_eq!(run.queries.len(), 16);
        assert!(!fresh(crate::common::now_ts() + 10.0, 60));
    }
}
