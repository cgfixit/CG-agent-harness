//! MCP JSON-RPC helper plus stdio child spawn.
//!
//! Stdio children live here so `src/server` never contains `Command::new`.
//! The child environment is constructed, not inherited. Secret names are
//! dropped even if an operator lists them under `env`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
use super::windows_job::JobChild as Child;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
#[cfg(not(windows))]
use tokio::process::{Child, ChildStdin, ChildStdout};
#[cfg(windows)]
type ChildStdin = tokio::fs::File;
#[cfg(windows)]
type ChildStdout = tokio::fs::File;

use super::child_env;
use super::errors::{HarnessError, Result};
use super::mcp_policy::{Containment, StdioCapabilities};
use super::sandbox_wrap::{wrap_mcp_stdio, WrappedStdio};

pub const SECRET_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CGAGENTHARNESS_API_KEY",
    "CGAGENTHARNESS_WEBHOOK_TOKEN",
    "CLAUDE_API_KEY",
    "DEEPAGENT_API_KEY",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GROK_API_KEY",
    "OPENAI_API_KEY",
    "SERPAPI_API_KEY",
    "XAI_API_KEY",
];

/// Dynamic-linker and interpreter hijack keys. Operator `env` must not restore them.
const HIJACK_ENV: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_DYNAMIC_WEAK",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
    "PYTHONHOME",
];

/// A message line may end in CRLF: its terminator is at most two bytes beyond the frame cap.
const LINE_TERMINATOR_BYTES: usize = 2;
const CHILD_CLOSED: &str = "stdio MCP child closed";

pub fn namespaced(server: &str, tool: &str) -> String {
    format!("mcp:{server}:{tool}")
}

pub fn validate_server_name(name: &str) -> Result<String> {
    let ok = name.len() <= 32
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (b == b'-' && i > 0));
    if name.is_empty() || !ok {
        return Err(HarnessError::config(
            "mcp server name must be 1-32 chars of [a-z0-9-] starting with a letter or digit",
        ));
    }
    Ok(name.to_string())
}

pub fn validate_tool_name(name: &str) -> Result<String> {
    let ok = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        return Err(HarnessError::config(
            "mcp tool name must be 1-64 chars of [A-Za-z0-9_-]",
        ));
    }
    Ok(name.to_string())
}

fn blocked_env_key(key: &str) -> bool {
    SECRET_ENV
        .iter()
        .chain(HIJACK_ENV.iter())
        .any(|blocked| blocked.eq_ignore_ascii_case(key))
}

