//! Operator-declared MCP client. Unknown servers fail closed. Tools never
//! auto-discover and never appear on `/loop`.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use url::Url;

use crate::common::audit::Audit;
use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::mcp::{call_stdio, mcp_err, namespaced, validate_server_name, validate_tool_name};
use crate::common::mcp_policy::StdioCapabilities;
use crate::common::tool_broker::assert_allowed;
use crate::llm::backend::LOOPBACK_HOSTS;

use super::web_policy::is_public_ip;
use super::web_search::validate_addresses;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Sse,
}

#[derive(Debug, Clone)]
pub struct DeclaredServer {
    pub name: String,
    pub transport: Transport,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    pub tools: BTreeSet<String>,
    pub env: BTreeMap<String, String>,
    pub capabilities: Option<StdioCapabilities>,
}

#[derive(Debug, Clone)]
pub struct McpRuntime {
    pub enabled: bool,
    pub sse_allow_loopback: bool,
    pub timeout: Duration,
    pub max_result_bytes: usize,
    pub servers: Vec<DeclaredServer>,
    pub test_resolve: Option<(String, SocketAddr)>,
}

impl McpRuntime {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        match cfg.get("mcp") {
            None => return Ok(Self::disabled()),
            Some(value) if value.is_mapping() => {}
            Some(_) => return Err(HarnessError::config("mcp must be a YAML mapping")),
        }
        let timeout_sec = cfg.u64_or("mcp.timeout_sec", 15);
        if !(1..=60).contains(&timeout_sec) {
            return Err(HarnessError::config("mcp.timeout_sec must be an integer from 1 to 60"));
        }
        let max_result_bytes = cfg.u64_or("mcp.max_result_bytes", 65_536);
        if !(1024..=262_144).contains(&max_result_bytes) {
            return Err(HarnessError::config(
                "mcp.max_result_bytes must be an integer from 1024 to 262144",
            ));
        }
        Ok(Self {
            enabled: cfg.flag_is_true("mcp.enabled"),
            sse_allow_loopback: cfg.flag_is_true("mcp.sse_allow_loopback"),
            timeout: Duration::from_secs(timeout_sec),
            max_result_bytes: max_result_bytes as usize,
            servers: parse_servers(cfg)?,
            test_resolve: None,
        })
    }

    fn disabled() -> Self {
        Self {
            enabled: false,
            sse_allow_loopback: false,
            timeout: Duration::from_secs(15),
            max_result_bytes: 65_536,
            servers: Vec::new(),
            test_resolve: None,
        }
    }

    pub fn broker_allowlist(&self) -> BTreeSet<String> {
        if !self.enabled {
            return BTreeSet::new();
        }
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().map(|tool| namespaced(&server.name, tool)))
            .collect()
    }

    pub fn status(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "sse_allow_loopback": self.sse_allow_loopback,
            "servers": self.servers.iter().map(|server| json!({
                "name": server.name,
                "transport": match server.transport {
                    Transport::Stdio => "stdio",
                    Transport::Sse => "sse",
                },
                "tools": server.tools.iter().map(|tool| namespaced(&server.name, tool)).collect::<Vec<_>>(),
                "capabilities": server.capabilities,
            })).collect::<Vec<_>>(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        confirm: bool,
        allowlist: &BTreeSet<String>,
        home: &Path,
        audit: &Audit,
        worker_exe: &Path,
    ) -> Result<Value> {
        let refuse = |code: &str, message: String| {
            audit.log(json!({
                "event": "mcp_refused",
                "code": code,
                "server": server,
                "tool": tool,
            }));
            Err(HarnessError::new(code, message))
        };
        if !confirm {
            return refuse(
                "MCP_CONFIRM_REQUIRED",
                "Explicit confirmation is required to call an MCP tool".into(),
            );
        }
        if !self.enabled {
            return refuse("MCP_DISABLED", "mcp.enabled is off".into());
        }
        let declared = match self.servers.iter().find(|item| item.name == server) {
            Some(item) => item,
            None => return refuse("MCP_UNKNOWN_SERVER", format!("mcp server '{server}' is not declared")),
        };
        if !declared.tools.contains(tool) {
            return refuse(
                "MCP_UNKNOWN_TOOL",
                format!("tool '{tool}' is not declared on mcp server '{server}'"),
            );
        }
        let namespaced = namespaced(server, tool);
        if let Err(err) = assert_allowed(&namespaced, &[namespaced.clone()], allowlist, audit) {
            audit.log(json!({
                "event": "mcp_refused",
                "code": err.code,
                "server": server,
                "tool": tool,
            }));
            return Err(err);
        }
        match declared.transport {
            Transport::Stdio => {
                match call_stdio(
                    &declared.argv,
                    declared.cwd.as_deref(),
                    &declared.env,
                    tool,
                    arguments,
                    self.timeout,
                    self.max_result_bytes,
                    Some(home),
                    declared
                        .capabilities
                        .as_ref()
                        .ok_or_else(|| HarnessError::config("MCP stdio capabilities are required"))?,
                    worker_exe,
                )
                .await
                {
                    Ok(outcome) => {
                        audit.log(json!({
                            "event": "mcp_stdio_spawn",
                            "transport": "stdio",
                            "backend": outcome.backend,
                            "probe_reason": outcome.probe_reason,
                            "capabilities": declared.capabilities,
                            "server": server,
                            "tool": tool,
                            "outcome": "ok",
                        }));
                        Ok(outcome.result)
                    }
                    Err(err) => {
                        let event = if err.code == "MCP_HOME_REFUSED" || err.code == "MCP_TIMEOUT" {
                            "mcp_refused"
                        } else {
                            "mcp_stdio_spawn"
                        };
                        audit.log(json!({
                            "event": event,
                            "transport": "stdio",
                            "code": err.code,
                            "server": server,
                            "tool": tool,
                            "outcome": "error",
                            "capabilities": declared.capabilities,
                        }));
                        Err(err)
                    }
                }
            }
            Transport::Sse => {
                let url = match declared.url.as_deref() {
                    Some(url) => url,
                    None => return refuse("MCP_SSE", "sse server missing url".into()),
                };
                match tokio::time::timeout(self.timeout, call_sse(self, url, tool, arguments)).await {
                    Ok(Ok(result)) => {
                        audit.log(json!({
                            "event": "mcp_sse_call",
                            "transport": "sse",
                            "server": server,
                            "tool": tool,
                            "outcome": "ok",
                        }));
                        Ok(result)
                    }
                    Ok(Err(err)) => {
                        let event = if err.code == "MCP_SSRF_DENIED" || err.code == "MCP_TIMEOUT" {
                            "mcp_refused"
                        } else {
                            "mcp_sse_call"
                        };
                        audit.log(json!({
                            "event": event,
                            "transport": "sse",
                            "code": err.code,
                            "server": server,
                            "tool": tool,
                            "outcome": "error",
                        }));
                        Err(err)
                    }
                    Err(_) => {
                        let err = mcp_err("MCP_TIMEOUT", "sse MCP call timed out");
                        audit.log(json!({
                            "event": "mcp_refused",
                            "transport": "sse",
                            "code": "MCP_TIMEOUT",
                            "server": server,
                            "tool": tool,
                            "outcome": "error",
                        }));
                        Err(err)
                    }
                }
            }
        }
    }
}

