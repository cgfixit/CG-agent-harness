//! Phase 8 scheduled agentic runs: persist, fail-closed unreviewed, at-most-once.

mod common;

use common::*;
use serde_json::json;

async fn staged_request(s: &TestServer) -> serde_json::Value {
    let (_, created) = s.post_json("/api/sessions", json!({})).await;
    let id = created["session_id"].as_str().unwrap().to_string();
    s.post_json(
        &format!("/api/sessions/{id}/goal"),
        json!({"goal":"Review the arithmetic implementation"}),
    )
    .await;
    let (status, stage) = s
        .post_json(
            &format!("/api/sessions/{id}/goal-stage"),
            json!({"branch":"codex/goal-fixture"}),
        )
        .await;
    assert_eq!(status, 200, "{stage}");
    let mut req = stage["request"].clone();
    req["reason"] = json!("Scheduled fixture");
    req["confirm"] = json!(true);
    req
}

async fn preview_body(s: &TestServer, req: serde_json::Value) -> serde_json::Value {
    let mut body = json!({"interval_secs":60,"request":req});
    let (status, preview) = s.post_json("/api/agent/schedules/preview", body.clone()).await;
    assert_eq!(status, 200, "{preview}");
    assert_eq!(preview["occurrences"].as_array().unwrap().len(), 5);
    body["preview_id"] = preview["preview_id"].clone();
    body
}

#[tokio::test]
async fn unreviewed_and_unbound_schedules_fail_closed() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = s
        .post_json(
            "/api/agent/schedules",
            json!({
                "interval_secs": 60,
                "request": {
                    "instruction": "no goal",
                    "branch": "codex/goal-fixture",
                    "commit_message": "no goal",
                    "reason": "x",
                    "confirm": true
                }
            }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    let mut req = staged_request(&s).await;
    req["confirm"] = json!(false);
    let (status, body) = s
        .post_json("/api/agent/schedules", json!({"interval_secs": 60, "request": req}))
        .await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn one_minute_schedule_fires_once_across_restart_and_audits() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let req = staged_request(&s).await;
    let (status, created) = s.post_json("/api/agent/schedules", preview_body(&s, req).await).await;
    assert_eq!(status, 201, "{created}");
    let schedule_id = created["schedule_id"].as_str().unwrap().to_string();
    let t0 = created["created_at"].as_f64().unwrap();
    cgagentharness::server::routes::agent::tick_schedules(&s.state, t0 + 30.0).await;
    assert!(s.state.jobs.list("local").is_empty(), "mid-window must not fire");
    let recovered = cgagentharness::server::agent_schedules::ScheduleStore::open(
        &s.home.join("data/agentic/console-schedules.json"),
    )
    .unwrap();
    assert!(recovered.due(t0 + 30.0).is_empty());
    cgagentharness::server::routes::agent::tick_schedules(&s.state, t0 + 60.0).await;
    assert_eq!(s.state.jobs.list("local").len(), 1, "first due occurrence fires");
    cgagentharness::server::routes::agent::tick_schedules(&s.state, t0 + 60.0).await;
    assert_eq!(
        s.state.jobs.list("local").len(),
        1,
        "same occurrence must not double-fire"
    );
    let job_id = s.state.jobs.list("local")[0]["job_id"].as_str().unwrap().to_string();
    let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let (_, j) = s.get_json(&format!("/api/agent/jobs/{job_id}")).await;
            if j["status"] != "running" {
                break j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job["status"], "failed");
    s.state.jobs.finish(&job_id, Ok(json!({"ok": true})));
    assert_eq!(
        s.get_json(&format!("/api/agent/jobs/{job_id}")).await.1["status"],
        "failed",
        "JobStore::finish stays a no-op after the job is no longer running"
    );
    let (status, cancelled) = s
        .post_json(&format!("/api/agent/schedules/{schedule_id}/cancel"), json!({}))
        .await;
    assert_eq!(status, 200, "{cancelled}");
    cgagentharness::server::routes::agent::tick_schedules(&s.state, t0 + 120.0).await;
    assert_eq!(s.state.jobs.list("local").len(), 1, "cancelled schedule must not fire");
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    assert!(audit.contains("agent_schedule_created"), "{audit}");
    assert!(audit.contains("agent_schedule_start"), "{audit}");
    assert!(
        audit.contains("agent_schedule_failed") || audit.contains("agent_schedule_completed"),
        "{audit}"
    );
    assert!(audit.contains("agent_schedule_cancelled"), "{audit}");
}

#[tokio::test]
async fn schedule_create_list_cancel_round_trip() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let req = staged_request(&s).await;
    let (status, created) = s.post_json("/api/agent/schedules", preview_body(&s, req).await).await;
    assert_eq!(status, 201, "{created}");
    let id = created["schedule_id"].as_str().unwrap();
    let (status, listed) = s.get_json("/api/agent/schedules").await;
    assert_eq!(status, 200);
    assert_eq!(listed["schedules"].as_array().unwrap().len(), 1);
    let (status, one) = s.get_json(&format!("/api/agent/schedules/{id}")).await;
    assert_eq!(status, 200);
    assert_eq!(one["status"], "active");
    let (status, _) = s
        .post_json(&format!("/api/agent/schedules/{id}/cancel"), json!({}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        s.get_json(&format!("/api/agent/schedules/{id}")).await.1["status"],
        "cancelled"
    );
}

#[tokio::test]
async fn cron_activation_requires_exact_fresh_single_use_preview_and_retains_fire_gates() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let req = staged_request(&s).await;
    let mut body = json!({"schema_version":1,"schedule":{"kind":"cron","expression":"*/5 * * * *","timezone":"UTC"},"request":req});
    assert_eq!(s.post_json("/api/agent/schedules", body.clone()).await.0, 400);
    let (status, preview) = s.post_json("/api/agent/schedules/preview", body.clone()).await;
    assert_eq!(status, 200, "{preview}");
    assert!(s.state.schedules.list("local").is_empty());
    body["preview_id"] = preview["preview_id"].clone();
    let mut changed = body.clone();
    changed["schedule"]["timezone"] = json!("America/New_York");
    assert_eq!(s.post_json("/api/agent/schedules", changed).await.0, 400);
    let (status, created) = s.post_json("/api/agent/schedules", body.clone()).await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["next_fire_at"], preview["occurrences"][0]["at"]);
    assert_eq!(created["next_occurrence_id"], preview["occurrences"][0]["identity"]);
    assert_eq!(s.post_json("/api/agent/schedules", body).await.0, 400);
    let next = created["next_fire_at"].as_f64().unwrap();
    // Existing process-wide gate prohibits overlap; occurrence is consumed.
    let held = s.state.agent_run_gate.claim("overlap-fixture").unwrap();
    cgagentharness::server::routes::agent::tick_schedules(&s.state, next).await;
    assert!(s.state.jobs.list("local").is_empty());
    drop(held);
    cgagentharness::server::routes::agent::tick_schedules(&s.state, next).await;
    assert!(
        s.state.jobs.list("local").is_empty(),
        "busy occurrences are skipped, never retried"
    );
    let id = created["schedule_id"].as_str().unwrap();
    let row = s.state.schedules.get("local", id).unwrap();
    assert!(row.next_fire_at > next);
    assert_ne!(row.last_dispatch.as_deref(), Some("attempted"));
}
