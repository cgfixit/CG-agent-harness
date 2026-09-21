//! Local-only webhook delivery and shared terminal-job transition regressions.
mod common;
use axum::{
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use cgagentharness::{
    common::home::Home,
    server::{agent_jobs::JobStore, notifications::Notifier},
};
use common::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn receiver(statuses: Vec<u16>) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    receiver_delayed(statuses, Duration::ZERO).await
}
async fn receiver_delayed(
    statuses: Vec<u16>,
    first_delay: Duration,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let saved = records.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture binds loopback only.
    let url = format!("http://{}/hook", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this is an isolated HTTP receiver.
    let redirected = records.clone();
    let router = Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let records = saved.clone();
            let statuses = statuses.clone();
            async move {
                assert!(headers.get("x-cgagentharness-batch-id").is_some());
                let (status, first) = {
                    let mut records = records.lock().unwrap();
                    let first = records.is_empty();
                    let status = statuses[records.len().min(statuses.len() - 1)];
                    records.push(body);
                    (status, first)
                };
                if first {
                    tokio::time::sleep(first_delay).await;
                }
                (
                    StatusCode::from_u16(status).unwrap(),
                    [("Location", "/other")],
                    "receiver body must not be logged",
                )
            }
        }),
    );
    let router = router.route(
        "/other",
        axum::routing::any(move || {
            let records = redirected.clone();
            async move {
                records.lock().unwrap().push(json!({"redirect_followed":true}));
                StatusCode::OK
            }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, records, task)
}
fn config(home: &Home, url: &str) -> cgagentharness::common::config::AppConfig {
    config_with(
        &home.root,
        &[
            ("notifications.enabled", "true"),
            ("auth.enabled", "false"),
            ("notifications.destinations", &json!([{"id":"fixture","owner":"local","enabled":true,"url":url,"events":["finished","failed","cancelled"],"use_bearer":false,"rate_per_minute":120}]).to_string()),
            ("notifications.jitter_percent", "0"),
            ("notifications.private_url_allowlist", &json!([url]).to_string()),
            ("notifications.batch_interval_sec", "1"),
            ("notifications.retry_delay_sec", "1"),
        ],
    )
}
async fn wait_records(records: &Arc<Mutex<Vec<Value>>>, count: usize) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while records.lock().unwrap().len() < count {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn completion_batches_retry_with_stable_identity_and_no_job_content() {
    let (url, records, task) = receiver(vec![503, 200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let cfg = config(&home, &url);
    let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg, None).unwrap());
    for (i, status) in ["finished", "failed", "cancelled"].iter().enumerate() {
        let id = format!("{i:032x}");
        store.insert_running("local", &id, "SECRET_ACTION", tokio::spawn(async {}));
        if *status == "cancelled" {
            store.cancel("local", &id);
            store.cancel("local", &id);
        } else {
            store.finish(&id, Ok(json!({"ok":*status == "finished","text":"SECRET_RESULT"})));
        }
        store.finish(&id, Ok(json!({"ok":true})));
        assert_eq!(store.get("local", &id).unwrap()["status"], *status);
    }
    wait_records(&records, 2).await;
    let rows = records.lock().unwrap().clone();
    assert_eq!(rows[0], rows[1], "retry must retain the batch identity and payload");
    assert_eq!(rows[0]["events"].as_array().unwrap().len(), 3);
    for event in rows[0]["events"].as_array().unwrap() {
        assert_eq!(event.as_object().unwrap().len(), 6);
        for key in [
            "event_id",
            "delivery_id",
            "job_id",
            "status",
            "created_at",
            "finished_at",
        ] {
            assert!(event.get(key).is_some());
        }
    }
    assert!(!rows[0].to_string().contains("SECRET"));
    task.abort();
}
#[tokio::test]
async fn redirect_is_not_followed_and_delivery_exhaustion_does_not_change_job_result() {
    for (statuses, attempts) in [(vec![302], 1), (vec![500], 3)] {
        let (url, records, task) = receiver(statuses).await;
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("home"));
        home.ensure_layout().unwrap();
        let cfg = config(&home, &url);
        let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg, None).unwrap());
        let id = "a".repeat(32);
        store.insert_running("local", &id, "fixture", tokio::spawn(async {}));
        store.finish(&id, Ok(json!({"ok":true})));
        wait_records(&records, attempts).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(records.lock().unwrap().len(), attempts);
        assert_eq!(store.get("local", &id).unwrap()["status"], "finished");
        let audit = std::fs::read_to_string(home.root.join("logs/audit.jsonl")).unwrap();
        assert!(audit.contains("notification_delivery"));
        assert!(!audit.contains("receiver body"));
        assert!(!audit.contains(&url));
        task.abort();
    }
}
#[tokio::test]
async fn disabled_and_ungranted_private_webhooks_never_send() {
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    for enabled in ["false", "\"true\""] {
        let cfg = config_with(&home.root, &[("notifications.enabled", enabled)]);
        assert!(Notifier::start(&home, &cfg, None).unwrap().is_none());
    }
    let cfg = config_with(
        &home.root,
        &[
            ("notifications.enabled", "true"),
            ("auth.enabled", "false"),
            ("notifications.destinations", &json!([{"id":"fixture","owner":"local","enabled":true,"url":url,"events":["finished","failed","cancelled"],"use_bearer":false,"rate_per_minute":120}]).to_string()),
            ("notifications.jitter_percent", "0"),
        ],
    );
    assert!(Notifier::start(&home, &cfg, None).is_err());
    assert_eq!(records.lock().unwrap().len(), 0);
    task.abort();
}