fn parse_servers(cfg: &AppConfig) -> Result<Vec<DeclaredServer>> {
    match cfg.get("mcp.servers") {
        None => Ok(Vec::new()),
        Some(serde_yaml_ng::Value::Sequence(items)) => {
            let mut out = Vec::with_capacity(items.len());
            let mut seen = BTreeSet::new();
            for item in items {
                let server = parse_server(item)?;
                if !seen.insert(server.name.clone()) {
                    return Err(HarnessError::config(format!(
                        "duplicate mcp server name '{}'",
                        server.name
                    )));
                }
                out.push(server);
            }
            Ok(out)
        }
        Some(_) => Err(HarnessError::config("mcp.servers must be a YAML sequence")),
    }
}

fn parse_server(value: &serde_yaml_ng::Value) -> Result<DeclaredServer> {
    let map = value
        .as_mapping()
        .ok_or_else(|| HarnessError::config("each mcp.servers entry must be a mapping"))?;
    let get = |key: &str| map.get(serde_yaml_ng::Value::String(key.into()));
    let name = validate_server_name(get("name").and_then(|v| v.as_str()).unwrap_or(""))?;
    let transport = match get("transport").and_then(|v| v.as_str()).unwrap_or("") {
        "stdio" => Transport::Stdio,
        "sse" => Transport::Sse,
        _ => {
            return Err(HarnessError::config(format!(
                "mcp server '{name}' transport must be stdio or sse"
            )))
        }
    };
    let tools = match get("tools") {
        Some(serde_yaml_ng::Value::Sequence(items)) => {
            let mut set = BTreeSet::new();
            for item in items {
                set.insert(validate_tool_name(item.as_str().unwrap_or(""))?);
            }
            set
        }
        None => BTreeSet::new(),
        Some(_) => {
            return Err(HarnessError::config(format!(
                "mcp server '{name}' tools must be a sequence of strings"
            )))
        }
    };
    let mut env = BTreeMap::new();
    if let Some(serde_yaml_ng::Value::Mapping(entries)) = get("env") {
        for (key, value) in entries {
            let key = key
                .as_str()
                .ok_or_else(|| HarnessError::config(format!("mcp server '{name}' env keys must be strings")))?;
            let value = value
                .as_str()
                .ok_or_else(|| HarnessError::config(format!("mcp server '{name}' env values must be strings")))?;
            env.insert(key.to_string(), value.to_string());
        }
    } else if get("env").is_some() {
        return Err(HarnessError::config(format!(
            "mcp server '{name}' env must be a mapping"
        )));
    }
    match transport {
        Transport::Stdio => {
            let capabilities: StdioCapabilities =
                serde_yaml_ng::from_value(get("capabilities").cloned().ok_or_else(|| {
                    HarnessError::config("MCP stdio requires explicit versioned capabilities; see docs/MCP_CLIENT.md")
                })?)
                .map_err(|_| HarnessError::config("invalid MCP stdio capabilities"))?;
            capabilities.validate()?;
            let argv = match get("command") {
                Some(serde_yaml_ng::Value::Sequence(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_string).ok_or_else(|| {
                            HarnessError::config(format!("mcp server '{name}' command entries must be strings"))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                _ => {
                    return Err(HarnessError::config(format!(
                        "mcp server '{name}' stdio transport requires command as a string sequence"
                    )))
                }
            };
            if argv.is_empty() || !PathBuf::from(&argv[0]).is_absolute() {
                return Err(HarnessError::config(format!(
                    "mcp server '{name}' command must start with an absolute path"
                )));
            }
            let cwd = match get("cwd") {
                Some(value) => {
                    let path = value
                        .as_str()
                        .ok_or_else(|| HarnessError::config("MCP cwd must be an absolute path string"))?;
                    let cwd = PathBuf::from(path);
                    if !cwd.is_absolute() {
                        return Err(HarnessError::config(format!(
                            "mcp server '{name}' cwd must be an absolute path"
                        )));
                    }
                    Some(cwd)
                }
                None => None,
            };
            Ok(DeclaredServer {
                name,
                transport,
                argv,
                cwd,
                url: None,
                tools,
                env,
                capabilities: Some(capabilities),
            })
        }
        Transport::Sse => {
            let url = get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| HarnessError::config(format!("mcp server '{name}' sse transport requires url")))?
                .to_string();
            Ok(DeclaredServer {
                name,
                transport,
                argv: Vec::new(),
                cwd: None,
                url: Some(url),
                tools,
                env,
                capabilities: None,
            })
        }
    }
}

struct RefuseDns;
impl reqwest::dns::Resolve for RefuseDns {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async { Err(std::io::Error::other("unvalidated DNS refused").into()) })
    }
}

