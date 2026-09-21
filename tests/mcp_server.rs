//! Real Streamable HTTP, namespace/auth separation, and dedicated key persistence.
mod common;
use cgagentharness::{
    common::home::Home,
    server::{mcp_keys::KeyStore, mcp_server, mcp_server_config::Settings},
};
use common::*;
use serde_json::{json, Value};
use std::time::Duration;

fn private_home(home: &Home) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = home;
}
async fn fixture(tools: &str, extra: &[(&str, &str)]) -> (TestServer, mcp_server::Listener, KeyStore, String, String) {
    let model = start_mock_model().await;
    let socket = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap(); // DevSkim: ignore DS162092 because fixture listeners are loopback only.
    let port = socket.local_addr().unwrap().port().to_string();
    drop(socket);
    let mut opts = ServerOptions::default()
        .with("mcp.server.enabled", "true")
        .with("mcp.server.port", &port)
        .with("mcp.server.tools", tools)
        .with("structured_memory.enabled", "true");
    for (k, v) in extra {
        opts = opts.with(k, v);
    }
    let s = spawn_server(&model.base_url(), opts).await;
    private_home(&s.state.home);
    let keys = KeyStore::open(&s.state.home, 8).unwrap();
    let (_, alice) = keys.mint("user_alice", "Alice fixture").unwrap();
    let (_, bob) = keys.mint("user_bob", "Bob fixture").unwrap();
    let gateway = mcp_server::start(s.state.clone()).await.unwrap().unwrap();
    (s, gateway, keys, alice, bob)
}
fn url(g: &mcp_server::Listener) -> String {
    format!("http://{}/mcp", g.address) // DevSkim: ignore DS137138 because this is the private loopback gateway fixture.
}
fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
async fn call(g: &mcp_server::Listener, key: &str, method: &str, params: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(url(g))
        .bearer_auth(key)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&rpc(method, params))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    let body = serde_json::from_str(&text).unwrap_or_else(|_| panic!("non-JSON response ({status}): {text}"));
    (status, body)
}
const ALL: &str = "[memory_list_facts, memory_get_fact, memory_search]";
#[tokio::test]
async fn real_protocol_reads_only_key_namespace_and_never_writes() {
    let (s, g, keys, alice, bob) = fixture(ALL, &[]).await;
    let store = s.state.structured_memory.as_ref().unwrap();
    let a = store
        .add_fact("user_alice", "Cedar orchard", "fixture", "gateway positive control")
        .unwrap();
    let b = store
        .add_fact("user_bob", "BIRCH_PRIVATE_CANARY", "fixture", "foreign namespace")
        .unwrap();
    let (status, init) = call(
        &g,
        &alice,
        "initialize",
        json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}),
    )
    .await;
    assert_eq!(status, 200, "{init}");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let (_, list) = call(&g, &alice, "tools/list", json!({})).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 3);
    let (_, read) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_list_facts","arguments":{}}),
    )
    .await;
    assert!(read.to_string().contains("Cedar orchard"), "{read}");
    assert!(!read.to_string().contains("BIRCH_PRIVATE_CANARY"));
    assert!(!read.to_string().contains("user_alice"));
    let (_, foreign) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_get_fact","arguments":{"id":b.id}}),
    )
    .await;
    assert!(foreign.to_string().contains("MCP_FACT_NOT_FOUND"), "{foreign}");
    let (_, own) = call(
        &g,
        &bob,
        "tools/call",
        json!({"name":"memory_get_fact","arguments":{"id":b.id}}),
    )
    .await;
    assert!(own.to_string().contains("BIRCH_PRIVATE_CANARY"));
    let (_, search) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_search","arguments":{"query":"OR *"}}),
    )
    .await;
    assert_eq!(
        search["result"]["structuredContent"]["facts"],
        json!([]),
        "literal search: {search}"
    );
    let (_, found) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_search","arguments":{"query":"cedar"}}),
    )
    .await;
    assert!(found.to_string().contains(&a.id));
    for (name, args) in [
        ("memory_propose", json!({"content":"new fact"})),
        ("memory_list_facts", json!({"owner_id":"user_bob"})),
        ("memory_list_facts", json!({"limit":999999})),
    ] {
        let (_, result) = call(&g, &alice, "tools/call", json!({"name":name,"arguments":args})).await;
        assert_eq!(result["result"]["isError"], true, "{result}");
    }
    assert_eq!(store.counts("user_alice").unwrap(), (1, 0));
    assert_eq!(store.counts("user_bob").unwrap(), (1, 0));
    let principal = keys.authenticate(&alice).unwrap();
    keys.revoke(&principal.key_id).unwrap();
    assert_eq!(call(&g, &alice, "tools/list", json!({})).await.0, 401);
    assert_eq!(call(&g, &bob, "tools/list", json!({})).await.0, 200);
    let audit = std::fs::read_to_string(s.home.join("logs/audit.jsonl")).unwrap();
    for secret in [&alice, &bob, "BIRCH_PRIVATE_CANARY", "Cedar orchard", "OR *"] {
        assert!(!audit.contains(secret));
    }
    assert!(audit.contains("mcp_server_call"));
    assert!(audit.contains("MCP_AUTH_REQUIRED"));
}
#[tokio::test]
async fn feature_off_has_no_listener_or_key_store_and_listener_alone_grants_no_tools() {
    let model = start_mock_model().await;
    let s = spawn_server(
        &model.base_url(),
        ServerOptions::default().with("mcp.server.enabled", "\"true\""),
    )
    .await;
    assert!(mcp_server::start(s.state.clone()).await.unwrap().is_none());
    assert!(!s.home.join("mcp_keys.sqlite3").exists());
    let (_s, g, _keys, alice, _) = fixture("[]", &[]).await;
    let (_, list) = call(&g, &alice, "tools/list", json!({})).await;
    assert_eq!(list["result"]["tools"], json!([]));
    let (_, read) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_list_facts","arguments":{}}),
    )
    .await;
    assert!(read.to_string().contains("MCP_TOOL_DISABLED"));
    let address = g.address;
    drop(g);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}