#[tokio::test]
async fn queue_overflow_is_audited_and_never_changes_job_completion() {
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let cfg = config(&home, &url);
    let text = std::fs::read_to_string(&cfg.path)
        .unwrap()
        .replace("queue_capacity: 128", "queue_capacity: 1");
    let cfg = cgagentharness::common::config::AppConfig::from_str(&text, &cfg.path).unwrap();
    let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg, None).unwrap());
    for i in 0..3 {
        let id = format!("{i:032x}");
        store.insert_running("local", &id, "fixture", tokio::spawn(async {}));
        store.finish(&id, Ok(json!({"ok":true})));
        assert_eq!(store.get("local", &id).unwrap()["status"], "finished");
    }
    let audit = std::fs::read_to_string(home.root.join("logs/audit.jsonl")).unwrap();
    assert_eq!(audit.matches("notification_dropped").count(), 2);
    wait_records(&records, 1).await;
    assert_eq!(records.lock().unwrap()[0]["events"].as_array().unwrap().len(), 1);
    task.abort();
}

#[tokio::test]
async fn persisted_delivery_recovers_and_reconciles_the_job_enqueue_crash_gap() {
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let cfg = config(&home, &url);
    let path = home.data_dir().join("agentic/console-jobs.json");
    let store = JobStore::open(&path).unwrap();
    let id = "b".repeat(32);
    store.insert_running("local", &id, "SECRET_ACTION", tokio::spawn(async {}));
    store.finish(&id, Ok(json!({"ok":true,"private":"DO_NOT_DELIVER"})));
    drop(store); // job is durable; no notification was enqueued before the crash
    let first = Notifier::start(&home, &cfg, None).unwrap().unwrap();
    first.stop();
    let store = JobStore::open(&path).unwrap().with_notifications(Some(first.clone()));
    assert_eq!(first.status("local")["deliveries"].as_array().unwrap().len(), 1);
    let delivery = first.status("local")["deliveries"][0]["delivery_id"].clone();
    drop(store);
    drop(first);
    let resumed = Notifier::start(&home, &cfg, None).unwrap().unwrap();
    let _store = JobStore::open(&path).unwrap().with_notifications(Some(resumed.clone()));
    assert_eq!(resumed.status("local")["deliveries"].as_array().unwrap().len(), 1);
    wait_records(&records, 1).await;
    assert_eq!(records.lock().unwrap()[0]["events"][0]["delivery_id"], delivery);
    assert!(!records.lock().unwrap()[0].to_string().contains("DO_NOT_DELIVER"));
    assert!(resumed.status("user_foreign")["deliveries"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        resumed
            .replay("user_foreign", delivery.as_str().unwrap())
            .unwrap_err()
            .code,
        "DELIVERY_NOT_FOUND"
    );
    resumed.stop();
    task.abort();
}

#[tokio::test]
async fn timeout_after_acceptance_retries_with_the_same_receiver_dedup_id() {
    let (url, records, task) = receiver_delayed(vec![200], Duration::from_secs(3)).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let base = config(&home, &url);
    let text = std::fs::read_to_string(&base.path)
        .unwrap()
        .replace("timeout_sec: 5 # 1–30", "timeout_sec: 1 # 1–30");
    std::fs::write(&base.path, &text).unwrap();
    let cfg = cgagentharness::common::config::AppConfig::from_str(&text, &base.path).unwrap();
    let notifier = Notifier::start(&home, &cfg, None).unwrap().unwrap();
    let store = JobStore::new().with_notifications(Some(notifier.clone()));
    let id = "c".repeat(32);
    store.insert_running("local", &id, "fixture", tokio::spawn(async {}));
    store.finish(&id, Ok(json!({"ok":true})));
    wait_records(&records, 2).await;
    let rows = records.lock().unwrap().clone();
    assert_eq!(
        rows[0]["events"], rows[1]["events"],
        "receiver accepted before first response timed out"
    );
    assert_eq!(store.get("local", &id).unwrap()["status"], "finished");
    notifier.stop();
    task.abort();
}

#[tokio::test]
async fn destination_removal_revokes_pending_delivery_and_replay_without_restart() {
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let cfg = config(&home, &url);
    let notifier = Notifier::start(&home, &cfg, None).unwrap().unwrap();
    let store = JobStore::new().with_notifications(Some(notifier.clone()));
    let id = "d".repeat(32);
    store.insert_running("local", &id, "fixture", tokio::spawn(async {}));
    store.finish(&id, Ok(json!({"ok":true})));
    let delivery = notifier.status("local")["deliveries"][0]["delivery_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let text = std::fs::read_to_string(&cfg.path)
        .unwrap()
        .replace("enabled: true", "enabled: false");
    std::fs::write(&cfg.path, text).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(records.lock().unwrap().len(), 0);
    assert_eq!(notifier.status("local")["deliveries"][0]["state"], "revoked");
    assert_eq!(
        notifier.replay("local", &delivery).unwrap_err().code,
        "DELIVERY_REVOKED"
    );
    assert_eq!(store.get("local", &id).unwrap()["status"], "finished");
    notifier.stop();
    task.abort();
}

#[tokio::test]
async fn current_account_revocation_stops_an_owned_destination() {
    use cgagentharness::common::auth_store::AuthManager;
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let base = config(&home, &url);
    let auth = Arc::new(AuthManager::open(&home.auth_path(), &base).unwrap());
    auth.create_user("delivery-owner", "isolated-notification-fixture-password", "operator")
        .unwrap();
    let owner = auth.get_user("delivery-owner").unwrap().user_id;
    let mut raw = base.raw.clone();
    raw["auth"]["enabled"] = serde_yaml_ng::Value::Bool(true);
    raw["notifications"]["destinations"][0]["owner"] = serde_yaml_ng::Value::String(owner.clone());
    let text = serde_yaml_ng::to_string(&raw).unwrap();
    std::fs::write(&base.path, &text).unwrap();
    let cfg = cgagentharness::common::config::AppConfig::from_str(&text, &base.path).unwrap();
    let notifier = Notifier::start(&home, &cfg, Some(auth.clone())).unwrap().unwrap();
    let store = JobStore::new().with_notifications(Some(notifier.clone()));
    let id = "e".repeat(32);
    store.insert_running(&owner, &id, "fixture", tokio::spawn(async {}));
    store.finish(&id, Ok(json!({"ok":true})));
    assert_eq!(notifier.status(&owner)["deliveries"].as_array().unwrap().len(), 1);
    auth.disable_user("delivery-owner").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(records.lock().unwrap().len(), 0);
    assert_eq!(notifier.status(&owner)["deliveries"][0]["state"], "revoked");
    notifier.stop();
    task.abort();
}

#[tokio::test]
async fn guarded_replay_requires_review_and_never_restarts_the_job() {
    let (url, records, task) = receiver(vec![500, 200]).await;
    let model = start_mock_model().await;
    let destination =
        json!([{"id":"fixture","owner":"local","enabled":true,"url":url,"events":["finished"],"rate_per_minute":120}])
            .to_string();
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default()
            .with("notifications.enabled", "true")
            .with("notifications.destinations", &destination)
            .with("notifications.private_url_allowlist", &json!([url]).to_string())
            .with("notifications.max_attempts", "1")
            .with("notifications.batch_interval_sec", "1"),
    )
    .await;
    let id = "f".repeat(32);
    s.state
        .jobs
        .insert_running("local", &id, "fixture", tokio::spawn(async {}));
    s.state.jobs.finish(&id, Ok(json!({"ok":true})));
    wait_records(&records, 1).await;
    let delivery = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, body) = s.get_json("/api/notifications").await;
            if body["deliveries"][0]["state"] == "failed" {
                break body["deliveries"][0]["delivery_id"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let path = format!("/api/notifications/{delivery}/replay");
    assert_eq!(
        s.post_json(&path, json!({"confirm":false,"reason":"fixture resend"}))
            .await
            .0,
        400
    );
    let (status, result) = s
        .post_json(&path, json!({"confirm":true,"reason":"fixture resend"}))
        .await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["job_restarted"], false);
    wait_records(&records, 2).await;
    let rows = records.lock().unwrap().clone();
    assert_eq!(rows[0]["events"], rows[1]["events"]);
    assert_eq!(s.state.jobs.list("local").len(), 1);
    assert_eq!(s.state.jobs.get("local", &id).unwrap()["status"], "finished");
    task.abort();
}

#[tokio::test]
async fn oversized_completion_is_dropped_without_persisting_sending_or_logging_content() {
    use cgagentharness::server::notifications::Completion;
    let (url, records, task) = receiver(vec![200]).await;
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    home.ensure_layout().unwrap();
    let cfg = config(&home, &url);
    let notifier = Notifier::start(&home, &cfg, None).unwrap().unwrap();
    notifier.enqueue(Completion {
        owner: "local".into(),
        job_id: "PRIVATE_OVERSIZED_CANARY".repeat(10000),
        status: "finished".into(),
        created_at: 1.0,
        finished_at: cgagentharness::common::now_ts(),
    });
    assert_eq!(notifier.status("local")["deliveries"], json!([]));
    let audit = std::fs::read_to_string(home.logs_dir().join("audit.jsonl")).unwrap();
    assert!(audit.contains("invalid_event"));
    assert!(!audit.contains("PRIVATE_OVERSIZED_CANARY"));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(records.lock().unwrap().is_empty());
    notifier.stop();
    task.abort();
}
