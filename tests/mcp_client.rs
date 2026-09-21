//! Phase 4 MCP client: broker gating, stdio/SSE fixtures, SSRF, secret stripping.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use cgagentharness::common::home::Home;
use cgagentharness::common::mcp_policy::{Containment, NetworkPolicy, StdioCapabilities};
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

fn fixture_capabilities() -> StdioCapabilities {
    let mut roots = vec![fixture_script()];
    let python = dunce::canonicalize(python3()).unwrap();
    for prefix in ["/opt/homebrew", "/usr/local", "/Library/Frameworks/Python.framework"] {
        if python.starts_with(prefix) {
            roots.push(prefix.into());
        }
    }
    StdioCapabilities {
        version: 1,
        filesystem: Default::default(),
        read_roots: roots,
        write_roots: vec![],
        network: NetworkPolicy::Deny,
        containment: Containment::ProcessGroup,
        limits: None,
    }
}

// Ordinary runners may lack namespace permission. They must prove refusal;
// the separate required job must execute every confinement assertion.
fn stdio_executed(status: u16, body: &Value) -> bool {
    if code(body) != "HARD_SANDBOX_UNAVAILABLE" {
        return true;
    }
    assert_eq!(status, 503, "{body}");
    assert!(
        std::env::var_os("CGAH_REQUIRE_LINUX_MCP").is_none(),
        "required MCP confinement could not execute: {body}"
    );
    assert!(!cfg!(target_os = "macos"), "native Seatbelt must execute: {body}");
    eprintln!("MCP refused unavailable protection; confinement not exercised on this runner");
    false
}

