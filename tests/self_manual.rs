//! The bundled self-manual and live-state block: the checked-in manual is
//! current and scanner-clean, and both reach local chat turns and /prompt only.
mod common;
use cgagentharness::server::self_manual::{parse_chunks, MANUAL};
use common::*;
use serde_json::{json, Value};

#[cfg(unix)]
#[test]
fn the_checked_in_manual_matches_the_generator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = tempfile::tempdir().unwrap().keep().join("MANUAL.md");
    let status = std::process::Command::new("python3")
        .arg(root.join("scripts/build-self-manual.py"))
        .arg("--root")
        .arg(root)
        .arg("--out")
        .arg(&out)
        .status()
        .expect("python3 runs the generator");
    assert!(status.success(), "generator failed");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        MANUAL,
        "assets/self/MANUAL.md is stale: run python3 scripts/build-self-manual.py"
    );
}

#[test]
fn every_chunk_is_clean_under_the_configured_scanner() {
    let cfg = cgagentharness::common::config::AppConfig::from_str(
        cgagentharness::common::config::AppConfig::embedded_default(),
        std::path::Path::new("config.yaml"),
    )
    .unwrap();
    let scanner = cgagentharness::common::injection::Scanner::from_config(&cfg);
    for chunk in parse_chunks(MANUAL) {
        let hits = scanner.scan_normalized(&format!("{}\n{}", chunk.title, chunk.text));
        assert!(hits.is_empty(), "{} ({}) trips {hits:?}", chunk.id, chunk.title);
    }
}

fn system_prompt(model: &MockModel) -> String {
    model.last_request().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

async fn enabled_server(model: &MockModel) -> TestServer {
    spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("diagnostics.self_knowledge", "true")
            .with("diagnostics.live_state", "true")
            .with("diagnostics.self_manual_max_chunks", "2")
            .with("diagnostics.self_manual_max_chars", "1500"),
    )
    .await
}

fn injected_rows(s: &TestServer) -> Vec<Value> {
    s.audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == "self_manual_injected")
        .collect()
}

#[tokio::test]
async fn local_turns_carry_bounded_manual_chunks_and_the_live_block() {
    let model = start_mock_model().await;
    let s = enabled_server(&model).await;
    let sha = cgagentharness::common::sha256_hex(MANUAL);
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"session_id": sid, "message": "How does the web URL allowlist work?"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let prompt = system_prompt(&model);
    assert!(prompt.contains("<<<SELF_MANUAL_DATA>>>") && prompt.contains(&format!("sha256 {sha}")));
    let chunks = prompt.matches("### self_manual m").count();
    assert!((1..=2).contains(&chunks), "{chunks} chunks");
    let manual =
        &prompt[prompt.find("<<<SELF_MANUAL_DATA>>>").unwrap()..prompt.find("<<<END_SELF_MANUAL_DATA>>>").unwrap()];
    assert!(manual.chars().count() <= 1500 + 200, "body plus the hash line");
    assert!(prompt.contains("<<<LIVE_STATE_DATA>>>") && prompt.contains(&format!("session: {sid}")));
    let rows = injected_rows(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["manual_sha256"], sha);
    assert!(rows[0]["chars"].as_u64().unwrap() <= 1500);
    assert!(rows[0].get("text").is_none() && rows[0].get("query").is_none());

    // /prompt shows the same blocks and the hash line without a model call.
    let calls = model.requests.lock().unwrap().len();
    let (status, preview) = s
        .post_json(
            "/api/prompt/preview",
            json!({"session_id": sid, "retrieve_query": "web URL allowlist"}),
        )
        .await;
    assert_eq!(status, 200, "{preview}");
    let shown = preview["prompt"].as_str().unwrap();
    assert!(shown.contains("self_manual: ") && shown.contains(&format!("sha256 {sha}")));
    assert_eq!(preview["self_manual"]["sha256"], sha);
    assert!(preview["self_manual"]["chunks"].as_array().unwrap().len() <= 2);
    assert_eq!(preview["live_state"], true);
    assert_eq!(model.requests.lock().unwrap().len(), calls);
}

#[tokio::test]
async fn loop_and_cloud_turns_never_carry_them() {
    let model = start_mock_model().await;
    let s = enabled_server(&model).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap().to_string();
    s.post_json(
        &format!("/api/sessions/{sid}/goal"),
        json!({"goal": "explain the web URL allowlist"}),
    )
    .await;
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"session_id": sid, "message": "How does the web URL allowlist work?", "loop": true}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let prompt = system_prompt(&model);
    assert!(!prompt.contains("SELF_MANUAL") && !prompt.contains("LIVE_STATE"));
    let calls = model.requests.lock().unwrap().len();
    s.post_json(
        "/api/chat",
        json!({"session_id": sid, "message": "How does the web URL allowlist work?", "model": "grok"}),
    )
    .await;
    assert_eq!(
        model.requests.lock().unwrap().len(),
        calls,
        "cloud never reaches the local model"
    );
    assert!(injected_rows(&s).is_empty());
}

#[tokio::test]
async fn switched_off_means_absent() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "How does the web URL allowlist work?"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let prompt = system_prompt(&model);
    assert!(!prompt.contains("SELF_MANUAL") && !prompt.contains("LIVE_STATE"));
    assert!(s.state.self_knowledge.manual.is_none());
}
