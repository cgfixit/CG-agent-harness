//! `/diag` routes and the `portal_auditor` role over real accounts: gates and
//! roles refuse with codes, never empty success; the file parameter names an
//! allowlist entry or is refused; audit rows carry no content; a staged excerpt
//! reaches exactly one local chat turn of its session.
mod common;
use common::*;
use reqwest::{Method, Response};
use serde_json::{json, Value};

async fn account(server: &TestServer, name: &str, role: &str) -> String {
    let auth = server.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user(name, &password, role).unwrap();
    let response = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":name,"password":password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn call(s: &TestServer, cookie: &str, method: Method, path: &str, body: Option<Value>) -> Response {
    let mut req = s.req(method, path).header("cookie", cookie);
    if let Some(body) = body {
        req = req.json(&body);
    }
    req.send().await.unwrap()
}

async fn answered(s: &TestServer, response: Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(Value::Null);
    s.state.audit.flush();
    (status, body)
}

async fn json_call(s: &TestServer, cookie: &str, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let response = call(s, cookie, method, path, body).await;
    answered(s, response).await
}

async fn server(model: &MockModel, logs_enabled: bool) -> TestServer {
    spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", if logs_enabled { "true" } else { "false" }),
    )
    .await
}

fn audit_rows(s: &TestServer, event: &str) -> Vec<Value> {
    s.audit_log()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == event)
        .collect()
}

#[tokio::test]
async fn a_closed_gate_and_wrong_roles_refuse_with_codes() {
    let model = start_mock_model().await;
    let s = server(&model, false).await;
    let admin = account(&s, "ops-admin", "admin").await;
    for path in ["/api/diag", "/api/diag/read?file=audit.jsonl"] {
        let (status, body) = json_call(&s, &admin, Method::GET, path, None).await;
        assert_eq!((status, code(&body).as_str()), (409, "DIAG_DISABLED"), "{path}: {body}");
    }
    assert!(audit_rows(&s, "diag_refused")
        .iter()
        .any(|r| r["code"] == "DIAG_DISABLED"));

    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let operator = account(&s, "ops-operator", "operator").await;
    let (status, body) = json_call(&s, &operator, Method::GET, "/api/diag", None).await;
    assert_eq!((status, code(&body).as_str()), (403, "DIAG_REFUSED"), "{body}");
    let auditor = account(&s, "ops-audit", "audit").await;
    let (status, body) = json_call(&s, &auditor, Method::GET, "/api/diag", None).await;
    assert_eq!(
        (status, code(&body).as_str()),
        (403, "AUTH_PERMISSION_DENIED"),
        "{body}"
    );
    for (role, name) in [("admin", "ops-admin"), ("portal_auditor", "ops-portal")] {
        let cookie = account(&s, name, role).await;
        let (status, body) = json_call(&s, &cookie, Method::GET, "/api/diag", None).await;
        assert_eq!(status, 200, "{role}: {body}");
        assert_eq!(body["logs_enabled"], true);
        assert_eq!(body["model_tool"], false);
        let files: Vec<&str> = body["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["file"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                "audit.jsonl",
                "audit.jsonl.1",
                "spend.jsonl",
                "spend.jsonl.1",
                "unslop.jsonl"
            ]
        );
    }
}

// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test below.
#[cfg(unix)]
#[tokio::test]
async fn reads_name_an_allowlist_entry_and_audit_no_content() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let portal = account(&s, "ops-portal", "portal_auditor").await;
    let (status, body) = json_call(&s, &portal, Method::GET, "/api/diag/read?file=audit.jsonl&last=5", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source"], "diag");
    assert_eq!(body["redacted"], true);
    assert!(body["lines_returned"].as_u64().unwrap() >= 1 && body["lines_returned"].as_u64().unwrap() <= 5);
    let text = body["text"].as_str().unwrap();
    assert_eq!(body["sha256"], cgagentharness::common::sha256_hex(text));
    let (status, body) = json_call(
        &s,
        &portal,
        Method::GET,
        "/api/diag/read?file=audit.jsonl&event=portal.request&since=1h",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["text"]
        .as_str()
        .unwrap()
        .lines()
        .all(|l| l.contains("\"portal.request\"")));
    let reads = audit_rows(&s, "diag_read");
    let last = reads.last().unwrap();
    assert_eq!(last["file"], "audit.jsonl");
    assert_eq!(last["event_filter"], "portal.request");
    assert_eq!(last["since_secs"], 3600);
    assert_eq!(last["outcome"], "ok");
    assert!(last.get("text").is_none() && last.get("sha256").is_none());

    for bad in [
        "../needle-7731.yaml",
        "logs/needle-7731.jsonl",
        "Audit.jsonl",
        "audit.jsonl%20",
        "audit.jsonl.",
        "%2Fneedle-7731%2Fpasswd",
        ".needle-7731",
    ] {
        let path = format!("/api/diag/read?file={bad}");
        let (status, body) = json_call(&s, &portal, Method::GET, &path, None).await;
        assert_eq!((status, code(&body).as_str()), (400, "DIAG_BAD_FILE"), "{bad}: {body}");
    }
    for bad in [
        "/api/diag/read?file=audit.jsonl&file=spend.jsonl",
        "/api/diag/read?file=audit.jsonl&path=/etc/passwd",
        "/api/diag/read?file=audit.jsonl&last=0",
        "/api/diag/read?file=audit.jsonl&event=Bad",
        "/api/diag/read?file=audit.jsonl&since=31d",
        "/api/diag/read",
    ] {
        let (status, body) = json_call(&s, &portal, Method::GET, bad, None).await;
        assert_eq!(
            (status, code(&body).as_str()),
            (400, "DIAG_BAD_REQUEST"),
            "{bad}: {body}"
        );
    }
    let log = s.audit_log();
    assert!(audit_rows(&s, "diag_refused")
        .iter()
        .any(|r| r["code"] == "DIAG_BAD_FILE"));
    for content in ["needle-7731", "Audit.jsonl"] {
        assert!(
            !log.contains(content),
            "audit must not record request content: {content}"
        );
    }
    let (status, body) = json_call(&s, &portal, Method::GET, "/api/diag/read?file=spend.jsonl", None).await;
    assert_eq!((status, code(&body).as_str()), (404, "DIAG_NOT_FOUND"), "{body}");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_refuses_reads_but_still_validates_the_file_name() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let admin = account(&s, "ops-admin", "admin").await;
    let (status, body) = json_call(&s, &admin, Method::GET, "/api/diag/read?file=audit.jsonl", None).await;
    assert_eq!((status, code(&body).as_str()), (409, "DIAG_UNSUPPORTED"), "{body}");
    let (status, body) = json_call(&s, &admin, Method::GET, "/api/diag/read?file=..%2Fconfig.yaml", None).await;
    assert_eq!((status, code(&body).as_str()), (400, "DIAG_BAD_FILE"), "{body}");
    let (status, body) = json_call(&s, &admin, Method::GET, "/api/diag", None).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn inject_and_forget_need_csrf() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let admin = account(&s, "ops-admin", "admin").await;
    for path in ["/api/diag/inject", "/api/diag/forget"] {
        let response = s
            .client
            .post(s.url(path))
            .header("cookie", &admin)
            .json(&json!({"session_id":"000000000000"}))
            .send()
            .await
            .unwrap();
        let (status, body) = answered(&s, response).await;
        assert_eq!(
            (status, code(&body).as_str()),
            (403, "CSRF_TOKEN_INVALID"),
            "{path}: {body}"
        );
    }
    let (status, body) = json_call(&s, &admin, Method::GET, "/api/diag/inject", None).await;
    assert_eq!(status, 405, "{body}");
}

#[cfg(unix)]
fn system_prompt(request: &Value) -> String {
    request["messages"][0]["content"].as_str().unwrap_or("").to_string()
}

// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test below.
#[cfg(unix)]
#[tokio::test]
async fn a_staged_excerpt_reaches_one_local_turn_and_is_refused_elsewhere() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let admin = account(&s, "ops-admin", "admin").await;
    let (_, session) = json_call(&s, &admin, Method::POST, "/api/sessions", Some(json!({"title":"diag"}))).await;
    let sid = session["session_id"].as_str().unwrap().to_string();
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/diag/inject",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!((status, code(&body).as_str()), (409, "DIAG_NOTHING_TO_STAGE"), "{body}");
    let (status, read) = json_call(&s, &admin, Method::GET, "/api/diag/read?file=audit.jsonl&last=3", None).await;
    assert_eq!(status, 200, "{read}");
    let (status, staged) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/diag/inject",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!(status, 200, "{staged}");
    assert_eq!(staged["file"], "audit.jsonl");
    let inject = audit_rows(&s, "diag_inject");
    assert_eq!(inject.last().unwrap()["sha256"], staged["sha256"]);
    assert!(inject.last().unwrap().get("text").is_none());

    let (status, preview) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/prompt/preview",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    let prompt = preview["prompt"].as_str().unwrap();
    assert!(prompt.contains("<<<DIAG_DATA>>>") && prompt.contains("UNTRUSTED LOG EXCERPT"));
    assert!(prompt.contains("source=diag"));
    assert_eq!(preview["diag"]["sha256"], staged["sha256"]);

    // Cloud chat and /agent refuse while it is staged; nothing is consumed.
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "hi", "model": "grok"})),
    )
    .await;
    assert_eq!(
        (status, code(&body).as_str()),
        (400, "DIAG_SURFACE_FORBIDDEN"),
        "{body}"
    );
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/agent/run",
        Some(json!({"instruction":"x","branch":"claude/x","commit_message":"m","reason":"r","checks":["cargo-fmt"]})),
    )
    .await;
    assert_eq!(
        (status, code(&body).as_str()),
        (400, "DIAG_SURFACE_FORBIDDEN"),
        "{body}"
    );

    // /loop never receives it and leaves it staged.
    let (status, _) = json_call(
        &s,
        &admin,
        Method::POST,
        &format!("/api/sessions/{sid}/goal"),
        Some(json!({"goal":"g"})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "loop turn", "loop": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(!system_prompt(&model.last_request().unwrap()).contains("DIAG_DATA"));
    let (_, status_body) = json_call(&s, &admin, Method::GET, "/api/diag", None).await;
    assert_eq!(status_body["staged"]["session_id"], sid.as_str());

    // The next local turn carries it once.
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "hi"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let sent = system_prompt(&model.last_request().unwrap());
    assert!(sent.contains("<<<DIAG_DATA>>>") && sent.contains("You did not read these files"));
    let (status, _) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "again"})),
    )
    .await;
    assert_eq!(status, 200);
    assert!(!system_prompt(&model.last_request().unwrap()).contains("DIAG_DATA"));

    // A turn in another session drops it; forget and clear drop it too.
    json_call(
        &s,
        &admin,
        Method::POST,
        "/api/diag/inject",
        Some(json!({"session_id": sid})),
    )
    .await;
    let (status, _) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"message": "elsewhere"})),
    )
    .await;
    assert_eq!(status, 200);
    assert!(!system_prompt(&model.last_request().unwrap()).contains("DIAG_DATA"));
    let (_, status_body) = json_call(&s, &admin, Method::GET, "/api/diag", None).await;
    assert!(status_body["staged"].is_null());
    json_call(
        &s,
        &admin,
        Method::POST,
        "/api/diag/inject",
        Some(json!({"session_id": sid})),
    )
    .await;
    let (_, forgot) = json_call(&s, &admin, Method::POST, "/api/diag/forget", None).await;
    assert_eq!(forgot["cleared"], true);
}