#[tokio::test]
async fn transport_refuses_console_cookie_wrong_host_origin_forwarding_and_large_bodies() {
    let (s, g, _keys, alice, _) = fixture(
        ALL,
        &[("auth.enabled", "true"), ("mcp.server.max_request_bytes", "1024")],
    )
    .await;
    assert_eq!(call(&g, "", "tools/list", json!({})).await.0, 401);
    assert_eq!(
        call(
            &g,
            "cgamcp_00000000000000000000000000000000.bad",
            "tools/list",
            json!({})
        )
        .await
        .0,
        401
    );
    let client = reqwest::Client::new();
    for (header, value, status) in [
        ("cookie", "session=console_cookie", 401),
        ("host", "attacker.invalid", 403),
        ("origin", "http://127.0.0.1", 403), // DevSkim: ignore DS162092 DS137138 because this adversarial header must be refused even when it claims loopback.
        ("x-forwarded-for", "127.0.0.1", 403), // DevSkim: ignore DS162092 DS137138 because this adversarial header must be refused even when it claims loopback.
        ("forwarded", "for=127.0.0.1", 403), // DevSkim: ignore DS162092 DS137138 because this adversarial header must be refused even when it claims loopback.
    ] {
        let mut request = client
            .post(url(&g))
            .header(header, value)
            .header("accept", "application/json, text/event-stream")
            .json(&rpc("tools/list", json!({})));
        if header != "cookie" {
            request = request.bearer_auth(&alice);
        }
        assert_eq!(request.send().await.unwrap().status().as_u16(), status, "{header}");
    }
    let response = client
        .post(url(&g))
        .bearer_auth(&alice)
        .header("accept", "application/json, text/event-stream")
        .json(&rpc(
            "tools/call",
            json!({"name":"memory_search","arguments":{"query":"x".repeat(4096)}}),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    // A machine bearer never establishes a console account session.
    let response = client
        .post(format!("{}/api/chat", s.base))
        .bearer_auth(&alice)
        .header("x-cyclaw-csrf", &s.csrf)
        .json(&json!({"text":"must not run"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        client.get(url(&g)).bearer_auth(&alice).send().await.unwrap().status(),
        405
    );
    assert_eq!(
        call(&g, &alice, "tools/list", json!({})).await.0,
        200,
        "positive control after refusals"
    );
}
#[tokio::test]
async fn bounded_results_and_rates_preserve_service() {
    let (s, g, _keys, alice, _) = fixture(
        ALL,
        &[
            ("mcp.server.max_result_bytes", "1024"),
            ("mcp.server.requests_per_minute", "2"),
        ],
    )
    .await;
    s.state
        .structured_memory
        .as_ref()
        .unwrap()
        .add_fact("user_alice", &"cedar ".repeat(150), "fixture", "result limit")
        .unwrap();
    let (_, result) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_list_facts","arguments":{}}),
    )
    .await;
    assert!(result.to_string().contains("MCP_RESULT_LIMIT"));
    assert_eq!(
        call(
            &g,
            &alice,
            "tools/call",
            json!({"name":"memory_search","arguments":{"query":"absent"}})
        )
        .await
        .0,
        200
    );
    assert_eq!(call(&g, &alice, "tools/list", json!({})).await.0, 429);
}
#[test]
fn key_store_hashes_caps_rotates_revokes_and_never_resets_initialized_data() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::at(temp.path().join("home"));
    home.ensure_layout().unwrap();
    private_home(&home);
    let keys = KeyStore::open(&home, 1).unwrap();
    let (info, secret) = keys.mint("user_owner", "fixture").unwrap();
    assert!(keys.authenticate(&secret).is_ok());
    assert!(keys.mint("user_other", "second").is_err());
    let bytes = std::fs::read(home.root.join("mcp_keys.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(secret.split('.').nth(1).unwrap()));
    let second = KeyStore::open(&home, 1).unwrap();
    second.revoke(&info.key_id).unwrap();
    assert!(keys.authenticate(&secret).is_err());
    let (_, replacement) = second.mint("user_new", "rotation").unwrap();
    assert!(keys.authenticate(&replacement).is_ok());
    assert_eq!(keys.list().unwrap().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(home.root.join("mcp_keys.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::remove_file(home.root.join("mcp_keys.sqlite3")).unwrap();
    assert!(KeyStore::open(&home, 1).is_err());
}
#[test]
fn invalid_store_schema_scope_and_config_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let home = Home::at(temp.path().join("home"));
    home.ensure_layout().unwrap();
    private_home(&home);
    let keys = KeyStore::open(&home, 8).unwrap();
    let (_, secret) = keys.mint("user_owner", "fixture").unwrap();
    let conn = rusqlite::Connection::open(home.root.join("mcp_keys.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE mcp_keys SET scopes='memory:write';")
        .unwrap();
    assert!(keys.authenticate(&secret).is_err());
    assert!(KeyStore::open(&home, 8).is_err());
    for (key, value) in [
        ("mcp.server.host", "0.0.0.0"),
        ("mcp.server.host", "192.168.1.2"),
        ("mcp.server.tools", "[memory_propose]"),
        ("mcp.server.max_keys", "33"),
        ("mcp.server.concurrency", "0"),
    ] {
        assert!(
            Settings::load(&config_with(temp.path(), &[(key, value)])).is_err(),
            "{key}"
        );
    }
    conn.execute_batch("PRAGMA user_version=999").unwrap();
    assert!(keys.list().is_err());
}
#[cfg(unix)]
#[test]
fn linked_or_unsafe_key_files_are_refused_without_changing_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let home = Home::at(temp.path().join("home"));
    home.ensure_layout().unwrap();
    private_home(&home);
    let outside = temp.path().join("outside");
    std::fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, home.root.join("mcp_keys.sqlite3")).unwrap();
    assert!(KeyStore::open(&home, 8).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
    std::fs::remove_file(home.root.join("mcp_keys.sqlite3")).unwrap();
    let keys = KeyStore::open(&home, 8).unwrap();
    std::fs::set_permissions(
        home.root.join("mcp_keys.sqlite3"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(keys.list().is_err());
}

#[tokio::test]
async fn slow_body_holds_bounded_permit_times_out_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_s, g, _keys, alice, _) = fixture(
        ALL,
        &[("mcp.server.concurrency", "1"), ("mcp.server.request_timeout_sec", "1")],
    )
    .await;
    let mut slow = tokio::net::TcpStream::connect(g.address).await.unwrap();
    let request=format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{{",g.address,alice);
    slow.write_all(request.as_bytes()).await.unwrap();
    // Poll for the occupied permit instead of assuming a fixed scheduling delay.
    tokio::time::timeout(Duration::from_millis(700), async {
        loop {
            if call(&g, &alice, "tools/list", json!({})).await.0 == 429 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), slow.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&response).contains("MCP_TIMEOUT"));
    assert_eq!(
        call(&g, &alice, "tools/list", json!({})).await.0,
        200,
        "timed-out body releases request authority"
    );
}
#[tokio::test]
async fn global_rate_limits_invalid_credentials_and_disabled_store_refuses() {
    let (_s, g, _keys, alice, _) = fixture(
        ALL,
        &[
            ("mcp.server.global_requests_per_minute", "2"),
            ("structured_memory.enabled", "false"),
        ],
    )
    .await;
    let (_, disabled) = call(
        &g,
        &alice,
        "tools/call",
        json!({"name":"memory_list_facts","arguments":{}}),
    )
    .await;
    assert!(disabled.to_string().contains("MCP_MEMORY_DISABLED"));
    assert_eq!(call(&g, "unknown", "tools/list", json!({})).await.0, 401);
    assert_eq!(call(&g, "different unknown", "tools/list", json!({})).await.0, 429);
}

#[test]
fn client_and_server_switches_are_independent_in_fixture_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with(dir.path(), &[("mcp.enabled", "true")]);
    assert!(cfg.flag_is_true("mcp.enabled"));
    assert!(!cfg.flag_is_true("mcp.server.enabled"));
    let cfg = config_with(dir.path(), &[("mcp.server.enabled", "true")]);
    assert!(!cfg.flag_is_true("mcp.enabled"));
    assert!(cfg.flag_is_true("mcp.server.enabled"));
}
#[tokio::test]
async fn current_stateless_protocol_carries_authenticated_owner_on_every_request() {
    let (s, g, _keys, alice, _) = fixture(ALL, &[]).await;
    s.state
        .structured_memory
        .as_ref()
        .unwrap()
        .add_fact(
            "user_alice",
            "Current protocol fixture",
            "fixture",
            "protocol acceptance",
        )
        .unwrap();
    let params = json!({"name":"memory_list_facts","arguments":{},"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"fixture","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}
    }});
    let response = reqwest::Client::new()
        .post(url(&g))
        .bearer_auth(&alice)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "memory_list_facts")
        .json(&rpc("tools/call", params))
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert!(!response.headers().contains_key("mcp-session-id"));
    let text = response.text().await.unwrap();
    assert_eq!(status, 200, "{text}");
    let result: Value = serde_json::from_str(&text).unwrap();
    assert!(result.to_string().contains("Current protocol fixture"), "{result}");
}
