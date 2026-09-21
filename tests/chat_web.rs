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
async fn loop_prompt_does_not_advertise_tools_even_with_web_enabled() {
    let model = common::start_mock_model().await;
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    s.post_json("/api/web", json!({"enabled":true})).await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let id = session["session_id"].as_str().unwrap();
    s.post_json(
        &format!("/api/sessions/{id}/goal"),
        json!({"goal":"Explain arithmetic"}),
    )
    .await;
    let (status, body) = s
        .post_json("/api/chat", json!({"message":"continue", "session_id":id, "loop":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let request = model.last_request().unwrap();
    assert!(request.get("tools").is_none());
    let prompt = request["messages"][0]["content"].as_str().unwrap();
    assert!(!prompt.contains("you MAY call:"));
    assert!(prompt.contains("This turn has no chat-callable web tools"));
}

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
            Json(common::ok_reply("Version 7, from http://docs.example/docs/item?q=version",5000,4)) // DevSkim: ignore DS137138 because this synthetic response names a test URL resolved only to the loopback fixture.
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
    assert_eq!(reply["usage"], json!({"prompt_tokens":5010,"completion_tokens":7}));
    let session = s.state.store.get(reply["session_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        session.token_calibration.unwrap().ratio,
        1.0,
        "calibrate from the initial 10-token prompt, not the 5010-token aggregate"
    );
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

/// Fresh homes enable web with an empty URL policy. Chat tools must fail closed
/// the same way dedicated `/api/web/*` routes do (`WEB_ALLOWLIST_EMPTY`).
#[tokio::test]
async fn chat_empty_allowlist_refuses_fetch_and_search() {
    let model = common::start_mock_model().await;
    model.set_reply(tool_call_reply(
        "web_fetch",
        r#"{"url":"http://docs.example/docs/item"}"#,
        "empty_fetch",
    ));
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200);
    assert_eq!(web["enabled"], true);
    assert!(
        web["allowlist"].as_array().map(|rows| rows.is_empty()).unwrap_or(false),
        "{web}"
    );
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Read http://docs.example/docs/item"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY", "{reply}");
    assert!(
        reply["reply"].as_str().unwrap_or("").contains("WEB_ALLOWLIST_EMPTY"),
        "{reply}"
    );
    assert_no_serpapi_secret(&reply, &s.home);

    model.set_reply(tool_call_reply(
        "web_search",
        r#"{"query":"docs.example"}"#,
        "empty_search",
    ));
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Search Google for docs.example"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(
        reply["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY",
        "empty policy fails closed before Google-pattern matching: {reply}"
    );
    assert_no_serpapi_secret(&reply, &s.home);
}

/// After an administrator grants a fixture-origin rule, `web_fetch` succeeds
/// within `web.chat_tool_calls` (default 3). The N+1 model tool request is
/// refused with `WEB_TOOL_LIMIT` and the generation gate is released.
#[tokio::test]
async fn chat_origin_grant_fetches_within_bound_and_refuses_n_plus_one() {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let router = Router::new().route(
        "/page",
        get(move || {
            let reads = read_count.clone();
            async move {
                reads.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "PHASE2_FETCH_OK")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let page_server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let model = common::start_mock_model().await;
    model.set_reply(tool_call_reply(
        "web_fetch",
        r#"{"url":"http://docs.example/page"}"#,
        "bound_fetch",
    ));
    let s = common::spawn_server(
        &model.base_url(),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default().with("web.pace_ms", "100")
        },
    )
    .await;
    assert_eq!(s.state.web.limits.chat_tool_calls, 3, "shipped default N");
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/*"}))
            .await
            .0,
        200
    );
    let (status, web) = s.get_json("/api/web").await;
    assert_eq!(status, 200, "{web}");
    assert_eq!(web["allowlist"], json!(["http://docs.example/*"]), "{web}");
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Read the granted origin repeatedly"}))
        .await;
    assert_eq!(status, 502, "{reply}");
    assert_eq!(common::code(&reply), "WEB_TOOL_LIMIT", "{reply}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        s.state.web.limits.chat_tool_calls,
        "exactly N fetches; the extra tool call must not hit the network"
    );
    assert_no_serpapi_secret(&reply, &s.home);
    model.set_reply(common::ok_reply("ready after tool-limit", 10, 2));
    let (status, next) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{next}");
    assert_eq!(next["reply"], "ready after tool-limit");
    page_server.abort();
}

/// A non-empty allowlist that is not a Google search grant still refuses
/// `web_search`. Tip returns `WEB_GOOGLE_PERMISSION` after `require_enabled`
/// succeeds (empty policy is the `WEB_ALLOWLIST_EMPTY` case above).
#[tokio::test]
async fn chat_search_without_google_pattern_returns_google_permission() {
    let model = common::start_mock_model().await;
    model.set_reply(tool_call_reply(
        "web_search",
        r#"{"query":"veeam software cve"}"#,
        "google_denied",
    ));
    let s = common::spawn_server(&model.base_url(), common::ServerOptions::default()).await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url": "http://docs.example/*"}))
            .await
            .0,
        200
    );
    let (status, reply) = s
        .post_json("/api/chat", json!({"message": "Search Google for veeam software cve"}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["web_tools"][0]["ok"], false, "{reply}");
    assert_eq!(reply["web_tools"][0]["code"], "WEB_GOOGLE_PERMISSION", "{reply}");
    assert!(
        reply["reply"].as_str().unwrap_or("").contains("WEB_GOOGLE_PERMISSION"),
        "{reply}"
    );
    assert_no_serpapi_secret(&reply, &s.home);

    let (status, body) = s
        .post_json(
            "/api/web/search",
            json!({"query": "veeam software cve", "engine": "google"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(common::code(&body), "WEB_GOOGLE_PERMISSION", "{body}");
    assert!(common::message(&body).contains("https://www.google.com/*"), "{body}");
    assert_no_serpapi_secret(&body, &s.home);
}

fn tool_call_reply(name: &str, arguments: &str, id: &str) -> Value {
    json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments}}]}}],
        "usage":{"prompt_tokens":10,"completion_tokens":2}})
}

fn assert_no_serpapi_secret(body: &Value, home: &std::path::Path) {
    let dumped = body.to_string();
    let audit = std::fs::read_to_string(home.join("logs").join("audit.jsonl")).unwrap_or_default();
    for (label, text) in [("HTTP JSON", dumped.as_str()), ("audit", audit.as_str())] {
        assert!(
            !text.contains("fixture-secret-key"),
            "{label} echoed the synthetic SerpAPI fixture key: {text}"
        );
        assert!(
            !text.contains("SERPAPI_API_KEY"),
            "{label} named SERPAPI_API_KEY: {text}"
        );
    }
}
