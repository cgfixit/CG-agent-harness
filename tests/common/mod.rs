//! Shared fixtures: a temp home, an in-process server on 127.0.0.1:0, a mock
//! OpenAI-compatible model, a fake `gh`, and a bare git repo.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Json, Router};
use cgagentharness::common::config::AppConfig;
use cgagentharness::common::home::Home;
use cgagentharness::server::state::AppState;
use cgagentharness::server::{build_app, AppOptions};
use serde_json::{json, Value};

pub const BIN: &str = env!("CARGO_BIN_EXE_cgagentharness");
pub const CSRF_HEADER: &str = "x-cyclaw-csrf";

/// A config built from the shipped default with YAML overrides applied as
/// dotted `key: value` replacements (simple scalar overrides only).
pub fn config_with(dir: &Path, overrides: &[(&str, &str)]) -> AppConfig {
    let mut text = AppConfig::embedded_default().to_string();
    for (dotted, value) in overrides {
        text = override_yaml(&text, dotted, value);
    }
    let path = dir.join("config.yaml");
    std::fs::write(&path, &text).unwrap();
    AppConfig::from_str(&text, &path).unwrap()
}

/// Replace `key:` under the nested path with `value`, matching indentation.
fn override_yaml(text: &str, dotted: &str, value: &str) -> String {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut done = false;
    let mut stack_indent: Vec<usize> = Vec::new();
    for line in text.lines() {
        if done {
            out.push(line.to_string());
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push(line.to_string());
            continue;
        }
        while let Some(last) = stack_indent.last() {
            if indent <= *last {
                stack_indent.pop();
                depth = depth.saturating_sub(1);
            } else {
                break;
            }
        }
        let key = trimmed.split(':').next().unwrap_or("").trim();
        if depth < parts.len() && key == parts[depth] {
            if depth == parts.len() - 1 {
                out.push(format!("{}{}: {}", " ".repeat(indent), key, value));
                done = true;
                continue;
            }
            stack_indent.push(indent);
            depth += 1;
        }
        out.push(line.to_string());
    }
    assert!(done, "override key not found: {dotted}");
    out.join("\n") + "\n"
}

pub struct TestServer {
    pub base: String,
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    pub home: PathBuf,
    pub api_key: String,
    pub csrf: String,
    pub client: reqwest::Client,
    _tmp: tempfile::TempDir,
    _task: tokio::task::JoinHandle<()>,
}

pub struct ServerOptions {
    pub overrides: Vec<(String, String)>,
    pub api_key: Option<String>,
    pub deny_all_tools: bool,
    pub web_resolve: Option<(String, SocketAddr)>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            overrides: Vec::new(),
            api_key: Some("test-api-key-0123456789".to_string()),
            deny_all_tools: false,
            web_resolve: None,
        }
    }
}

impl ServerOptions {
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.overrides.push((key.to_string(), value.to_string()));
        self
    }
}

/// Spawn the app on an ephemeral loopback port with `model_url` as the chat backend.
pub async fn spawn_server(model_url: &str, opts: ServerOptions) -> TestServer {
    let tmp = tempfile::tempdir().unwrap();
    let home_dir = tmp.path().join("home");
    let home = Home::at(home_dir.clone());
    home.ensure_layout().unwrap();
    let mut overrides: Vec<(&str, &str)> = vec![("models.local_llm.base_url", model_url)];
    let quoted = format!("\"{model_url}\"");
    overrides[0] = ("models.local_llm.base_url", &quoted);
    let owned: Vec<(String, String)> = opts.overrides.clone();
    for (k, v) in &owned {
        overrides.push((k.as_str(), v.as_str()));
    }
    let cfg = config_with(&home_dir, &overrides);
    let mut app_opts = AppOptions::new(home);
    app_opts.config = Some(cfg);
    app_opts.api_key = opts.api_key.clone();
    if opts.deny_all_tools {
        app_opts.tool_allowlist_override = Some(BTreeSet::new());
    }
    app_opts.web_test_resolve = opts.web_resolve;
    app_opts.shim_exe = Some(PathBuf::from(BIN));
    let (router, state) = build_app(app_opts).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    let csrf = state.csrf_token.clone();
    TestServer {
        base: format!("http://127.0.0.1:{}", addr.port()),
        addr,
        state,
        home: home_dir,
        api_key: opts.api_key.unwrap_or_default(),
        csrf,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        _tmp: tmp,
        _task: task,
    }
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Fully authenticated request (Bearer + CSRF), like the console's `api()` helper.
    pub fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, self.url(path))
            .bearer_auth(&self.api_key)
            .header(CSRF_HEADER, &self.csrf)
    }

    pub async fn get_json(&self, path: &str) -> (u16, Value) {
        let resp = self.req(reqwest::Method::GET, path).send().await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, body)
    }

    pub async fn post_json(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = self.req(reqwest::Method::POST, path).json(&body).send().await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, body)
    }

    pub async fn open_get(&self, path: &str) -> (u16, Value) {
        let resp = self.client.get(self.url(path)).send().await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, body)
    }
}

