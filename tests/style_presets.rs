//! Track 5 output-style presets: session select, compose, suggestions.

mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn style_concise_changes_compose_and_off_reverts_without_touching_soul() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let soul_before = std::fs::read_to_string(s.home.join("soul.md")).unwrap();
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();

    let (status, preview) = s.post_json("/api/prompt/preview", json!({"session_id": sid})).await;
    assert_eq!(status, 200, "{preview}");
    let off_prompt = preview["prompt"].as_str().unwrap();
    assert!(off_prompt.contains("style=off"));
    assert!(!off_prompt.contains("## Output style (concise, read-only)"));
    assert!(off_prompt.contains("You have no filesystem, shell, gh, account or policy-editing tools"));

    let (status, set) = s
        .post_json("/api/style", json!({"session_id": sid, "name": "concise"}))
        .await;
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["style"], "concise");
    assert_eq!(set["origin"], "builtin");
    assert_eq!(s.state.store.get(sid).unwrap().style.as_deref(), Some("concise"));

    let (status, preview) = s.post_json("/api/prompt/preview", json!({"session_id": sid})).await;
    assert_eq!(status, 200, "{preview}");
    let on_prompt = preview["prompt"].as_str().unwrap();
    assert!(on_prompt.contains("style=concise"));
    assert!(on_prompt.contains("## Output style (concise, read-only)"));
    assert!(on_prompt.contains("Answer first"));
    let soul_at = on_prompt.find("## Operator persona (soul, read-only)").unwrap();
    let style_at = on_prompt.find("## Output style (concise, read-only)").unwrap();
    let header_at = on_prompt.find("You are CG Agent Harness").unwrap();
    assert!(soul_at < style_at);
    assert!(style_at < header_at);
    assert_eq!(preview["style"]["name"], "concise");

    let (status, reply) = s
        .post_json("/api/chat", json!({"session_id": sid, "message": "hello"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    let request = model.last_request().unwrap();
    let chat_prompt = request["messages"][0]["content"].as_str().unwrap();
    assert!(chat_prompt.contains("## Output style (concise, read-only)"));
    assert!(chat_prompt.contains("Answer first"));

    let (status, cleared) = s
        .post_json("/api/style", json!({"session_id": sid, "name": "off"}))
        .await;
    assert_eq!(status, 200, "{cleared}");
    assert!(cleared["style"].is_null());
    let (status, preview) = s.post_json("/api/prompt/preview", json!({"session_id": sid})).await;
    assert_eq!(status, 200, "{preview}");
    let reverted = preview["prompt"].as_str().unwrap();
    assert!(reverted.contains("style=off"));
    assert!(!reverted.contains("## Output style (concise, read-only)"));
    assert_eq!(std::fs::read_to_string(s.home.join("soul.md")).unwrap(), soul_before);
}

#[tokio::test]
async fn missing_style_name_suggests_closest_builtin() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    let (status, body) = s
        .post_json("/api/style", json!({"session_id": sid, "name": "concis"}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "STYLE_UNKNOWN");
    assert_eq!(body["detail"]["details"]["suggestion"], "concise");
    assert!(message(&body).contains("concise"));
    assert!(s.state.store.get(sid).unwrap().style.is_none());
}

#[tokio::test]
async fn overlay_style_wins_builtin_and_does_not_import_agentic() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    std::fs::create_dir_all(s.home.join("styles")).unwrap();
    std::fs::write(
        s.home.join("styles").join("concise.md"),
        "OVERLAY_WINS the next local reply.\n",
    )
    .unwrap();
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    let (status, set) = s
        .post_json("/api/style", json!({"session_id": sid, "name": "concise"}))
        .await;
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["origin"], "overlay");
    let (_, preview) = s.post_json("/api/prompt/preview", json!({"session_id": sid})).await;
    let prompt = preview["prompt"].as_str().unwrap();
    assert!(prompt.contains("OVERLAY_WINS"));
    assert!(!prompt.contains("Answer first"));
}
