//! Two real guarded accounts: storage, every session surface, jobs and schedules.
mod common;
use common::*;
use reqwest::{Method, Response};
use serde_json::{json, Value};
use std::time::Duration;

async fn account(server: &TestServer, name: &str, role: &str) -> (String, String) {
    let auth = server.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user(name, &password, role).unwrap();
    let owner = auth.get_user(name).unwrap().user_id;
    let response = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username":name,"password":password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (owner, cookie)
}
async fn request(s: &TestServer, cookie: &str, method: Method, path: &str, body: Value) -> Response {
    s.req(method, path)
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn json_request(s: &TestServer, cookie: &str, method: Method, path: &str, body: Value) -> Value {
    let response = request(s, cookie, method, path, body).await;
    assert!(response.status().is_success(), "{}", response.text().await.unwrap());
    response.json().await.unwrap()
}

#[tokio::test]
async fn accounts_cannot_read_mutate_search_export_or_schedule_foreign_sessions() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("auth.enabled", "true")
            .with("structured_memory.enabled", "true"),
    )
    .await;
    let (alice_id, alice) = account(&s, "alice", "admin").await;
    let (bob_id, bob) = account(&s, "bob", "operator").await;
    let session = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/sessions",
        json!({"title":"alice-private"}),
    )
    .await;
    let id = session["session_id"].as_str().unwrap();
    json_request(
        &s,
        &alice,
        Method::POST,
        "/api/chat",
        json!({"session_id":id,"message":"private-needle-102"}),
    )
    .await;
    let path = format!("/api/sessions/{id}");
    for suffix in ["", "/export", "/goal-stage"] {
        assert_eq!(
            request(&s, &bob, Method::GET, &format!("{path}{suffix}"), json!({}))
                .await
                .status(),
            404
        );
    }
    for (url, body) in [
        (format!("{path}/rename"), json!({"title":"stolen"})),
        (format!("{path}/goal"), json!({"goal":"stolen"})),
        (format!("{path}/goal-stage"), json!({"branch":"codex/stolen"})),
        ("/api/chat".into(), json!({"session_id":id,"message":"steal"})),
        ("/api/style".into(), json!({"session_id":id,"name":"off"})),
        (format!("{path}/skills"), json!({"ids":[]})),
        (format!("{path}/structured-facts"), json!({"facts":[]})),
        ("/api/prompt/preview".into(), json!({"session_id":id})),
    ] {
        let response = request(&s, &bob, Method::POST, &url, body).await;
        assert_eq!(response.status(), 404, "{url}: {}", response.text().await.unwrap());
    }
    json_request(
        &s,
        &alice,
        Method::POST,
        &format!("{path}/structured-facts"),
        json!({"facts":[]}),
    )
    .await;
    json_request(
        &s,
        &alice,
        Method::POST,
        "/api/prompt/preview",
        json!({"session_id":id}),
    )
    .await;
    let list = json_request(&s, &bob, Method::GET, "/api/sessions", json!({})).await;
    assert_eq!(list["sessions"], json!([]));
    let hits = json_request(
        &s,
        &bob,
        Method::POST,
        "/api/sessions/search",
        json!({"query":"private-needle-102"}),
    )
    .await;
    assert_eq!(hits["hits"], json!([]));
    let mine = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/sessions/search",
        json!({"query":"private-needle-102"}),
    )
    .await;
    assert!(!mine["hits"].as_array().unwrap().is_empty());
    assert!(!s.home.join("exports").join(format!("{id}.md")).exists());
    let export = request(&s, &alice, Method::GET, &format!("{path}/export"), json!({})).await;
    assert_eq!(export.status(), 200);
    assert!(export.text().await.unwrap().contains("private-needle-102"));
    assert!(s.state.store.for_owner(&bob_id).get(id).is_err());
    assert!(s.state.store.for_owner(&bob_id).list().is_empty());

    json_request(
        &s,
        &alice,
        Method::POST,
        &format!("{path}/goal"),
        json!({"goal":"Review arithmetic"}),
    )
    .await;
    let stage = json_request(
        &s,
        &alice,
        Method::POST,
        &format!("{path}/goal-stage"),
        json!({"branch":"codex/owner-fixture"}),
    )
    .await;
    let mut run = stage["request"].clone();
    run["confirm"] = json!(true);
    run["reason"] = json!("fixture review");
    assert_eq!(
        request(
            &s,
            &bob,
            Method::POST,
            "/api/agent/schedules",
            json!({"interval_secs":60,"request":run})
        )
        .await
        .status(),
        409
    );
    let preview = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/agent/schedules/preview",
        json!({"interval_secs":60,"request":run}),
    )
    .await;
    let schedule = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/agent/schedules",
        json!({"interval_secs":60,"request":run,"preview_id":preview["preview_id"]}),
    )
    .await;
    let sid = schedule["schedule_id"].as_str().unwrap();
    for (method, suffix) in [(Method::GET, ""), (Method::POST, "/cancel")] {
        assert_eq!(
            request(
                &s,
                &bob,
                method,
                &format!("/api/agent/schedules/{sid}{suffix}"),
                json!({})
            )
            .await
            .status(),
            404
        );
    }
    assert_eq!(
        json_request(&s, &bob, Method::GET, "/api/agent/schedules", json!({})).await["schedules"],
        json!([])
    );
    assert!(s.state.schedules.get(&bob_id, sid).is_none());
    assert!(s.state.schedules.cancel(&bob_id, sid).unwrap().is_none());

    let jid = "c".repeat(32);
    s.state
        .jobs
        .insert_running(&alice_id, &jid, "fixture", tokio::spawn(std::future::pending()));
    for (method, suffix) in [(Method::GET, ""), (Method::POST, "/cancel")] {
        assert_eq!(
            request(&s, &bob, method, &format!("/api/agent/jobs/{jid}{suffix}"), json!({}))
                .await
                .status(),
            404
        );
    }
    let jobs = json_request(&s, &bob, Method::GET, "/api/agent/jobs", json!({})).await;
    assert_eq!(jobs["jobs"], json!([]));
    assert_eq!(jobs["running"], 0);
    assert!(s.state.jobs.get(&bob_id, &jid).is_none());
    assert!(s.state.jobs.cancel(&bob_id, &jid).is_none());
    s.state.jobs.cancel(&alice_id, &jid).unwrap();
    // Revocation after activation cancels the row. It does not start a job.
    s.state.auth.as_ref().unwrap().disable_user("alice").unwrap();
    let fire_at = schedule["next_fire_at"].as_f64().unwrap();
    cgagentharness::server::routes::agent::tick_schedules(&s.state, fire_at).await;
    cgagentharness::server::routes::agent::tick_schedules(&s.state, fire_at).await;
    assert_eq!(
        s.state.jobs.list(&alice_id).len(),
        1,
        "the earlier cancelled job stays; no schedule run starts"
    );
    assert!(s.state.schedules.due(fire_at).is_empty());
    let row = s.state.schedules.get(&alice_id, sid).unwrap();
    assert_eq!(row.status, "cancelled");
    assert_eq!(row.last_dispatch.as_deref(), Some("skipped_revoked"));
    assert!(std::fs::read_to_string(s.home.join("logs/audit.jsonl"))
        .unwrap()
        .contains("SCHEDULE_OWNER_REVOKED"));
}