/// Error code from a `{"detail": {...}}` envelope.
pub fn code(body: &Value) -> String {
    body["detail"]["code"].as_str().unwrap_or("").to_string()
}

pub fn message(body: &Value) -> String {
    body["detail"]["message"].as_str().unwrap_or("").to_string()
}

// ---------------------------------------------------------------- mock model

#[derive(Clone)]
pub struct MockModel {
    pub base: String,
    pub reply: Arc<std::sync::Mutex<Value>>,
    pub requests: Arc<std::sync::Mutex<Vec<Value>>>,
    pub delay_ms: Arc<std::sync::Mutex<u64>>,
}

impl MockModel {
    pub fn base_url(&self) -> String {
        format!("{}/v1", self.base)
    }

    pub fn set_reply(&self, v: Value) {
        *self.reply.lock().unwrap() = v;
    }

    pub fn set_delay_ms(&self, ms: u64) {
        *self.delay_ms.lock().unwrap() = ms;
    }

    pub fn last_request(&self) -> Option<Value> {
        self.requests.lock().unwrap().last().cloned()
    }
}

pub fn ok_reply(text: &str, prompt: u64, completion: u64) -> Value {
    json!({
        "model": "mock-model",
        "choices": [{"message": {"role": "assistant", "content": text}}],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion},
    })
}

/// A minimal OpenAI-compatible server on 127.0.0.1:0.
pub async fn start_mock_model() -> MockModel {
    let reply = Arc::new(std::sync::Mutex::new(ok_reply("pong", 10, 2)));
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let delay = Arc::new(std::sync::Mutex::new(0u64));
    let r2 = reply.clone();
    let q2 = requests.clone();
    let d2 = delay.clone();
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "mock-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let r = r2.clone();
                let q = q2.clone();
                let d = d2.clone();
                async move {
                    q.lock().unwrap().push(body);
                    let ms = *d.lock().unwrap();
                    if ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    }
                    let reply = r.lock().unwrap().clone();
                    if let Some(status) = reply.get("__status").and_then(|s| s.as_u64()) {
                        return (
                            axum::http::StatusCode::from_u16(status as u16).unwrap(),
                            Json(json!({"error": "x"})),
                        );
                    }
                    (axum::http::StatusCode::OK, Json(reply))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    MockModel {
        base: format!("http://127.0.0.1:{}", addr.port()),
        reply,
        requests,
        delay_ms: delay,
    }
}

// ---------------------------------------------------------------- git + gh

