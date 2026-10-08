//! Real HTTP tool-protocol checks; model output cannot grant URL permission.
mod common;
use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[tokio::test]
async fn chat_batches_multiple_reads_with_one_protocol_budget_and_no_malformed_prefix_execution() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r = reads.clone();
    let fixture=Router::new().route("/docs/{page}",get(move || {let r=r.clone();async move {r.fetch_add(1,Ordering::SeqCst); ([("content-type","text/plain")],"BATCH_BACKUP_EVIDENCE")}}))
        .route("/v1/chat/completions",post(|Json(body):Json<Value>|async move {
            let messages=body["messages"].as_array().unwrap();
            let mode=messages.iter().find(|m|m["role"]=="user").unwrap()["content"].as_str().unwrap();
            let finished=messages.last().unwrap()["role"]=="tool";
            if finished && mode!="cumulative" {
                let tool_messages:Vec<_>=messages.iter().filter(|m|m["role"]=="tool").collect();
                assert_eq!(tool_messages.len(),2);
                assert!(tool_messages.iter().all(|m|m["content"].as_str().unwrap().contains("BATCH_BACKUP_EVIDENCE")));
                return Json(common::ok_reply("Compared both fetched sources.",20,5));
            }
            let mut calls:Vec<_>=(0..if mode=="overlimit" {4}else{2}).map(|i|json!({"id":format!("batch_{i}"),"type":"function","function":{"name":"web_fetch","arguments":json!({"url":format!("http://docs.example/docs/{i}")}).to_string()}})).collect(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            if mode=="duplicate" { calls[1]["id"]=calls[0]["id"].clone(); }
            if mode=="malformed" { calls[1]["function"]["arguments"]=json!("{}"); }
            if mode=="denied" { calls[1]["function"]["arguments"]=json!(json!({"url":"http://docs.example/private"}).to_string()); } // DevSkim: ignore DS137138 because this synthetic URL tests permission refusal against the loopback fixture.
            Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":calls}}],"usage":{"prompt_tokens":10,"completion_tokens":3}}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default().with("web.chat_tool_calls", "3")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, reply) = s.post_json("/api/chat", json!({"message":"compare"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["reply"], "Compared both fetched sources.");
    assert_eq!(reply["web_tools"].as_array().unwrap().len(), 2);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    for (mode, code) in [
        ("overlimit", "WEB_TOOL_LIMIT"),
        ("duplicate", "WEB_TOOL_RESPONSE"),
        ("malformed", "WEB_TOOL_ARGUMENTS"),
    ] {
        let (status, reply) = s.post_json("/api/chat", json!({"message":mode})).await;
        assert_eq!(status, 502, "{reply}");
        assert_eq!(common::code(&reply), code, "{reply}");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "no valid prefix reads before malformed/over-budget suffix refusal"
        );
        assert!(!s.state.generation_gate.is_held());
    }
    let (_, reply) = s.post_json("/api/chat", json!({"message":"cumulative"})).await;
    assert_eq!(common::code(&reply), "WEB_TOOL_LIMIT");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "two calls plus two more exceeds total budget three"
    );
    let (_, reply) = s.post_json("/api/chat", json!({"message":"denied"})).await;
    assert!(
        reply["reply"].as_str().unwrap().contains("Earlier reads succeeded"),
        "{reply}"
    );
    assert_eq!(reply["web_tools"][1]["code"], "WEB_HOST_DENIED");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        5,
        "a permitted call cannot authorize its denied sibling"
    );
    task.abort();
}

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
    let session = s
        .state
        .store
        .for_owner("local")
        .get(reply["session_id"].as_str().unwrap())
        .unwrap();
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

