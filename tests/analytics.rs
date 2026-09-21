//! Analytics composes existing retained sources without transcript or write authority.
mod common;
use cgagentharness::agentic::run_store::{save_run, RealRepoRunRecord};
use common::*;
use serde_json::json;

#[tokio::test]
async fn guarded_summary_matches_chat_ledger_and_sessions_with_disabled_code() {
    let model = start_mock_model().await;
    model.set_reply(ok_reply("reply", 11, 3));
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    assert_eq!(s.open_get("/api/analytics/summary").await.0, 403);
    let empty = s.get_json("/api/analytics/summary").await;
    assert_eq!(empty.0, 200);
    assert_eq!(empty.1["sessions"]["sessions"], json!([]));
    assert_eq!(empty.1["code"]["error"]["code"], "AGENTIC_DISABLED");
    assert!(empty.1["code"].get("runs").is_none());
    let (status, chat) = s
        .post_json("/api/chat", json!({"message":"PRIVATE_ANALYTICS_PROMPT"}))
        .await;
    assert_eq!(status, 200, "{chat}");
    let before = std::fs::read(s.home.join("logs/spend.jsonl")).unwrap();
    let (status, summary) = s.get_json("/api/analytics/summary").await;
    assert_eq!(status, 200, "{summary}");
    assert_eq!(summary["spend"], s.get_json("/api/spend/summary").await.1);
    assert_eq!(summary["sessions"], s.get_json("/api/sessions").await.1);
    assert_eq!(summary["status"]["total_tokens"], 14);
    assert_eq!(summary["session_days"][0]["count"], 1);
    assert_eq!(
        summary["session_days"][0]["day"],
        time::OffsetDateTime::now_utc().date().to_string()
    );
    assert!(!summary.to_string().contains("PRIVATE_ANALYTICS_PROMPT"));
    assert!(!summary.to_string().contains(&s.api_key));
    assert!(summary["sessions"]["sessions"][0].get("messages").is_none());
    assert_eq!(before, std::fs::read(s.home.join("logs/spend.jsonl")).unwrap());
    // Legacy sessions omit created_ts and deserialize it as zero, not a
    // known January 1970 creation date.
    std::fs::write(
        s.home.join("sessions/eeeeeeeeeeee.json"),
        r#"{"session_id":"eeeeeeeeeeee"}"#,
    )
    .unwrap();
    let legacy = s.get_json("/api/analytics/summary").await.1;
    assert_eq!(legacy["sessions_without_created_date"], 1);
    assert_eq!(legacy["session_days"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn run_metrics_cross_the_real_shim_and_keep_truncation_and_unreadable_evidence() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default().with("agentic.enabled", "true"),
    )
    .await;
    let dir = s.home.join("data/agentic/workspaces/runs");
    let mut record = RealRepoRunRecord::new(&"a".repeat(32), "fixture/repository", "unused", "exhausted");
    record.iterations = 3;
    record.changed_files = vec!["private-source.rs".into(), "another.rs".into()];
    record.reject_code = Some("VERIFY_FAILED_LOOP".into());
    save_run(&dir, &mut record).unwrap();
    std::fs::write(dir.join(format!("{}.json", "b".repeat(32))), "not-json").unwrap();
    let (status, data) = s.get_json("/api/analytics/summary").await;
    assert_eq!(status, 200, "{data}");
    assert_eq!(data["code"]["truncated"], false);
    assert_eq!(data["code"]["outcomes"], json!({"exhausted":1,"unreadable":1}));
    let run = data["code"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == record.run_id)
        .unwrap();
    assert_eq!(run["iterations"], 3);
    assert_eq!(run["changed_file_count"], 2);
    assert_eq!(run["reject_code"], "VERIFY_FAILED_LOOP");
    assert!(!data.to_string().contains("private-source.rs"));
    for n in 0..129 {
        let mut r = RealRepoRunRecord::new(&format!("{n:032x}"), "fixture/repository", "unused", "rejected");
        save_run(&dir, &mut r).unwrap();
    }
    let listed = s.get_json("/api/analytics/summary").await.1;
    assert_eq!(listed["code"]["truncated"], true);
    assert_eq!(listed["code"]["runs"].as_array().unwrap().len(), 128);
}

#[tokio::test]
async fn shim_failure_is_unavailable_not_an_empty_success() {
    let model = start_mock_model().await;
    let options = ServerOptions {
        shim_exe: Some("/nonexistent/analytics-fixture".into()),
        ..Default::default()
    };
    let s = spawn_server(&model.base_url(), options).await;
    let (status, data) = s.get_json("/api/analytics/summary").await;
    assert_eq!(status, 200);
    assert_eq!(data["code"]["error"]["code"], "SHIM_IO_ERROR");
    assert!(data["code"].get("runs").is_none());
    assert_eq!(data["spend"]["complete"], true);
}
