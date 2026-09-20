//! Slice A spend ledger: shipped parse/append/rollup on the real chat path.

mod common;

use common::*;
use serde_json::{json, Value};

#[tokio::test]
async fn local_chat_appends_unpriced_usage_without_prompt_or_key() {
    let model = start_mock_model().await;
    model.set_reply(ok_reply("pong", 11, 3));
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let secret = "SECRET_PROMPT_ledger_xyz";
    let (status, body) = s.post_json("/api/chat", json!({"message": secret})).await;
    assert_eq!(status, 200, "{body}");
    let path = s.home.join("logs/spend.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(secret));
    assert!(!text.contains(&s.api_key));
    assert!(!text.contains("\"usd\""));
    let row: Value = serde_json::from_str(text.trim().lines().next().unwrap()).unwrap();
    assert_eq!(row["provider"], "local");
    assert_eq!(row["source"], "chat");
    assert_eq!(row["input_tokens"], 11);
    assert_eq!(row["output_tokens"], 3);
    assert_eq!(row["usage_missing"], false);
    let (status, summary) = s.get_json("/api/spend/summary").await;
    assert_eq!(status, 200, "{summary}");
    assert_eq!(summary["rows"], 1);
    let day = &summary["days"][0];
    assert_eq!(day["provider"], "local");
    assert!(day["estimate"]["usd"].is_null());
    assert_eq!(day["estimate"]["usd_source"], "local_unpriced");
    let reread = cgagentharness::llm::spend::summarize_file(&path);
    assert_eq!(reread["rows"], 1);
}

#[tokio::test]
async fn missing_local_usage_marks_usage_missing() {
    let model = start_mock_model().await;
    model.set_reply(json!({
        "model": "mock-model",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "ok"}}],
        "usage": {}
    }));
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    let text = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let row: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(row["usage_missing"], true);
    assert!(row["input_tokens"].is_null());
}

#[tokio::test]
async fn spend_summary_is_csrf_guarded() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, _) = s.open_get("/api/spend/summary").await;
    assert_ne!(status, 200);
}

#[test]
fn summary_distinguishes_empty_corrupt_unreadable_and_oversized_generations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spend.jsonl");
    let summarize = || cgagentharness::llm::spend::summarize_file(&path);
    assert_eq!(summarize()["complete"], true);
    let good = json!({"provider":"local","model":"fixture","timestamp":"2026-09-20T00:00:00Z","input_tokens":4,"output_tokens":2});
    std::fs::write(&path, format!("{good}\nnot-json\nnull\n{{}}\n")).unwrap();
    let partial = summarize();
    assert_eq!(partial["complete"], false);
    assert_eq!(partial["rows"], 1);
    assert_eq!(partial["skipped_rows"], 3);
    assert_eq!(partial["days"][0]["input_tokens"], 4);
    std::fs::rename(&path, path.with_file_name("spend.jsonl.1")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(summarize()["files"][0]["status"], "unreadable");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, [0xff]).unwrap();
    assert_eq!(summarize()["files"][0]["status"], "invalid_utf8");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert_eq!(summarize()["files"][0]["status"], "too_large");
    std::fs::write(&path, format!("{good}\n")).unwrap();
    std::fs::write(path.with_file_name("spend.jsonl.1"), format!("{good}\n")).unwrap();
    assert_eq!(summarize()["complete"], true);
    assert_eq!(summarize()["rows"], 2);
}