/// A tool round sends the whole prompt again with the result. Web chat must keep
/// that follow-up inside web.total_tokens: clip a fetched page to the room left,
/// and stop offering tools once even a minimal result would not fit. Before, the
/// fetch ran and the follow-up call failed with WEB_TOKEN_BUDGET (502).
#[tokio::test]
async fn web_chat_tool_rounds_stay_inside_the_web_token_budget() {
    let reads = Arc::new(AtomicUsize::new(0));
    let last_prompt = Arc::new(AtomicU64::new(0));
    let offered = Arc::new(AtomicBool::new(false));
    let (read_count, prompt_seen, tools_seen) = (reads.clone(), last_prompt.clone(), offered.clone());
    let router = Router::new()
        .route(
            "/docs/item",
            get(move || {
                let reads = read_count.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(2000))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let (prompt_seen, tools_seen) = (prompt_seen.clone(), tools_seen.clone());
                async move {
                    // Report the real prompt size so the web budget charges what was sent.
                    let prompt = (body["messages"].to_string().len()
                        + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                        as u64
                        / 4;
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let tools = body.get("tools").is_some_and(|t| t.as_array().is_some_and(|t| !t.is_empty()));
                    if last["role"] == "tool" {
                        Json(common::ok_reply("Answer from the fetched page", prompt, 20))
                    } else if last["content"].as_str().is_some_and(|text| text.starts_with("fetch")) {
                        tools_seen.store(tools, Ordering::SeqCst);
                        if !tools {
                            return Json(common::ok_reply("Answered without the page", prompt, 20));
                        }
                        let arguments = json!({"url":"http://docs.example/docs/item"}).to_string(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":"web_fetch","arguments":arguments}}]}}],
                            "usage":{"prompt_tokens":prompt,"completion_tokens":20}}))
                    } else {
                        // Track the history's size from seed turns only.
                        prompt_seen.store(prompt, Ordering::SeqCst);
                        Json(common::ok_reply("Noted.", prompt, 20))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // LM Studio doubles the reply reservation: 4096 tokens for max_tokens 2048. A
    // round needs both replies, so with the shipped 28000-token budget rounds fit up
    // to about 9,912 input tokens ((28000 - 256 - 2 x 4096 + 272) / 2, the follow-up
    // counted without the ~272 definition tokens), and web chat admits prompts up to
    // 19,808.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.provider", "lmstudio")
        .with("models.local_llm.max_tokens", "2048"),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let mut session = Value::Null;
    // Phase 1 (about 9k input tokens): both prompts, a minimal result and both replies
    // fit, so tools stay on and the 6000-token page is clipped to what the follow-up allows.
    for turn in 0..30 {
        if last_prompt.load(Ordering::SeqCst) >= 9_000 {
            break;
        }
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        session = body["session_id"].clone();
    }
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "the follow-up after the fetch must fit: {body}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(body["reply"], "Answer from the fetched page");
    let text = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !text.is_empty() && text.len() < 24_000 && "PAGE_EVIDENCE ".repeat(2000).starts_with(text),
        "the page is clipped to the room left: {} chars",
        text.len()
    );
    // Phase 2 (past about 10k input tokens, still admitted): a round no longer fits,
    // so tools are not offered and nothing is fetched.
    for turn in 30..60 {
        if last_prompt.load(Ordering::SeqCst) >= 10_200 {
            break;
        }
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(!offered.load(Ordering::SeqCst), "no tools once a tool round cannot fit");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(body["reply"], "Answered without the page");
    assert_eq!(body["web_tools"][0]["code"], "WEB_TOKEN_BUDGET");
    server.abort();
}

#[tokio::test]
async fn web_chat_offers_tools_only_when_both_replies_of_a_round_fit() {
    let reads = Arc::new(AtomicUsize::new(0));
    let last_prompt = Arc::new(AtomicU64::new(0));
    let offered = Arc::new(AtomicBool::new(false));
    let (read_count, prompt_seen, tools_seen) = (reads.clone(), last_prompt.clone(), offered.clone());
    let router = Router::new()
        .route(
            "/docs/item",
            get(move || {
                let reads = read_count.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(200))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let (prompt_seen, tools_seen) = (prompt_seen.clone(), tools_seen.clone());
                async move {
                    let prompt = (body["messages"].to_string().len()
                        + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                        as u64
                        / 4;
                    let messages = body["messages"].as_array().unwrap();
                    let last = messages.last().unwrap();
                    let tools = body.get("tools").is_some_and(|t| t.as_array().is_some_and(|t| !t.is_empty()));
                    if last["role"] == "tool" {
                        Json(common::ok_reply("Answer from the page", prompt, 20))
                    } else if last["content"] == "fetch the page" {
                        tools_seen.fetch_or(tools, Ordering::SeqCst);
                        if !tools {
                            return Json(common::ok_reply("Answered without the page", prompt, 20));
                        }
                        // The tool-call reply spends its whole cap, as a reasoning model can.
                        let arguments = json!({"url":"http://docs.example/docs/item"}).to_string(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":"call_fetch_1","type":"function","function":{"name":"web_fetch","arguments":arguments}}]}}],
                            "usage":{"prompt_tokens":prompt,"completion_tokens":2048}}))
                    } else {
                        prompt_seen.store(prompt, Ordering::SeqCst);
                        Json(common::ok_reply("Noted.", prompt, 20))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Shipped Ollama with reasoning "none" reserves one 2048-token reply per call.
    // A round needs both replies, so tools fit up to about 5,960 input tokens
    // ((16000 - 256 - 2 x 2048 + 272) / 2, the follow-up counted without the ~272
    // definition tokens); counting one reply would offer them past 6,848.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.max_tokens", "2048")
        .with("web.total_tokens", "16000"),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let mut session = Value::Null;
    for turn in 0..20 {
        if last_prompt.load(Ordering::SeqCst) >= 6_100 {
            break;
        }
        let (status, body) = s
            .post_json(
                "/api/chat",
                json!({"message": format!("seed {turn} {}", "x".repeat(2000)), "session_id": session}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        session = body["session_id"].clone();
    }
    let seeded = last_prompt.load(Ordering::SeqCst);
    assert!((6_100..6_848).contains(&seeded), "seeded to {seeded} input tokens");
    let (status, body) = s
        .post_json("/api/chat", json!({"message": "fetch the page", "session_id": session}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answered without the page", "{body}");
    assert!(
        !offered.load(Ordering::SeqCst),
        "tools were offered for a round that cannot fit"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no read runs when a round cannot fit");
    assert_eq!(body["web_tools"][0]["code"], "WEB_TOKEN_BUDGET", "{body}");
    server.abort();
}

#[tokio::test]
async fn web_chat_batches_keep_calibrated_room_for_each_later_result() {
    // A 504-character title makes the second page's smallest result about 180
    // tokens, 360 at the learned ratio of 2: more than an uncalibrated 256-token
    // reserve, less than the calibrated 512.
    let title = "Release notes ".repeat(36);
    let page = format!("<html><head><title>{title}</title></head><body><p>TITLED_EVIDENCE</p></body></html>");
    let router = Router::new()
        .route(
            "/docs/big",
            get(|| async { ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(2000)) }),
        )
        .route(
            "/docs/titled",
            get(move || {
                let page = page.clone();
                async move { ([("content-type", "text/html")], page) }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                // Report twice the sent size: the session learns a token ratio of about 2.
                let prompt = (body["messages"].to_string().len()
                    + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                    as u64
                    / 2;
                let messages = body["messages"].as_array().unwrap();
                let last = messages.last().unwrap();
                if last["role"] == "tool" {
                    Json(common::ok_reply("Answer from both pages", prompt, 20))
                } else if last["content"] == "fetch both pages" {
                    let calls: Vec<_> = ["big", "titled"]
                        .iter()
                        .map(|page| {
                            json!({"id":format!("call_{page}"),"type":"function","function":{"name":"web_fetch",
                                "arguments":json!({"url":format!("http://docs.example/docs/{page}")}).to_string()}}) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                        })
                        .collect();
                    Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                        "tool_calls":calls}}],"usage":{"prompt_tokens":prompt,"completion_tokens":20}}))
                } else {
                    Json(common::ok_reply("Noted.", prompt, 20))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // 25000 tokens leave the batch about 6k after two ~8.7k-token prompts and the
    // 1024-token reply reservation, so the first page (about 12k at ratio 2) is cut.
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.provider", "lmstudio")
        .with("models.local_llm.max_tokens", "512")
        .with("web.total_tokens", "25000"),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, body) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message": "fetch both pages", "session_id": body["session_id"]}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from both pages", "{body}");
    let first = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !first.is_empty() && first.len() < 24_000,
        "the first page is cut to leave room: {} chars",
        first.len()
    );
    let second = &body["web_tools"][1]["result"];
    assert_eq!(second["title"], title.trim(), "{body}");
    assert!(
        second["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty() && "TITLED_EVIDENCE".starts_with(text)),
        "{body}"
    );
    server.abort();
}

/// A mock model for the budget tests below. It reports bytes / 4 of what it is
/// sent and records every request. Offered tools for a message starting with
/// "fetch", it asks for `calls` parallel reads of `url` (default: the counted
/// page) from a reply that carries `content` and spends its whole 2048-token
/// cap. Without tools, or asked to "search", it says it cannot search the web.
fn budget_model(
    calls: usize,
    content: Option<String>,
    url: Option<String>,
    requests: Arc<Mutex<Vec<Value>>>,
) -> Router {
    let url = url.unwrap_or_else(|| "http://docs.example/docs/item".into()); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
    Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let (requests, content, url) = (requests.clone(), content.clone(), url.clone());
            async move {
                requests.lock().unwrap().push(body.clone());
                let prompt = (body["messages"].to_string().len()
                    + body.get("tools").map_or(0, |tools| tools.to_string().len()))
                    as u64
                    / 4;
                let last = body["messages"].as_array().unwrap().last().unwrap().clone();
                let fetch = last["content"].as_str().is_some_and(|text| text.starts_with("fetch"));
                let search = last["content"].as_str().is_some_and(|text| text.starts_with("search"));
                if search {
                    Json(common::ok_reply("I cannot search the web.", prompt, 20))
                } else if last["role"] == "tool" {
                    Json(common::ok_reply("Answer from the page", prompt, 20))
                } else if !fetch {
                    Json(common::ok_reply("Noted.", prompt, 20))
                } else if body.get("tools").is_none() {
                    Json(common::ok_reply("I cannot search the web.", prompt, 20))
                } else {
                    let calls: Vec<_> = (0..calls)
                        .map(|n| {
                            json!({"id":format!("call_{n}"),"type":"function","function":{"name":"web_fetch",
                                "arguments":json!({"url":url}).to_string()}})
                        })
                        .collect();
                    Json(json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":content,
                        "tool_calls":calls}}],"usage":{"prompt_tokens":prompt,"completion_tokens":2048}}))
                }
            }
        }),
    )
}