pub fn filter_env(extra: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    extra
        .iter()
        .filter(|(key, _)| !blocked_env_key(key))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub struct StdioClient {
    child: Child,
    session: Session<ChildStdout, ChildStdin>,
    pub backend: &'static str,
    pub probe_reason: String,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: tokio::task::JoinHandle<()>,
    _wrap: WrappedStdio,
    #[cfg(target_os = "linux")]
    owner: Option<tokio::net::UnixStream>,
}

pub struct StdioOutcome {
    pub result: Value,
    pub backend: &'static str,
    pub probe_reason: String,
}

// Preserve at most 512 Unicode characters. Drain excess bytes so stderr cannot
// block a valid response, without storing an unbounded log on disk or in RAM.
const STDERR_BYTES: usize = 512 * 4;
async fn drain_stderr(mut pipe: impl tokio::io::AsyncRead + Unpin, captured: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    loop {
        let Ok(n) = pipe.read(&mut chunk).await else { break };
        if n == 0 {
            break;
        }
        let mut bytes = captured.lock().unwrap_or_else(|p| p.into_inner());
        let keep = n.min(STDERR_BYTES - bytes.len());
        bytes.extend_from_slice(&chunk[..keep]);
    }
}

impl StdioClient {
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn(
        argv: &[String],
        cwd: Option<&Path>,
        extra_env: &BTreeMap<String, String>,
        max_frame: usize,
        home: Option<&Path>,
        capabilities: &StdioCapabilities,
        worker_exe: &Path,
        timeout: Duration,
    ) -> Result<Self> {
        if argv.is_empty() {
            return Err(mcp_err("MCP_STDIO", "stdio command is empty"));
        }
        let program = PathBuf::from(&argv[0]);
        if !program.is_absolute() {
            return Err(mcp_err("MCP_STDIO", "stdio command must be an absolute path"));
        }
        let wrap = wrap_mcp_stdio(argv, cwd, home, capabilities)?;
        let mut cmd = Command::new(&wrap.argv[0]);
        cmd.args(&wrap.argv[1..]);
        cmd.env_clear();
        let mut env = filter_env(extra_env);
        // After filter_env, so an operator-declared `env` cannot switch
        // telemetry back on for a server granted `network: unrestricted`.
        child_env::apply(&mut env, child_env::Child::McpServer);
        for (key, value) in env {
            cmd.env(key, value);
        }
        #[cfg(not(windows))]
        cmd.env("PATH", "/usr/bin:/bin");
        #[cfg(windows)]
        {
            // Native executables use an absolute path. A fixed OS directory is
            // sufficient for Windows DLL resolution; no operator PATH is inherited.
            let system = super::windows_job::windows_directory()
                .map_err(|_| mcp_err("MCP_STDIO", "Windows directory is unavailable"))?;
            cmd.env("SystemRoot", &system);
            cmd.env("PATH", system.join("System32"));
        }
        cmd.env("LANG", "C");
        cmd.env("LC_ALL", "C");
        let scratch = wrap.scratch.display().to_string();
        for key in ["HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP"] {
            cmd.env(key, &scratch);
        }
        cmd.current_dir(&wrap.child_cwd);
        #[cfg(target_os = "linux")]
        let supervisor = if capabilities.containment == Containment::Strict {
            Some(super::mcp_worker::prepare(
                &mut cmd,
                worker_exe,
                timeout,
                capabilities
                    .limits
                    .ok_or_else(|| mcp_err("MCP_CONTAINMENT_UNAVAILABLE", "strict limits missing"))?,
            )?)
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let _ = (worker_exe, timeout);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        #[cfg(not(windows))]
        let mut child = cmd
            .spawn()
            .map_err(|_| mcp_err("MCP_STDIO", "cannot spawn stdio MCP child"))?;
        #[cfg(windows)]
        let mut child = {
            if capabilities.containment != Containment::JobObject {
                return Err(mcp_err(
                    "MCP_CONTAINMENT_UNAVAILABLE",
                    "Windows MCP requires the explicit job_object exception",
                ));
            }
            let limits = capabilities
                .limits
                .ok_or_else(|| mcp_err("MCP_CONTAINMENT_UNAVAILABLE", "Job Object limits missing"))?;
            Child::spawn(cmd.as_std(), limits.processes, Some(limits.memory_mb)).map_err(|_| {
                mcp_err(
                    "MCP_CONTAINMENT_UNAVAILABLE",
                    "cannot atomically create MCP child in Job Object",
                )
            })?
        };
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let pipe = child
            .stderr
            .take()
            .ok_or_else(|| mcp_err("MCP_STDIO", "stdio stderr missing"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| mcp_err("MCP_STDIO", "stdio stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| mcp_err("MCP_STDIO", "stdio stdout missing"))?;
        #[cfg(not(windows))]
        let mut stdin = stdin;
        #[cfg(windows)]
        let (mut stdin, stdout, pipe) = (
            tokio::fs::File::from_std(stdin),
            tokio::fs::File::from_std(stdout),
            tokio::fs::File::from_std(pipe),
        );
        #[cfg(target_os = "linux")]
        let owner = match supervisor {
            Some(supervisor) => Some(
                tokio::time::timeout(timeout, supervisor.attach(&mut stdin))
                    .await
                    .map_err(|_| {
                        mcp_err(
                            "MCP_CONTAINMENT_UNAVAILABLE",
                            "strict supervisor did not establish ownership",
                        )
                    })??,
            ),
            None => None,
        };
        #[cfg(not(target_os = "linux"))]
        let _ = &mut stdin;
        let backend = if capabilities.containment == Containment::Strict {
            "linux-systemd-bwrap"
        } else {
            wrap.backend
        };
        let stderr_task = tokio::spawn(drain_stderr(pipe, stderr.clone()));
        Ok(Self {
            child,
            session: Session::new(stdout, stdin, max_frame),
            backend,
            probe_reason: wrap.probe_reason.clone(),
            stderr,
            stderr_task,
            _wrap: wrap,
            #[cfg(target_os = "linux")]
            owner,
        })
    }

    pub async fn initialize(&mut self) -> Result<()> {
        let result = self.session.initialize().await;
        self.explain_close(result).await
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let result = self.session.call_tool(name, arguments).await;
        self.explain_close(result).await
    }

    /// A child that closed stdout is explained by its clipped stderr.
    async fn explain_close<T>(&mut self, result: Result<T>) -> Result<T> {
        match result {
            Err(err) if err.code == "MCP_STDIO" && err.message == CHILD_CLOSED => {
                // Normally EOF follows the completed stderr write. Do not wait
                // indefinitely for a descendant that inherited the pipe.
                let _ = tokio::time::timeout(Duration::from_millis(50), &mut self.stderr_task).await;
                let bytes = self.stderr.lock().unwrap_or_else(|p| p.into_inner());
                let snip: String = String::from_utf8_lossy(&bytes).chars().take(512).collect();
                Err(mcp_err("MCP_STDIO", format!("{CHILD_CLOSED}; stderr={snip}")))
            }
            other => other,
        }
    }
}

/// JSON-RPC over the MCP stdio transport: one message per line. serde_json
/// never writes a raw newline, so each serialized message is exactly one line.
struct Session<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_id: i64,
    max_frame: usize,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Session<R, W> {
    fn new(reader: R, writer: W, max_frame: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            max_frame,
        }
    }

    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "cgagentharness", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        if result.get("protocolVersion").is_none() {
            return Err(mcp_err("MCP_PROTOCOL", "initialize missing protocolVersion"));
        }
        self.notify("notifications/initialized", json!({})).await
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let reply = loop {
            let message = self.read().await?;
            let Some(server_method) = message.get("method") else {
                break message;
            };
            // A server notification (progress, logging) or request, not the
            // reply. This client declares no capabilities: it answers ping
            // and refuses any other request rather than leaving it pending.
            if let Some(server_id) = message.get("id") {
                let answer = if server_method == "ping" {
                    json!({"jsonrpc": "2.0", "id": server_id, "result": {}})
                } else {
                    json!({"jsonrpc": "2.0", "id": server_id,
                        "error": {"code": -32601, "message": "method not supported by this client"}})
                };
                self.write(&answer).await?;
            }
        };
        if reply.get("id") != Some(&json!(id)) {
            return Err(mcp_err("MCP_PROTOCOL", "stdio response id mismatch"));
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
            .ok_or_else(|| mcp_err("MCP_PROTOCOL", "stdio response missing result"))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn write(&mut self, message: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(message).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()))?;
        if line.len() > self.max_frame {
            return Err(mcp_err("MCP_RESULT_TOO_LARGE", "outgoing MCP frame exceeds cap"));
        }
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))
    }

    /// The next message, skipping blank lines.
    async fn read(&mut self) -> Result<Value> {
        loop {
            let line = self.read_line().await?;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return serde_json::from_slice(&line).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()));
        }
    }

    /// One line without its `\n` or `\r\n`. Past `max_frame` bytes only a final
    /// `\r` may still arrive, so any other byte is refused as soon as it is read,
    /// without waiting for a newline that may never come. At most
    /// `max_frame + LINE_TERMINATOR_BYTES` bytes are ever held.
    async fn read_line(&mut self) -> Result<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
            if available.is_empty() {
                // End of stream, at a message boundary or partway through one.
                return Err(mcp_err("MCP_STDIO", CHILD_CLOSED));
            }
            let newline = available.iter().position(|&b| b == b'\n');
            let room = self.max_frame + LINE_TERMINATOR_BYTES - line.len();
            let take = newline.unwrap_or(available.len()).min(room);
            line.extend_from_slice(&available[..take]);
            let ended = newline == Some(take);
            self.reader.consume(take + usize::from(ended));
            if line.len() > self.max_frame + 1 || (line.len() == self.max_frame + 1 && line[self.max_frame] != b'\r') {
                return Err(mcp_err("MCP_RESULT_TOO_LARGE", "stdio MCP frame exceeds cap"));
            }
            if ended {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl StdioClient {
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        drop(self.owner.take());
        self.stderr_task.abort();
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                crate::common::process::kill_pid_group(pid);
            }
        }
        let _ = self.child.start_kill();
        for _ in 0..50 {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn call_stdio(
    argv: &[String],
    cwd: Option<&Path>,
    extra_env: &BTreeMap<String, String>,
    tool: &str,
    arguments: Value,
    timeout: Duration,
    max_frame: usize,
    home: Option<&Path>,
    capabilities: &StdioCapabilities,
    worker_exe: &Path,
) -> Result<StdioOutcome> {
    tokio::time::timeout(timeout, async {
        let mut client =
            StdioClient::spawn(argv, cwd, extra_env, max_frame, home, capabilities, worker_exe, timeout).await?;
        let backend = client.backend;
        let probe_reason = client.probe_reason.clone();
        client.initialize().await?;
        let result = client.call_tool(tool, arguments).await?;
        Ok(StdioOutcome {
            result,
            backend,
            probe_reason,
        })
    })
    .await
    .map_err(|_| mcp_err("MCP_TIMEOUT", "stdio MCP call timed out"))?
}

pub fn mcp_err(code: &str, message: impl Into<String>) -> HarnessError {
    HarnessError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities, ServerConfig,
    };
    use rmcp::service::RequestContext;
    use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};

    /// An independent stdio MCP server: rmcp's own newline JSON-RPC codec and
    /// handshake, served over an in-process pipe instead of a child's stdio.
    #[derive(Clone)]
    struct RmcpEcho;

    impl ServerHandler for RmcpEcho {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> std::result::Result<CallToolResponse, ErrorData> {
            // A notification ahead of the reply: the client must skip it.
            let _ = context.peer.notify_tool_list_changed().await;
            let arguments = Value::Object(request.arguments.unwrap_or_default());
            Ok(CallToolResult::success(vec![ContentBlock::text(
                json!({"tool": request.name, "echo": arguments}).to_string(),
            )])
            .into())
        }
    }

    #[tokio::test]
    async fn stdio_session_interoperates_with_an_rmcp_server() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move { RmcpEcho.serve(server).await.expect("rmcp handshake").waiting().await });
        let (read, write) = tokio::io::split(client);
        let mut session = Session::new(read, write, 65_536);
        session.initialize().await.unwrap();
        for text in ["first", "a\nnewline stays inside one JSON line"] {
            let result = session.call_tool("echo", json!({"text": text})).await.unwrap();
            let body: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(body, json!({"tool": "echo", "echo": {"text": text}}));
        }
        drop(session);
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn stdio_session_answers_server_requests_and_bounds_every_line() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(client);
        let mut session = Session::new(read, write, 1024);
        let (server_read, mut server_write) = tokio::io::split(server);
        let fixture = tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let script = [
                "".to_string(),
                r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}"#.to_string(),
                r#"{"jsonrpc":"2.0","id":"s1","method":"ping"}"#.to_string(),
                r#"{"jsonrpc":"2.0","id":"s2","method":"roots/list"}"#.to_string(),
            ];
            server_write
                .write_all((script.join("\r\n") + "\r\n").as_bytes())
                .await
                .unwrap();
            let mut answers = Vec::new();
            for _ in 0..2 {
                answers.push(serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap()).unwrap());
            }
            let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": {"ok": true}});
            server_write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            // Then a line past the cap that never ends, from a server that stays open.
            server_write.write_all(&[b'X'; 4096]).await.unwrap();
            (answers, server_write)
        });
        assert_eq!(session.call_tool("echo", json!({})).await.unwrap(), json!({"ok": true}));
        let (answers, _open) = fixture.await.unwrap();
        assert_eq!(answers[0], json!({"jsonrpc": "2.0", "id": "s1", "result": {}}));
        assert_eq!(
            (&answers[1]["id"], &answers[1]["error"]["code"]),
            (&json!("s2"), &json!(-32601))
        );
        let refused = tokio::time::timeout(Duration::from_secs(5), session.read())
            .await
            .expect("refused at the cap, not at a newline");
        assert_eq!(refused.unwrap_err().code, "MCP_RESULT_TOO_LARGE");
    }

    #[tokio::test]
    async fn stdio_session_fails_closed_on_oversized_partial_or_non_json_lines() {
        let oversized = format!("\"{}\"\n", "x".repeat(1023));
        for (bytes, code) in [
            (oversized.as_bytes(), "MCP_RESULT_TOO_LARGE"),
            (&b"{\"jsonrpc\":"[..], "MCP_STDIO"),
            (&b""[..], "MCP_STDIO"),
            (&b"Content-Length: 2\r\n\r\n{}"[..], "MCP_PROTOCOL"),
        ] {
            let (client, mut server) = tokio::io::duplex(4096);
            server.write_all(bytes).await.unwrap();
            drop(server);
            let (read, write) = tokio::io::split(client);
            let err = Session::new(read, write, 1024).read().await.unwrap_err();
            assert_eq!(err.code, code, "{}", String::from_utf8_lossy(bytes));
        }
        // A reply of exactly the cap is accepted.
        let exact = format!("\"{}\"\r\n", "x".repeat(1022));
        let (client, mut server) = tokio::io::duplex(4096);
        server.write_all(exact.as_bytes()).await.unwrap();
        let (read, write) = tokio::io::split(client);
        assert_eq!(
            Session::new(read, write, 1024).read().await.unwrap(),
            json!("x".repeat(1022))
        );
    }

    /// One byte past the cap can no longer be a frame unless it is the `\r` of a
    /// `\r\n`; refuse it at once rather than wait for more output or a timeout.
    #[tokio::test]
    async fn stdio_session_refuses_one_byte_past_the_cap_without_waiting() {
        for close in [false, true] {
            let (client, mut server) = tokio::io::duplex(4096);
            server.write_all(&[b'X'; 1025]).await.unwrap();
            let open = (!close).then_some(server);
            let (read, write) = tokio::io::split(client);
            let refused = tokio::time::timeout(Duration::from_secs(5), Session::new(read, write, 1024).read())
                .await
                .expect("refused at max_frame + 1 bytes, not after one more");
            assert_eq!(refused.unwrap_err().code, "MCP_RESULT_TOO_LARGE", "closed: {close}");
            drop(open);
        }
        // A frame of exactly the cap whose `\r` and `\n` arrive separately still fits.
        let (client, server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let writer = tokio::spawn(async move {
            let mut server = server;
            server
                .write_all(format!("\"{}\"\r", "x".repeat(1022)).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            server.write_all(b"\n").await.unwrap();
            server
        });
        let frame = tokio::time::timeout(Duration::from_secs(5), Session::new(read, write, 1024).read())
            .await
            .expect("a CRLF split across writes still completes the frame");
        assert_eq!(frame.unwrap(), json!("x".repeat(1022)));
        drop(writer.await.unwrap());
    }

    #[test]
    fn namespaced_tool_identity() {
        assert_eq!(namespaced("docs", "search"), "mcp:docs:search");
    }

    #[test]
    fn secret_env_names_are_stripped() {
        let mut extra = BTreeMap::new();
        extra.insert("GROK_API_KEY".into(), "secret".into());
        extra.insert("SAFE_FLAG".into(), "1".into());
        extra.insert("grok_api_key".into(), "also".into());
        extra.insert("LD_PRELOAD".into(), "/tmp/evil.so".into());
        extra.insert("dyld_insert_libraries".into(), "/tmp/evil.dylib".into());
        extra.insert("PYTHONHOME".into(), "/tmp/py".into());
        extra.insert("DO_NOT_TRACK".into(), "0".into());
        let filtered = filter_env(&extra);
        assert_eq!(filtered.get("SAFE_FLAG").map(String::as_str), Some("1"));
        // Not a secret, so the filter passes it through; the opt-out applied
        // after the filter is what wins, exactly as spawn() orders them.
        assert_eq!(filtered.get("DO_NOT_TRACK").map(String::as_str), Some("0"));
        let mut env = filtered.clone();
        child_env::apply(&mut env, child_env::Child::McpServer);
        assert_eq!(env.get("DO_NOT_TRACK").map(String::as_str), Some("1"));
        assert_eq!(env.get("SAFE_FLAG").map(String::as_str), Some("1"));
        assert_eq!(env.len(), filtered.len());
        // A lowercase alias would win on Windows (case-insensitive names, later
        // key applied last); apply() must leave only the canonical spelling.
        let mut aliased = BTreeMap::new();
        aliased.insert("do_not_track".to_string(), "0".to_string());
        let mut env = filter_env(&aliased);
        assert_eq!(env.get("do_not_track").map(String::as_str), Some("0"));
        child_env::apply(&mut env, child_env::Child::McpServer);
        assert_eq!(env.len(), 1);
        assert_eq!(env.get("DO_NOT_TRACK").map(String::as_str), Some("1"));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("GROK_API_KEY")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("LD_PRELOAD")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("DYLD_INSERT_LIBRARIES")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("PYTHONHOME")));
    }
}
