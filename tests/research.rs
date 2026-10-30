//! Operator research corpus: disabled by default; enabled is read-only,
//! confirm-gated retrieval over an operator-chosen directory.

mod common;
use common::*;
use serde_json::json;

fn corpus_with_note() -> (tempfile::TempDir, String) {
    let corpus = tempfile::tempdir().unwrap();
    std::fs::write(corpus.path().join("note.md"), "veeam immutability hardened repository").unwrap();
    let path = corpus.path().to_string_lossy().replace('\\', "/");
    (corpus, format!("'{path}'"))
}

#[tokio::test]
async fn disabled_gate_creates_no_database_and_refuses_search_and_reindex() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let db = s.home.join("data").join("research").join("rag.sqlite3");
    assert!(!db.exists(), "disabled startup must not create the research index");
    let (status, body) = s.get_json("/api/research/status").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["chunks"], 0);
    let (status, body) = s
        .post_json("/api/research/search", json!({"query": "immutability"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "RESEARCH_DISABLED");
    let (status, body) = s.post_json("/api/research/reindex", json!({"confirm": true})).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(code(&body), "RESEARCH_DISABLED");
    assert!(!db.exists(), "refused mutations must not create the research index");
}

#[tokio::test]
async fn enabled_keyword_only_indexes_and_searches_the_corpus() {
    let model = start_mock_model().await;
    let (_corpus, corpus_path) = corpus_with_note();
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("research.enabled", "true")
            .with("research.corpus_dir", &corpus_path),
    )
    .await;
    let (status, body) = s.get_json("/api/research/status").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["embeddings_configured"], false);
    assert_eq!(body["chunks"], 0, "nothing is indexed before a confirm");

    for body in [json!({}), json!({"confirm": false})] {
        let (status, refused) = s.post_json("/api/research/reindex", body).await;
        assert_eq!(status, 400, "{refused}");
        assert_eq!(code(&refused), "RESEARCH_CONFIRM");
    }
    let (status, indexed) = s.post_json("/api/research/reindex", json!({"confirm": true})).await;
    assert_eq!(status, 200, "{indexed}");
    assert_eq!(indexed["files"], 1);
    assert!(indexed["chunks"].as_u64().unwrap() >= 1);
    assert_eq!(indexed["embedded"], 0, "no embeddings endpoint is configured");

    let (status, body) = s
        .post_json("/api/research/search", json!({"query": "immutability"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["hits"][0]["source"], "note.md");
    assert_eq!(body["lists"], 1, "keyword-only without an embedder");
    assert_eq!(body["type"], "untrusted_background_context");
    let (status, body) = s
        .post_json("/api/research/search", json!({"query": "zzz-no-such-term"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["hits"].as_array().unwrap().len(), 0);
    assert_eq!(body["confident"], false);

    let status = s.get_json("/api/research/status").await.1;
    assert!(status["chunks"].as_u64().unwrap() >= 1);
    assert!(s.audit_log().contains("research_reindex"));
}

#[tokio::test]
async fn search_query_never_lands_in_the_audit_log() {
    let model = start_mock_model().await;
    let (_corpus, corpus_path) = corpus_with_note();
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("research.enabled", "true")
            .with("research.corpus_dir", &corpus_path),
    )
    .await;
    let (status, _) = s.post_json("/api/research/reindex", json!({"confirm": true})).await;
    assert_eq!(status, 200);
    let unique = "UNIQUE_RESEARCH_QUERY_private_terms";
    let (status, body) = s.post_json("/api/research/search", json!({"query": unique})).await;
    assert_eq!(status, 200, "{body}");
    let audit = s.audit_log();
    assert!(audit.contains("research_search"), "{audit}");
    assert!(!audit.contains(unique), "raw query must never be logged: {audit}");
    assert!(audit.contains("query_hash"), "{audit}");
}

#[tokio::test]
async fn reindex_without_a_corpus_is_refused() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default().with("research.enabled", "true"),
    )
    .await;
    let (status, body) = s.post_json("/api/research/reindex", json!({"confirm": true})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "RESEARCH_NO_CORPUS");
}

#[tokio::test]
async fn search_validates_the_query_and_top_k() {
    let model = start_mock_model().await;
    let (_corpus, corpus_path) = corpus_with_note();
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("research.enabled", "true")
            .with("research.corpus_dir", &corpus_path),
    )
    .await;
    for body in [
        json!({"query": ""}),
        json!({"query": "   "}),
        json!({"query": "immutability", "top_k": 0}),
        json!({"query": "immutability", "surprise": true}),
    ] {
        let (status, refused) = s.post_json("/api/research/search", body).await;
        assert_eq!(status, 422, "{refused}");
    }
}