async fn pinned_client(runtime: &McpRuntime, target: &Url) -> Result<reqwest::Client> {
    let host = target
        .host_str()
        .ok_or_else(|| mcp_err("MCP_SSE", "sse url missing host"))?
        .trim_matches(['[', ']'])
        .to_string();
    let port = target
        .port_or_known_default()
        .ok_or_else(|| mcp_err("MCP_SSE", "sse url missing port"))?;
    let loopback_host = LOOPBACK_HOSTS.contains(&host.to_lowercase().as_str());
    if loopback_host && !runtime.sse_allow_loopback {
        return Err(mcp_err(
            "MCP_SSRF_DENIED",
            "loopback SSE MCP is disabled (mcp.sse_allow_loopback)",
        ));
    }
    let addresses = if let Some((test_host, addr)) = &runtime.test_resolve {
        if test_host == &host {
            vec![*addr]
        } else {
            lookup_sse_addresses(&host, port).await?
        }
    } else {
        lookup_sse_addresses(&host, port).await?
    };
    if loopback_host {
        if addresses.is_empty() || addresses.len() > 32 || addresses.iter().any(|a| !a.ip().is_loopback()) {
            return Err(mcp_err(
                "MCP_SSRF_DENIED",
                "loopback SSE DNS answer was not loopback-only",
            ));
        }
    } else {
        validate_addresses(&addresses).map_err(|e| mcp_err("MCP_SSRF_DENIED", e.message))?;
        if addresses.iter().any(|a| !is_public_ip(a.ip())) {
            return Err(mcp_err(
                "MCP_SSRF_DENIED",
                "sse DNS answer includes prohibited addresses",
            ));
        }
    }
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(RefuseDns))
        .resolve_to_addrs(&host, &addresses)
        .timeout(runtime.timeout)
        .user_agent("CGagentHarness-mcp/1.0")
        .build()
        .map_err(sse_transport_error)
}