fn stdio_yaml() -> String {
    format!(
        "\n    - name: fixture\n      transport: stdio\n      capabilities: {}\n      command:\n        - \"{}\"\n        - \"{}\"\n        - --stdio\n      env:\n        HOME: /tmp/not-scratch\n        PATH: /tmp/evil\n        GROK_API_KEY: should-never-reach-child\n        LD_PRELOAD: /tmp/evil.so\n      tools:\n        - echo\n        - env_probe\n        - crash\n        - read_path\n        - fs_probe\n        - network_probe\n",
        serde_json::to_string(&fixture_capabilities()).unwrap(),
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
        Self::boot_inner(servers, extra, None, false).await
    }

    async fn boot_inner(
        servers: &str,
        extra: &[(&str, &str)],
        agent_override: Option<std::collections::BTreeSet<String>>,
        cwd_is_home: bool,
    ) -> Self {
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
        let mut servers_yaml = servers.to_string();
        if cwd_is_home {
            servers_yaml =
                servers_yaml.replacen("tools:", &format!("cwd: \"{}\"\n      tools:", home.root.display()), 1);
        }
        yaml = yaml.replacen("servers: []", &format!("servers:{servers_yaml}"), 1);
        std::fs::write(home.config_path(), &yaml).unwrap();
        let cfg = cgagentharness::common::config::AppConfig::from_str(&yaml, &home.config_path()).unwrap();
        let mut app_opts = AppOptions::new(home);
        app_opts.config = Some(cfg);
        app_opts.api_key = Some("test-api-key-0123456789".into());
        app_opts.shim_exe = Some(PathBuf::from(BIN));
        app_opts.tool_allowlist_override = agent_override;
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

#[test]
fn required_linux_mcp_acceptance_cannot_run_on_another_platform() {
    assert!(cfg!(target_os = "linux") || std::env::var_os("CGAH_REQUIRE_LINUX_MCP").is_none());
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn strict_containment_is_refused_before_the_declared_program_runs() {
    let yaml = stdio_yaml().replace(
        "\"containment\":\"process_group\"",
        "\"containment\":\"strict\",\"limits\":{\"processes\":32,\"memory_mb\":512}",
    );
    let server = McpServer::boot(&yaml, &[]).await;
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"echo","confirm":true}))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(code(&body), "MCP_CONTAINMENT_UNAVAILABLE");
    let (status, inventory) = server.get("/api/mcp").await;
    assert_eq!(status, 200);
    assert_eq!(inventory["servers"][0]["capabilities"]["containment"], "strict");
    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap();
    assert!(audit.contains("MCP_CONTAINMENT_UNAVAILABLE"));
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_roots_allow_inputs_and_scratch_but_deny_secrets_and_symlink_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    // An unmounted host directory differs from bubblewrap's private mount
    // scaffolding under /tmp. Protect a real host canary in its own directory.
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("protected");
    std::fs::write(&outside_file, "protected-host-content").unwrap();
    let input = tmp.path().join("input");
    let output = tmp.path().join("output");
    std::fs::create_dir(&input).unwrap();
    std::fs::create_dir(&output).unwrap();
    std::fs::write(input.join("visible"), "permitted-input").unwrap();
    let secret = tmp.path().join("outside-secret");
    std::fs::write(&secret, "outside-canary").unwrap();
    std::os::unix::fs::symlink(&secret, input.join("escape")).unwrap();
    let mut grants = fixture_capabilities();
    grants.read_roots.push(input.clone());
    grants.write_roots.push(output.clone());
    let yaml = stdio_yaml().replace(
        &serde_json::to_string(&fixture_capabilities()).unwrap(),
        &serde_json::to_string(&grants).unwrap(),
    );
    let server = McpServer::boot(&yaml, &[]).await;
    std::fs::write(server.state.home.env_path(), "synthetic-credential").unwrap();
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"fs_probe","confirm":true,
        "arguments":{"reads":{"allowed":input.join("visible"),"secret":secret,
          "escape":input.join("escape"),"credential":server.state.home.env_path()},
          "writes":{"allowed_write":output.join("created"),"readonly":input.join("forbidden"),
            "outside_write":outside_file,"private_alias":tmp.path().join("forbidden")}}}))
        .await;
    if !stdio_executed(status, &body) {
        return;
    }
    assert_eq!(status, 200, "{body}");
    let mut result: Value = serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let alias = result.as_object_mut().unwrap().remove("private_alias").unwrap();
    assert!(alias == "written" || alias == "denied", "{alias}");
    assert_eq!(
        result,
        json!({"allowed":"permitted-input","secret":"denied","escape":"denied",
        "credential":"denied","allowed_write":"written","readonly":"denied",
        "outside_write":"denied","scratch":"written"})
    );
    assert_eq!(
        std::fs::read_to_string(output.join("created")).unwrap(),
        "fixture-write"
    );
    assert!(!input.join("forbidden").exists());
    assert!(!tmp.path().join("forbidden").exists());
    assert_eq!(std::fs::read_to_string(outside_file).unwrap(), "protected-host-content");
    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap();
    assert!(!audit.contains("permitted-input") && !audit.contains("synthetic-credential"));
}

#[tokio::test]
async fn network_denial_and_explicit_unrestricted_grant_match_an_owned_listener() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    // Positive control: a refusal cannot pass just because the destination is down.
    let control = std::net::TcpStream::connect(addr).unwrap();
    let accepted = listener.accept().unwrap();
    drop((control, accepted));
    for (policy, expected) in [("deny", false), ("unrestricted", true)] {
        let yaml = stdio_yaml().replace("\"network\":\"deny\"", &format!("\"network\":\"{policy}\""));
        let server = McpServer::boot(&yaml, &[]).await;
        let (status, body) = server
            .call(json!({"server":"fixture","tool":"network_probe","confirm":true,
            "arguments":{"port":addr.port()}}))
            .await;
        if !stdio_executed(status, &body) {
            continue;
        }
        assert_eq!(status, 200, "{body}");
        let result: Value = serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["connected"], expected, "{policy}: {body}");
        let audit = std::fs::read_to_string(server.state.audit.path()).unwrap();
        assert!(audit.contains(&format!("\"network\":\"{policy}\"")));
    }
}

fn named_backends() -> &'static [&'static str] {
    &["darwin-seatbelt", "linux-bwrap", "linux-bwrap-fs"]
}

fn confined_backends() -> &'static [&'static str] {
    &["darwin-seatbelt", "linux-bwrap", "linux-bwrap-fs"]
}

fn audit_field(audit: &str, event: &str, field: &str) -> Option<String> {
    for line in audit.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["event"].as_str() == Some(event) {
            if let Some(item) = value.get(field).and_then(Value::as_str) {
                return Some(item.to_string());
            }
        }
    }
    None
}

