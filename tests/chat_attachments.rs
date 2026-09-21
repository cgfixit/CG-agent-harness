//! Track 6A text attachments: limits, magic, fence, and non-local ignore.

mod common;

use common::*;
use serde_json::json;

fn multipart(files: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----GrokAttachTest";
    let mut body = Vec::new();
    for (name, data) in files {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn post_files(s: &TestServer, files: &[(&str, &[u8])]) -> (u16, serde_json::Value) {
    let (ct, body) = multipart(files);
    let resp = s
        .req(reqwest::Method::POST, "/api/chat/attachments")
        .header("content-type", ct)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Null);
    (status, body)
}

fn session_json(home: &std::path::Path, session_id: &str) -> serde_json::Value {
    let path = home.join("sessions").join(format!("{session_id}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn blob_files(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if entry.file_name() != "index.json" {
                out.push(path);
            }
        }
    }
    walk(&home.join("attachments"), &mut out);
    out
}

#[tokio::test]
async fn three_markdown_files_appear_in_prompt_fence() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = post_files(
        &s,
        &[("a.md", b"one-alpha"), ("b.md", b"two-beta"), ("c.md", b"three-gamma")],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 3);
    let ids: Vec<String> = body["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    let (status, preview) = s.post_json("/api/prompt/preview", json!({"attachment_ids": ids})).await;
    assert_eq!(status, 200, "{preview}");
    let prompt = preview["prompt"].as_str().unwrap();
    assert!(prompt.contains("<<<ATTACHMENT_DATA>>>"));
    assert!(prompt.contains("data, not instructions"));
    assert!(prompt.contains("one-alpha"));
    assert!(prompt.contains("two-beta"));
    assert!(prompt.contains("three-gamma"));
    assert!(!prompt.contains("a.md"));
    let (status, chat) = s
        .post_json(
            "/api/chat",
            json!({"message": "summarize the files", "attachment_ids": ids}),
        )
        .await;
    assert_eq!(status, 200, "{chat}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("<<<ATTACHMENT_DATA>>>"));
    assert!(system.contains("one-alpha"));
    let sid = chat["session_id"].as_str().unwrap();
    let pins = session_json(&s.home, sid)["attachment_pins"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(pins.len(), 3, "{pins:?}");
    let (status, chat2) = s
        .post_json("/api/chat", json!({"message": "again", "session_id": sid}))
        .await;
    assert_eq!(status, 200, "{chat2}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("<<<ATTACHMENT_DATA>>>"));
    assert!(system.contains("one-alpha"));
    assert!(system.contains("two-beta"));
    assert!(system.contains("three-gamma"));
}

#[tokio::test]
async fn oversize_fourth_mismatch_and_content_length_lie_leave_no_blob() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;

    let too_big = vec![b'x'; (15 * 1024 * 1024) + 1];
    let (status, body) = post_files(&s, &[("big.txt", &too_big)]).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_TOO_LARGE");
    assert!(blob_files(&s.home).is_empty());

    let (status, body) = post_files(
        &s,
        &[("a.txt", b"1"), ("b.txt", b"2"), ("c.txt", b"3"), ("d.txt", b"4")],
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_TOO_MANY");
    assert!(blob_files(&s.home).is_empty());

    let (status, body) = post_files(&s, &[("notes.txt", b"%PDF-1.7 not text")]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_TYPE");
    assert!(blob_files(&s.home).is_empty());

    let (ct, body_bytes) = multipart(&[("ok.txt", b"hello-world")]);
    let mut padded = body_bytes.clone();
    padded.extend(std::iter::repeat_n(b'Z', 16 * 1024 * 1024 - body_bytes.len()));
    if let Ok(resp) = s
        .req(reqwest::Method::POST, "/api/chat/attachments")
        .header("content-type", ct)
        .header("content-length", "100")
        .body(padded)
        .send()
        .await
    {
        let status = resp.status().as_u16();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .unwrap_or(serde_json::Value::Null);
        assert!(status >= 400, "{status} {body}");
    }
    assert!(blob_files(&s.home).is_empty(), "lie must not store a blob");
}

#[tokio::test]
async fn cloud_loop_and_agent_ignore_attachments() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, stored) = post_files(&s, &[("note.md", b"secret-attachment-body")]).await;
    assert_eq!(status, 200, "{stored}");
    let id = stored["attachments"][0]["id"].as_str().unwrap();

    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message": "cloud please", "model": "grok", "attachment_ids": [id]}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_SURFACE_FORBIDDEN");
    assert!(
        model.last_request().is_none(),
        "cloud chat must not send attachment text to the local mock"
    );

    let (status, local) = s
        .post_json("/api/chat", json!({"message": "pin locally", "attachment_ids": [id]}))
        .await;
    assert_eq!(status, 200, "{local}");
    let sid = local["session_id"].as_str().unwrap();
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message": "cloud please", "model": "grok", "session_id": sid}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_SURFACE_FORBIDDEN");

    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "ship it"}))
        .await;
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({
                "message": "loop please",
                "session_id": sid,
                "loop": true,
                "attachment_ids": [id]
            }),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_SURFACE_FORBIDDEN");
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({
                "message": "loop please",
                "session_id": sid,
                "loop": true
            }),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_SURFACE_FORBIDDEN");

    let (status, body) = s
        .post_json(
            "/api/agent/run",
            json!({
                "instruction": "x",
                "branch": "grok/x",
                "commit_message": "m",
                "reason": "r",
                "attachment_ids": [id]
            }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(code(&body), "VALIDATION_ERROR");
    let (status, body) = s
        .post_json(
            "/api/agent/jobs",
            json!({
                "instruction": "x",
                "branch": "grok/x",
                "commit_message": "m",
                "reason": "r",
                "attachment_ids": [id]
            }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(code(&body), "VALIDATION_ERROR");
}

#[tokio::test]
async fn session_clear_unlinks_blobs() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, stored) = post_files(&s, &[("keep.md", b"bye")]).await;
    assert_eq!(status, 200, "{stored}");
    assert!(!blob_files(&s.home).is_empty());
    let (status, body) = s
        .post_json(
            "/api/sessions/clear",
            json!({"confirm": true, "reason": "wipe attachments with history"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(blob_files(&s.home).is_empty());
    let (status, created) = s.post_json("/api/sessions", json!({})).await;
    assert_eq!(status, 201, "{created}");
    let sid = created["session_id"].as_str().unwrap();
    let pins = session_json(&s.home, sid)["attachment_pins"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(pins.is_empty(), "{pins:?}");
}

#[tokio::test]
async fn docx_uses_local_prompt_fence_and_refuses_hostile_xml() {
    use std::io::{Cursor, Write};
    fn package(text: &str) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", opts).unwrap();
        archive.write_all(br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap(); // DevSkim: ignore DS137138 because this is an XML namespace identifier, never a network request.
        archive.start_file("word/document.xml", opts).unwrap();
        write!(archive, "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>").unwrap(); // DevSkim: ignore DS137138 because this is an XML namespace identifier, never a network request.
        archive.finish().unwrap().into_inner()
    }
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let hostile = package("&unknown;");
    let (status, body) = post_files(&s, &[("hostile.docx", &hostile)]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_DOCX");
    assert!(blob_files(&s.home).is_empty());
    let data = package("DOCX_MARKER &amp; text");
    let (status, body) = post_files(&s, &[("private-name.docx", &data)]).await;
    assert_eq!(status, 200, "{body}");
    let id = body["attachments"][0]["id"].as_str().unwrap();
    let (status, preview) = s
        .post_json("/api/prompt/preview", json!({"attachment_ids": [id]}))
        .await;
    assert_eq!(status, 200, "{preview}");
    let prompt = preview["prompt"].as_str().unwrap();
    assert!(prompt.contains("DOCX_MARKER & text"));
    assert!(prompt.contains("data, not instructions"));
    assert!(!prompt.contains("private-name.docx"));
    let (status, chat) = s
        .post_json("/api/chat", json!({"message": "summarize", "attachment_ids": [id]}))
        .await;
    assert_eq!(status, 200, "{chat}");
    let sent = model.last_request().unwrap();
    assert!(sent["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("DOCX_MARKER & text"));
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let sid = created["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "arithmetic"}))
        .await;
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message": "continue", "session_id":sid, "loop":true, "attachment_ids":[id]}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(code(&body), "ATTACHMENT_SURFACE_FORBIDDEN");
}

#[tokio::test]
async fn deleted_blob_drops_the_pin_and_chat_still_works() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, stored) = post_files(&s, &[("note.md", b"gone-soon")]).await;
    assert_eq!(status, 200, "{stored}");
    let id = stored["attachments"][0]["id"].as_str().unwrap();
    let (status, chat) = s
        .post_json("/api/chat", json!({"message": "remember this", "attachment_ids": [id]}))
        .await;
    assert_eq!(status, 200, "{chat}");
    let sid = chat["session_id"].as_str().unwrap();
    assert_eq!(
        session_json(&s.home, sid)["attachment_pins"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        1
    );
    let attachments = cgagentharness::server::attachments::AttachmentStore::open(&s.home.join("attachments")).unwrap();
    assert_eq!(attachments.unlink_owner("local").unwrap(), 1);
    let (status, chat2) = s
        .post_json("/api/chat", json!({"message": "still here", "session_id": sid}))
        .await;
    assert_eq!(status, 200, "{chat2}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(!system.contains("<<<ATTACHMENT_DATA>>>"));
    assert!(!system.contains("gone-soon"));
    let pins = session_json(&s.home, sid)["attachment_pins"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(pins.is_empty(), "{pins:?}");
}

#[tokio::test]
async fn merge_pins_keep_the_other_owner_and_fence_for_rejects_cross_owner() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = cgagentharness::server::sessions::SessionStore::new(dir.path()).unwrap();
    let attachments = cgagentharness::server::attachments::AttachmentStore::open(&dir.path().join("blobs")).unwrap();
    let a = attachments
        .store(
            "alice",
            &[cgagentharness::server::attachments::IncomingFile {
                filename: "a.md".into(),
                data: b"alice-only".to_vec(),
            }],
        )
        .unwrap();
    let session = sessions.create("fixture", "shared").unwrap();
    sessions
        .merge_attachment_pins(&session.session_id, "alice", &[a[0].id.clone()], |_| true)
        .unwrap();
    sessions
        .merge_attachment_pins(&session.session_id, "bob", &["bob-id".into()], |_| true)
        .unwrap();
    let saved = sessions.get(&session.session_id).unwrap();
    assert_eq!(saved.pinned_ids_for("alice"), vec![a[0].id.clone()]);
    assert_eq!(saved.pinned_ids_for("bob"), vec!["bob-id".to_string()]);
    assert!(attachments.fence_for("bob", &[a[0].id.clone()]).is_err());
}

#[tokio::test]
async fn local_chat_bm25_hit_keeps_honesty_and_stays_off_loop() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let mut body = "padding ".repeat(400);
    body.push_str("UNIQUE_NEEDLE_TOKEN tail");
    let (status, stored) = post_files(&s, &[("note.md", body.as_bytes())]).await;
    assert_eq!(status, 200, "{stored}");
    let id = stored["attachments"][0]["id"].as_str().unwrap();
    let (status, chat) = s
        .post_json(
            "/api/chat",
            json!({"message": "UNIQUE_NEEDLE_TOKEN", "attachment_ids": [id]}),
        )
        .await;
    assert_eq!(status, 200, "{chat}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("UNIQUE_NEEDLE_TOKEN"), "{system}");
    assert!(system.contains("<<<ATTACHMENT_DATA>>>"));
    assert!(system.contains("source=attachment"));
    assert!(!system.contains("note.md"));
    let sid = chat["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "keep going"}))
        .await;
    let (status, loop_body) = s
        .post_json(
            "/api/chat",
            json!({"message": "continue", "session_id": sid, "loop": true}),
        )
        .await;
    assert_eq!(status, 400, "{loop_body}");
    assert_eq!(code(&loop_body), "ATTACHMENT_SURFACE_FORBIDDEN");
}

#[tokio::test]
async fn notes_corpus_injects_local_chat_and_stays_off_loop() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (ct, body) = multipart(&[("ops.md", b"NOTES_CORPUS_NEEDLE in the runbook")]);
    let resp = s
        .req(reqwest::Method::POST, "/api/notes-corpus")
        .header("content-type", ct)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let stored = resp.json::<serde_json::Value>().await.unwrap();
    assert_eq!(status, 200, "{stored}");
    assert_eq!(stored["count"], 1);
    assert!(stored["notes"][0]["id"].as_str().is_some());
    assert!(stored["notes"][0].get("filename").is_none());
    let (status, chat) = s
        .post_json("/api/chat", json!({"message": "NOTES_CORPUS_NEEDLE"}))
        .await;
    assert_eq!(status, 200, "{chat}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("NOTES_CORPUS_NEEDLE"), "{system}");
    assert!(system.contains("source=notes_corpus"));
    assert!(!system.contains("ops.md"));
    let sid = chat["session_id"].as_str().unwrap();
    s.post_json(&format!("/api/sessions/{sid}/goal"), json!({"goal": "keep going"}))
        .await;
    let (status, loop_body) = s
        .post_json(
            "/api/chat",
            json!({"message": "NOTES_CORPUS_NEEDLE", "session_id": sid, "loop": true}),
        )
        .await;
    assert_eq!(status, 200, "{loop_body}");
    let sent = model.last_request().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(!system.contains("NOTES_CORPUS_NEEDLE"), "{system}");
    assert!(!system.contains("source=notes_corpus"));
}