/// Starts `router` plus a counted 6000-token page at /docs/item, and a server
/// with shipped Ollama (reasoning "none": one 2048-token reply reserved per
/// call), `web.total_tokens` 16000 and `calls` web calls a turn. Returns the
/// server, the read counter, and the input tokens of a fresh session's first
/// short message, which the tests pad to the size they need.
async fn budget_server(router: Router, calls: usize) -> (common::TestServer, Arc<AtomicUsize>, u64) {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let router = router.route(
        "/docs/item",
        get(move || {
            let reads = read_count.clone();
            async move {
                reads.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "PAGE_EVIDENCE ".repeat(2000))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..Default::default()
        }
        .with("models.local_llm.max_tokens", "2048")
        .with("web.total_tokens", "16000")
        .with("web.chat_tool_calls", &calls.to_string()),
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
    let (status, body) = s.post_json("/api/chat", json!({"message": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    let base = body["usage"]["prompt_tokens"].as_u64().unwrap();
    assert!(
        base < 4_600,
        "the budget tests pad a short first message of {base} tokens"
    );
    (s, reads, base)
}

/// The model may batch every call it has left (`parallel_tool_calls`). A batch
/// whose calls cannot each fit a minimal result in the follow-up runs none of
/// its reads, and the model answers without tools. Before, the first of ten
/// reads ran, then the turn ended with WEB_TOKEN_BUDGET and no answer.
#[tokio::test]
async fn web_chat_runs_no_read_of_a_batch_without_room_for_each_result() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(10, None, None, requests.clone()), 10).await;
    // About 5,250 input tokens: a round fits (up to about 5,960 tokens), but after the
    // tool-call reply spends its cap, ten minimal results (2,560) do not.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_250 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    // Verbatim: the answering request offered no tools, so grounding must not
    // claim this turn includes them.
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs for a batch that cannot fit"
    );
    assert_eq!(body["web_tools"][0]["code"], "WEB_TOKEN_BUDGET", "{body}");
    let text = body["web_tools"][0]["message"].as_str().unwrap();
    assert!(text.contains("10-call batch; none ran"), "{text}");
    // Asked once with tools (the batch), then once without.
    let requests = requests.lock().unwrap();
    let asked: Vec<bool> = requests
        .iter()
        .filter(|r| r["messages"].as_array().unwrap().last().unwrap()["content"] == message)
        .map(|r| r.get("tools").is_some())
        .collect();
    assert_eq!(asked, [true, false]);
}

/// The follow-up resends the model's own tool-call message, so a call whose reply
/// carries long content can leave no room for its result. The batch check counts
/// that echo before the read; without it, the read ran and the turn ended with
/// WEB_TOKEN_BUDGET.
#[tokio::test]
async fn web_chat_counts_the_echoed_tool_call_message_before_a_read() {
    let content = format!("Let me look that up. {}", "y".repeat(6_000));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, Some(content), None, requests), 1).await;
    // About 5,575 input tokens: a one-call round fits (up to about 5,960), but after a
    // cap-spending reply the ~1,500-token echo leaves no room for a result.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_575 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs when the echo leaves no room"
    );
    let text = body["web_tools"][0]["message"].as_str().unwrap();
    assert!(text.contains("1-call batch; none ran"), "{text}");
}