pub fn git(args: &[&str], cwd: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare origin plus a seed commit containing `target.txt` and a small tree.
pub fn real_bare_repo(dir: &Path) -> PathBuf {
    let bare = dir.join("origin.git");
    let work = dir.join("seed");
    std::fs::create_dir_all(&work).unwrap();
    git(
        &["init", "-q", "--bare", "--initial-branch=main", bare.to_str().unwrap()],
        dir,
    );
    git(&["init", "-q", "--initial-branch=main"], &work);
    git(&["config", "user.email", "seed@example.com"], &work);
    git(&["config", "user.name", "Seed"], &work);
    std::fs::write(work.join("target.txt"), "hello\n").unwrap();
    std::fs::write(work.join("README.md"), "# seed\n").unwrap();
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::write(work.join("src").join("lib.txt"), "lib\n").unwrap();
    git(&["add", "."], &work);
    git(&["commit", "-q", "-m", "seed"], &work);
    git(&["remote", "add", "origin", bare.to_str().unwrap()], &work);
    git(&["push", "-q", "origin", "main"], &work);
    bare
}

/// Install a fake `gh` on a private bin dir and return that dir (prepend to PATH).
/// Handles: `--version`, `repo view`, `pr view/list/diff`, `issue view/list`,
/// `repo clone <slug> <dest> -- --depth 1`, `pr create` (prints a URL).
pub fn install_fake_gh(dir: &Path, bare: &Path) -> PathBuf {
    let bin = dir.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            r#"#!/bin/sh
BARE='{bare}'
LOG="${{FAKE_GH_LOG:-}}"
if [ -n "$LOG" ]; then echo "$@" >> "$LOG"; fi
case "$1" in
  --version) echo "gh version 2.60.0 (2026-01-01)"; exit 0 ;;
  repo)
    case "$2" in
      view) echo '{{"name":"repo","description":"fake","defaultBranchRef":{{"name":"main"}},"url":"https://example.invalid/r"}}'; exit 0 ;;
      clone)
        DEST="$4"
        git clone -q --depth 1 "$BARE" "$DEST"
        exit $? ;;
    esac ;;
  pr)
    case "$2" in
      view) echo '{{"number":1,"title":"t","body":"b","headRefName":"h","baseRefName":"main","url":"https://example.invalid/pr/1","state":"OPEN"}}'; exit 0 ;;
      list) echo '[]'; exit 0 ;;
      diff) echo 'diff --git a/x b/x'; exit 0 ;;
      create) echo 'https://example.invalid/pull/42'; exit 0 ;;
    esac ;;
  issue)
    case "$2" in
      view) echo '{{"number":1,"title":"t","body":"b","url":"https://example.invalid/issue/1","state":"OPEN"}}'; exit 0 ;;
      list) echo '[]'; exit 0 ;;
    esac ;;
esac
echo "fake gh: unsupported $*" 1>&2
exit 1
"#,
            bare = bare.display()
        );
        let path = bin.join("gh");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    {
        let script = format!(
            "@echo off\r\nif \"%1\"==\"--version\" (echo gh version 2.60.0 & exit /b 0)\r\n\
             if \"%1\"==\"repo\" if \"%2\"==\"clone\" (git clone -q --depth 1 \"{bare}\" %4 & exit /b %errorlevel%)\r\n\
             if \"%1\"==\"repo\" if \"%2\"==\"view\" (echo {{\"name\":\"repo\",\"description\":\"fake\",\"defaultBranchRef\":{{\"name\":\"main\"}},\"url\":\"https://example.invalid/r\"}} & exit /b 0)\r\n\
             if \"%1\"==\"pr\" if \"%2\"==\"list\" (echo [] & exit /b 0)\r\n\
             if \"%1\"==\"pr\" if \"%2\"==\"create\" (echo https://example.invalid/pull/42 & exit /b 0)\r\n\
             if \"%1\"==\"issue\" if \"%2\"==\"list\" (echo [] & exit /b 0)\r\n\
             echo fake gh: unsupported %* 1>&2\r\nexit /b 1\r\n",
            bare = bare.display()
        );
        std::fs::write(bin.join("gh.cmd"), script).unwrap();
    }
    bin
}

/// PATH with `bin` prepended.
pub fn path_with(bin: &Path) -> String {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).unwrap().to_string_lossy().into_owned()
}

/// Run the real binary's hidden `agentic` subcommand.
pub fn run_agentic(config: &Path, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> (i32, String, String) {
    let mut cmd = std::process::Command::new(BIN);
    cmd.arg("agentic")
        .arg("--config")
        .arg(config)
        .args(args)
        .current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
