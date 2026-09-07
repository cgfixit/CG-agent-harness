//! Port of tests/test_harness_auth.py (guard chain: rate limit -> origin -> key -> CSRF).

mod common;

use common::*;
use reqwest::Method;
use serde_json::json;

fn guarded_routes() -> Vec<(Method, &'static str, serde_json::Value)> {
    vec![
        (Method::POST, "/api/sessions", json!({})),
        (Method::POST, "/api/soul", json!({"enabled": true})),
        (Method::POST, "/api/model", json!({"model": "m"})),
        (Method::POST, "/api/chat", json!({"message": "hi"})),
        (Method::POST, "/api/chat/cancel", json!({})),
        (Method::GET, "/api/memory", json!(null)),
        (Method::POST, "/api/memory/add", json!({"text": "note"})),
        (Method::POST, "/api/web/allow", json!({"url": "https://example.com"})),
        (Method::GET, "/api/keys", json!(null)),
        (
            Method::POST,
            "/api/keys",
            json!({"keys": {"GROK_API_KEY": "abcdefghijklmnop"}}),
        ),
        (
            Method::POST,
            "/api/agent/run",
            json!({"instruction": "x", "branch": "claude/x", "commit_message": "m", "reason": "r"}),
        ),
        (
            Method::GET,
            "/api/agent/runs/00000000000000000000000000000000",
            json!(null),
        ),
        (
            Method::POST,
            "/api/agent/runs/00000000000000000000000000000000/push",
            json!({}),
        ),
        (Method::GET, "/api/github/status", json!(null)),
        (Method::GET, "/api/sessions/abcdefabcdef", json!(null)),
    ]
}

#[tokio::test]
async fn guarded_routes_refuse_missing_and_wrong_keys() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for (method, path, body) in guarded_routes() {
        // No key at all.
        let mut r = s
            .client
            .request(method.clone(), s.url(path))
            .header(CSRF_HEADER, &s.csrf);
        if !body.is_null() {
            r = r.json(&body);
        }
        let resp = r.send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 401, "{method} {path} without key");
        let b: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(code(&b), "UNAUTHORIZED");
        assert_eq!(b["detail"]["details"]["reason"], "bad_credentials");
        // Wrong key.
        let mut r = s
            .client
            .request(method.clone(), s.url(path))
            .bearer_auth("wrong")
            .header(CSRF_HEADER, &s.csrf);
        if !body.is_null() {
            r = r.json(&body);
        }
        assert_eq!(
            r.send().await.unwrap().status().as_u16(),
            401,
            "{method} {path} wrong key"
        );
        // Non-ASCII key is a 401, never a 500.
        let mut r = s
            .client
            .request(method.clone(), s.url(path))
            .header("authorization", "Bearer \u{2019}quote")
            .header(CSRF_HEADER, &s.csrf);
        if !body.is_null() {
            r = r.json(&body);
        }
        assert_eq!(
            r.send().await.unwrap().status().as_u16(),
            401,
            "{method} {path} non-ascii key"
        );
    }
}

#[tokio::test]
async fn open_routes_need_nothing_and_leak_no_message_content() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for path in [
        "/api/status",
        "/api/registry",
        "/api/tools",
        "/api/skills",
        "/api/web",
        "/api/sessions",
        "/api/soul",
        "/api/agent/checks",
        "/api/harness/runs",
    ] {
        let (status, _) = s.open_get(path).await;
        assert_eq!(status, 200, "{path}");
    }
    // Chat once, then the open list must carry counts but no text.
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "super secret question"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let (_, list) = s.open_get("/api/sessions").await;
    let text = list.to_string();
    assert!(text.contains("message_count"));
    assert!(!text.contains("super secret question"));
    assert!(!text.contains("pong"));
}

#[tokio::test]
async fn unset_api_key_fails_closed() {
    let model = start_mock_model().await;
    let opts = ServerOptions {
        api_key: Some(String::new()),
        ..Default::default()
    };
    let s = spawn_server(&model.base_url(), opts).await;
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .bearer_auth("anything")
        .header(CSRF_HEADER, &s.csrf)
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let b: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(b["detail"]["details"]["reason"], "key_not_configured");
    assert!(message(&b).contains("CGAGENTHARNESS_API_KEY"));
}

