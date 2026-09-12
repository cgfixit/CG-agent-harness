//! Fixed-corpus benchmark. `cargo run --example web_research_benchmark -- OUTPUT.json [--live]`.
//! Fixture DNS pins are programmatic only. --live uses the configured local model;
//! content always comes from these disposable local virtual hosts.
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cgagentharness::{
    common::{config::AppConfig, home::Home},
    server::{build_app, web_research, web_search::extract, AppOptions},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

#[derive(Clone, Deserialize, Serialize)]
struct Document {
    site: String,
    path: String,
    html: String,
}
#[derive(Clone, Deserialize, Serialize)]
struct Question {
    kind: String,
    query: String,
    relevant: Vec<String>,
}
#[derive(Clone, Deserialize)]
struct Corpus {
    pages: Vec<Document>,
    queries: Vec<Question>,
}
#[derive(Clone)]
struct Pages {
    corpus: Arc<Corpus>,
    calls: Arc<AtomicUsize>,
    legacy_offline: Arc<AtomicBool>,
    forbidden: Arc<AtomicUsize>,
}
async fn page(State(state): State<Pages>, req: Request) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let site = if req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .starts_with("reference.invalid")
    {
        "reference"
    } else {
        "manual"
    };
    let path = req.uri().path();
    if path == "/private" {
        state.forbidden.fetch_add(1, Ordering::SeqCst);
    }
    if path == "/robots.txt" {
        return ([("content-type", "text/plain")], "User-agent: *\nDisallow: /private\n").into_response();
    }
    if path == "/legacy" && state.legacy_offline.load(Ordering::SeqCst) {
        return axum::http::StatusCode::BAD_GATEWAY.into_response();
    }
    let Some(doc) = state.corpus.pages.iter().find(|d| d.site == site && d.path == path) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    if req.headers().get("if-none-match").is_some_and(|v| v == "fixture-v1") {
        return axum::http::StatusCode::NOT_MODIFIED.into_response();
    }
    (
        [("content-type", "text/html; charset=utf-8"), ("etag", "fixture-v1")],
        doc.html.clone(),
    )
        .into_response()
}
async fn model(Json(request): Json<Value>) -> Json<Value> {
    assert!(request.get("tools").is_none());
    let prompt: Value = serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let question = prompt["question"].as_str().unwrap_or("");
    let answer = if request["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("Return only JSON")
    {
        let queries = if question.contains("credentials") {
            vec!["password replacement", "changing password", "session cookies"]
        } else {
            vec![question]
        };
        json!({"queries":queries,"gaps":[]})
    } else {
        let evidence = prompt["evidence"].as_array().unwrap();
        let claims:Vec<_>=evidence.iter().take(2).map(|p|json!({"text":p["text"].as_str().unwrap().chars().take(200).collect::<String>(),"citations":[{"id":p["id"],"quote":p["text"].as_str().unwrap().chars().take(120).collect::<String>()}]})).collect();
        let conflicts = if question.contains("retry count") && claims.len() == 2 {
            vec![
                json!({"text":"The Atlas v1 sources disagree: the manual says three attempts and the reference says five.","citations":[claims[0]["citations"][0],claims[1]["citations"][0]]}),
            ]
        } else {
            vec![]
        };
        json!({"supported":claims,"conflicts":conflicts,"inferences":[],"missing":["Synthetic fixture synthesis; not live model evidence."]})
    };
    Json(
        json!({"model":"fixture","choices":[{"finish_reason":"stop","message":{"content":answer.to_string()}}],"usage":{"prompt_tokens":120,"completion_tokens":80}}),
    )
}
fn source(url: &str) -> String {
    let url = url::Url::parse(url).unwrap();
    format!("{}{}", url.host_str().unwrap().split('.').next().unwrap(), url.path())
}
fn quality(returned: &BTreeSet<String>, relevant: &[String]) -> Value {
    let expected: BTreeSet<_> = relevant.iter().cloned().collect();
    let correct = returned.intersection(&expected).count();
    json!({"returned_sources":returned,"correct_sources":correct,"recall":if expected.is_empty(){if returned.is_empty(){1.0}else{0.0}}else{correct as f64/expected.len() as f64},"precision":if returned.is_empty(){if expected.is_empty(){1.0}else{0.0}}else{correct as f64/returned.len() as f64}})
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 2, "provide output JSON path");
    let live = args.iter().any(|s| s == "--live");
    let corpus: Corpus = serde_json::from_str(include_str!("../tests/fixtures/web-research/corpus.json"))?;
    let corpus = Arc::new(corpus);
    let fixture = Pages {
        corpus: corpus.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
        legacy_offline: Arc::new(AtomicBool::new(false)),
        forbidden: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = Router::new().fallback(get(page)).with_state(fixture.clone());
    let content_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let model_base = format!("http://{}/v1", listener.local_addr()?);
    let model_task = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/chat/completions", post(model)))
            .await
            .unwrap();
    });
    let dir = tempfile::tempdir()?;
    let home = Home::at(dir.path().into());
    home.ensure_layout()?;
    let mut config: serde_yaml_ng::Value = serde_yaml_ng::from_str(AppConfig::embedded_default())?;
    config["auth"]["enabled"] = false.into();
    config["tls"]["enabled"] = false.into();
    config["web"]["pace_ms"] = 100.into();
    config["web"]["research_seconds"] = 300.into();
    if !live {
        config["models"]["local_llm"]["base_url"] = model_base.into();
    }
    let text = serde_yaml_ng::to_string(&config)?;
    std::fs::write(home.config_path(), &text)?;
    let mut options = AppOptions::new(home);
    options.config = Some(AppConfig::from_str(&text, &dir.path().join("config.yaml"))?);
    options.web_test_resolve = Some(("manual.invalid".into(), address));
    options.web_test_resolve_extra = vec![("reference.invalid".into(), address)];
    let (router, state) = build_app(options).await?;
    drop(router);
    state.settings.lock().unwrap().web_enabled = true;
    for site in ["manual", "reference"] {
        let root = format!("http://{site}.invalid:{}/", address.port());
        state.web.allow_rule(&format!("{root}*"), "corpus", &[root], true)?;
    }
    let revision = state.web.policy()?.revision;
    let baseline_re = regex::Regex::new("<[^>]*>")?;
    // Historical algorithm: escaped case-insensitive literal matching over only
    // the saved initial URLs. Fixture roots contain no entities/scripts/styles,
    // so removing tags gives identical searchable text to the old extractor.
    let baseline: Vec<_> = corpus
        .pages
        .iter()
        .filter(|p| p.path == "/")
        .map(|p| {
            (
                format!("{}{}", p.site, p.path),
                baseline_re
                    .replace_all(&p.html, " ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })
        .collect();
    let mut results = Vec::new();
    for case in &corpus.queries {
        eprintln!(
            "benchmark {} ({})",
            case.kind,
            if live { "local model" } else { "fixture model" }
        );
        if case.kind == "stale" {
            let url = format!("http://manual.invalid:{}/legacy", address.port());
            let path = state
                .home
                .tools_dir()
                .join("web_cache")
                .join(format!("{}.json", cgagentharness::common::sha256_hex(&url)));
            let mut cached: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            cached["fetched_at"] = json!(cgagentharness::common::now_ts() - 30.0 * 86400.0);
            std::fs::write(path, serde_json::to_vec(&cached)?)?;
            fixture.legacy_offline.store(true, Ordering::SeqCst);
        }
        let start = Instant::now();
        let query = regex::RegexBuilder::new(&regex::escape(&case.query))
            .case_insensitive(true)
            .build()?;
        let old: BTreeSet<_> = baseline
            .iter()
            .filter(|(_, text)| query.is_match(text))
            .map(|(id, _)| id.clone())
            .collect();
        let old_us = start.elapsed().as_micros();
        let requests = fixture.calls.load(Ordering::SeqCst);
        let start = Instant::now();
        let search = state
            .web
            .search(&case.query, Some("corpus"), true, &state.audit, "local")
            .await?;
        let search_ms = start.elapsed().as_millis();
        let search_requests = fixture.calls.load(Ordering::SeqCst) - requests;
        let found: BTreeSet<_> = search["passages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| source(p["url"].as_str().unwrap()))
            .collect();
        let before = fixture.calls.load(Ordering::SeqCst);
        let result = web_research::run(state.clone(), "local", &case.query, Some("corpus")).await?;
        let evidence: BTreeSet<_> = result["passages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| source(p["url"].as_str().unwrap()))
            .collect();
        let mut citations = 0;
        let mut valid = 0;
        for category in ["supported", "conflicts", "inferences"] {
            for claim in result["answer"][category].as_array().unwrap() {
                for citation in claim["citations"].as_array().unwrap() {
                    citations += 1;
                    if result["passages"].as_array().unwrap().iter().any(|p| {
                        p["id"] == citation["id"]
                            && p["text"]
                                .as_str()
                                .unwrap()
                                .contains(citation["quote"].as_str().unwrap())
                    }) {
                        valid += 1;
                    }
                }
            }
        }
        for passage in result["passages"].as_array().unwrap() {
            let url = url::Url::parse(passage["url"].as_str().unwrap())?;
            let doc = corpus
                .pages
                .iter()
                .find(|d| format!("{}.invalid", d.site) == url.host_str().unwrap() && d.path == url.path())
                .unwrap();
            let (_, text, _) = extract(&doc.html, "text/html", &url);
            assert_eq!(
                &text[passage["start"].as_u64().unwrap() as usize..passage["end"].as_u64().unwrap() as usize],
                passage["text"].as_str().unwrap()
            );
        }
        results.push(json!({"case":case,"baseline":{"quality":quality(&old,&case.relevant),"matching_microseconds":old_us,"requests_per_query":2,"matching_only":true},"search":{"quality":quality(&found,&case.relevant),"elapsed_ms":search_ms,"requests":search_requests,"coverage":search["coverage"]},"research":{"quality":quality(&evidence,&case.relevant),"requests":fixture.calls.load(Ordering::SeqCst)-before,"citation_checks":{"valid_exact_references":valid,"references":citations,"semantic_support":"requires human review"},"result":result}}));
        assert_eq!(state.web.policy()?.revision, revision, "no model may mutate policy");
    }
    assert_eq!(
        fixture.forbidden.load(Ordering::SeqCst),
        0,
        "robots-denied path was fetched"
    );
    let report = json!({"mode":if live {"live-local-model"}else{"deterministic-model-fixture"},"model":state.current_model(),"corpus_sha256":cgagentharness::common::sha256_hex(include_str!("../tests/fixtures/web-research/corpus.json")),"sites":2,"documents":corpus.pages.len(),"limits":{"pages":state.web.limits.pages,"subqueries":state.web.limits.subqueries,"rounds":state.web.limits.rounds,"evidence_tokens":state.web.limits.evidence_tokens,"total_tokens":state.web.limits.total_tokens},"baseline_reference":"44a205e src/server/web_search.rs search: initial URLs only, escaped case-insensitive literal query","scope":"fixed virtual sites served by local fixture; no internet content or cloud model", "results":results});
    std::fs::write(&args[1], serde_json::to_vec_pretty(&report)?)?;
    content_task.abort();
    model_task.abort();
    Ok(())
}
