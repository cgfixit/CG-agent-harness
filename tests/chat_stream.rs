//! Streaming uses the same guards, tools, session commit and generation gate as JSON chat.
mod common;
use axum::{
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json, Router,
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
    let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |Json(request): Json<Value>| {
        let count = count.clone();
        async move {
            assert_eq!(request["stream"], true);
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
                if index == 1 { tokio::time::sleep(Duration::from_millis(if prompt == "slow" { 30_000 } else { 150 })).await; }
                Some((Ok::<_, std::convert::Infallible>(Event::default().data(&frames[index])), (index+1, frames, prompt, guard)))
            });
            Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_millis(20)))
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
async fn multiple_streamed_tools_reach_reads_and_keep_the_total_call_budget() {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_counter = reads.clone();
    let fixture = Router::new()
        .route("/docs/{page}", axum::routing::get(move || {
            let reads=read_counter.clone();
            async move { reads.fetch_add(1, Ordering::SeqCst); ([("content-type","text/plain")],"STREAMED_SOURCE") }
        }))
        .route("/v1/chat/completions", axum::routing::post(|Json(request):Json<Value>| async move {
            assert_eq!(request["stream"],true);
            let messages=request["messages"].as_array().unwrap();
            let finished=messages.last().unwrap()["role"]=="tool";
            let delta=if finished {
                let results:Vec<_>=messages.iter().filter(|m|m["role"]=="tool").collect();
                assert_eq!(results.len(),2);
                for (i,result) in results.iter().enumerate() {
                    assert_eq!(result["tool_call_id"],format!("call_{i}"));
                    assert!(result["content"].as_str().unwrap().contains("STREAMED_SOURCE"));
                }
                json!({"content":"Both streamed reads succeeded."})
            } else {
                assert_eq!(request["parallel_tool_calls"],true);
                let over=messages.last().unwrap()["content"]=="overlimit";
                let calls:Vec<_>=(0..if over {4}else{2}).map(|index|json!({"index":index,"id":format!("call_{index}"),"type":"function","function":{"name":"web_fetch","arguments":json!({"url":format!("http://docs.example/docs/{index}")}).to_string()}})).collect();
                json!({"tool_calls":calls})
            };
            let frames = vec![
                json!({"choices":[{"index":0,"delta":delta,"finish_reason":if finished {"stop"} else {"tool_calls"}}]}).to_string(),
                json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}).to_string(),
                "[DONE]".into(),
            ];
            Sse::new(futures_util::stream::iter(frames.into_iter().map(|frame|Ok::<_,std::convert::Infallible>(Event::default().data(frame)))))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let s = spawn_server(
        &format!("http://{address}/v1"),
        ServerOptions {
            web_resolve: Some(("docs.example".into(), address)),
            ..ServerOptions::default().with("web.chat_tool_calls", "3")
        },
    )
    .await;
    assert_eq!(
        s.post_json("/api/web/allow", json!({"url":"http://docs.example/docs/*"}))
            .await
            .0,
        200
    );
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
            assert_eq!(last["type"], "done");
            assert_eq!(last["data"]["web_tools"][0]["code"], "WEB_ALLOWLIST_EMPTY");
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
