//! Streaming uses the same guards, tools, session commit and generation gate as JSON chat.
mod common;
use axum::{
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    Json,
};
use common::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

async fn model() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let active = Arc::new(AtomicUsize::new(0));
    let count = active.clone();
    let app = common::ollama_router().route("/v1/chat/completions", axum::routing::post(move |Json(request): Json<Value>| {
        let count = count.clone();
        async move {
            // Only the tool-free call after a batch that failed entirely is
            // buffered; every other chat request streams.
            if request["stream"] != true {
                assert_eq!(request["tool_choice"], "none");
                let last = request["messages"].as_array().unwrap().last().unwrap();
                assert_eq!(last["role"], "tool");
                return Json(json!({"model":"fixture","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"That page could not be read."}}],"usage":{"prompt_tokens":3,"completion_tokens":2}})).into_response();
            }
            struct Active(Arc<AtomicUsize>);
            impl Drop for Active { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); } }
            count.fetch_add(1, Ordering::SeqCst);
            let guard = Active(count);
            let prompt = request["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
            let tool = prompt == "tool";
            let frames = if tool {
                vec![json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"web_fetch","arguments":"{\"url\":\"https://example.com/denied\"}"}}]},"finish_reason":"tool_calls"}]}).to_string(), "[DONE]".into()]
            } else {
                vec![
                    json!({"model":"fixture","choices":[{"index":0,"delta":{"content":"hello "},"finish_reason":null}]}).to_string(),
                    if prompt == "malformed" { "SECRET malformed upstream".into() } else { json!({"choices":[{"index":0,"delta":{"content":"world"},"finish_reason":"stop"}]}).to_string() },
                    json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}).to_string(),
                    "[DONE]".into(),
                ]
            };
            let stream = futures_util::stream::unfold((0, frames, prompt, guard), |(index, frames, prompt, guard)| async move {
                if index >= frames.len() { return None; }
                if index == 1 { tokio::time::sleep(Duration::from_millis(match prompt.as_str() { "slow" => 30_000, "paced" => 1_000, _ => 150 })).await; }
                Some((Ok::<_, std::convert::Infallible>(Event::default().data(&frames[index])), (index+1, frames, prompt, guard)))
            });
            Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_millis(20))).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let base = format!("http://{}/v1", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, active, task)
}
async fn start(s: &TestServer, id: &str, message: &str) -> reqwest::Response {
    s.req(reqwest::Method::POST, "/api/chat")
        .header("Accept", "text/event-stream")
        .json(&json!({"message":message,"session_id":id}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn multiple_streamed_tools_then_buffered_synthesis_keep_the_total_call_budget() {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_counter = reads.clone();
    let fixture = common::ollama_router()
        .route(
            "/docs/{page}",
            axum::routing::get(move || {
                let reads = read_counter.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "STREAMED_SOURCE")
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            axum::routing::post(|Json(request): Json<Value>| async move {
                let messages = request["messages"].as_array().unwrap();
                let finished = messages.last().unwrap()["role"] == "tool";
                if finished {
                    assert_eq!(
                        request["stream"], false,
                        "final synthesis must avoid stale tool calls from local SSE servers"
                    );
                    assert_eq!(request["tool_choice"], "none");
                    assert!(request.get("tools").is_none());
                    let results: Vec<_> = messages.iter().filter(|m| m["role"] == "tool").collect();
                    assert_eq!(results.len(), 2);
                    for (i, result) in results.iter().enumerate() {
                        assert_eq!(result["tool_call_id"], format!("call_{i}"));
                        assert!(result["content"].as_str().unwrap().contains("STREAMED_SOURCE"));
                    }
                    return Json(ok_reply("Both streamed reads succeeded.", 10, 2)).into_response();
                }
                assert_eq!(request["stream"], true);
                let delta = {
                    assert_eq!(request["parallel_tool_calls"], true);
                    let over = messages.last().unwrap()["content"] == "overlimit";
                    let calls:Vec<_>=(0..if over {4}else{2}).map(|index|json!({"index":index,"id":format!("call_{index}"),"type":"function","function":{"name":"web_fetch","arguments":json!({"url":format!("http://docs.example/docs/{index}")}).to_string()}})).collect(); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                    json!({"tool_calls":calls})
                };
                let frames = vec![
                    json!({"choices":[{"index":0,"delta":delta,"finish_reason":"tool_calls"}]}).to_string(),
                    json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}).to_string(),
                    "[DONE]".into(),
                ];
                Sse::new(futures_util::stream::iter(frames.into_iter().map(|frame| {
                    Ok::<_, std::convert::Infallible>(Event::default().data(frame))
                })))
                .into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = spawn_server(
        &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
        ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..ServerOptions::default().with("web.chat_tool_calls", "2")
        },
    )
    .await;
    // Exact rules: these reads are named by policy, so no confirmation pauses
    // the turn (#901 step 4; composed reads are covered below).
    let exact: Vec<String> = (0..4)
        .map(|index| format!("http://docs.example/docs/{index}")) // DevSkim: ignore DS137138 because this synthetic URL is test text resolved only to a loopback fixture or never fetched.
        .collect(); // DevSkim: ignore DS137138 because these synthetic grants resolve only to the loopback fixture.
    assert_eq!(s.post_json("/api/web/allow", json!({"urls": exact})).await.0, 200);
    for (message, expected) in [("compare", "done"), ("overlimit", "error")] {
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let id = session["session_id"].as_str().unwrap();
        let response = start(&s, id, message).await.text().await.unwrap();
        let events = events(&response);
        let last = events.last().unwrap();
        assert_eq!(last["type"], expected, "{response}");
        if expected == "done" {
            assert_eq!(last["data"]["reply"], "Both streamed reads succeeded.");
            assert_eq!(last["data"]["web_tools"].as_array().unwrap().len(), 2);
            assert_eq!(s.state.store.for_owner("local").get(id).unwrap().messages.len(), 2);
        } else {
            assert!(response.contains("WEB_TOOL_LIMIT"), "{response}");
            assert!(s.state.store.for_owner("local").get(id).unwrap().messages.is_empty());
        }
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "over-budget batches cannot execute a valid prefix"
        );
        assert!(!s.state.generation_gate.is_held());
    }
    s.state.audit.flush_async().await;
    let ledger = std::fs::read_to_string(s.home.join("logs/spend.jsonl")).unwrap();
    let rows: Vec<Value> = ledger.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["outcome"], "failed_after_billing");
    assert_eq!(rows[1]["input_tokens"], 10);
    assert_eq!(rows[1]["output_tokens"], 2);
    assert_eq!(rows[1]["usage_missing"], false);
    task.abort();
}
/// A docs site whose `/docs/{page}` reads are counted, and a model that
/// streams one `web_fetch` of a URL only the `http://docs.example/docs/*`
/// subtree rule permits (composed: no search lists it, no exact rule names
/// it), then answers once it holds the tool result. The rule is installed.
async fn composed_fetch_server() -> (Arc<TestServer>, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_counter = reads.clone();
    let fixture = common::ollama_router()
        .route(
            "/docs/{page}",
            axum::routing::get(move || {
                let reads = read_counter.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/plain")], "COMPOSED_SOURCE")
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            axum::routing::post(|Json(request): Json<Value>| async move {
                let messages = request["messages"].as_array().unwrap();
                if messages.last().unwrap()["role"] == "tool" {
                    return Json(ok_reply("Answered.", 10, 2)).into_response();
                }
                let call = json!({"index":0,"id":"call_0","type":"function","function":{"name":"web_fetch","arguments":json!({"url":"http://docs.example/docs/page?d=private-memo"}).to_string()}}); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
                let body = json!({"choices":[{"index":0,"delta":{"tool_calls":[call]},"finish_reason":"tool_calls"}]}).to_string();
                if request["stream"] == true {
                    let frames = vec![body, json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}).to_string(), "[DONE]".into()];
                    Sse::new(futures_util::stream::iter(frames.into_iter().map(|frame| {
                        Ok::<_, std::convert::Infallible>(Event::default().data(frame))
                    })))
                    .into_response()
                } else {
                    let call = json!({"id":"call_0","type":"function","function":call["function"]});
                    Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"","tool_calls":[call]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = Arc::new(
        spawn_server(
            &format!("http://{address}/v1"), // DevSkim: ignore DS137138 because this test-only model has no credentials and binds only to loopback.
            ServerOptions {
                web_resolve: Some(("docs.example".into(), address)),
                ..ServerOptions::default()
            },
        )
        .await,
    );
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"})) // DevSkim: ignore DS137138 because this synthetic grant resolves only to the loopback fixture.
            .await
            .0,
        200
    );
    (s, reads, task)
}

/// Reads `response` until its `confirm_fetch` event and returns that event.
async fn confirm_question(response: &mut reqwest::Response, text: &mut String) -> Value {
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(20), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        text.push_str(std::str::from_utf8(&chunk).unwrap());
        if let Some(event) = events(text).into_iter().find(|event| event["type"] == "confirm_fetch") {
            return event;
        }
    }
}

/// The `outcome` of every `web_fetch_provenance` audit line, in order.
async fn provenance_outcomes(s: &TestServer) -> Vec<String> {
    s.state.audit.flush_async().await;
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    audit
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == "web_fetch_provenance")
        .map(|row| row["outcome"].as_str().unwrap().to_string())
        .collect()
}

/// #901 step 4: a URL only a subtree rule permits, that no search listed and
/// the operator did not write, pauses a live administrator turn on a
/// `confirm_fetch` event. Approve reads it once; decline refuses it with
/// `WEB_FETCH_COMPOSED`; a non-streaming turn cannot be asked and is refused.
#[tokio::test]
async fn a_composed_fetch_waits_for_the_operator_and_reads_only_on_approval() {
    let (s, reads, task) = composed_fetch_server().await;
    // Decline first, so the read count shows the declined read never ran.
    for approve in [false, true] {
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let id = session["session_id"].as_str().unwrap().to_string();
        let mut response = start(&s, &id, "read the docs").await;
        let mut text = String::new();
        let question = confirm_question(&mut response, &mut text).await;
        assert_eq!(question["url"], "http://docs.example/docs/page?d=private-memo"); // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing is read before the answer");
        let answer = json!({"id": question["id"], "approve": approve});
        assert_eq!(s.post_json("/api/chat/fetch-confirm", answer.clone()).await.0, 200);
        assert_eq!(
            s.post_json("/api/chat/fetch-confirm", answer).await.0,
            404,
            "answered once"
        );
        text.push_str(&response.text().await.unwrap());
        let last = events(&text).pop().unwrap();
        assert_eq!(last["type"], "done", "{text}");
        let fetch = &last["data"]["web_tools"][0];
        if approve {
            assert_eq!(fetch["ok"], true, "{fetch}");
            assert_eq!(reads.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(fetch["code"], "WEB_FETCH_COMPOSED", "{fetch}");
            assert_eq!(reads.load(Ordering::SeqCst), 0, "a declined read never runs");
        }
    }
    // A JSON (non-streaming) turn has no one to ask: refused, not read.
    let (status, body) = s.post_json("/api/chat", json!({"message":"read the docs"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["web_tools"][0]["code"], "WEB_FETCH_COMPOSED", "{body}");
    // Naming the URL in the message makes it given: read without a question.
    let (status, body) = s
        .post_json(
            "/api/chat",
            json!({"message":"read http://docs.example/docs/page?d=private-memo"}), // DevSkim: ignore DS137138 because this synthetic URL is test text resolved only to a loopback fixture or never fetched.
        ) // DevSkim: ignore DS137138 because this synthetic URL resolves only to the loopback fixture.
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["web_tools"][0]["ok"], true, "{body}");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    s.state.audit.flush_async().await;
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    let outcomes: Vec<String> = audit
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["event"] == "web_fetch_provenance")
        .map(|row| row["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(outcomes, ["asked", "denied", "asked", "approved", "refused"]);
    assert!(
        !audit.contains("private-memo"),
        "audit names the origin, never the path or query"
    );
    task.abort();
}

/// #772 provenance matrix: a session goal (`/goal`) does not make a turn
/// detached. A typed streamed turn under the goal still pauses for the
/// operator; a streamed `/loop` turn toward the same goal is refused at once,
/// with no question, and nothing is read.
#[tokio::test]
async fn a_composed_fetch_in_a_streamed_loop_turn_is_refused_without_a_question() {
    let (s, reads, task) = composed_fetch_server().await;
    let (_, session) = s.post_json("/api/sessions", json!({})).await;
    let id = session["session_id"].as_str().unwrap().to_string();
    let (status, goal) = s
        .post_json(&format!("/api/sessions/{id}/goal"), json!({"goal":"read the docs"}))
        .await;
    assert_eq!(status, 200, "{goal}");
    let mut typed = start(&s, &id, "read the docs").await;
    let mut text = String::new();
    let question = confirm_question(&mut typed, &mut text).await;
    let answer = json!({"id": question["id"], "approve": false});
    assert_eq!(s.post_json("/api/chat/fetch-confirm", answer).await.0, 200);
    text.push_str(&typed.text().await.unwrap());
    assert_eq!(events(&text).pop().unwrap()["type"], "done", "{text}");
    let response = s
        .req(reqwest::Method::POST, "/api/chat")
        .header("Accept", "text/event-stream")
        .json(&json!({"message":"read the docs","session_id":id,"loop":true}))
        .send()
        .await
        .unwrap();
    let text = tokio::time::timeout(Duration::from_secs(20), response.text())
        .await
        .expect("a loop turn never waits for an answer")
        .unwrap();
    let all = events(&text);
    assert!(
        all.iter().all(|event| event["type"] != "confirm_fetch"),
        "a loop turn asks nobody: {text}"
    );
    let last = all.last().unwrap();
    assert_eq!(last["type"], "done", "{text}");
    assert_eq!(last["data"]["web_tools"][0]["code"], "WEB_FETCH_COMPOSED", "{text}");
    assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing was read");
    assert_eq!(provenance_outcomes(&s).await, ["asked", "denied", "refused"]);
    task.abort();
}

/// #772 provenance matrix: an approval given after the URL policy or the web
/// switch changed is stale, and nothing is read. Blocking the URL while the
/// question is open makes the approved read `WEB_POLICY_CHANGED`; turning web
/// off also ends the turn, unsaved, with `WEB_DISABLED`.
#[tokio::test]
async fn a_policy_change_while_a_composed_fetch_waits_reads_nothing() {
    let (s, reads, task) = composed_fetch_server().await;
    // Web off first: a URL still blocked would never be asked about.
    for change in ["web_off", "block"] {
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let id = session["session_id"].as_str().unwrap().to_string();
        let mut response = start(&s, &id, "read the docs").await;
        let mut text = String::new();
        let question = confirm_question(&mut response, &mut text).await;
        let (status, body) = match change {
            "block" => {
                s.post_json(
                    "/api/web/block",
                    json!({"urls":["http://docs.example/docs/*"]}), // DevSkim: ignore DS137138 because this synthetic block rule names only the loopback fixture.
                )
                .await
            }
            _ => s.post_json("/api/web", json!({"enabled": false})).await,
        };
        assert_eq!(status, 200, "{change}: {body}");
        let answer = json!({"id": question["id"], "approve": true});
        assert_eq!(s.post_json("/api/chat/fetch-confirm", answer).await.0, 200, "{change}");
        text.push_str(&response.text().await.unwrap());
        let last = events(&text).pop().unwrap();
        if change == "web_off" {
            // The refused read is the call's result, and the turn's next
            // evidence check then ends the whole turn: nothing is saved.
            assert_eq!(last["type"], "error", "{text}");
            assert_eq!(last["error"]["detail"]["code"], "WEB_DISABLED", "{text}");
            let (_, saved) = s.get_json(&format!("/api/sessions/{id}")).await;
            assert!(!saved.to_string().contains("Answered."), "{saved}");
        } else {
            assert_eq!(last["type"], "done", "{text}");
            assert_eq!(last["data"]["web_tools"][0]["code"], "WEB_POLICY_CHANGED", "{text}");
        }
        assert_eq!(reads.load(Ordering::SeqCst), 0, "{change}: nothing was read");
        if change == "web_off" {
            assert_eq!(s.post_json("/api/web", json!({"enabled": true})).await.0, 200);
        }
    }
    assert_eq!(
        provenance_outcomes(&s).await,
        ["asked", "policy_changed", "asked", "policy_changed"]
    );
    task.abort();
}

fn events(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
async fn released(s: &TestServer, active: &AtomicUsize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while s.state.generation_gate.is_held() || active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("disconnect must stop the model and release the gate");
}

#[tokio::test]
async fn incremental_delivery_commits_only_completed_exchange_with_and_without_tools() {
    let (url, active, task) = model().await;
    let s = spawn_server(&url, ServerOptions::default()).await;
    for web in [true, false] {
        s.post_json("/api/web", json!({"enabled":web})).await;
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let id = session["session_id"].as_str().unwrap();
        let mut response = start(&s, id, "normal").await;
        assert!(response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        let first = tokio::time::timeout(Duration::from_secs(2), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut text = String::from_utf8(first.to_vec()).unwrap();
        assert!(text.contains("hello "), "first delta: {text}");
        assert!(s.state.store.for_owner("local").get(id).unwrap().messages.is_empty());
        while let Some(chunk) = response.chunk().await.unwrap() {
            text.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        let events = events(&text);
        let done = events.last().unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(done["data"]["reply"], "hello world");
        assert_eq!(done["data"]["usage"]["prompt_tokens"], 3);
        assert_eq!(done["data"]["tally"]["total"], 5);
        assert_eq!(s.state.store.for_owner("local").get(id).unwrap().messages.len(), 2);
        released(&s, &active).await;
    }
    task.abort();
}

/// A plain streamed turn re-reads the account before each chunk, as web and
/// log-tool chat do: once the account is disabled mid-reply, no further text
/// reaches that sign-in, the turn ends `AUTH_REQUIRED` and nothing is stored.
#[tokio::test]
async fn a_plain_stream_stops_at_the_next_chunk_once_the_account_changes() {
    let (url, active, task) = model().await;
    let s = spawn_server(&url, ServerOptions::default().with("auth.enabled", "true")).await;
    let auth = s.state.auth.as_ref().unwrap();
    let password = cgagentharness::common::random_hex(24);
    auth.create_user("alice", &password, "operator").unwrap();
    let owner = auth.get_user("alice").unwrap().user_id;
    let login = s
        .client
        .post(s.url("/api/auth/login"))
        .json(&json!({"username":"alice","password":password}))
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
    // Web off: the plain arm, not web chat's own per-chunk evidence check.
    s.state.settings.lock().unwrap().web_enabled = false;
    let session: Value = s
        .req(reqwest::Method::POST, "/api/sessions")
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = session["session_id"].as_str().unwrap();
    let mut response = s
        .req(reqwest::Method::POST, "/api/chat")
        .header("cookie", &cookie)
        .header("Accept", "text/event-stream")
        .json(&json!({"message":"paced","session_id":id}))
        .send()
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut text = String::from_utf8(first.to_vec()).unwrap();
    assert!(text.contains("hello "), "first delta: {text}");
    // The fixture holds its next chunk for a second.
    auth.disable_user("alice").unwrap();
    while let Some(chunk) = response.chunk().await.unwrap() {
        text.push_str(std::str::from_utf8(&chunk).unwrap());
    }
    assert!(
        !text.contains("world"),
        "text streamed after the account changed: {text}"
    );
    let events = events(&text);
    let last = events.last().unwrap();
    assert_eq!(last["type"], "error", "{text}");
    assert_eq!(last["status"], 401, "{text}");
    assert_eq!(last["error"]["detail"]["code"], "AUTH_REQUIRED", "{text}");
    assert!(s.state.store.for_owner(&owner).get(id).unwrap().messages.is_empty());
    let audit = s.audit_log();
    assert!(
        audit
            .lines()
            .any(|l| l.contains("\"chat_reply_withheld\"") && l.contains("\"web\":false")),
        "{audit}"
    );
    released(&s, &active).await;
    task.abort();
}

#[tokio::test]
async fn disconnect_and_explicit_cancel_stop_upstream_without_saving_partial_text() {
    let (url, active, task) = model().await;
    let s = spawn_server(&url, ServerOptions::default()).await;
    for web in [true, false] {
        s.post_json("/api/web", json!({"enabled":web})).await;
        for explicit in [true, false] {
            let (_, session) = s.post_json("/api/sessions", json!({})).await;
            let id = session["session_id"].as_str().unwrap();
            let mut response = start(&s, id, "slow").await;
            response.chunk().await.unwrap().unwrap();
            assert_eq!(s.post_json("/api/chat", json!({"message":"busy"})).await.0, 409);
            if explicit {
                assert_eq!(s.post_json("/api/chat/cancel", json!({})).await.0, 200);
                let text = response.text().await.unwrap();
                assert!(text.contains("\"type\":\"error\""));
            } else {
                drop(response);
            }
            released(&s, &active).await;
            assert!(s.state.store.for_owner("local").get(id).unwrap().messages.is_empty());
        }
    }
    task.abort();
}

#[tokio::test]
async fn streamed_tool_calls_keep_url_policy_and_malformed_output_never_commits_or_echoes() {
    let (url, active, task) = model().await;
    let s = spawn_server(&url, ServerOptions::default()).await;
    for message in ["tool", "malformed"] {
        let (_, session) = s.post_json("/api/sessions", json!({})).await;
        let id = session["session_id"].as_str().unwrap();
        let text = start(&s, id, message).await.text().await.unwrap();
        assert!(!text.contains("SECRET"));
        let events = events(&text);
        let last = events.last().unwrap();
        if message == "tool" {
            // The denied read is that call's result; the model then answers
            // without tools and the refusal stays on the turn's events.
            assert_eq!(last["type"], "done");
            assert_eq!(last["data"]["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY");
            assert_eq!(last["data"]["web_tools"][0]["ok"], false);
            assert_eq!(last["data"]["reply"], "That page could not be read.");
        } else {
            assert_eq!(last["type"], "error");
            assert!(s.state.store.for_owner("local").get(id).unwrap().messages.is_empty());
        }
        released(&s, &active).await;
    }
    task.abort();
}

#[tokio::test]
async fn completed_call_cannot_orphan_another_calls_cancellation() {
    use cgagentharness::llm::{
        openai_chat::{ChatClient, ChatMessage},
        openai_stream::Output,
    };
    let (url, active, task) = model().await;
    let client = Arc::new(ChatClient::new(&url, "fixture", 5.0, "", None).unwrap());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let slow = {
        let client = client.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            client
                .chat_stream(
                    "",
                    &[ChatMessage {
                        role: "user".into(),
                        content: "slow".into(),
                    }],
                    None,
                    50,
                    0.0,
                    Some(Output {
                        sender: &sender,
                        validate: &|| Ok(()),
                    }),
                )
                .await
        })
    };
    receiver.recv().await.unwrap();
    client
        .chat_stream(
            "",
            &[ChatMessage {
                role: "user".into(),
                content: "normal".into(),
            }],
            None,
            50,
            0.0,
            Some(Output {
                sender: &sender,
                validate: &|| Ok(()),
            }),
        )
        .await
        .unwrap();
    client.abort_in_flight();
    assert!(tokio::time::timeout(Duration::from_secs(1), slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err()
        .details
        .to_string()
        .contains("cancelled"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
}
