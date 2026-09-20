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
                let mut records = records.lock().unwrap();
                let status = statuses[records.len().min(statuses.len() - 1)];
                records.push(body);
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
            ("notifications.webhook_url", &json!(url).to_string()),
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
    let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg).unwrap());
    for (i, status) in ["finished", "failed", "cancelled"].iter().enumerate() {
        let id = format!("{i:032x}");
        store.insert_running(&id, "SECRET_ACTION", tokio::spawn(async {}));
        if *status == "cancelled" {
            store.cancel(&id);
            store.cancel(&id);
        } else {
            store.finish(&id, Ok(json!({"ok":*status == "finished","text":"SECRET_RESULT"})));
        }
        store.finish(&id, Ok(json!({"ok":true})));
        assert_eq!(store.get(&id).unwrap()["status"], *status);
    }
    wait_records(&records, 2).await;
    let rows = records.lock().unwrap().clone();
    assert_eq!(rows[0], rows[1], "retry must retain the batch identity and payload");
    assert_eq!(rows[0]["events"].as_array().unwrap().len(), 3);
    for event in rows[0]["events"].as_array().unwrap() {
        assert_eq!(event.as_object().unwrap().len(), 4);
        for key in ["job_id", "status", "created_at", "finished_at"] {
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
        let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg).unwrap());
        let id = "a".repeat(32);
        store.insert_running(&id, "fixture", tokio::spawn(async {}));
        store.finish(&id, Ok(json!({"ok":true})));
        wait_records(&records, attempts).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(records.lock().unwrap().len(), attempts);
        assert_eq!(store.get(&id).unwrap()["status"], "finished");
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
        assert!(Notifier::start(&home, &cfg).unwrap().is_none());
    }
    let cfg = config_with(
        &home.root,
        &[
            ("notifications.enabled", "true"),
            ("notifications.webhook_url", &json!(url).to_string()),
        ],
    );
    assert!(Notifier::start(&home, &cfg).is_err());
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
    let store = JobStore::new().with_notifications(Notifier::start(&home, &cfg).unwrap());
    for i in 0..3 {
        let id = format!("{i:032x}");
        store.insert_running(&id, "fixture", tokio::spawn(async {}));
        store.finish(&id, Ok(json!({"ok":true})));
        assert_eq!(store.get(&id).unwrap()["status"], "finished");
    }
    let audit = std::fs::read_to_string(home.root.join("logs/audit.jsonl")).unwrap();
    assert_eq!(audit.matches("notification_dropped").count(), 2);
    wait_records(&records, 1).await;
    assert_eq!(records.lock().unwrap()[0]["events"].as_array().unwrap().len(), 1);
    task.abort();
}
