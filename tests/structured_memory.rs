//! M1 structured facts + governed proposals (#87).
//! Pinned `/memory` notes stay on their own JSON path and are not migrated.

mod common;
use common::*;
use serde_json::json;

fn enabled() -> ServerOptions {
    ServerOptions::default().with("structured_memory.enabled", "true")
}

#[tokio::test]
async fn disabled_gate_creates_no_database_and_refuses_writes() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    assert!(
        !s.home.join("memory").join("structured.sqlite3").exists(),
        "disabled startup must not create the structured-memory database"
    );
    let (status, body) = s.get_json("/api/structured-memory").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["facts"], false);
    assert_eq!(body["proposals"], false);
    assert_eq!(body["episodes"], false);
    assert_eq!(body["retrieval"], false);
    assert_eq!(body["retrieval_fusion"], false);
    assert_eq!(body["consolidation"], false);
    assert_eq!(body["rag"], false);
    assert_eq!(body["writable_from_model"], false);
    assert_eq!(body["at_rest_encryption"], false);
    assert!(body["at_rest"].as_str().unwrap().contains("not encryption"));
    let (status, body) = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({"action":"add","content":"should not persist"}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "STRUCTURED_MEMORY_DISABLED");
    assert!(
        !s.home.join("memory").join("structured.sqlite3").exists(),
        "disabled mutations must not create a store"
    );
    let (status, body) = s.get_json("/api/memory").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["structured_memory"]["separate"], true);
    assert_eq!(body["rag"]["facts"], false);
}

#[tokio::test]
async fn propose_without_confirm_does_not_create_a_fact() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), enabled()).await;
    let (status, proposal) = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({"action":"add","content":"Prefer metric units in examples.","category":"pref"}),
        )
        .await;
    assert_eq!(status, 200, "{proposal}");
    assert_eq!(proposal["status"], "pending");
    let id = proposal["id"].as_str().unwrap();
    let (status, refused) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{id}"),
            json!({"revision":proposal["revision"],"reason":"looks good","apply":true}),
        )
        .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(code(&refused), "STRUCTURED_MEMORY_CONFIRM");
    assert_eq!(s.get_json("/api/structured-memory/facts").await.1["count"], 0);
    assert_eq!(
        s.get_json(&format!("/api/structured-memory/proposals/{id}")).await.1["status"],
        "pending"
    );
}

#[tokio::test]
async fn propose_confirm_recall_fixture_flow() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/structured_memory/propose-confirm-recall.json")).unwrap();
    assert_eq!(fixture["name"], "propose-confirm-recall");
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), enabled()).await;
    let (status, proposal) = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({"action":"add","content":"Prefer metric units in examples.","category":"pref"}),
        )
        .await;
    assert_eq!(status, 200, "{proposal}");
    let id = proposal["id"].as_str().unwrap().to_string();
    let revision = proposal["revision"].as_str().unwrap().to_string();
    let (status, refused) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{id}"),
            json!({"revision":revision,"reason":"Reviewed operator fact","apply":true}),
        )
        .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(code(&refused), "STRUCTURED_MEMORY_CONFIRM");
    assert_eq!(s.get_json("/api/structured-memory/facts").await.1["count"], 0);
    let (status, applied) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{id}"),
            json!({"revision":revision,"reason":"Reviewed operator fact","confirm":true,"apply":true}),
        )
        .await;
    assert_eq!(status, 200, "{applied}");
    assert_eq!(applied["status"], "applied");
    let (status, listed) = s.get_json("/api/structured-memory/facts").await;
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["facts"][0]["content"], "Prefer metric units in examples.");
    let fact_id = listed["facts"][0]["id"].as_str().unwrap();
    let (status, recalled) = s.get_json(&format!("/api/structured-memory/facts/{fact_id}")).await;
    assert_eq!(status, 200, "{recalled}");
    assert_eq!(recalled["content"], "Prefer metric units in examples.");
    assert_eq!(recalled["active"], true);
    let (status, again) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{id}"),
            json!({"revision":revision,"reason":"second apply","confirm":true,"apply":true}),
        )
        .await;
    assert_eq!(status, 409, "{again}");
    assert_eq!(code(&again), "STRUCTURED_MEMORY_PROPOSAL_CHANGED");
}

