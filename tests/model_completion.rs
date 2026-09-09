//! A syntactically complete prefix is not complete when the provider says length.
mod common;

use cgagentharness::agentic::proposer::{LocalProposerClient, ProposerClient};
use cgagentharness::common::audit::Audit;
use cgagentharness::llm::openai_chat::parse_chat_response;
use common::*;
use serde_json::json;

#[test]
fn chat_requires_a_completed_choice_before_returning_content() {
    for reason in [
        json!("length"),
        json!("tool_calls"),
        json!("content_filter"),
        json!(null),
    ] {
        let response =
            json!({"choices": [{"finish_reason": reason, "message": {"content": "complete-looking prefix"}}]});
        assert!(parse_chat_response(&response, "local").is_err(), "{reason}");
    }
    assert!(parse_chat_response(
        &json!({"choices": [{"message": {"content": "missing completion state"}}]}),
        "local"
    )
    .is_err());
    let response = json!({"choices": [{"finish_reason": "stop", "message": {"content": "complete"}}]});
    assert_eq!(parse_chat_response(&response, "local").unwrap().body_text, "complete");
}

#[tokio::test]
async fn planner_refuses_provider_truncation_even_with_a_complete_edit_block() {
    let model = start_mock_model().await;
    model.set_reply(json!({"choices": [{"finish_reason": "length", "message": {"content": "=== FILE: src/lib.rs ===\nvalid prefix\n=== END FILE ==="}}]}));
    let base = model.base_url();
    let rejected = tokio::task::spawn_blocking(move || {
        let tmp = tempfile::tempdir().unwrap();
        let audit = Audit::new(tmp.path().join("audit.jsonl"), &config_with(tmp.path(), &[]));
        let client = LocalProposerClient::new(&audit, &base, "local", 5, "", Some("none".into())).unwrap();
        client.invoke("system", "user", 32, None).is_err()
    })
    .await
    .unwrap();
    assert!(rejected, "truncated complete prefix must never reach patch application");
}
