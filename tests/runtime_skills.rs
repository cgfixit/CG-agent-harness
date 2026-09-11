mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn selected_skill_is_bounded_persisted_and_has_actual_inclusion_evidence() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default().with("personality.prompt_skill_max_chars", "12"),
    )
    .await;
    let folder = s.home.join("skills/custom");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        "---\nname: misleading\n---\nABCDEFGHIJKL_NOT_INCLUDED",
    )
    .unwrap();
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let id = created["session_id"].as_str().unwrap();
    let path = format!("/api/sessions/{id}/skills");
    let (status, selected) = s.post_json(&path, json!({"ids":["custom"]})).await;
    assert_eq!(status, 200, "{selected}");
    assert!(s.get_json(&path).await.1["last_result"].as_array().unwrap().is_empty());
    let reopened = cgagentharness::server::sessions::SessionStore::new(&s.home.join("sessions")).unwrap();
    assert_eq!(reopened.get(id).unwrap().selected_skills, vec!["custom"]);
    let (_, preview) = s.post_json("/api/prompt/preview", json!({"session_id":id})).await;
    assert_eq!(
        s.post_json("/api/chat", json!({"session_id":id,"message":"test selected context"}))
            .await
            .0,
        200
    );
    let request = model.last_request().unwrap();
    let prompt = request["messages"][0]["content"].as_str().unwrap();
    assert_eq!(preview["prompt"], prompt);
    assert!(prompt.contains("Selected prompt skill: custom"));
    assert!(prompt.contains("ABCDEFGHIJKL"));
    assert!(!prompt.contains("NOT_INCLUDED"));
    assert!(!prompt.contains("name: misleading"));
    let (_, result) = s.get_json(&path).await;
    assert_eq!(result["last_result"][0]["id"], "custom");
    assert_eq!(result["last_result"][0]["chars"], 12);
    let calls = model.requests.lock().unwrap().len();
    std::fs::remove_file(folder.join("SKILL.md")).unwrap();
    assert_eq!(
        s.post_json(
            "/api/chat",
            json!({"session_id":id,"message":"must refuse missing skill"})
        )
        .await
        .0,
        400
    );
    assert_eq!(model.requests.lock().unwrap().len(), calls);
    assert_eq!(s.get_json(&path).await.1["ready"], false);
    assert_eq!(s.post_json(&path, json!({"ids":[]})).await.0, 200);
    assert_eq!(
        s.post_json("/api/chat", json!({"session_id":id,"message":"cleared"}))
            .await
            .0,
        200
    );
    assert!(!model.last_request().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Selected prompt skill"));
}

#[tokio::test]
async fn unknown_ids_shell_text_and_escaping_links_cannot_execute_or_load() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let id = created["session_id"].as_str().unwrap();
    let path = format!("/api/sessions/{id}/skills");
    for id in ["../secret", "custom;touch marker", "/etc/passwd", "unknown"] {
        assert_ne!(s.post_json(&path, json!({"ids":[id]})).await.0, 200);
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("SKILL.md"), "OUTSIDE").unwrap();
        std::os::unix::fs::symlink(outside.path(), s.home.join("skills/escape")).unwrap();
        assert_eq!(s.post_json(&path, json!({"ids":["escape"]})).await.0, 400);
    }
    let (status, check) = s.post_json("/api/skills/check", json!({"id":"check:cargo-test"})).await;
    assert_eq!(status, 200);
    assert_eq!(check["executed"], false);
    assert_eq!(check["adapter"]["argv"], json!(["cargo", "test", "--quiet"]));
    for id in ["check:sh -c touch marker", "check:unknown", "custom"] {
        assert_eq!(s.post_json("/api/skills/check", json!({"id":id})).await.0, 400);
    }
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(s.state.jobs.running_count(), 0);
}