#[tokio::test]
async fn legacy_sessions_require_explicit_admin_adoption_and_clear_only_affects_owner() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let (alice_id, alice) = account(&s, "alice", "admin").await;
    let (bob_id, bob) = account(&s, "bob", "operator").await;
    let old = s
        .state
        .store
        .for_owner("local")
        .create("fixture", "legacy retained")
        .unwrap();
    let mut legacy = serde_json::to_value(old).unwrap();
    legacy.as_object_mut().unwrap().remove("owner");
    legacy.as_object_mut().unwrap().remove("schema_version");
    legacy["goal_stage"] = json!({"approved":true,"job_id":"old-shared-approval"});
    let id = legacy["session_id"].as_str().unwrap();
    let file = s.home.join("sessions").join(format!("{id}.json"));
    std::fs::write(&file, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let own = s
        .state
        .store
        .for_owner(&bob_id)
        .create("fixture", "bob retained")
        .unwrap();
    s.state
        .store
        .for_owner(&alice_id)
        .create("fixture", "alice clear")
        .unwrap();
    let cleared = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/sessions/clear",
        json!({"confirm":true,"reason":"own only"}),
    )
    .await;
    assert_eq!(cleared["deleted_sessions"], 1);
    assert!(file.exists());
    assert!(s.state.store.for_owner(&bob_id).get(&own.session_id).is_ok());
    assert!(s.state.store.for_owner(&alice_id).get(id).is_err());
    assert!(s.state.store.for_owner("local").get(id).is_err());
    assert_eq!(
        request(&s, &bob, Method::GET, "/api/sessions/legacy", json!({}))
            .await
            .status(),
        403
    );
    let listed = json_request(&s, &alice, Method::GET, "/api/sessions/legacy", json!({})).await;
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert!(listed["sessions"][0].get("messages").is_none());
    let url = format!("/api/sessions/{id}/adopt");
    for (cookie, body, code) in [
        (&bob, json!({"confirm":true,"reason":"steal"}), 403),
        (&alice, json!({"confirm":false,"reason":"no"}), 422),
        (&alice, json!({"confirm":true,"reason":"adopt","owner":bob_id}), 422),
    ] {
        assert_eq!(
            request(&s, cookie, Method::POST, &url, body).await.status().as_u16(),
            code
        );
    }
    json_request(
        &s,
        &alice,
        Method::POST,
        &url,
        json!({"confirm":true,"reason":"authorized legacy migration"}),
    )
    .await;
    let adopted = s.state.store.for_owner(&alice_id).get(id).unwrap();
    assert!(adopted.goal_stage.is_none());
    assert_eq!(adopted.schema_version, 1);
    assert!(s.state.store.for_owner(&bob_id).get(id).is_err());
    assert_eq!(
        request(&s, &alice, Method::POST, &url, json!({"confirm":true,"reason":"again"}))
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn foreign_cancel_and_clear_cannot_stop_an_active_generation() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let (_, alice) = account(&s, "alice", "operator").await;
    let (_, bob) = account(&s, "bob", "operator").await;
    model.set_delay_ms(5000);
    let chat = json_request(
        &s,
        &alice,
        Method::POST,
        "/api/chat",
        json!({"message":"owner isolated slow turn"}),
    );
    let controls = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while model.requests.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            request(&s, &bob, Method::POST, "/api/chat/cancel", json!({}))
                .await
                .status(),
            403
        );
        json_request(
            &s,
            &bob,
            Method::POST,
            "/api/sessions/clear",
            json!({"confirm":true,"reason":"own only"}),
        )
        .await;
        assert!(s.state.generation_gate.is_held());
    };
    let (reply, ()) = tokio::join!(chat, controls);
    assert!(reply["session_id"].is_string());
}

