//! Phase 4 MCP client: broker gating, stdio/SSE fixtures, SSRF, secret stripping.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use cgagentharness::common::home::Home;
use cgagentharness::server::state::AppState;
use cgagentharness::server::{build_app, AppOptions};
use common::*;
use serde_json::{json, Value};

fn python3() -> String {
    let out = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "python3 -c failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn fixture_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mcp-fixture.py")
}

fn stdio_yaml() -> String {
    format!(
        "\n    - name: fixture\n      transport: stdio\n      command:\n        - \"{}\"\n        - \"{}\"\n        - --stdio\n      env:\n        HOME: /tmp/not-scratch\n        PATH: /tmp/evil\n        GROK_API_KEY: should-never-reach-child\n      tools:\n        - echo\n        - env_probe\n        - crash\n        - read_path\n",
        python3(),
        fixture_script().display()
    )
}

struct McpServer {
    base: String,
    csrf: String,
    api_key: String,
    client: reqwest::Client,
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
    _task: tokio::task::JoinHandle<()>,
    _model: MockModel,
}

impl McpServer {
    async fn boot(servers: &str, extra: &[(&str, &str)]) -> Self {
        let model = start_mock_model().await;
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("home"));
        home.ensure_layout().unwrap();
        let quoted = format!("\"{}\"", model.base_url());
        let mut pairs: Vec<(String, String)> = vec![
            ("models.local_llm.base_url".into(), quoted),
            ("auth.enabled".into(), "false".into()),
            ("tls.enabled".into(), "false".into()),
            ("memory.enabled".into(), "false".into()),
            ("structured_memory.enabled".into(), "false".into()),
            ("mcp.enabled".into(), "true".into()),
        ];
        for (k, v) in extra {
            pairs.push((k.to_string(), v.to_string()));
        }
        let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        config_with(&home.root, &refs);
        let mut yaml = std::fs::read_to_string(home.config_path()).unwrap();
        yaml = yaml.replacen("servers: []", &format!("servers:{servers}"), 1);
        std::fs::write(home.config_path(), &yaml).unwrap();
        let cfg = cgagentharness::common::config::AppConfig::from_str(&yaml, &home.config_path()).unwrap();
        let mut app_opts = AppOptions::new(home);
        app_opts.config = Some(cfg);
        app_opts.api_key = Some("test-api-key-0123456789".into());
        app_opts.shim_exe = Some(PathBuf::from(BIN));
        let (router, state) = build_app(app_opts).await.unwrap();
        let transport =
            cgagentharness::server::transport::Transport::load(&state.home, &state.cfg, "127.0.0.1").unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
        let addr = listener.local_addr().unwrap();
        let scheme = transport.scheme();
        let task = tokio::spawn(async move {
            transport.serve(listener, router).await.unwrap();
        });
        let csrf = state.csrf_token.clone();
        Self {
            base: format!("{scheme}://127.0.0.1:{}", addr.port()), // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
            csrf,
            api_key: "test-api-key-0123456789".into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            state,
            _tmp: tmp,
            _task: task,
            _model: model,
        }
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = self
            .client
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.api_key)
            .header(CSRF_HEADER, &self.csrf)
            .send()
            .await
            .unwrap();
        (
            resp.status().as_u16(),
            resp.json::<Value>().await.unwrap_or(Value::Null),
        )
    }

    async fn call(&self, body: Value) -> (u16, Value) {
        let resp = self
            .client
            .post(format!("{}/api/mcp/call", self.base))
            .bearer_auth(&self.api_key)
            .header(CSRF_HEADER, &self.csrf)
            .json(&body)
            .send()
            .await
            .unwrap();
        (
            resp.status().as_u16(),
            resp.json::<Value>().await.unwrap_or(Value::Null),
        )
    }
}

