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

#[tokio::test]
async fn concurrent_google_search_returns_busy_without_echoing_secrets() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let resume = release.clone();
    let pages = Router::new().route(
        "/search",
        get(move || {
            let signal = signal.clone();
            let resume = resume.clone();
            async move {
                signal.notify_one();
                resume.notified().await;
                ([("content-type", "text/html")], "<html>unreadable listing</html>")
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
            web_resolve: Some(("www.google.com".into(), address)),
            ..ServerOptions::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "https://www.google.com/*"}))
            .await
            .0,
        200
    );
    let first = s.post_json(
        "/api/web/search",
        json!({"query": "veeam software cve", "engine": "google"}),
    );
    let second = async {
        started.notified().await;
        let result = s
            .post_json(
                "/api/web/search",
                json!({"query": "second concurrent search", "engine": "google"}),
            )
            .await;
        release.notify_one();
        result
    };
    let ((first_status, first_body), (second_status, second_body)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(first, second)
        })
        .await
        .unwrap();
    assert_eq!(second_status, 409, "{second_body}");
    assert_eq!(code(&second_body), "WEB_BUSY", "{second_body}");
    no_secret(&second_body, &s.home);
    assert!(
        first_status == 400 || first_status == 502,
        "held search must fail closed without a live listing: {first_body}"
    );
    no_secret(&first_body, &s.home);
    page_task.abort();
}