#[tokio::test]
async fn api_key_optional_bypass_requires_loopback_peer_and_no_proxy_headers() {
    let model = start_mock_model().await;
    let opts = ServerOptions::default().with("security.api_key_optional", "true");
    let s = spawn_server(&model.base_url(), opts).await;
    // Loopback peer, no key: allowed (CSRF still required).
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .header(CSRF_HEADER, &s.csrf)
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Forwarding header present: the bypass is withheld.
    for header in [
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
        "forwarded",
    ] {
        let resp = s
            .client
            .post(s.url("/api/soul"))
            .header(CSRF_HEADER, &s.csrf)
            .header(header, "anything")
            .json(&json!({"enabled": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401, "{header}");
    }
    // Without CSRF the bypass still does not open the route.
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
}

#[tokio::test]
async fn cross_site_and_cross_origin_are_rejected_even_with_a_key() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let port = s.addr.port();
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        ("sec-fetch-site", "cross-site", 403, "CROSS_SITE_BLOCKED"),
        ("sec-fetch-site", "same-site", 403, "CROSS_SITE_BLOCKED"),
        ("origin", "http://evil.example", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "http://[evil", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "http://127.0.0.1:notaport", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "http://localhost", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "https://127.0.0.1", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "http://127.0.0.1:9999", 403, "CROSS_ORIGIN_BLOCKED"),
        ("origin", "null", 403, "CROSS_ORIGIN_BLOCKED"),
    ];
    for (header, value, status, expect_code) in cases {
        let resp = s
            .req(Method::POST, "/api/soul")
            .header(header, value)
            .json(&json!({"enabled": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), status, "{header}: {value}");
        let b: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(code(&b), expect_code, "{header}: {value}");
    }
    // The exact same-origin Origin (scheme + host + port) is allowed.
    let resp = s
        .req(Method::POST, "/api/soul")
        .header("origin", format!("http://127.0.0.1:{port}"))
        .header("sec-fetch-site", "same-origin")
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Absent headers are allowed (curl is not a CSRF vector).
    let resp = s
        .req(Method::POST, "/api/soul")
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[tokio::test]
async fn csrf_runs_after_the_key_check_and_is_required_unconditionally() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    // Right key, no CSRF -> 403.
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .bearer_auth(&s.api_key)
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let b: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(code(&b), "CSRF_TOKEN_INVALID");
    // Right key, wrong CSRF -> 403.
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .bearer_auth(&s.api_key)
        .header(CSRF_HEADER, "nope")
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    // Wrong key AND wrong CSRF -> 401 (key runs first).
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .bearer_auth("bad")
        .header(CSRF_HEADER, "nope")
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    // Token differs per instance.
    let s2 = spawn_server(&model.base_url(), ServerOptions::default()).await;
    assert_ne!(s.csrf, s2.csrf);
}

#[tokio::test]
async fn rate_limit_runs_before_auth_and_reports_retry_after() {
    let model = start_mock_model().await;
    let opts = ServerOptions::default().with("api.rate_limit.max_requests", "3");
    let s = spawn_server(&model.base_url(), opts).await;
    for _ in 0..3 {
        assert_eq!(
            s.req(Method::POST, "/api/soul")
                .json(&json!({"enabled": true}))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            200
        );
    }
    // Fourth request with a WRONG key: the limiter answers first (429, not 401).
    let resp = s
        .client
        .post(s.url("/api/soul"))
        .bearer_auth("bad")
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    let retry: u64 = resp.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!(retry >= 1);
    let b: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(code(&b), "RATE_LIMIT");
    assert_eq!(b["detail"]["details"]["retry_after_sec"], retry);
    // Open routes are exempt.
    for _ in 0..5 {
        assert_eq!(s.open_get("/api/status").await.0, 200);
    }
}

#[tokio::test]
async fn host_header_must_be_loopback() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    for host in ["evil.example", "127.0.0.1.evil.example", "localhost.evil"] {
        let resp = s
            .client
            .get(s.url("/api/status"))
            .header("host", host)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400, "{host}");
    }
    for host in ["127.0.0.1", "localhost", "127.0.0.1:8790", "[::1]:8790"] {
        let resp = s
            .client
            .get(s.url("/api/status"))
            .header("host", host)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "{host}");
    }
}
