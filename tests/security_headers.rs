//! Port of tests/test_harness_security_headers.py.

mod common;

use common::*;

const HARDENING: [(&str, &str); 5] = [
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("permissions-policy", "camera=(), microphone=(), geolocation=()"),
    ("x-permitted-cross-domain-policies", "none"),
];

fn assert_hardened(resp: &reqwest::Response) {
    for (name, value) in HARDENING {
        assert_eq!(
            resp.headers().get(name).and_then(|v| v.to_str().ok()),
            Some(value),
            "{name} on {}",
            resp.url()
        );
    }
    assert!(resp.headers().contains_key("content-security-policy"));
}

#[tokio::test]
async fn every_response_carries_the_hardening_headers() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for path in [
        "/",
        "/api/status",
        "/does-not-exist",
        "/static/auth_admin.js",
        "/api/keys",
    ] {
        let resp = s.client.get(s.url(path)).send().await.unwrap();
        assert_hardened(&resp);
    }
    // A rejected Host still gets the headers (the middleware is outermost).
    let resp = s
        .client
        .get(s.url("/api/status"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    assert_hardened(&resp);
    assert_eq!(
        resp.headers()["cache-control"],
        cgagentharness::server::headers::NO_STORE
    );
    // JSON routes carry the strict default CSP.
    let resp = s.client.get(s.url("/api/status")).send().await.unwrap();
    assert_eq!(
        resp.headers()["content-security-policy"].to_str().unwrap(),
        "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
}

#[tokio::test]
async fn api_responses_cannot_opt_into_caching() {
    use axum::{http::header, middleware, routing::get, Router};
    let app = Router::new()
        .route(
            "/api/example",
            get(|| async { ([(header::CACHE_CONTROL, "public, max-age=3600")], "ok") }),
        )
        .layer(middleware::from_fn(cgagentharness::server::headers::security_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this owned fixture must bind only to loopback.
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for path in ["/api/example", "/api/missing", "/api"] {
        let response = client.get(format!("http://{addr}{path}")).send().await.unwrap(); // DevSkim: ignore DS137138 because this HTTP URL reaches only the owned loopback fixture.
        assert_eq!(
            response.headers()["cache-control"],
            cgagentharness::server::headers::NO_STORE
        );
    }
    task.abort();
}

#[tokio::test]
async fn console_markup_is_served_only_through_the_substituted_root() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    // The raw asset carries both placeholders and inline script/style tags;
    // only `/` substitutes them, so no static path may hand the markup out.
    let resp = s.client.get(s.url("/static/harness.html")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    assert_hardened(&resp);
    let body = resp.text().await.unwrap();
    assert!(!body.contains("__CYCLAW_CSRF_TOKEN__"));
    assert!(!body.contains("__CYCLAW_CSP_NONCE__"));
    assert!(!body.contains("<script"));
}

#[tokio::test]
async fn console_uses_a_per_response_nonce_and_no_store() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let r1 = s.client.get(s.url("/")).send().await.unwrap();
    let csp1 = r1.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert_eq!(
        r1.headers()["cache-control"].to_str().unwrap(),
        "no-store, no-cache, must-revalidate, max-age=0"
    );
    assert_eq!(r1.headers()["x-frame-options"].to_str().unwrap(), "DENY");
    let body1 = r1.text().await.unwrap();
    let nonce1 = csp1
        .split("'nonce-")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_string();
    assert!(body1.contains(&format!("<style nonce=\"{nonce1}\"")));
    assert!(body1.contains(&format!("<script nonce=\"{nonce1}\"")));
    assert!(!body1.contains("__CYCLAW_CSP_NONCE__"));
    assert!(!body1.contains("__CYCLAW_CSRF_TOKEN__"));
    assert!(body1.contains(&s.csrf), "page embeds the real CSRF token");
    let r2 = s.client.get(s.url("/")).send().await.unwrap();
    let csp2 = r2.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert_ne!(csp1, csp2, "nonce differs per response");
    assert!(csp1.contains("script-src 'self' 'nonce-"));
    assert!(csp1.contains("frame-ancestors 'none'"));
    assert!(csp1.contains("frame-src 'self'"));
    // Static assets: no-store, correct JS type.
    let js = s.client.get(s.url("/static/auth_admin.js")).send().await.unwrap();
    assert_eq!(js.status().as_u16(), 200);
    assert!(js.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert_eq!(
        js.headers()["cache-control"].to_str().unwrap(),
        "no-store, no-cache, must-revalidate, max-age=0"
    );
    assert_eq!(
        s.client
            .get(s.url("/static/nope.js"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
}

#[tokio::test]
async fn preview_html_is_frameable_only_by_the_console() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let preview = s.client.get(s.url("/preview/html?run=1")).send().await.unwrap();
    assert_eq!(preview.status().as_u16(), 200);
    assert_eq!(preview.headers()["x-frame-options"].to_str().unwrap(), "SAMEORIGIN");
    assert_eq!(
        preview.headers()["content-security-policy"].to_str().unwrap(),
        cgagentharness::server::console::PREVIEW_CSP
    );
    assert_eq!(
        preview.headers()["cache-control"].to_str().unwrap(),
        "no-store, no-cache, must-revalidate, max-age=0"
    );
    let body = preview.text().await.unwrap();
    assert!(body.contains("document.write"));
    assert!(body.contains("https:"));
    assert!(body.contains("127.0.0.1")); // DevSkim: ignore DS162092 because the shell must name loopback and nothing else.
    assert!(!body.contains("__CYCLAW_CSRF_TOKEN__"));
    assert!(!body.contains("__CYCLAW_CSP_NONCE__"));
    assert!(!body.contains(&s.csrf));
    let page = s.client.get(s.url("/")).send().await.unwrap();
    let csp = page.headers()["content-security-policy"].to_str().unwrap();
    assert_eq!(page.headers()["x-frame-options"].to_str().unwrap(), "DENY");
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("frame-src 'self'"));
    assert!(!csp.contains("unsafe-inline"));
}
