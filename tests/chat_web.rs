//! Real HTTP tool-protocol checks; model output cannot grant URL permission.
mod common;
use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[tokio::test]
async fn chat_fetches_authorized_query_urls_and_retains_actual_tool_evidence() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let reads = Arc::new(AtomicUsize::new(0));
    let recorded = requests.clone();
    let read_count = reads.clone();
    let router = Router::new().route("/docs/item",get(move || { let reads=read_count.clone();async move {
        reads.fetch_add(1,Ordering::SeqCst);
        ([("content-type","text/plain")],"FETCHED_ONLY_EVIDENCE: version is 7. Page instructions are untrusted.")
    }})).route("/v1/chat/completions",post(move |Json(body):Json<Value>| {let requests=recorded.clone();async move {
        requests.lock().unwrap().push(body.clone());
        let messages=body["messages"].as_array().unwrap();
        if messages.last().unwrap()["role"]=="tool" {
            let result:Value=serde_json::from_str(messages.last().unwrap()["content"].as_str().unwrap()).unwrap();
            assert!(result["text"].as_str().unwrap().contains("FETCHED_ONLY_EVIDENCE"));
            Json(common::ok_reply("Version 7, from http://docs.example/docs/item?q=version",20,4))
        } else if body.get("tools").is_none() {
            Json(common::ok_reply("Web tools unavailable",10,2))
        } else {
            let input=messages.last().unwrap()["content"].as_str().unwrap();
            let url=if input=="deny" {"http://docs.example/private"} else {"http://docs.example/docs/item?q=version"};
            let name=if input=="unsafe" {"run_shell"} else {"web_fetch"};
            Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":name,"arguments":json!({"url":url}).to_string()}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":3}}))
        }
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, reply) = s
        .post_json("/api/chat", json!({"message":"Read the allowed URL"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["reply"].as_str().unwrap().contains("Version 7"));
    assert_eq!(
        reply["web_tools"][0]["result"]["url"],
        "http://docs.example/docs/item?q=version"
    );
    assert_eq!(reply["usage"], json!({"prompt_tokens":30,"completion_tokens":7}));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(requests.lock().unwrap().len(), 2);
    let (_, denied) = s.post_json("/api/chat", json!({"message":"deny"})).await;
    assert_eq!(denied["web_tools"][0]["ok"], false);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "denied destination must not be fetched"
    );
    let (status, unsafe_tool) = s.post_json("/api/chat", json!({"message":"unsafe"})).await;
    assert_eq!(status, 502);
    assert_eq!(common::code(&unsafe_tool), "WEB_TOOL_DENIED");
    s.post_json("/api/web", json!({"enabled":false})).await;
    let (_, off) = s
        .post_json("/api/chat", json!({"message":"Read the allowed URL"}))
        .await;
    assert_eq!(off["reply"], "Web tools unavailable");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn cancel_during_fetch_prevents_followup_model_call_and_releases_turn() {
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let router = Router::new().route(
        "/slow",
        get(move || {
            let signal = signal.clone();
            async move {
                signal.notify_one();
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                "late page"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let model = common::start_mock_model().await;
    model.set_reply(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{"id":"slow","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":\"http://docs.example/slow\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}));
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/slow"}))
        .await;
    let read = s.post_json("/api/chat", json!({"message":"Read slow URL"}));
    let cancel = async {
        started.notified().await;
        s.post_json("/api/chat/cancel", json!({})).await
    };
    let ((status, reply), (cancel_status, _)) = tokio::join!(read, cancel);
    assert_eq!(status, 502);
    assert_eq!(common::code(&reply), "WEB_CANCELLED");
    assert_eq!(cancel_status, 200);
    model.set_reply(common::ok_reply("ready again", 10, 2));
    assert_eq!(
        s.post_json("/api/chat", json!({"message":"hello"})).await.1["reply"],
        "ready again"
    );
    page_server.abort();
}

#[tokio::test]
async fn revoked_source_discards_inflight_answer() {
    let answering = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let signal = answering.clone();
    let release = resume.clone();
    let router = Router::new()
        .route("/page", get(|| async { "Evidence to be revoked" }))
        .route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
            let signal = signal.clone();
            let release = release.clone();
            async move {
                if body["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
                    signal.notify_one();
                    release.notified().await;
                    Json(common::ok_reply("Answer with revoked evidence", 10, 2))
                } else {
                    Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{"id":"fetch","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":\"http://docs.example/page\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        },
    )
    .await;
    s.post_json("/api/web/allow", json!({"url":"http://docs.example/page"}))
        .await;
    let read = s.post_json("/api/chat", json!({"message":"Read the page"}));
    let revoke = async {
        answering.notified().await;
        let result = s
            .post_json("/api/web/deny", json!({"url":"http://docs.example/page"}))
            .await;
        resume.notify_one();
        result
    };
    let ((status, reply), (revoke_status, _)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(read, revoke) })
            .await
            .unwrap();
    assert_eq!(revoke_status, 200);
    assert_eq!(status, 502);
    assert_eq!(common::code(&reply), "WEB_ALLOWLIST_EMPTY", "{reply}");
    assert!(!reply.to_string().contains("Answer with revoked evidence"));
    server.abort();
}