/// Fixed text for a reqwest failure. Its `Display` embeds the request URL,
/// and an SSE URL may carry a token in its query, so the operator-facing
/// error names only the failure kind.
fn sse_transport_error(e: reqwest::Error) -> HarnessError {
    let what = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else if e.is_body() || e.is_decode() {
        "response read failed"
    } else if e.is_builder() {
        "client setup failed"
    } else {
        "request failed"
    };
    mcp_err("MCP_SSE", format!("sse {what}"))
}

async fn lookup_sse_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    tokio::net::lookup_host((host, port))
        .await
        .map(|iter| iter.take(33).collect())
        .map_err(|_| mcp_err("MCP_SSE", "DNS lookup failed"))
}

async fn call_sse(runtime: &McpRuntime, raw_url: &str, tool: &str, arguments: Value) -> Result<Value> {
    let target = Url::parse(raw_url).map_err(|_| mcp_err("MCP_SSE", "sse url is not a valid URL"))?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err(mcp_err("MCP_SSE", "sse url must be http or https"));
    }
    let client = pinned_client(runtime, &target).await?;
    let sse = client
        .get(target.clone())
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(sse_transport_error)?;
    if !sse.status().is_success() {
        return Err(mcp_err("MCP_SSE", format!("sse GET status {}", sse.status())));
    }
    let body = read_sse_handshake(sse, runtime.max_result_bytes).await?;
    let endpoint = sse_endpoint(&target, &body)?;
    jsonrpc_post(
        &client,
        runtime,
        &endpoint,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "cgagentharness", "version": env!("CARGO_PKG_VERSION")},
        }),
    )
    .await?;
    let _ = jsonrpc_post(&client, runtime, &endpoint, "notifications/initialized", json!({})).await;
    jsonrpc_post(
        &client,
        runtime,
        &endpoint,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    )
    .await
}

async fn read_sse_handshake(mut response: reqwest::Response, max: usize) -> Result<String> {
    let mut buf = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(sse_transport_error)?;
        let Some(chunk) = chunk else {
            break;
        };
        buf.extend_from_slice(&chunk);
        if buf.len() > max {
            return Err(mcp_err("MCP_RESULT_TOO_LARGE", "sse handshake exceeds cap"));
        }
        if let Ok(text) = std::str::from_utf8(&buf) {
            if text.contains("event: endpoint") && (text.contains("\n\n") || text.contains("\r\n\r\n")) {
                return Ok(text.to_string());
            }
        }
    }
    Err(mcp_err("MCP_SSE", "sse stream closed before endpoint"))
}

fn sse_endpoint(base: &Url, body: &str) -> Result<Url> {
    let mut event = "";
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if event == "endpoint" {
                let endpoint = base
                    .join(rest.trim())
                    .map_err(|_| mcp_err("MCP_SSE", "sse endpoint is not a valid URL"))?;
                // The pinned client only validated the configured host. An endpoint
                // on another scheme, host, or port (an IP literal skips DNS entirely)
                // would send JSON-RPC to an unvalidated target, so refuse it without
                // echoing either URL.
                if endpoint.origin() != base.origin() {
                    return Err(mcp_err(
                        "MCP_SSE",
                        "sse endpoint must share the configured url's scheme, host, and port",
                    ));
                }
                return Ok(endpoint);
            }
        }
    }
    Err(mcp_err("MCP_SSE", "sse stream missing endpoint event"))
}