#[tokio::test]
async fn unknown_fields_stale_apply_and_audit_omit_content() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), enabled()).await;
    let (status, body) = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({"action":"add","content":"secret-fact-payload","extra":true}),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    let fact = s
        .post_json(
            "/api/structured-memory/facts",
            json!({"content":"original","reason":"direct","confirm":true}),
        )
        .await
        .1;
    let proposal = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({
                "action":"update",
                "content":"replacement",
                "target_fact_id":fact["id"],
                "expected_revision":fact["revision"],
                "expected_digest":fact["content_digest"]
            }),
        )
        .await
        .1;
    s.post_json(
        "/api/structured-memory/facts",
        json!({
            "content":"intervening human edit",
            "reason":"change base",
            "confirm":true
        }),
    )
    .await;
    // Direct add does not change the original fact revision; bump it explicitly.
    let (status, _) = s
        .post_json(
            &format!(
                "/api/structured-memory/facts/{}/deactivate",
                fact["id"].as_str().unwrap()
            ),
            json!({"expected_revision":fact["revision"],"reason":"supersede","confirm":true}),
        )
        .await;
    assert_eq!(status, 200);
    let (status, stale) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{}", proposal["id"].as_str().unwrap()),
            json!({
                "revision":proposal["revision"],
                "reason":"apply stale",
                "confirm":true,
                "apply":true
            }),
        )
        .await;
    assert_eq!(status, 409, "{stale}");
    assert_eq!(code(&stale), "STRUCTURED_MEMORY_CHANGED");
    let audit = std::fs::read_to_string(s.home.join("logs").join("audit.jsonl")).unwrap_or_default();
    assert!(!audit.contains("secret-fact-payload"));
    assert!(!audit.contains("replacement"));
    assert!(!audit.contains("intervening human edit"));
    assert!(audit.contains("structured_memory_proposal_created") || audit.contains("structured_memory_fact_added"));
}

#[tokio::test]
async fn deactivated_facts_do_not_reappear_as_active() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), enabled()).await;
    let fact = s
        .post_json(
            "/api/structured-memory/facts",
            json!({"content":"temporary","reason":"add","confirm":true}),
        )
        .await
        .1;
    let proposal = s
        .post_json(
            "/api/structured-memory/proposals",
            json!({
                "action":"deactivate",
                "target_fact_id":fact["id"],
                "expected_revision":fact["revision"],
                "expected_digest":fact["content_digest"]
            }),
        )
        .await
        .1;
    let (status, applied) = s
        .post_json(
            &format!("/api/structured-memory/proposals/{}", proposal["id"].as_str().unwrap()),
            json!({
                "revision":proposal["revision"],
                "reason":"no longer true",
                "confirm":true,
                "apply":true
            }),
        )
        .await;
    assert_eq!(status, 200, "{applied}");
    assert_eq!(s.get_json("/api/structured-memory/facts").await.1["count"], 0);
    let recalled = s
        .get_json(&format!(
            "/api/structured-memory/facts/{}",
            fact["id"].as_str().unwrap()
        ))
        .await
        .1;
    assert_eq!(recalled["active"], false);
}

#[tokio::test]
async fn pinned_notes_remain_on_notes_json() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), enabled()).await;
    let (status, added) = s.post_json("/api/memory/add", json!({"text":"keep note"})).await;
    assert_eq!(status, 200, "{added}");
    assert!(s.home.join("memory").join("notes.json").is_file());
    let notes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(s.home.join("memory").join("notes.json")).unwrap()).unwrap();
    assert_eq!(notes["notes"][0]["text"], "keep note");
    s.post_json(
        "/api/structured-memory/facts",
        json!({"content":"structured only","reason":"separate","confirm":true}),
    )
    .await;
    let notes_after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(s.home.join("memory").join("notes.json")).unwrap()).unwrap();
    assert_eq!(notes_after, notes);
}