async fn owned_schedule(s: &TestServer, cookie: &str) -> serde_json::Value {
    let response = request(s, cookie, Method::POST, "/api/sessions", json!({})).await;
    assert!(response.status().is_success());
    let created: Value = response.json().await.unwrap();
    let id = created["session_id"].as_str().unwrap();
    json_request(
        s,
        cookie,
        Method::POST,
        &format!("/api/sessions/{id}/goal"),
        json!({"goal":"Review arithmetic"}),
    )
    .await;
    let stage = json_request(
        s,
        cookie,
        Method::POST,
        &format!("/api/sessions/{id}/goal-stage"),
        json!({"branch":"codex/owner-fixture"}),
    )
    .await;
    let mut run = stage["request"].clone();
    run["confirm"] = json!(true);
    run["reason"] = json!("fixture review");
    let preview = json_request(
        s,
        cookie,
        Method::POST,
        "/api/agent/schedules/preview",
        json!({"interval_secs":60,"request":run}),
    )
    .await;
    json_request(
        s,
        cookie,
        Method::POST,
        "/api/agent/schedules",
        json!({"interval_secs":60,"request":run,"preview_id":preview["preview_id"]}),
    )
    .await
}

#[tokio::test]
async fn password_change_between_activate_and_fire_cancels_the_schedule() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let (alice_id, alice) = account(&s, "alice", "admin").await;
    let schedule = owned_schedule(&s, &alice).await;
    let sid = schedule["schedule_id"].as_str().unwrap();
    s.state.auth.as_ref().unwrap().require_password_change("alice").unwrap();
    let fire_at = schedule["next_fire_at"].as_f64().unwrap();
    cgagentharness::server::routes::agent::tick_schedules(&s.state, fire_at).await;
    cgagentharness::server::routes::agent::tick_schedules(&s.state, fire_at).await;
    assert!(s.state.jobs.list(&alice_id).is_empty());
    assert!(s.state.schedules.due(fire_at).is_empty());
    let row = s.state.schedules.get(&alice_id, sid).unwrap();
    assert_eq!(row.status, "cancelled");
    assert_eq!(row.last_dispatch.as_deref(), Some("skipped_revoked"));
}

#[tokio::test]
async fn disabling_one_account_cancels_only_that_owners_schedules() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default().with("auth.enabled", "true")).await;
    let (alice_id, alice) = account(&s, "alice", "admin").await;
    let (bob_id, bob) = account(&s, "bob", "operator").await;
    let alice_schedule = owned_schedule(&s, &alice).await;
    let bob_schedule = owned_schedule(&s, &bob).await;
    json_request(
        &s,
        &alice,
        Method::POST,
        "/api/auth/users/bob/disabled",
        json!({"disabled": true}),
    )
    .await;
    assert_eq!(
        s.state
            .schedules
            .get(&bob_id, bob_schedule["schedule_id"].as_str().unwrap())
            .unwrap()
            .status,
        "cancelled"
    );
    assert_eq!(
        s.state
            .schedules
            .get(&alice_id, alice_schedule["schedule_id"].as_str().unwrap())
            .unwrap()
            .status,
        "active"
    );
}