/// Read a JSON-RPC POST body under `max` bytes, refusing early on an
/// oversized Content-Length and otherwise as soon as the bytes read pass it.
async fn read_capped_body(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    let too_large = || mcp_err("MCP_RESULT_TOO_LARGE", "sse MCP frame exceeds cap");
    if response.content_length().is_some_and(|len| len > max as u64) {
        return Err(too_large());
    }
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(sse_transport_error)? {
        if buf.len() + chunk.len() > max {
            return Err(too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn jsonrpc_post(
    client: &reqwest::Client,
    runtime: &McpRuntime,
    endpoint: &Url,
    method: &str,
    params: Value,
) -> Result<Value> {
    static SSE_ID: AtomicI64 = AtomicI64::new(1);
    let mut message = json!({"jsonrpc": "2.0", "method": method, "params": params});
    let id = if method == "notifications/initialized" {
        None
    } else {
        let id = SSE_ID.fetch_add(1, Ordering::Relaxed);
        message["id"] = json!(id);
        Some(id)
    };
    let response = client
        .post(endpoint.clone())
        .json(&message)
        .send()
        .await
        .map_err(sse_transport_error)?;
    if method == "notifications/initialized" {
        return Ok(json!({}));
    }
    if !response.status().is_success() {
        return Err(mcp_err("MCP_SSE", format!("sse POST status {}", response.status())));
    }
    let bytes = read_capped_body(response, runtime.max_result_bytes).await?;
    let reply: Value = serde_json::from_slice(&bytes).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()))?;
    if let Some(id) = id {
        if reply.get("id") != Some(&json!(id)) {
            return Err(mcp_err("MCP_PROTOCOL", "sse response id mismatch"));
        }
    }
    if let Some(err) = reply.get("error") {
        return Err(mcp_err(
            "MCP_CALL_FAILED",
            err.get("message").and_then(Value::as_str).unwrap_or("tool call failed"),
        ));
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| mcp_err("MCP_PROTOCOL", "sse response missing result"))
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[tokio::test]
    async fn sse_transport_errors_never_echo_the_url() {
        // An owned listener that accepts and drops the connection fails the
        // request deterministically; a released port could be re-bound by a
        // concurrent test.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // DevSkim: ignore DS162092 because this fixture listener is loopback only.
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                drop(stream);
            }
        });
        let runtime = McpRuntime {
            sse_allow_loopback: true,
            ..McpRuntime::disabled()
        };
        let url = format!("http://127.0.0.1:{port}/sse?token=sse-url-secret-token"); // DevSkim: ignore DS162092 because the request must reach that owned loopback listener.
        let err = call_sse(&runtime, &url, "echo", json!({})).await.unwrap_err();
        server.join().unwrap();
        // Fixed failure messages: the error under test may carry the token.
        assert!(err.code == "MCP_SSE", "unexpected error code");
        assert!(
            err.message.starts_with("sse ") && !err.message.contains("http"),
            "transport error message is not a fixed kind"
        );
        let shown = format!("{err:?}");
        assert!(
            !shown.contains("sse-url-secret-token") && !shown.contains("/sse?"),
            "transport error echoed the SSE URL"
        );
    }

    fn endpoint_event(data: &str) -> String {
        format!("event: endpoint\ndata: {data}\n\n")
    }

    #[test]
    fn sse_endpoint_accepts_same_origin() {
        let base = Url::parse("http://127.0.0.1:8791/sse?token=t").unwrap(); // DevSkim: ignore DS137138 because this is a loopback fixture URL.
        let relative = sse_endpoint(&base, &endpoint_event("/messages?session=1")).unwrap();
        assert_eq!(relative.as_str(), "http://127.0.0.1:8791/messages?session=1"); // DevSkim: ignore DS137138 because this is a loopback fixture URL.
        let absolute = sse_endpoint(&base, &endpoint_event("http://127.0.0.1:8791/m")).unwrap(); // DevSkim: ignore DS137138 because this is a loopback fixture URL.
        assert_eq!(absolute.path(), "/m");
        let https = Url::parse("https://mcp.example.org/sse").unwrap();
        let default_port = sse_endpoint(&https, &endpoint_event("https://MCP.example.org:443/m")).unwrap();
        assert_eq!(default_port.host_str(), Some("mcp.example.org"));
    }

    #[test]
    fn sse_endpoint_refuses_cross_origin_without_echoing() {
        let base = Url::parse("http://127.0.0.1:8791/sse").unwrap(); // DevSkim: ignore DS137138 because this is a loopback fixture URL.
        for data in [
            "http://127.0.0.1:8792/m",       // other port
            "http://169.254.169.254/latest", // other host (metadata IP literal)
            "http://[::1]:8791/m",           // other host (v6 loopback)
            "//evil.example/m",              // scheme-relative other host
            "https://127.0.0.1:8791/m",      // scheme switch
        ] {
            let err = sse_endpoint(&base, &endpoint_event(data)).unwrap_err();
            assert_eq!(err.code, "MCP_SSE", "{data}");
            assert!(
                !err.message.contains("://") && !err.message.contains("169.254") && !err.message.contains("evil"),
                "error echoed a URL for {data}"
            );
        }
        let https = Url::parse("https://mcp.example.org/sse").unwrap();
        assert!(sse_endpoint(&https, &endpoint_event("http://mcp.example.org/m")).is_err());
    }

    /// One-shot loopback HTTP server that answers the first request with `response`.
    fn one_shot_server(response: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // DevSkim: ignore DS162092 because this fixture listener is loopback only.
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut seen = Vec::new();
                let mut buf = [0u8; 4096];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = stream.write_all(&response);
            }
        });
        (port, handle)
    }

    async fn post_to(port: u16, max: usize) -> Result<Value> {
        let runtime = McpRuntime {
            max_result_bytes: max,
            ..McpRuntime::disabled()
        };
        let endpoint = Url::parse(&format!("http://127.0.0.1:{port}/m")).unwrap(); // DevSkim: ignore DS137138 because this is a loopback fixture URL.
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        jsonrpc_post(&client, &runtime, &endpoint, "tools/call", json!({})).await
    }

    #[tokio::test]
    async fn jsonrpc_post_refuses_oversized_content_length_before_reading() {
        // Declares 10 MB but sends nothing more: must refuse on the header alone.
        let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10485760\r\n\r\n".to_vec();
        let (port, server) = one_shot_server(head);
        let err = post_to(port, 1024).await.unwrap_err();
        assert_eq!(err.code, "MCP_RESULT_TOO_LARGE");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn jsonrpc_post_refuses_streamed_body_past_cap() {
        let mut resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".to_vec();
        resp.extend(std::iter::repeat_n(b' ', 4096));
        let (port, server) = one_shot_server(resp);
        let err = post_to(port, 1024).await.unwrap_err();
        assert_eq!(err.code, "MCP_RESULT_TOO_LARGE");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn jsonrpc_post_accepts_body_at_cap() {
        let body = br#"{"jsonrpc":"2.0","id":ID,"result":{}}"#;
        // The id is process-global; accept any id by checking the error kind instead.
        let mut resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        resp.extend_from_slice(body);
        let (port, server) = one_shot_server(resp);
        let err = post_to(port, body.len()).await.unwrap_err();
        // Not refused for size: it reached JSON parsing (the placeholder id is not valid JSON).
        assert_eq!(err.code, "MCP_PROTOCOL");
        server.join().unwrap();
    }

    #[test]
    fn stdio_never_invents_missing_or_malformed_capabilities() {
        let base = "name: fixture\ntransport: stdio\ncommand: [/bin/echo]\ntools: [echo]\n";
        for suffix in [
            "",
            "capabilities: null",
            "capabilities: {}",
            "capabilities: {version: 2, network: deny, containment: process_group}",
            "capabilities: {version: 1, network: [example.org], containment: process_group}",
            "capabilities: {version: 1, network: deny, containment: process_group, read_roots: [/]}",
        ] {
            let input = serde_yaml_ng::from_str(&format!("{base}{suffix}")).unwrap();
            assert!(parse_server(&input).is_err(), "{suffix}");
        }
        let valid = "capabilities: {version: 1, network: deny, containment: process_group}";
        let input = serde_yaml_ng::from_str(&format!("{base}{valid}")).unwrap();
        assert!(parse_server(&input).is_ok());
    }
}