#[tokio::test]
async fn stdio_echo_requires_confirm_and_broker_allowlist() {
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
    if !stdio_executed(status, &body) {
        return;
    }
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"x\":1"), "{text}");

    let (status, body) = server
        .call(json!({"server":"fixture","tool":"env_probe","confirm":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("GROK_API_KEY"), "{text}");
    assert!(!text.contains("LD_PRELOAD"), "{text}");
    assert!(!text.contains("/tmp/not-scratch"), "{text}");
    assert!(!text.contains("/tmp/evil"), "{text}");
    assert!(text.contains("\"PATH\":\"/usr/bin:/bin\""), "{text}");

    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap_or_default();
    assert!(audit.contains("tool_broker_decision"), "{audit}");
    assert!(audit.contains("mcp:fixture:echo"), "{audit}");
    assert!(audit.contains("mcp_refused"), "{audit}");
    assert!(audit.contains("MCP_CONFIRM_REQUIRED"), "{audit}");
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
    if !stdio_executed(status, &body) {
        return;
    }
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    let audit = std::fs::read_to_string(server.state.audit.path()).unwrap_or_default();
    let backend = audit_field(&audit, "mcp_stdio_spawn", "backend").expect("backend audited");
    eprintln!("mcp stdio backend={backend}");
    assert!(
        named_backends().contains(&backend.as_str()),
        "unnamed backend {backend}: {audit}"
    );
    assert!(confined_backends().contains(&backend.as_str()));
    assert!(!text.contains("LEAKME-MCP-SECRET"), "{text}\n{audit}");
    assert!(text.contains("error"), "{text}");
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
async fn agent_run_allowlist_override_does_not_become_mcp_allowlist() {
    let server = McpServer::boot_inner(
        &stdio_yaml(),
        &[],
        Some(["agent_run".to_string()].into_iter().collect()),
        false,
    )
    .await;
    let mcp_tools = server.state.mcp_tool_allowlist();
    assert!(mcp_tools.contains("mcp:fixture:echo"), "{mcp_tools:?}");
    assert!(!mcp_tools.contains("agent_run"), "{mcp_tools:?}");
    let agent_tools = server.state.agent_run_tool_allowlist();
    assert!(agent_tools.contains("agent_run"), "{agent_tools:?}");
}

#[tokio::test]
async fn stdio_crash_is_fail_closed() {
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"crash","confirm":true}))
        .await;
    if !stdio_executed(status, &body) {
        return;
    }
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
    let denied = std::fs::read_to_string(off.state.audit.path()).unwrap_or_default();
    assert!(denied.contains("mcp_refused"), "{denied}");
    assert!(denied.contains("MCP_SSRF_DENIED"), "{denied}");
    assert!(denied.contains("\"server\":\"local\""), "{denied}");

    let on = McpServer::boot(&yaml, &[("mcp.sse_allow_loopback", "true")]).await;
    let (status, body) = on
        .call(json!({"server":"local","tool":"echo","arguments":{"k":"v"},"confirm":true}))
        .await;
    assert_eq!(status, 200, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"k\":\"v\""), "{text}");
    let sse_audit = std::fs::read_to_string(on.state.audit.path()).unwrap_or_default();
    assert!(sse_audit.contains("mcp_sse_call"), "{sse_audit}");
    assert!(!sse_audit.contains("\"k\":\"v\""), "{sse_audit}");
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

#[tokio::test]
async fn mcp_child_cannot_read_harness_env_and_home_cwd_is_refused() {
    let server = McpServer::boot(&stdio_yaml(), &[]).await;
    let env_path = server.state.home.env_path();
    std::fs::write(&env_path, "CANARY-HARNESS-ENV\n").unwrap();
    let (status, body) = server
        .call(json!({
            "server":"fixture",
            "tool":"read_path",
            "arguments":{"path": env_path.display().to_string()},
            "confirm":true
        }))
        .await;
    if stdio_executed(status, &body) {
        assert_eq!(status, 200, "{body}");
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        let audit = std::fs::read_to_string(server.state.audit.path()).unwrap_or_default();
        let backend = audit_field(&audit, "mcp_stdio_spawn", "backend").expect("backend");
        assert!(confined_backends().contains(&backend.as_str()));
        assert!(!text.contains("CANARY-HARNESS-ENV"), "{text}\n{audit}");
    }

    let refused = McpServer::boot_inner(&stdio_yaml(), &[], None, true).await;
    let (status, body) = refused
        .call(json!({"server":"fixture","tool":"echo","confirm":true}))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(code(&body), "MCP_HOME_REFUSED");
    let refused_audit = std::fs::read_to_string(refused.state.audit.path()).unwrap_or_default();
    assert!(refused_audit.contains("MCP_HOME_REFUSED"), "{refused_audit}");
}