/// URLs are never cut, so a long one makes even a minimal result large. The batch
/// check reserves each call's own minimal result: a flat 256-token floor let this
/// read of a ~2 KB URL run, and the turn then failed with WEB_TOKEN_BUDGET.
#[tokio::test]
async fn web_chat_reserves_a_long_urls_minimal_result_before_the_read() {
    let url = format!("http://docs.example/docs/item?ref={}", "a".repeat(2_000)); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
    let (s, reads, base) = budget_server(budget_model(1, None, Some(url), Arc::default()), 1).await;
    // About 5,610 input tokens: a one-call round fits (up to about 5,960). With the
    // URL's ~560-token minimal result reserved the batch does not; with 256, it did.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_610 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "no read runs when the URL's result cannot fit"
    );
}

/// Grounding rewrites "I cannot search the web" only when the request that
/// produced it offered the web tools. Web chat withholds them when no tool round
/// fits, and then the statement is true.
#[tokio::test]
async fn web_chat_grounds_a_web_denial_only_when_the_answer_was_offered_tools() {
    let (s, _reads, base) = budget_server(budget_model(1, None, None, Arc::default()), 1).await;
    let (status, body) = s.post_json("/api/chat", json!({"message": "search the web"})).await;
    assert_eq!(status, 200, "{body}");
    let reply = body["reply"].as_str().unwrap();
    assert!(
        reply.starts_with("This turn includes web_fetch, web_search."),
        "{reply}"
    );
    // Past a round's limit (about 5,960 input tokens with one call), tools are withheld.
    let message = format!("search the web {}", "x".repeat(4 * (6_200 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "I cannot search the web.", "{body}");
}

/// The last call of a turn carries no tools, so its result is cut to the room a
/// tool-free follow-up leaves. Counting the definitions anyway cut the page to
/// leave their room unused, and the follow-up's own admission check refused it.
#[tokio::test]
async fn web_chat_sizes_a_tool_free_follow_up_without_the_tool_definitions() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, None, None, requests.clone()), 1).await;
    // About 5,000 input tokens: a one-call round fits (up to about 5,960), and after the
    // tool-call reply spends its cap, the 6000-token page must be cut.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_000 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page", "{body}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let text = body["web_tools"][0]["result"]["text"].as_str().unwrap();
    assert!(
        !text.is_empty() && text.len() < 24_000,
        "the page is cut: {} chars",
        text.len()
    );
    let requests = requests.lock().unwrap();
    let last = |request: &Value| request["messages"].as_array().unwrap().last().unwrap().clone();
    let call = requests
        .iter()
        .find(|r| r.get("tools").is_some() && last(r)["content"] != "hello")
        .unwrap();
    let follow_up = requests.iter().find(|r| last(r)["role"] == "tool").unwrap();
    assert!(follow_up.get("tools").is_none(), "the last call carries no tools");
    // What the web budget charged the tool call, and its estimate of the follow-up.
    let definitions = call["tools"].to_string().len() as u64;
    let charged = (call["messages"].to_string().len() as u64 + definitions) / 4 + 2048;
    let messages = follow_up["messages"].as_array().unwrap();
    let sent = messages[0]["content"].as_str().unwrap().len() + serde_json::to_vec(&messages[1..]).unwrap().len();
    let spent = charged + (sent as u64).div_ceil(4) + 2048;
    assert!(
        spent <= 16_000 && 16_000 - spent < definitions / 8,
        "the result fills the room left: {spent} of 16000, definitions {} tokens",
        definitions / 4
    );
}