fn start_sse_fixture() -> (u16, std::process::Child) {
    let mut child = Command::new(python3())
        .args([
            fixture_script().as_os_str().to_str().unwrap(),
            "--sse",
            "--host",
            "127.0.0.1", // DevSkim: ignore DS162092 because this fixture must bind only to loopback.
            "--port",
            "0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let port = serde_json::from_str::<Value>(&line).unwrap()["port"].as_u64().unwrap() as u16;
    (port, child)
}

#[tokio::test]
async fn shipped_mcp_is_off_and_undeclared_servers_fail_closed() {
    let model = start_mock_model().await;
    let server = spawn_server(&model.base_url(), ServerOptions::default()).await;
    let (status, body) = server.get_json("/api/mcp").await;
    assert_eq!(status, 200);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["sse_allow_loopback"], false);
    assert_eq!(body["servers"].as_array().unwrap().len(), 0);
    let (status, body) = server
        .post_json(
            "/api/mcp/call",
            json!({"server":"fixture","tool":"echo","confirm":true}),
        )
        .await;
    assert_eq!(status, 403);
    assert_eq!(code(&body), "MCP_DISABLED");
}

#[tokio::test]
async fn stdio_echo_requires_confirm_and_broker_allowlist() {
    std::env::set_var("GROK_API_KEY", "should-never-reach-child");
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let (status, body) = server.get("/api/mcp").await;
    assert_eq!(status, 200);
    let tools: Vec<&str> = body["servers"][0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(tools.contains(&"mcp:fixture:echo"), "{tools:?}");

    let (status, body) = server
        .call(json!({"server":"fixture","tool":"echo","arguments":{"x":1}}))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(code(&body), "MCP_CONFIRM_REQUIRED");

    let (status, body) = server
        .call(json!({"server":"fixture","tool":"missing","confirm":true}))
        .await;
    assert_eq!(status, 403);
    assert_eq!(code(&body), "MCP_UNKNOWN_TOOL");

    let (status, body) = server.call(json!({"server":"nope","tool":"echo","confirm":true})).await;
    assert_eq!(status, 403);
    assert_eq!(code(&body), "MCP_UNKNOWN_SERVER");

    let (status, body) = server
        .call(json!({"server":"fixture","tool":"echo","arguments":{"x":1},"confirm":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"x\":1"), "{text}");

    let (status, body) = server
        .call(json!({"server":"fixture","tool":"env_probe","confirm":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("GROK_API_KEY"), "{text}");
    assert!(!text.contains("/tmp/not-scratch"), "{text}");
    assert!(!text.contains("/tmp/evil"), "{text}");
    assert!(text.contains("\"PATH\":\"/usr/bin:/bin\""), "{text}");

    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap_or_default();
    assert!(audit.contains("tool_broker_decision"), "{audit}");
    assert!(audit.contains("mcp:fixture:echo"), "{audit}");
}

#[tokio::test]
async fn stdio_sandbox_denies_host_secret_when_fs_confined() {
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let secret = server._tmp.path().join("host-secret.txt");
    std::fs::write(&secret, "LEAKME-MCP-SECRET").unwrap();
    let (status, body) = server
        .call(json!({
            "server":"fixture",
            "tool":"read_path",
            "arguments":{"path": secret.display().to_string()},
            "confirm":true
        }))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap_or_default();
    assert!(audit.contains("mcp_stdio_spawn"), "{audit}");
    let confined = audit.contains("darwin-seatbelt") || audit.contains("linux-bwrap");
    if confined {
        assert!(!text.contains("LEAKME-MCP-SECRET"), "{text}\n{audit}");
        assert!(text.contains("error"), "{text}");
    } else {
        assert!(
            audit.contains("linux-netns") || audit.contains("windows-stdio"),
            "expected a named residual backend, got {audit}"
        );
    }
}

#[tokio::test]
async fn loop_allowlist_does_not_include_mcp_tools() {
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let loop_tools = server.state.loop_tool_allowlist();
    assert!(
        loop_tools.iter().all(|name| !name.starts_with("mcp:")),
        "{loop_tools:?}"
    );
    let mcp_tools = server.state.mcp_tool_allowlist();
    assert!(mcp_tools.contains("mcp:fixture:echo"), "{mcp_tools:?}");
}

#[tokio::test]
async fn stdio_crash_is_fail_closed() {
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"crash","confirm":true}))
        .await;
    assert_eq!(status, 502, "{body}");
    assert!(
        matches!(code(&body).as_str(), "MCP_STDIO" | "MCP_CALL_FAILED" | "MCP_PROTOCOL"),
        "{}",
        code(&body)
    );
}

#[tokio::test]
async fn sse_loopback_is_denied_until_explicitly_enabled() {
    let (port, mut child) = start_sse_fixture();
    let yaml = format!(
        "\n    - name: local\n      transport: sse\n      url: http://127.0.0.1:{port}/sse\n      tools:\n        - echo\n", // DevSkim: ignore DS162092 DS137138 because this SSRF test uses loopback HTTP on purpose.
    );
    let off = McpServer::boot(&yaml, &[]).await;
    let (status, body) = off
        .call(json!({"server":"local","tool":"echo","arguments":{"k":"v"},"confirm":true}))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(code(&body), "MCP_SSRF_DENIED");

    let on = McpServer::boot(&yaml, &[("mcp.sse_allow_loopback", "true")]).await;
    let (status, body) = on
        .call(json!({"server":"local","tool":"echo","arguments":{"k":"v"},"confirm":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"k\":\"v\""), "{text}");
    let _ = child.kill();
}

#[tokio::test]
async fn sse_private_literal_is_denied() {
    let yaml =
        "\n    - name: bogon\n      transport: sse\n      url: http://10.0.0.1/sse\n      tools:\n        - echo\n"; // DevSkim: ignore DS137138 because this SSRF test must use a non-TLS private URL.
    let server = McpServer::boot(yaml, &[("mcp.sse_allow_loopback", "true")]).await;
    let (status, body) = server
        .call(json!({"server":"bogon","tool":"echo","confirm":true}))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(code(&body), "MCP_SSRF_DENIED");
}
