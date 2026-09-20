//! MCP JSON-RPC helper plus stdio child spawn.
//!
//! Stdio children live here so `src/server` never contains `Command::new`.
//! The child environment is constructed, not inherited. Secret names are
//! dropped even if an operator lists them under `env`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::errors::{HarnessError, Result};
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

const MAX_HEADER_BYTES: usize = 4096;

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
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    max_frame: usize,
    pub backend: &'static str,
    pub probe_reason: String,
    stderr_path: PathBuf,
    _wrap: WrappedStdio,
}

pub struct StdioOutcome {
    pub result: Value,
    pub backend: &'static str,
    pub probe_reason: String,
}

fn stderr_snip(path: &Path) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        // At most four UTF-8 bytes per retained character.
        let _ = file.take(512 * 4).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).chars().take(512).collect()
}

impl StdioClient {
    pub async fn spawn(
        argv: &[String],
        cwd: Option<&Path>,
        extra_env: &BTreeMap<String, String>,
        max_frame: usize,
        home: Option<&Path>,
    ) -> Result<Self> {
        if argv.is_empty() {
            return Err(mcp_err("MCP_STDIO", "stdio command is empty"));
        }
        let program = PathBuf::from(&argv[0]);
        if !program.is_absolute() {
            return Err(mcp_err("MCP_STDIO", "stdio command must be an absolute path"));
        }
        let wrap = wrap_mcp_stdio(argv, cwd, home)?;
        let mut cmd = Command::new(&wrap.argv[0]);
        cmd.args(&wrap.argv[1..]);
        cmd.env_clear();
        for (key, value) in filter_env(extra_env) {
            cmd.env(key, value);
        }
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env("LANG", "C");
        cmd.env("LC_ALL", "C");
        let scratch = wrap.scratch.display().to_string();
        for key in ["HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP"] {
            cmd.env(key, &scratch);
        }
        cmd.current_dir(&wrap.child_cwd);
        let stderr_path = wrap.scratch.join("mcp-stderr.log");
        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| mcp_err("MCP_STDIO", format!("stderr file: {e}")))?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr_file)
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| {
            let snip = stderr_snip(&stderr_path);
            mcp_err("MCP_STDIO", format!("cannot spawn: {e}; stderr={snip}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| mcp_err("MCP_STDIO", "stdio stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| mcp_err("MCP_STDIO", "stdio stdout missing"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            max_frame,
            backend: wrap.backend,
            probe_reason: wrap.probe_reason.clone(),
            stderr_path,
            _wrap: wrap,
        })
    }

    pub async fn initialize(&mut self) -> Result<()> {
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

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let reply = self.read().await?;
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
        let body = serde_json::to_vec(message).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()))?;
        if body.len() > self.max_frame {
            return Err(mcp_err("MCP_RESULT_TOO_LARGE", "outgoing MCP frame exceeds cap"));
        }
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
        self.stdin
            .write_all(&body)
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))
    }

    async fn read(&mut self) -> Result<Value> {
        let mut headers = Vec::new();
        loop {
            let n = (&mut self.stdout)
                .take((MAX_HEADER_BYTES - headers.len() + 1) as u64)
                .read_until(b'\n', &mut headers)
                .await
                .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
            if n == 0 {
                let snip = stderr_snip(&self.stderr_path);
                return Err(mcp_err("MCP_STDIO", format!("stdio MCP child closed; stderr={snip}")));
            }
            if headers.len() > MAX_HEADER_BYTES {
                return Err(mcp_err("MCP_PROTOCOL", "stdio headers exceeded 4 KiB"));
            }
            if headers.windows(4).any(|w| w == b"\r\n\r\n") || headers.windows(2).any(|w| w == b"\n\n") {
                break;
            }
        }
        let text = std::str::from_utf8(&headers).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()))?;
        let mut length = 0usize;
        for line in text.split(['\r', '\n']) {
            if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = rest.trim().parse().unwrap_or(0);
            }
        }
        if length == 0 || length > self.max_frame {
            return Err(mcp_err("MCP_RESULT_TOO_LARGE", "stdio MCP frame exceeds cap"));
        }
        let mut body = vec![0u8; length];
        self.stdout
            .read_exact(&mut body)
            .await
            .map_err(|e| mcp_err("MCP_STDIO", e.to_string()))?;
        serde_json::from_slice(&body).map_err(|e| mcp_err("MCP_PROTOCOL", e.to_string()))
    }
}

impl StdioClient {
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
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
) -> Result<StdioOutcome> {
    tokio::time::timeout(timeout, async {
        let mut client = StdioClient::spawn(argv, cwd, extra_env, max_frame, home).await?;
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
        let filtered = filter_env(&extra);
        assert_eq!(filtered.get("SAFE_FLAG").map(String::as_str), Some("1"));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("GROK_API_KEY")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("LD_PRELOAD")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("DYLD_INSERT_LIBRARIES")));
        assert!(!filtered.keys().any(|k| k.eq_ignore_ascii_case("PYTHONHOME")));
    }
}
