//! Exercise the shared slash grammar through real HTTP, not a duplicated UI parser.
mod common;
use axum::{routing::get, Router};
use common::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn batch_grants_checks_and_fetches_preserve_permission_and_atomicity() {
    let reads = Arc::new(AtomicUsize::new(0));
    let count = reads.clone();
    let fixture = Router::new().route(
        "/docs/{page}",
        get(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "Synthetic backup documentation.")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
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
    let (_, parsed) = s
        .post_json(
            "/api/slash/parse",
            json!({"line":"/web allow http://docs.example/docs/* https://www.veeam.com/* --group vendors"}), // DevSkim: ignore DS137138 because the synthetic HTTP URL resolves only to the loopback fixture.
        )
        .await;
    assert_eq!(parsed["dispatch"], true, "{parsed}");
    assert_eq!(parsed["web"]["action"], "allow");
    let (status, granted) = s.post_json("/api/web/allow", parsed["web"]["body"].clone()).await;
    assert_eq!(status, 200, "{granted}");
    assert_eq!(granted["rules"].as_array().unwrap().len(), 2);
    assert_eq!(granted["policy_scope"], "shared_home");
    let revision = granted["policy_revision"].clone();
    let too_many: Vec<_> = (0..32).map(|i| format!("https://site{i}.example/")).collect();
    let (_, full) = s.post_json("/api/web/allow", json!({"urls":too_many})).await;
    assert_eq!(code(&full), "WEB_ALLOWLIST_FULL");
    assert_eq!(
        s.get_json("/api/web").await.1["policy_revision"],
        revision,
        "cap refusal cannot retain a prefix"
    );
    for invalid in [
        json!({"urls":["https://safe.example/*","http://127.0.0.1/*"]}),
        json!({"urls":["https://safe.example/*"],"seeds":["https://unrelated.example/"]}),
        json!({"url":"https://safe.example/*","urls":["https://another.example/*"]}),
    ] {
        assert!(s.post_json("/api/web/allow", invalid).await.0 >= 400);
        assert_eq!(
            s.get_json("/api/web").await.1["policy_revision"],
            revision,
            "no partial grant"
        );
    }
    let (_,checks)=s.post_json("/api/web/check",json!({"urls":["http://docs.example/docs/one","https://www.veeam.com/","https://veeam.com/","http://127.0.0.1/"],"group":"vendors"})).await;
    assert_eq!(checks["checks"][0]["permitted"], true);
    assert_eq!(checks["checks"][1]["permitted"], true);
    assert_eq!(checks["checks"][2]["permitted"], false, "www must not broaden to apex");
    assert_eq!(checks["checks"][3]["code"], "WEB_SSRF_DENIED");
    assert_eq!(reads.load(Ordering::SeqCst), 0, "diagnostic makes no network request");
    assert!(
        model.last_request().is_none(),
        "slash and permission checks are not model calls"
    );
    let (_, parsed) = s
        .post_json(
            "/api/slash/parse",
            json!({"line":"/web fetch http://docs.example/docs/one http://docs.example/docs/two --group vendors"}),
        )
        .await;
    let (status, result) = s.post_json("/api/web/fetch", parsed["web"]["body"].clone()).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["pages"].as_array().unwrap().len(), 2);
    assert_eq!(result["complete"], true);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    for body in [
        json!({"urls":["http://docs.example/docs/one","http://docs.example/outside"]}),
        json!({"urls":["http://docs.example/docs/one"],"group":"other"}),
    ] {
        let (_, bad) = s.post_json("/api/web/fetch", body).await;
        assert_eq!(code(&bad), "WEB_HOST_DENIED");
        assert_eq!(reads.load(Ordering::SeqCst), 2, "validate entire batch before any read");
    }
    let (_, text) = s.post_json("/api/prompt/preview", json!({})).await;
    assert!(text.to_string().contains("NOT per-session"));
    task.abort();
}

#[tokio::test]
async fn slash_refuses_ambiguous_authority_but_exposes_inert_help_and_flags() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let initial = s.get_json("/api/web").await.1;
    for line in [
        "/web please allow https://example.com/*",
        "/web alow https://example.com/*",
        "/web fetch https://www.veeam.com and its embedded internal links",
        "/web allow https://example.com/* --dry-run",
        "/web off\n/web on",
        "/web search --count 99 backup",
        "/web research --url https://example.com/* summarize",
        "/web nonsense",
    ] {
        let (_, parsed) = s.post_json("/api/slash/parse", json!({"line":line})).await;
        assert_eq!(parsed["dispatch"], false, "{line}: {parsed}");
        assert!(parsed.get("web").is_none(), "no executable web payload on suggestion");
    }
    for line in [
        "/web --help",
        "/web allow --help",
        "/web off --help",
        "/agent help",
        "/clear --help",
    ] {
        let (_, parsed) = s.post_json("/api/slash/parse", json!({"line":line})).await;
        assert_eq!(parsed["command"], "help", "{line}: {parsed}");
        assert!(parsed.get("web").is_none());
    }
    let (_, parsed) = s
        .post_json(
            "/api/slash/parse",
            json!({"line":"/web please search --count 2 backup security"}),
        )
        .await;
    assert_eq!(parsed["dispatch"], true);
    assert_eq!(parsed["web"]["body"]["count"], 2);
    assert_eq!(parsed["web"]["body"]["query"], "backup security");
    assert_eq!(s.get_json("/api/web").await.1, initial, "parsing has no side effects");
    assert!(model.last_request().is_none());
    // The new diagnostic is still CSRF-guarded, including on direct HTTP calls.
    let response = s
        .client
        .post(format!("{}/api/web/check", s.base))
        .json(&json!({"urls":["https://example.com/"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(code(&body), "CSRF_TOKEN_INVALID");
}
