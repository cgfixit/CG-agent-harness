//! `/diag` routes and the `auditor` role over real accounts: gates and
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
    // The capability gate narrows diag reads to admin/auditor before the
    // handler's own role check runs (#965).
    assert_eq!(
        (status, code(&body).as_str()),
        (403, "AUTH_PERMISSION_DENIED"),
        "{body}"
    );
    // The retired `audit` role cannot be created at all (#965): no account
    // without log authority can reach the handler.
    assert!(s
        .state
        .auth
        .as_ref()
        .unwrap()
        .create_user("ops-legacy", &cgagentharness::common::random_hex(24), "audit")
        .is_err());
    for (role, name) in [("admin", "ops-admin"), ("auditor", "ops-portal")] {
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
    let portal = account(&s, "ops-portal", "auditor").await;
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
/// Everything the model was sent: a local chat turn carries a staged excerpt
/// with its new message, so "absent" is checked across the whole request.
fn system_prompt(request: &Value) -> String {
    request["messages"].to_string()
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
    // A detached job refuses the same way, before any broker decision or gate claim.
    let decisions = audit_rows(&s, "tool_broker_decision").len();
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/agent/jobs",
        Some(json!({"instruction":"x","branch":"claude/x","commit_message":"m","reason":"r","checks":["cargo-fmt"]})),
    )
    .await;
    assert_eq!(
        (status, code(&body).as_str()),
        (400, "DIAG_SURFACE_FORBIDDEN"),
        "{body}"
    );
    assert_eq!(audit_rows(&s, "tool_broker_decision").len(), decisions);

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
    let request = model.last_request().unwrap();
    assert!(!request["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("DIAG_DATA"));
    let sent = common::turn_message(&request);
    assert!(sent.contains("<<<DIAG_DATA>>>") && sent.contains("You did not read these files"));
    assert!(
        sent.ends_with("\n\nhi"),
        "the operator's text follows the fenced context"
    );
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

/// An excerpt staged by a request that was already past its role check when an
/// admin changed the role (the route's `forget_owner` ran first) still never
/// reaches the account's next turn under a role without log reads: delivery
/// rereads the caller's live grant. Bypassing the route's `forget_owner` with a
/// direct store change models exactly that interleaving.
// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test above.
#[cfg(unix)]
#[tokio::test]
async fn an_excerpt_staged_across_a_demotion_never_reaches_the_new_role() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let auth = s.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user("ops-demoted", &password, "auditor").unwrap();
    let login = || async {
        let response = s
            .client
            .post(s.url("/api/auth/login"))
            .json(&json!({"username":"ops-demoted","password":password}))
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
    };
    let auditor = login().await;
    let (_, session) = json_call(&s, &auditor, Method::POST, "/api/sessions", Some(json!({"title":"d"}))).await;
    let sid = session["session_id"].as_str().unwrap().to_string();
    let (status, read) = json_call(
        &s,
        &auditor,
        Method::GET,
        "/api/diag/read?file=audit.jsonl&last=3",
        None,
    )
    .await;
    assert_eq!(status, 200, "{read}");
    let (status, staged) = json_call(
        &s,
        &auditor,
        Method::POST,
        "/api/diag/inject",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!(status, 200, "{staged}");
    auth.set_role("ops-demoted", "operator").unwrap();
    let operator = login().await;
    let (status, preview) = json_call(
        &s,
        &operator,
        Method::POST,
        "/api/prompt/preview",
        Some(json!({"session_id": sid})),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    assert!(
        !preview["prompt"].as_str().unwrap_or("").contains("DIAG_DATA") && preview["diag"].is_null(),
        "{preview}"
    );
    let (status, body) = json_call(
        &s,
        &operator,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "hi"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(!common::turn_message(&model.last_request().unwrap()).contains("DIAG_DATA"));
    // Dropped, not withheld: the store keeps nothing for the account.
    assert!(s.state.diag.staged(&staged_owner(&s, "ops-demoted")).is_none());
}

/// A password reset or change revokes the account's sign-ins, so the /diag
/// read and excerpt one of them left go too, as on logout, disable, a role
/// change or deletion (INVARIANTS.md: account changes drop it).
// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test above.
#[cfg(unix)]
#[tokio::test]
async fn a_password_reset_or_change_drops_staged_diag_state() {
    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let boss = account(&s, "ops-boss", "admin").await;
    let auth = s.state.auth.as_ref().unwrap();
    for reset in [true, false] {
        let name = if reset { "ops-reset" } else { "ops-change" };
        let password = cgagentharness::common::random_hex(24);
        auth.create_user(name, &password, "admin").unwrap();
        let login = s
            .client
            .post(s.url("/api/auth/login"))
            .json(&json!({"username":name,"password":password}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), 200);
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (_, session) = json_call(
            &s,
            &cookie,
            Method::POST,
            "/api/sessions",
            Some(json!({"title":"diag"})),
        )
        .await;
        let sid = session["session_id"].as_str().unwrap().to_string();
        let (status, read) = json_call(&s, &cookie, Method::GET, "/api/diag/read?file=audit.jsonl&last=3", None).await;
        assert_eq!(status, 200, "{read}");
        let (status, staged) = json_call(
            &s,
            &cookie,
            Method::POST,
            "/api/diag/inject",
            Some(json!({"session_id": sid})),
        )
        .await;
        assert_eq!(status, 200, "{staged}");
        let owner = staged_owner(&s, name);
        assert!(s.state.diag.staged(&owner).is_some() && s.state.diag.last(&owner).is_some());
        let fresh = cgagentharness::common::random_hex(24);
        let (status, body) = if reset {
            json_call(
                &s,
                &boss,
                Method::POST,
                &format!("/api/auth/users/{name}/password"),
                Some(json!({"password": fresh})),
            )
            .await
        } else {
            json_call(
                &s,
                &cookie,
                Method::POST,
                "/api/auth/password",
                Some(json!({"current_password": password, "password": fresh})),
            )
            .await
        };
        assert_eq!(status, 200, "{name}: {body}");
        assert!(
            s.state.diag.staged(&owner).is_none(),
            "{name}: the staged excerpt survived"
        );
        assert!(s.state.diag.last(&owner).is_none(), "{name}: the last read survived");
    }
}

/// The blocking task commits a password reset or change even when the client
/// hangs up and the handler is dropped, so the /diag state and the audit line
/// go with that commit, not with the handler.
// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test above.
#[cfg(unix)]
#[tokio::test]
async fn a_password_reset_or_change_whose_client_hangs_up_still_drops_staged_diag_state() {
    use axum::extract::{Path, State};
    use cgagentharness::server::routes::auth;
    use std::time::Duration;

    let model = start_mock_model().await;
    let s = server(&model, true).await;
    let boss = account(&s, "ops-boss", "admin").await;
    let accounts = s.state.auth.as_ref().unwrap();
    let request = |cookie: &str, body: Value| {
        axum::http::Request::post("/")
            .header("cookie", cookie)
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };
    for reset in [true, false] {
        let name = if reset { "ops-hung-reset" } else { "ops-hung-change" };
        let password = cgagentharness::common::random_hex(24);
        accounts.create_user(name, &password, "admin").unwrap();
        let login = s
            .client
            .post(s.url("/api/auth/login"))
            .json(&json!({"username":name,"password":password}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), 200);
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (_, session) = json_call(
            &s,
            &cookie,
            Method::POST,
            "/api/sessions",
            Some(json!({"title":"diag"})),
        )
        .await;
        let sid = session["session_id"].as_str().unwrap().to_string();
        let (status, _) = json_call(&s, &cookie, Method::GET, "/api/diag/read?file=audit.jsonl&last=3", None).await;
        assert_eq!(status, 200, "{name}: the log read");
        let (status, _) = json_call(
            &s,
            &cookie,
            Method::POST,
            "/api/diag/inject",
            Some(json!({"session_id": sid})),
        )
        .await;
        assert_eq!(status, 200, "{name}: the excerpt was staged");
        let owner = staged_owner(&s, name);
        assert!(s.state.diag.staged(&owner).is_some() && s.state.diag.last(&owner).is_some());

        let fresh = cgagentharness::common::random_hex(24);
        let state = State(s.state.clone());
        let handler = if reset {
            let call = auth::set_password(
                state,
                Path(name.to_string()),
                request(&boss, json!({"password": fresh})),
            );
            tokio::spawn(async move { drop(call.await) })
        } else {
            let body = json!({"current_password": password, "password": fresh});
            let call = auth::change_password(state, request(&cookie, body));
            tokio::spawn(async move { drop(call.await) })
        };
        // The permit is taken in the poll that hands the hash to the blocking
        // pool, so a missing permit means the handler is parked on that task.
        let permits = s.state.auth_operation_permits.clone();
        let idle = permits.available_permits();
        tokio::time::timeout(Duration::from_secs(10), async {
            while permits.available_permits() == idle {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the password operation started");
        handler.abort();
        assert!(
            handler.await.unwrap_err().is_cancelled(),
            "{name}: the handler was dropped"
        );
        // The blocking task cannot be aborted: it returns the permit when done.
        tokio::time::timeout(Duration::from_secs(30), async {
            while permits.available_permits() != idle {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the password operation finished");

        assert!(
            accounts.login(name, &fresh).is_ok(),
            "{name}: the new password was committed"
        );
        assert!(
            s.state.diag.staged(&owner).is_none(),
            "{name}: the staged excerpt survived"
        );
        assert!(s.state.diag.last(&owner).is_none(), "{name}: the last read survived");
        s.state.audit.flush();
        let event = if reset {
            "account.password_reset"
        } else {
            "account.password_changed"
        };
        let logged = audit_rows(&s, event);
        assert_eq!(
            logged.iter().filter(|row| row["target"] == name).count(),
            1,
            "{name}: {event}"
        );
    }
}

#[cfg(unix)]
fn staged_owner(s: &TestServer, username: &str) -> String {
    s.state.auth.as_ref().unwrap().get_user(username).unwrap().user_id
}

#[tokio::test]
async fn auditor_chats_locally_without_memory_or_other_surfaces() {
    let model = start_mock_model().await;
    // Live state on, so its absence below is the role's doing, not the fixture's.
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", "true")
            .with("diagnostics.live_state", "true"),
    )
    .await;
    let portal = account(&s, "ops-portal", "auditor").await;
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
    assert_eq!(body["memory_suggestion"]["reason"], "local_only");
    let request = model.last_request().unwrap();
    assert!(!request["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .contains("SHARED-PERSONA-MARKER"));
    assert!(!request["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .contains("LIVE_STATE"));
    assert!(request
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
    let (status, _) = json_call(&s, &portal, Method::GET, &format!("/api/sessions/{sid}"), None).await;
    assert_eq!(status, 200);
    // Refused before a session is created: a session-less refused turn leaves
    // the owner's session list as it was.
    let (_, before) = json_call(&s, &portal, Method::GET, "/api/sessions", None).await;
    for body in [
        json!({"message": "x", "retrieve": true}),
        json!({"message": "x", "model": "other-local-model"}),
        json!({"message": "x", "attachment_ids": ["0123456789abcdef0123456789abcdef"]}), // DevSkim: ignore DS173237 because this is a 32-hex attachment-id fixture, not a credential.
    ] {
        let (status, reply) = json_call(&s, &portal, Method::POST, "/api/chat", Some(body.clone())).await;
        assert_eq!(
            (status, code(&reply).as_str()),
            (403, "AUTH_PERMISSION_DENIED"),
            "{body}: {reply}"
        );
    }
    let (_, after) = json_call(&s, &portal, Method::GET, "/api/sessions", None).await;
    assert_eq!(after, before, "a refused turn created a session");
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
        // Routed owner-data mutations: an unrouted method would be refused by
        // the auditor's method-exact rule whatever the role's grants were.
        (Method::POST, format!("/api/sessions/{sid}/rename")),
        (Method::DELETE, format!("/api/sessions/{sid}/attachment-pins")),
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
    assert_eq!(body["live_state"], false, "{body}");
    assert!(!body["prompt"].as_str().unwrap_or("").contains("LIVE_STATE"), "{body}");
    // Control: an admin's local turn on the same server carries the block, so the
    // absence checks above can fail.
    let admin = account(&s, "ops-admin", "admin").await;
    let (status, asession) = json_call(&s, &admin, Method::POST, "/api/sessions", Some(json!({"title":"a"}))).await;
    assert_eq!(status, 201, "{asession}");
    let (status, body) = json_call(
        &s,
        &admin,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": asession["session_id"], "message": "hello"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        model.last_request().unwrap()["messages"][0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("<<<LIVE_STATE_DATA>>>"),
        "an admin local turn carries the live state"
    );
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

// Windows builds refuse every read (`DIAG_UNSUPPORTED`); see the windows test above.
#[cfg(unix)]
#[tokio::test]
async fn auditor_turn_reads_logs_through_bounded_tools() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", "true")
            .with("diagnostics.model_tool", "true"),
    )
    .await;
    let portal = account(&s, "ops-portal", "auditor").await;
    let (_, session) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/sessions",
        Some(json!({"title":"diag-tools"})),
    )
    .await;
    let sid = session["session_id"].as_str().unwrap().to_string();
    model.set_reply(json!([
        json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call_list_1","type":"function","function":{"name":"logs_list","arguments":"{}"}}]}}],
            "usage":{"prompt_tokens":10,"completion_tokens":2}}),
        ok_reply("The audit log is available.", 20, 4),
    ]));
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "what logs can you see?"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"].as_str().unwrap_or(""), "The audit log is available.");

    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    let offered: Vec<&str> = requests[0]["tools"]
        .as_array()
        .expect("the first request offers the log tools")
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(offered, ["logs_list", "logs_read", "logs_search", "logs_stats"]);
    // The follow-up resends the batch and one tool message carrying the
    // untrusted header, never silent authority over log text.
    let second = requests[1]["messages"].as_array().unwrap();
    let tool_message = second
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the follow-up carries the tool result");
    assert_eq!(tool_message["tool_call_id"], "call_list_1");
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("UNTRUSTED LOG EXCERPT"), "{content}");
    assert!(content.contains("audit.jsonl"), "{content}");

    // The audit row holds the filter shape and the row count, never log text.
    let calls = audit_rows(&s, "diag_tool_call");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let row = &calls[0];
    assert_eq!(row["tool"], "logs_list");
    assert_eq!(row["outcome"], "ok");
    assert!(row["rows"].as_u64().unwrap() >= 1, "{row}");
    for content_key in ["text", "content", "lines", "sha256"] {
        assert!(
            row.get(content_key).is_none(),
            "audit must not carry {content_key}: {row}"
        );
    }

    // A refused read ends as that call's tool result; the turn answers anyway.
    model.set_reply(json!([
        json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call_bad_1","type":"function","function":{"name":"logs_read",
                "arguments":"{\"file\":\"../config.yaml\"}"}}]}}],
            "usage":{"prompt_tokens":10,"completion_tokens":2}}),
        ok_reply("I could not read that file.", 20, 4),
    ]));
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "read ../config.yaml"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let requests = model.requests.lock().unwrap().clone();
    let tool_message = requests.last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .cloned()
        .expect("the refused read still produces a tool result");
    assert!(tool_message["content"].as_str().unwrap().contains("DIAG_BAD_FILE"));
    let calls = audit_rows(&s, "diag_tool_call");
    assert_eq!(calls.last().unwrap()["outcome"], "DIAG_BAD_FILE");
    assert!(
        calls.last().unwrap()["file"].is_null(),
        "a refused file name is not audited"
    );
    assert!(
        !s.audit_log().contains("../config.yaml"),
        "audit must not record the file argument"
    );

    // An allowlisted file is audited by name; a rejected group_by is model
    // text and never reaches the audit log (#965: filter shape, not content).
    model.set_reply(json!([
        json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[
                {"id":"call_stats_1","type":"function","function":{"name":"logs_stats",
                    "arguments":"{\"file\":\"audit.jsonl\",\"group_by\":\"UNCHECKED-GROUP-TEXT\"}"}},
                {"id":"call_stats_2","type":"function","function":{"name":"logs_stats",
                    "arguments":"{\"file\":\"audit.jsonl\",\"group_by\":\"event\"}"}}]}}],
            "usage":{"prompt_tokens":10,"completion_tokens":2}}),
        ok_reply("Counted.", 20, 4),
    ]));
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "count events"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let calls = audit_rows(&s, "diag_tool_call");
    let stats = &calls[calls.len() - 2..];
    assert_eq!(
        stats[0]["outcome"], "DIAG_TOOL_ARGUMENTS",
        "an invalid group_by is refused"
    );
    assert!(
        stats[0]["filter"]["group_by"].is_null(),
        "a rejected group_by is not audited"
    );
    assert_eq!(stats[1]["filter"]["group_by"], "event", "a fixed group_by is audited");
    for row in stats {
        assert_eq!(row["file"], "audit.jsonl", "the allowlisted file is audited");
    }
    assert!(
        !s.audit_log().contains("UNCHECKED-GROUP-TEXT"),
        "audit must not record a rejected group_by"
    );

    // A limit that is not a positive whole number refuses only its own call:
    // the numeric string "3" reads, the junk one is that call's
    // DIAG_TOOL_ARGUMENTS result, and the turn still answers (#1035 review).
    model.set_reply(json!([
        json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[
                {"id":"call_limit_1","type":"function","function":{"name":"logs_read",
                    "arguments":"{\"file\":\"audit.jsonl\",\"limit\":\"UNCHECKED-LIMIT-TEXT\"}"}},
                {"id":"call_limit_2","type":"function","function":{"name":"logs_search",
                    "arguments":"{\"file\":\"audit.jsonl\",\"limit\":\"3\"}"}}]}}],
            "usage":{"prompt_tokens":10,"completion_tokens":2}}),
        ok_reply("Read what I could.", 20, 4),
    ]));
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"session_id": sid, "message": "read a few rows"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"].as_str().unwrap_or(""), "Read what I could.");
    let calls = audit_rows(&s, "diag_tool_call");
    let limited = &calls[calls.len() - 2..];
    assert_eq!(limited[0]["outcome"], "DIAG_TOOL_ARGUMENTS", "a junk limit is refused");
    assert!(
        limited[0]["filter"]["limit"].is_null(),
        "a refused limit is not audited"
    );
    assert_eq!(limited[1]["filter"]["limit"], 3, "a numeric-string limit is parsed");
    assert_ne!(
        limited[1]["outcome"], "DIAG_TOOL_ARGUMENTS",
        "a numeric-string limit is accepted"
    );
    let refused = model.requests.lock().unwrap().last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == "call_limit_1")
        .cloned()
        .expect("the refused call still gets a tool result");
    assert!(refused["content"].as_str().unwrap().contains("DIAG_TOOL_ARGUMENTS"));
    assert!(
        !s.audit_log().contains("UNCHECKED-LIMIT-TEXT"),
        "audit must not record a refused limit"
    );
}