#[tokio::test]
async fn sse_crash_mid_call_is_fail_closed() {
    let (port, mut child) = start_sse_fixture();
    let yaml = format!(
        "\n    - name: local\n      transport: sse\n      url: http://127.0.0.1:{port}/sse\n      tools:\n        - crash\n", // DevSkim: ignore DS162092 DS137138 because this SSRF test uses loopback HTTP on purpose.
    );
    let server = McpServer::boot(&yaml, &[("mcp.sse_allow_loopback", "true")]).await;
    let (status, body) = server
        .call(json!({"server":"local","tool":"crash","confirm":true}))
        .await;
    assert_ne!(status, 200, "{body}");
    assert_ne!(code(&body), "ok");
    let _ = child.kill();
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_drop_kills_process_group_leader() {
    let argv = vec![python3(), fixture_script().display().to_string(), "--stdio".into()];
    let extra = std::collections::BTreeMap::new();
    let spawned = cgagentharness::common::mcp::StdioClient::spawn(
        &argv,
        None,
        &extra,
        65536,
        None,
        &fixture_capabilities(),
        std::path::Path::new(BIN),
        std::time::Duration::from_secs(15),
    )
    .await;
    if let Err(ref e) = spawned {
        if !stdio_executed(503, &json!({"detail":{"code":e.code}})) {
            return;
        }
    }
    let client = spawned.expect("spawn");
    let pid = client.child_pid().expect("child pid");
    assert!(pid > 1);
    drop(client);
    let dead = (0..100).any(|_| {
        if !cgagentharness::common::process::pid_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        false
    });
    assert!(dead, "direct child {pid} still alive after Drop");
}

#[tokio::test]
async fn stdio_unterminated_header_is_refused_before_timeout_and_stderr_is_clipped() {
    let yaml = stdio_yaml().replace(
        "        - crash",
        "        - header_flood\n        - stderr_flood\n        - crash",
    );
    let server = McpServer::boot(&yaml, &[("mcp.timeout_sec", "2")]).await;
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"header_flood","confirm":true}))
        .await;
    if !stdio_executed(status, &body) {
        return;
    }
    assert_eq!(status, 502, "{body}");
    assert_eq!(
        code(&body),
        "MCP_PROTOCOL",
        "unterminated oversized headers must fail before the timeout: {body}"
    );
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"stderr_flood","confirm":true}))
        .await;
    assert_eq!(status, 502, "{body}");
    assert_eq!(code(&body), "MCP_STDIO");
    let message = body.to_string();
    assert_eq!(
        message.matches('界').count(),
        512,
        "diagnostic must retain 512 Unicode characters"
    );
    let (status, body) = server
        .call(json!({"server":"fixture","tool":"echo","confirm":true,"arguments":{"after":"refusal"}}))
        .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn stdio_stderr_is_drained_without_a_log_file_or_blocked_response() {
    let yaml = stdio_yaml().replace("        - crash", "        - stderr_then_echo\n        - crash");
    let server = McpServer::boot(&yaml, &[("mcp.timeout_sec", "2")]).await;
    let (status, body) = server
        .call(
            json!({"server":"fixture","tool":"stderr_then_echo","confirm":true,"arguments":{"marker":"after_stderr"}}),
        )
        .await;
    if !stdio_executed(status, &body) {
        return;
    }
    assert_eq!(status, 200, "{body}");
    let result: Value = serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["echo"]["marker"], "after_stderr");
    assert_eq!(result["stderr_file_exists"], false);
}