/// A round's follow-up carries the tool definitions only after passing its own
/// round check, so the check counts them once. Counting them in both prompts
/// withheld tools from rounds that fit.
#[tokio::test]
async fn web_chat_counts_the_tool_definitions_once_per_round() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (s, reads, base) = budget_server(budget_model(1, None, None, requests.clone()), 1).await;
    // Web chat estimates this call at about 5,866 tokens (the model counts ~47
    // more). A one-call round fits up to 5,824 with the ~272 definition tokens in
    // both prompts, and up to about 5,960 with them in the tool call only.
    let message = format!("fetch the page {}", "x".repeat(4 * (5_910 - base) as usize));
    let (status, body) = s.post_json("/api/chat", json!({"message": message})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"], "Answer from the page", "{body}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let requests = requests.lock().unwrap();
    let call = requests
        .iter()
        .find(|r| r["messages"].as_array().unwrap().last().unwrap()["content"] == message)
        .unwrap();
    let messages = call["messages"].as_array().unwrap();
    let sent = messages[0]["content"].as_str().unwrap().len() + serde_json::to_vec(&messages[1..]).unwrap().len();
    let with_tools = ((sent + call["tools"].to_string().len()) as u64).div_ceil(4);
    assert!(
        2 * with_tools + 256 + 2 * 2048 > 16_000,
        "estimated at {with_tools} tokens, the round fits even with the definitions counted twice"
    );
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
/// within `web.chat_tool_calls` (default 10). The N+1 model tool request is
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
            ..common::ServerOptions::default()
                .with("web.pace_ms", "100")
                .with("models.local_llm.max_tokens", "256")
        },
    )
    .await;
    assert_eq!(s.state.web.limits.chat_tool_calls, 10, "shipped default N");
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