#[tokio::test]
async fn accounts_off_never_offers_log_tools() {
    // `/diag` refuses without accounts (DIAG_REFUSED), so its chat tools must
    // not open a log read path there either.
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("diagnostics.logs_enabled", "true")
            .with("diagnostics.model_tool", "true"),
    )
    .await;
    s.state.settings.lock().unwrap().web_enabled = false;
    let (status, body) = json_call(&s, "", Method::GET, "/api/diag", None).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = json_call(&s, "", Method::POST, "/api/chat", Some(json!({"message": "hello"}))).await;
    assert_eq!(status, 200, "{body}");
    assert!(model
        .last_request()
        .unwrap()
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
    assert!(audit_rows(&s, "diag_tool_call").is_empty());
}

#[tokio::test]
async fn an_operator_turn_a_disabled_flag_and_a_loop_turn_never_offer_log_tools() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", "true")
            .with("diagnostics.model_tool", "true"),
    )
    .await;
    // Web tools would mask the check: with web on an operator turn is a web
    // turn. With it off the operator's turn must be plain, never a diag turn.
    s.state.settings.lock().unwrap().web_enabled = false;
    // An operator has no InvestigateLogs grant (#965): plain turn, no tools.
    let operator = account(&s, "ops-operator", "operator").await;
    let (status, body) = json_call(
        &s,
        &operator,
        Method::POST,
        "/api/chat",
        Some(json!({"message": "hello"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(model
        .last_request()
        .unwrap()
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
    // Even the auditor's /loop request is refused before any tools exist; an
    // admin loop turn (InvestigateLogs held) never offers them either.
    let admin = account(&s, "ops-admin", "admin").await;
    let (_, session) = json_call(&s, &admin, Method::POST, "/api/sessions", Some(json!({"title":"loop"}))).await;
    let sid = session["session_id"].as_str().unwrap().to_string();
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
    assert!(model
        .last_request()
        .unwrap()
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));

    // The flag is the operator's opt-in: with it unset the same auditor turn
    // is a plain chat turn.
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", "true"),
    )
    .await;
    let portal = account(&s, "ops-portal", "auditor").await;
    let (status, body) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/chat",
        Some(json!({"message": "hello"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(model
        .last_request()
        .unwrap()
        .get("tools")
        .is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
}

/// Diagnostic turns run under `web.total_tokens`, so compaction must plan for
/// that budget as it does for web chat (#1036 review). Before, a session that
/// outgrew the web budget but not the plain-chat trigger reached the tool loop
/// uncompacted and failed with `DIAG_TOKEN_BUDGET`; now it is compacted and
/// keeps its log tools.
#[tokio::test]
async fn a_long_diag_session_is_compacted_to_the_web_budget_and_keeps_its_tools() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    let last_prompt = Arc::new(AtomicU64::new(0));
    let offered = Arc::new(AtomicBool::new(false));
    let (prompt_seen, tools_seen) = (last_prompt.clone(), offered.clone());
    let router = common::ollama_router().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let (prompt_seen, tools_seen) = (prompt_seen.clone(), tools_seen.clone());
            async move {
                let prompt = (body["messages"].to_string().len()
                    + body.get("tools").map_or(0, |tools| tools.to_string().len())) as u64
                    / 4;
                let last = body["messages"].as_array().unwrap().last().unwrap().clone();
                if last["content"].as_str().is_some_and(|c| c.starts_with("seed ")) {
                    prompt_seen.store(prompt, Ordering::SeqCst);
                    tools_seen.store(
                        body.get("tools")
                            .is_some_and(|t| t.as_array().is_some_and(|t| !t.is_empty())),
                        Ordering::SeqCst,
                    );
                }
                // Every call answers plainly; a compaction summary is "Noted." too.
                axum::Json(ok_reply("Noted.", prompt, 20))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because the mock model listens only on loopback.
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("diagnostics.logs_enabled", "true")
            .with("diagnostics.model_tool", "true")
            .with("models.local_llm.max_tokens", "2048")
            .with("web.total_tokens", "9000"),
    )
    .await;
    let portal = account(&s, "ops-portal", "auditor").await;
    let (_, session) = json_call(
        &s,
        &portal,
        Method::POST,
        "/api/sessions",
        Some(json!({"title":"long"})),
    )
    .await;
    let sid = session["session_id"].as_str().unwrap().to_string();
    let (mut previous, mut peak, mut compactions) = (0u64, 0u64, 0usize);
    for turn in 0..30 {
        let (status, body) = json_call(
            &s,
            &portal,
            Method::POST,
            "/api/chat",
            Some(json!({"session_id": sid, "message": format!("seed {turn} {}", "x".repeat(2000))})),
        )
        .await;
        assert_eq!(status, 200, "turn {turn} fails instead of compacting: {body}");
        assert!(offered.load(Ordering::SeqCst), "turn {turn} keeps its log tools");
        // Each seed adds about 500 fixture tokens; a smaller prompt than the
        // last one means the history was compacted.
        let now = last_prompt.load(Ordering::SeqCst);
        compactions += usize::from(now < previous);
        (previous, peak) = (now, peak.max(now));
    }
    assert!(
        compactions >= 2,
        "the session is compacted again as it regrows: {compactions}"
    );
    assert!(peak <= 9000, "no diagnostic prompt outgrew the web budget: {peak}");
    mock.abort();
}