#[tokio::test]
async fn portal_auditor_chats_locally_without_memory_or_other_surfaces() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let portal = account(&s, "ops-portal", "portal_auditor").await;
    let (status, session) = json_call(&s, &portal, Method::POST, "/api/sessions", Some(json!({"title":"p"}))).await;
    assert_eq!(status, 201, "{session}");
    let sid = session["session_id"].as_str().unwrap().to_string();
    std::fs::write(s.home.join("soul.md"), "SHARED-PERSONA-MARKER").unwrap();
    let (status, body) = json_call(&s, &portal, Method::GET, "/api/status", None).await;
    assert_eq!(status, 200, "{body}");
    for key in ["home", "layout", "base_url", "soul", "model_status"] {
        assert!(body.get(key).is_none(), "status exposes {key}: {body}");
    }
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "hello"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["memory_suggestion"]["reason"], "portal_auditor");
    let request = model.last_request().unwrap();
    assert!(!request["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .contains("SHARED-PERSONA-MARKER"));
    assert!(request
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
    let (status, _) = json_call(&s, &portal, Method::GET, &format!("/api/sessions/{sid}"), None).await;
    assert_eq!(status, 200);
    for body in [
        json!({"session_id": sid, "message": "x", "model": "grok"}),
        json!({"session_id": sid, "message": "x", "model": "other-local-model"}),
        json!({"session_id": sid, "message": "x", "retrieve": true}),
        json!({"session_id": sid, "message": "x", "selected_facts": []}),
        json!({"session_id": sid, "message": "x", "loop": true}),
    ] {
        let (status, reply) = json_call(&s, &portal, Method::POST, "/api/chat", Some(body.clone())).await;
        assert_eq!(
            (status, code(&reply).as_str()),
            (403, "AUTH_PERMISSION_DENIED"),
            "{body}: {reply}"
        );
    }
    for (method, path) in [
        (Method::GET, "/api/memory".to_string()),
        (Method::GET, "/api/structured-memory".to_string()),
        (Method::DELETE, format!("/api/sessions/{sid}")),
        (Method::POST, "/api/sessions/clear".to_string()),
        (Method::GET, format!("/api/sessions/{sid}/export")),
        (Method::GET, "/api/keys".to_string()),
        (Method::POST, "/api/model".to_string()),
        (Method::POST, "/api/web/inject".to_string()),
        (Method::GET, "/api/notes-corpus".to_string()),
        (Method::GET, "/api/tools".to_string()),
    ] {
        let (status, reply) = json_call(&s, &portal, method.clone(), &path, Some(json!({}))).await;
        assert_eq!(
            (status, code(&reply).as_str()),
            (403, "AUTH_PERMISSION_DENIED"),
            "{method} {path}: {reply}"
        );
    }
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/prompt/preview",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.to_string().contains("SHARED-PERSONA-MARKER"), "{body}");
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/prompt/preview",
        Some(json!({"session_id": sid, "soul_content": "x"})),
    )
    .await;
    assert_eq!(
        (status, code(&body).as_str()),
        (403, "AUTH_PERMISSION_DENIED"),
        "{body}"
    );
}
