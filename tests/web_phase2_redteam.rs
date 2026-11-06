//! Phase 2 (#86 Session W1) non-empty allowlist red-team.
//! Owned temp home + ephemeral loopback listeners. No live Google or SerpAPI.
mod common;
use axum::{routing::get, Router};
use common::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn no_secret(body: &Value, home: &std::path::Path) {
    let dumped = body.to_string();
    let audit = std::fs::read_to_string(home.join("logs").join("audit.jsonl")).unwrap_or_default();
    for text in [dumped.as_str(), audit.as_str()] {
        assert!(!text.contains("fixture-secret-key"), "{text}");
        assert!(!text.contains("SERPAPI_API_KEY"), "{text}");
    }
}

#[tokio::test]
async fn empty_allowlist_and_invalid_grammar_fail_closed() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200);
    assert_eq!(web["enabled"], true);
    assert!(web["allowlist"].as_array().unwrap().is_empty(), "{web}");
    let (status, body) = s
        .post_json("/api/web/fetch", json!({"url": "https://example.com/docs"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "WEB_ALLOWLIST_EMPTY");
    no_secret(&body, &s.home);

    for bad in [
        "https://*/*",
        "https://*.com/*",
        "https://u:p@example.com/",
        "https://example.com/a/../b",
        "https://example.com/%2e%2e/b",
    ] {
        let (status, body) = s.post_json("/api/web/allow", json!({"url": bad})).await;
        assert_eq!(status, 400, "{bad}: {body}");
        assert!(
            matches!(code(&body).as_str(), "WEB_BAD_URL" | "WEB_SSRF_DENIED"),
            "{bad}: {body}"
        );
        no_secret(&body, &s.home);
    }
}

#[tokio::test]
async fn armed_origin_refuses_sibling_apex_and_compressed_bodies() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let pages = Router::new()
        .route(
            "/ok",
            get(move || {
                let hits = counted.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "PHASE2_ORIGIN_OK")
                }
            }),
        )
        .route(
            "/gzip",
            get(|| async {
                (
                    [("content-type", "text/plain"), ("content-encoding", "gzip")],
                    "not-gzip",
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_task = tokio::spawn(async move {
        axum::serve(listener, pages).await.unwrap();
    });
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..ServerOptions::default().with("web.pace_ms", "100")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/*"}))
            .await
            .0,
        200
    );
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200, "{web}");
    assert_eq!(web["allowlist"], json!(["http://docs.example/*"]));
    assert_eq!(web["allowlist"].as_array().unwrap().len(), 1);

    let (status, body) = s
        .post_json("/api/web/fetch", json!({"url": "http://docs.example/ok"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["text"].as_str().unwrap().contains("PHASE2_ORIGIN_OK"), "{body}");
    no_secret(&body, &s.home);

    let (status, body) = s
        .post_json("/api/web/fetch", json!({"url": "http://evil.example/ok"}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "WEB_HOST_DENIED");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "sibling host must not be fetched");
    no_secret(&body, &s.home);

    let (status, body) = s
        .post_json("/api/web/fetch", json!({"url": "http://docs.example/gzip"}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "WEB_ENCODING_REFUSED");
    no_secret(&body, &s.home);

    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "https://*.example.com/*"}))
            .await
            .0,
        200
    );
    let (status, body) = s
        .post_json("/api/web/fetch", json!({"url": "https://example.com/docs"}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        code(&body),
        "WEB_HOST_DENIED",
        "apex is excluded from https://*.example.com/* by design: {body}"
    );
    no_secret(&body, &s.home);
    page_task.abort();
}

/// Public Google listings are HTTPS; the in-process fixture is HTTP. Hold
/// `search_gate` with permitted-page search against an HTTP origin instead.
#[tokio::test]
async fn concurrent_page_search_returns_busy_without_echoing_secrets() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let resume = release.clone();
    let pages = Router::new().route(
        "/slow",
        get(move || {
            let signal = signal.clone();
            let resume = resume.clone();
            async move {
                signal.notify_one();
                resume.notified().await;
                ([("content-type", "text/plain")], "PHASE2_SLOW_PAGE")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_task = tokio::spawn(async move {
        axum::serve(listener, pages).await.unwrap();
    });
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..ServerOptions::default().with("web.pace_ms", "100")
        },
    )
    .await;
    // Exact URL: the rule covers the origin, so robots.txt is read for the
    // politeness decision; the explicit start proceeds and discover holds on /slow.
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/slow"}))
            .await
            .0,
        200
    );
    let first = s.post_json("/api/web/search", json!({"query": "PHASE2_SLOW_PAGE"}));
    let second = async {
        started.notified().await;
        let result = s
            .post_json("/api/web/search", json!({"query": "second concurrent search"}))
            .await;
        release.notify_one();
        result
    };
    let ((_first_status, first_body), (second_status, second_body)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(first, second)
        })
        .await
        .expect("page-search hold should notify before the deadline");
    assert_eq!(second_status, 409, "{second_body}");
    assert_eq!(code(&second_body), "WEB_BUSY", "{second_body}");
    no_secret(&second_body, &s.home);
    no_secret(&first_body, &s.home);
    page_task.abort();
}

/// A prepared server whose `config.yaml` still carries an older home's
/// `web.search.searxng_url` line (the shipped YAML no longer has the key).
async fn prepared_with_searxng_url(value: &str) -> PreparedServer {
    let mut prepared = prepare_server(
        "http://127.0.0.1:9/v1", // DevSkim: ignore DS137138,DS162092 because this test-only model is never contacted.
        ServerOptions::default(),
    )
    .await;
    let path = prepared.app_opts.config.as_ref().unwrap().path.clone();
    let text = std::fs::read_to_string(&path).unwrap();
    let anchor = "  search:\n";
    assert!(text.contains(anchor), "the shipped YAML has a web.search block");
    let text = text.replacen(anchor, &format!("{anchor}    searxng_url: {value}\n"), 1);
    prepared.app_opts.config = Some(cgagentharness::common::config::AppConfig::from_str(&text, &path).unwrap());
    prepared
}

/// SearXNG support was removed, and with it the one private-address exception
/// for search listings. A home that still names it fails boot by the field, never
/// quoting the value; an older home's empty `searxng_url` keeps booting.
#[tokio::test]
async fn the_removed_searxng_settings_fail_boot_by_name() {
    let provider = prepare_server(
        "http://127.0.0.1:9/v1", // DevSkim: ignore DS137138,DS162092 because this test-only model is never contacted.
        ServerOptions::default().with("web.search.provider", "searxng"),
    )
    .await;
    let stale = prepared_with_searxng_url("\"http://127.0.0.1:8888\"").await; // DevSkim: ignore DS137138,DS162092 because this asserts the removed loopback setting is refused.
    for (prepared, message, value) in [
        (provider, "web.search.provider searxng was removed", None),
        (
            stale,
            "web.search.searxng_url is no longer read",
            Some("127.0.0.1:8888"),
        ),
    ] {
        let err = match cgagentharness::server::build_app(prepared.app_opts).await {
            Ok(_) => panic!("{message} must fail boot"),
            Err(err) => err,
        };
        assert_eq!(err.code, "CONFIG_ERROR", "{err}");
        assert!(err.message.contains(message), "{err}");
        if let Some(value) = value {
            assert!(!err.message.contains(value), "the value is never quoted: {err}");
        }
    }
    let older = prepared_with_searxng_url("\"\"").await;
    assert!(
        cgagentharness::server::build_app(older.app_opts).await.is_ok(),
        "an older seeded home with an empty searxng_url boots"
    );
}
