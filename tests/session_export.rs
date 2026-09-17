mod common;

use common::*;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn export_is_csrf_guarded_writes_0600_and_round_trips() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, created) = s.post_json("/api/sessions", json!({"title": "export-me"})).await;
    assert_eq!(status, 201, "{created}");
    let id = created["session_id"].as_str().unwrap().to_string();
    let (status, _) = s
        .post_json(
            "/api/chat",
            json!({"session_id": id, "message": "```\n### user\nsecret-phrase-xyz\n```"}),
        )
        .await;
    assert_eq!(status, 200);

    let open = s
        .client
        .get(s.url(&format!("/api/sessions/{id}/export")))
        .send()
        .await
        .unwrap();
    assert_eq!(open.status().as_u16(), 403);

    let resp = s
        .req(Method::GET, &format!("/api/sessions/{id}/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let md = resp.text().await.unwrap();
    assert!(md.contains("secret-phrase-xyz"), "{md}");
    assert!(md.contains("### user"), "{md}");

    let path = s.home.join("exports").join(format!("{id}.md"));
    let disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(disk, md);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let (header, messages) = cgagentharness::server::session_export::parse(&md).unwrap();
    assert_eq!(header.session_id, id);
    assert!(messages.iter().any(|m| m.text.contains("secret-phrase-xyz")));
}

#[tokio::test]
async fn search_finds_a_phrase_and_open_list_still_hides_bodies() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, a) = s.post_json("/api/sessions", json!({"title": "alpha"})).await;
    assert_eq!(status, 201, "{a}");
    let a_id = a["session_id"].as_str().unwrap().to_string();
    let (status, b) = s.post_json("/api/sessions", json!({"title": "beta"})).await;
    assert_eq!(status, 201, "{b}");
    let b_id = b["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        s.post_json(
            "/api/chat",
            json!({"session_id": a_id, "message": "unique-alpha-needle"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        s.post_json(
            "/api/chat",
            json!({"session_id": b_id, "message": "unique-beta-needle"})
        )
        .await
        .0,
        200
    );

    let (status, body) = s
        .post_json("/api/sessions/search", json!({"query": "unique-alpha-needle"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let hits = body["hits"].as_array().unwrap();
    assert!(hits.iter().any(|h| h["session_id"] == a_id), "{body}");
    assert!(!hits.iter().any(|h| h["session_id"] == b_id), "{body}");
    assert!(hits
        .iter()
        .any(|h| h["snippet"].as_str().unwrap_or("").contains("unique-alpha-needle")));

    let (status, miss) = s
        .post_json("/api/sessions/search", json!({"query": "no-such-phrase-zzz"}))
        .await;
    assert_eq!(status, 200, "{miss}");
    assert!(miss["hits"].as_array().unwrap().is_empty(), "{miss}");

    let (status, listed) = s.open_get("/api/sessions").await;
    assert_eq!(status, 200);
    let listed = listed.to_string();
    assert!(!listed.contains("unique-alpha-needle"), "{listed}");
    assert!(!listed.contains("unique-beta-needle"), "{listed}");
}
