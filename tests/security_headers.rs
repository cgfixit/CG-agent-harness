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
    // JSON routes carry the strict default CSP.
    let resp = s.client.get(s.url("/api/status")).send().await.unwrap();
    assert_eq!(
        resp.headers()["content-security-policy"].to_str().unwrap(),
        "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
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
