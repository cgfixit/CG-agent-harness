mod common;
use common::*;
use serde_json::{json, Value};

async fn stage(s: &TestServer) -> (String, Value) {
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
    (id, stage)
}
#[tokio::test]
async fn goal_staging_is_durable_and_never_executes_or_confirms() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (id, stage) = stage(&s).await;
    assert_eq!(stage["executed"], false);
    assert!(stage["request"]["confirm"].is_null());
    assert_eq!(stage["request"]["max_iterations"], 1);
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(s.state.jobs.list().is_empty());
    let reopened = cgagentharness::server::sessions::SessionStore::new(&s.home.join("sessions")).unwrap();
    assert_eq!(
        reopened.get(&id).unwrap().goal_stage.unwrap()["stage_id"],
        stage["stage"]["stage_id"]
    );
    assert_eq!(
        s.get_json(&format!("/api/sessions/{id}/goal-stage")).await.1["status"],
        "staged"
    );
    let mut req = stage["request"].clone();
    req["reason"] = json!("Review goal request");
    assert_eq!(s.post_json("/api/agent/jobs", req.clone()).await.0, 400);
    assert!(s.state.jobs.list().is_empty());
    req["confirm"] = json!(true);
    assert_eq!(
        s.post_json("/api/agent/run", req.clone()).await.0,
        400,
        "goal uses durable job route only"
    );
    s.post_json(&format!("/api/sessions/{id}/goal"), json!({"goal":"Changed goal"}))
        .await;
    assert_eq!(s.post_json("/api/agent/jobs", req).await.0, 409);
    assert!(s.state.jobs.list().is_empty());
    assert_eq!(
        s.get_json(&format!("/api/sessions/{id}/goal-stage")).await.1["status"],
        "stale"
    );
}
#[tokio::test]
async fn goal_job_preserves_policy_refusal_and_cannot_replay_a_claimed_stage() {
    let model = start_mock_model().await;
    let s = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (id, stage) = stage(&s).await;
    let mut req = stage["request"].clone();
    req["reason"] = json!("Verify closed gates");
    req["confirm"] = json!(true);
    let (status, ack) = s.post_json("/api/agent/jobs", req.clone()).await;
    assert_eq!(status, 202, "{ack}");
    let job_id = ack["job_id"].as_str().unwrap();
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
    assert_eq!(
        job["status"], "failed",
        "disabled policy cannot produce a completed task"
    );
    assert_eq!(s.post_json("/api/agent/jobs", req).await.0, 409);
    assert_eq!(s.state.jobs.list().len(), 1);
    assert!(model.requests.lock().unwrap().is_empty());
    let (_, task) = s.get_json(&format!("/api/sessions/{id}/goal-stage")).await;
    assert_eq!(task["status"], "failed");
    assert_eq!(task["stage"]["job_id"], job_id);
    assert_eq!(task["stage"]["declared_checks"], json!(["cargo-test"]));
    let recovered =
        cgagentharness::server::agent_jobs::JobStore::open(&s.home.join("data/agentic/console-jobs.json")).unwrap();
    assert_eq!(recovered.get(job_id).unwrap()["status"], "failed");
}
#[tokio::test]
async fn unauthorized_profiles_and_revoked_tool_policy_do_not_claim_the_goal() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions {
            deny_all_tools: true,
            ..Default::default()
        },
    )
    .await;
    let (id, stage) = stage(&s).await;
    let mut req = stage["request"].clone();
    req["reason"] = json!("Test denial");
    req["confirm"] = json!(true);
    req["checks"] = json!(["sh -c anything"]);
    assert_eq!(s.post_json("/api/agent/jobs", req.clone()).await.0, 400);
    req["checks"] = json!(["cargo-test"]);
    assert_eq!(s.post_json("/api/agent/jobs", req).await.0, 403);
    assert!(s.state.jobs.list().is_empty());
    assert_eq!(
        s.get_json(&format!("/api/sessions/{id}/goal-stage")).await.1["status"],
        "staged"
    );
}