/// The shapes a 7B–27B local model or an older OpenAI-compatible server sends:
/// a complete tool call ending in `stop`, an argument object with a key the
/// schema does not name, a final answer led by an inline `<think>` block and
/// carrying `"tool_calls": null`. Each is a normal round; nothing extra is read.
#[tokio::test]
async fn small_model_tool_call_shapes_complete_one_bounded_round() {
    let reads = Arc::new(AtomicUsize::new(0));
    let r = reads.clone();
    let fixture = Router::new()
        .route("/docs/a", get(move || {
            let r = r.clone();
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "text/plain")], "SMALL_MODEL_EVIDENCE")
            }
        }))
        .route("/v1/chat/completions", post(|Json(body): Json<Value>| async move {
            let messages = body["messages"].as_array().unwrap();
            if messages.last().unwrap()["role"] == "tool" {
                assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("SMALL_MODEL_EVIDENCE"));
                return Json(json!({"model":"mock","choices":[{"finish_reason":"stop","message":{"role":"assistant",
                    "content":"<think>\nThe page says SMALL_MODEL_EVIDENCE.\n</think>\n\nThe page was read.","tool_calls":null}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":3}}));
            }
            let mut reply = tool_call_reply(
                "web_fetch",
                &json!({"url":"http://docs.example/docs/a","reason":"the user asked"}).to_string(), // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                "call_small",
            );
            reply["choices"][0]["finish_reason"] = json!("stop");
            Json(reply)
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = common::spawn_server(
        &format!("http://{address}/v1"),
        common::ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..common::ServerOptions::default()
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    let (status, reply) = s.post_json("/api/chat", json!({"message":"read the doc"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["reply"], "The page was read.",
        "the reasoning block is not the answer: {reply}"
    );
    assert_eq!(reply["web_tools"][0]["ok"], true, "{reply}");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let (_, session) = s
        .get_json(&format!("/api/sessions/{}", reply["session_id"].as_str().unwrap()))
        .await;
    assert!(
        !session.to_string().contains("<think>"),
        "stored history must not re-send reasoning: {session}"
    );
    task.abort();
}
