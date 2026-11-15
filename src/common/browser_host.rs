//! One browser session process. It owns the Playwright sidecar for the whole
//! session. The coding check sandbox is not involved, and this is not an MCP call.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use super::browser::{
    AssertionRecord, AssertionStatus, ElementDraft, ExpectationSource, InputKind, Session, VerifyReport,
};
use super::home::Home;

/// Model turns for the fixture. The board needs about four tool calls.
const MAX_TURNS: usize = 6;
/// Refused or failed actions in a row before a task stops.
const MAX_REFUSALS_IN_A_ROW: usize = 3;
/// Longest operator task text a run accepts.
pub const MAX_TASK_CHARS: usize = 2_000;

/// The only variables a browser child sees. Chrome renderers parse hostile
/// pages and inherit this, so provider keys and tokens never enter it.
pub fn child_environment() -> std::collections::BTreeMap<String, String> {
    [
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SystemRoot",
        "WINDIR",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "CGAGENTHARNESS_HOME",
    ]
    .iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).to_string(), value)))
    .collect()
}

/// The first granted origin the shared web block list covers, or that cannot
/// be checked at all. It checks the normalized origin Chrome is actually
/// granted, not the operator's raw text, and fails closed on a parse error.
pub fn blocked_origin(policy: &crate::server::web_policy::Policy, session: &Session) -> Option<String> {
    session.grant_strings().into_iter().find(|origin| {
        crate::server::web_policy::canonical_url(&format!("{origin}/")).map_or(true, |url| policy.blocks(&url))
    })
}

/// The shared policy re-read now, because it can change during a task.
/// Fails closed when it cannot be read.
fn policy_refusal_now(session: &Session) -> Option<String> {
    let home = Home::resolve(None);
    let policy = home
        .load_config()
        .ok()
        .and_then(|cfg| crate::server::web_search::WebTool::new(&home.tools_dir(), &cfg).ok())
        .and_then(|web| web.policy().ok());
    match policy {
        Some(policy) => {
            blocked_origin(&policy, session).map(|origin| format!("{origin} is on the shared web block list"))
        }
        None => Some("the shared web policy could not be read".into()),
    }
}

/// One observe/act surface the model loop drives. It takes the wire ops that
/// `sidecar_op` builds. The browser sidecar is the only driver today;
/// `computer::kimi_cu_call` maps the same ops for the native one later.
pub trait Driver {
    fn call(&mut self, op: Value) -> Result<Value, String>;
}

/// The Node sidecar for one session. Dropping it closes stdin, so the sidecar
/// closes Chrome and exits. A sidecar that lingers is killed.
struct Sidecar {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Sidecar {
    /// `allow_private` lets IP-literal grants reach loopback and private
    /// addresses; only test-support fixtures pass true.
    fn start(script: &Path, session: &Session, headless: bool, allow_private: bool) -> Result<Self, String> {
        if !script
            .parent()
            .is_some_and(|dir| dir.join("node_modules/playwright").exists())
        {
            return Err("playwright is not prepared. From browser/, run PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci. A session does not install it.".into());
        }
        let mut child = Command::new("node")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env_clear()
            .envs(child_environment())
            .env("PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD", "1")
            .spawn()
            .map_err(|err| format!("node sidecar failed to start: {err}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("sidecar pipes missing".into());
        };
        let mut sidecar = Self {
            child,
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            next_id: 0,
        };
        let started = sidecar.call(
            json!({"op":"start","grants": session.grant_strings(),"headless": headless,"allow_private": allow_private}),
        )?;
        if started.get("ok") != Some(&Value::Bool(true)) {
            return Err(format!("sidecar refused start: {started}"));
        }
        Ok(sidecar)
    }
}

impl Driver for Sidecar {
    fn call(&mut self, mut message: Value) -> Result<Value, String> {
        self.next_id += 1;
        message["id"] = json!(self.next_id);
        let stdin = self.stdin.as_mut().ok_or("sidecar stdin closed")?;
        writeln!(stdin, "{message}").map_err(|err| err.to_string())?;
        stdin.flush().map_err(|err| err.to_string())?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|err| err.to_string())?;
        if line.is_empty() {
            return Err("sidecar closed the pipe".into());
        }
        serde_json::from_str(&line)
            .map_err(|err| format!("sidecar reply was not json: {err}: {}", truncate(&line, 300)))
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        drop(self.stdin.take());
        for _ in 0..50 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Decide, execute and re-mint one action. A refusal or handoff comes back as
/// `decision` and a sidecar failure as `error`. Neither ends the session.
fn step(session: &mut Session, sidecar: &mut dyn Driver, wire: &Value) -> Value {
    let decision = session.decide(wire);
    if !decision.is_allow() {
        return json!({"decision": format!("{decision:?}")});
    }
    let reply = match sidecar_op(session, wire).and_then(|op| sidecar.call(op)) {
        Ok(reply) => reply,
        Err(err) => return json!({"error": err}),
    };
    if reply.get("ok") != Some(&Value::Bool(true)) {
        return json!({"error": reply.get("error").cloned().unwrap_or(reply)});
    }
    let result = reply.get("result").cloned().unwrap_or(json!({}));
    let mut outcome = json!({"url": result.get("url"), "text": result.get("text")});
    if let Some(held) = result
        .get("held")
        .filter(|held| held.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["held_for_operator"] = held.clone();
    }
    if let Some(elements) = result.get("elements").and_then(Value::as_array) {
        let drafts = drafts_from(elements);
        let tabs = drafts
            .iter()
            .filter(|draft| draft.role.eq_ignore_ascii_case("tab"))
            .map(|draft| draft.name.clone())
            .collect();
        match session.mint(tabs, drafts) {
            Ok(obs) => {
                let rows: Vec<Value> = obs
                    .elements
                    .iter()
                    .map(|el| {
                        let mut row = json!({"ref_id": el.ref_id, "role": el.role, "name": el.name});
                        if let Some(href) = session.fact(&el.ref_id).and_then(|fact| fact.href.as_deref()) {
                            row["href"] = json!(href);
                        }
                        row
                    })
                    .collect();
                outcome["elements"] = json!(rows);
            }
            Err(err) => outcome["observation_error"] = json!(format!("{err:?}")),
        }
    }
    outcome
}

/// Serve `page`, navigate to it, run `steps`, then require `done` on the last
/// page text. A step may name its ref as `@Name` from the latest observation.
fn run_fixture(
    page: &str,
    steps: &[Value],
    done: impl Fn(&str) -> bool,
    sidecar: &Path,
    headless: bool,
) -> Result<String, String> {
    let (origin, port) = serve_html(page)?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let mut all = vec![json!({"action":"navigate","url": format!("http://127.0.0.1:{port}/")})];
    all.extend_from_slice(steps);
    let log = drive(session, sidecar, &all, headless)?;
    if !done(log.lines().next_back().unwrap_or("")) {
        return Err(format!("fixture text was not visible:\n{log}"));
    }
    Ok(log)
}

pub fn run_browse_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let steps = [
        json!({"action":"select_tab","tab":"Open"}),
        json!({"action":"type","ref_id":"@Filter","text":"open"}),
    ];
    run_fixture(FIXTURE_PAGE, &steps, board_complete, sidecar, headless)
}

pub fn run_verify_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let expectation = ExpectationSource::TrustedTest {
        id: "fixture-marker".into(),
    };
    let (_, base_port) = serve_html("<p>base has no marker</p>")?;
    let (_, head_port) = serve_html("<p>MARKER-OK</p>")?;
    let base = read_page(sidecar, base_port, headless)?;
    let head = read_page(sidecar, head_port, headless)?;
    let base_pass = base.contains("MARKER-OK");
    let head_pass = head.contains("MARKER-OK");
    let report = VerifyReport {
        scenario: "marker is present after the fix".into(),
        expectation,
        revision: "demo-fixture".into(),
        assertions: vec![
            AssertionRecord {
                name: "base lacks MARKER-OK".into(),
                status: if base_pass {
                    AssertionStatus::Failed
                } else {
                    AssertionStatus::Passed
                },
            },
            AssertionRecord {
                name: "head shows MARKER-OK".into(),
                status: if head_pass {
                    AssertionStatus::Passed
                } else {
                    AssertionStatus::Failed
                },
            },
        ],
        containment: "chromium proxy plus route abort; the sidecar closes Chrome on stdin EOF and is killed if it lingers, not whole-tree containment",
    };
    let body = serde_json::to_string_pretty(&report_json(&report, &base, &head)).map_err(|err| err.to_string())?;
    if !report.passed() {
        return Err(body);
    }
    Ok(body)
}

fn read_page(sidecar: &Path, port: u16, headless: bool) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/");
    let session = Session::pr_verify(&url, &[]).map_err(|err| format!("{err:?}"))?;
    drive(session, sidecar, &[json!({"action":"navigate","url": url})], headless)
}

/// Run fixed steps. A step may carry `expect`: `handoff` (the policy must hand
/// it to the operator) or `held` (the sidecar must hold a form submission).
/// Any other refusal or sidecar error fails the run.
fn drive(mut session: Session, script: &Path, steps: &[Value], headless: bool) -> Result<String, String> {
    // Fixtures serve on 127.0.0.1: a private grant needs a test-support build.
    let mut sidecar = Sidecar::start(script, &session, headless, cfg!(feature = "test-support"))?;
    let mut last_text = String::new();
    let mut rows: Vec<Value> = Vec::new();
    let mut lines = Vec::new();
    for spec in steps {
        let mut wire = spec.clone();
        let expect = wire.as_object_mut().and_then(|fields| fields.remove("expect"));
        if let Some(name) = wire
            .get("ref_id")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix('@'))
        {
            let id = rows
                .iter()
                .find(|row| row["name"] == name)
                .and_then(|row| row["ref_id"].as_str())
                .ok_or_else(|| format!("{name} was not in the last observation"))?
                .to_string();
            wire["ref_id"] = json!(id);
        }
        let outcome = step(&mut session, &mut sidecar, &wire);
        lines.push(format!("{} -> {}", wire["action"], truncate(&outcome.to_string(), 300)));
        match expect.as_ref().and_then(Value::as_str) {
            Some("handoff") if outcome["decision"].as_str().is_some_and(|d| d.starts_with("Handoff")) => continue,
            Some("held") if outcome.get("held_for_operator").is_some() => {}
            Some(other) => return Err(format!("expected {other} on {wire}:\n{}", lines.join("\n"))),
            None if outcome.get("error").is_some() || outcome.get("decision").is_some() => {
                return Err(format!("refused or failed on {wire}:\n{}", lines.join("\n")))
            }
            None => {}
        }
        if let Some(text) = outcome.get("text").and_then(Value::as_str) {
            last_text = text.to_string();
        }
        if let Some(seen) = outcome.get("elements").and_then(Value::as_array) {
            rows = seen.clone();
        }
    }
    lines.push(last_text);
    Ok(lines.join("\n"))
}

fn sidecar_op(session: &Session, wire: &Value) -> Result<Value, String> {
    let action = wire.get("action").and_then(Value::as_str).unwrap_or("");
    match action {
        "navigate" => Ok(json!({"op":"navigate","url": wire["url"]})),
        "observe" => Ok(json!({"op":"observe"})),
        "select_tab" => Ok(json!({"op":"select_tab","tab": wire["tab"]})),
        "wait" => Ok(json!({"op":"wait","timeout_ms": wire["timeout_ms"]})),
        "scroll" => Ok(json!({"op":"scroll","dy": wire["dy"]})),
        "click" | "follow_link" | "type" | "press_enter" => {
            let id = wire.get("ref_id").and_then(Value::as_str).unwrap_or("");
            let target = session
                .fact(id)
                .and_then(|fact| fact.snapshot_ref.clone())
                .ok_or_else(|| format!("stale or unknown ref {id}"))?;
            Ok(json!({"op": action, "target": {"ref": target}, "text": wire.get("text")}))
        }
        // shortcut: select passes policy but has no sidecar op; add selectOption with a fixture that needs it.
        "select" => Err("select is not wired to the sidecar".into()),
        _ => Err("malformed browser action".into()),
    }
}

fn drafts_from(elements: &[Value]) -> Vec<ElementDraft> {
    elements
        .iter()
        .filter_map(|el| {
            let name = el.get("name")?.as_str()?.to_string();
            let role = el.get("role")?.as_str()?.to_string();
            let input = match el.get("input").and_then(|v| v.as_str()) {
                Some("secret") => InputKind::Secret,
                Some("text") => InputKind::Text,
                _ => InputKind::None,
            };
            Some(ElementDraft {
                role,
                name,
                href: el.get("href").and_then(|v| v.as_str()).map(str::to_string),
                input,
                snapshot_ref: el.get("ref").and_then(|v| v.as_str()).map(str::to_string),
            })
        })
        .collect()
}

fn serve_html(body: &str) -> Result<(String, u16), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let page = format!("<!doctype html><html><body>{body}</body></html>");
    thread::spawn(move || {
        for _ in 0..32 {
            let Ok((mut socket, _)) = listener.accept() else { break };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut socket, &mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = std::io::Write::write_all(&mut socket, response.as_bytes());
        }
    });
    Ok((format!("http://127.0.0.1:{port}"), port))
}

fn report_json(report: &VerifyReport, base: &str, head: &str) -> Value {
    json!({
        "scenario": report.scenario,
        "expectation_source": format!("{:?}", report.expectation),
        "passed": report.passed(),
        "revision": report.revision,
        "containment": report.containment,
        "assertions": report.assertions.iter().map(|row| {
            json!({"name": row.name, "status": format!("{:?}", row.status)})
        }).collect::<Vec<_>>(),
        "base_excerpt": base.chars().take(400).collect::<String>(),
        "head_excerpt": head.chars().take(400).collect::<String>(),
    })
}

/// Run one operator task with the local model. Grants come only from
/// `origins`. The report is `finished` once the model calls `finish` or replies
/// without a tool call; a spent turn budget is not success.
pub fn run_task(
    script: &Path,
    headless: bool,
    (model, api): (&str, &str),
    task: &str,
    origins: &[String],
    max_turns: usize,
    allow_private: bool,
) -> Result<Value, String> {
    let mut session = Session::general(origins).map_err(|err| format!("{err:?}"))?;
    let mut sidecar = Sidecar::start(script, &session, headless, allow_private)?;
    let tools = browser_tools();
    let mut messages = vec![
        json!({"role":"system","content": format!("You drive a browser with these tools. Granted origins: {}. Start with navigate. Element refs expire after every observation. You may click buttons, menus and tabs. A refusal is not success. A Handoff, or held_for_operator, means only the operator can do that step: say so and stop. Call finish with your answer once the page shows it.", origins.join(", "))}),
        json!({"role":"user","content": task}),
    ];
    let mut calls = 0usize;
    let mut answer = None;
    let mut last_text = String::new();
    let mut last_url = Value::Null;
    let mut transcript = Vec::new();
    let mut refused_in_a_row = 0;
    let mut stopped = None;
    'turns: for turn in 0..max_turns {
        // Permission checked at launch is stale after a wait (AGENTS.md).
        if let Some(reason) = policy_refusal_now(&session) {
            stopped = Some(reason);
            break;
        }
        let response = ollama_chat(api, model, &messages, &tools)?;
        let message = response
            .get("message")
            .cloned()
            .ok_or_else(|| format!("ollama reply had no message: {}", truncate(&response.to_string(), 400)))?;
        let tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let content = message.get("content").and_then(Value::as_str).unwrap_or("").to_string();
        transcript.push(format!(
            "turn {turn} tools={} content={}",
            tool_calls.len(),
            truncate(&content, 180)
        ));
        messages.push(message);
        if tool_calls.is_empty() {
            answer = Some(content);
            break;
        }
        for call in tool_calls {
            calls += 1;
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = coerce_args(call.pointer("/function/arguments").cloned().unwrap_or(json!({})));
            if name == "finish" {
                transcript.push(format!("finish {}", truncate(&args.to_string(), 160)));
                answer = Some(args["answer"].as_str().unwrap_or("").to_string());
                break 'turns;
            }
            let outcome = match model_wire(&name, &args) {
                Some(wire) => step(&mut session, &mut sidecar, &wire),
                None => json!({"refused":"malformed browser action"}),
            };
            if let Some(text) = outcome.get("text").and_then(Value::as_str) {
                last_text = text.to_string();
                last_url = outcome["url"].clone();
            }
            transcript.push(format!("{name} {}", truncate(&outcome.to_string(), 240)));
            if outcome.get("elements").is_some() {
                retire_observations(&mut messages);
            }
            let failed = ["decision", "error", "refused"]
                .iter()
                .any(|key| outcome.get(*key).is_some());
            refused_in_a_row = if failed { refused_in_a_row + 1 } else { 0 };
            messages.push(json!({"role":"tool","tool_name": name,"content": outcome.to_string()}));
            // A model that keeps retrying refused actions only spends the deadline.
            if refused_in_a_row >= MAX_REFUSALS_IN_A_ROW {
                stopped = Some("three refused or failed actions in a row".to_string());
                break 'turns;
            }
        }
    }
    drop(sidecar);
    Ok(json!({
        "finished": answer.is_some(),
        "answer": answer,
        "stopped": stopped,
        "model": model,
        "origins": origins,
        "model_calls_with_tools": calls,
        "final_url": last_url,
        "page_text": last_text,
        "transcript": transcript,
    }))
}

pub fn run_qwen_fixture(script: &Path, headless: bool, model: &str, api: &str) -> Result<String, String> {
    let (origin, port) = serve_html(FIXTURE_PAGE)?;
    let task = format!("A board is at http://127.0.0.1:{port}/. Select the Open tab and set the Filter box to the word open. The result paragraph has to show both the open tab and that filter. Finish with the paragraph you verified.");
    let mut report = run_task(
        script,
        headless,
        (model, api),
        &task,
        &[origin],
        MAX_TURNS,
        cfg!(feature = "test-support"),
    )?;
    let ps = ollama_get(api, "/api/ps").unwrap_or_else(|err| json!({"error": err}));
    report["context_length"] = ps["models"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find_map(|row| {
                (row["name"].as_str() == Some(model) || row["model"].as_str() == Some(model))
                    .then(|| row["context_length"].clone())
            })
        })
        .unwrap_or(Value::Null);
    report["ollama"] = ollama_get(api, "/api/version").unwrap_or(json!(null));
    let passed = board_complete(report["page_text"].as_str().unwrap_or(""));
    report["passed"] = json!(passed);
    let body = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
    if !passed {
        return Err(body);
    }
    Ok(body)
}

fn model_wire(name: &str, args: &Value) -> Option<Value> {
    Some(match name {
        "navigate" => json!({"action":"navigate","url": args["url"]}),
        "observe" => json!({"action":"observe"}),
        "select_tab" => json!({"action":"select_tab","tab": args["tab"]}),
        "type" => json!({"action":"type","ref_id": args["ref_id"],"text": args["text"]}),
        "press_enter" | "click" | "follow_link" => json!({"action": name,"ref_id": args["ref_id"]}),
        _ => return None,
    })
}

/// Refs expire with every observation, so older page dumps only spend the 32k window.
fn retire_observations(messages: &mut [Value]) {
    for message in messages.iter_mut().filter(|message| message["role"] == "tool") {
        if message["content"]
            .as_str()
            .is_some_and(|text| text.contains("\"elements\""))
        {
            message["content"] = json!("{\"retired\":\"older observation; its refs expired\"}");
        }
    }
}

fn browser_tools() -> Value {
    json!([
        {"type":"function","function":{"name":"navigate","description":"Open an http(s) URL in this session.","parameters":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}},
        {"type":"function","function":{"name":"observe","description":"Read the current page and mint fresh element refs.","parameters":{"type":"object","properties":{}}}},
        {"type":"function","function":{"name":"select_tab","description":"Select a tab by its visible name.","parameters":{"type":"object","properties":{"tab":{"type":"string"}},"required":["tab"]}}},
        {"type":"function","function":{"name":"click","description":"Click a button, menu item, tab or link by its ref from the latest observation. Requests that would send data are held for the operator.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"}},"required":["ref_id"]}}},
        {"type":"function","function":{"name":"type","description":"Type into a text box ref from the latest observation.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"},"text":{"type":"string"}},"required":["ref_id","text"]}}},
        {"type":"function","function":{"name":"press_enter","description":"Press Enter on a text box ref from the latest observation. The key is always Enter.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"}},"required":["ref_id"]}}},
        {"type":"function","function":{"name":"follow_link","description":"Follow a link ref from the latest observation.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"}},"required":["ref_id"]}}},
        {"type":"function","function":{"name":"finish","description":"Stop after the visible paragraph is correct.","parameters":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}}}
    ])
}

fn ollama_chat(api: &str, model: &str, messages: &[Value], tools: &Value) -> Result<Value, String> {
    // No options block: context length is server-side, so num_ctx is never sent.
    let payload = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": false,
        "think": false,
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .post(format!("{api}/api/chat"))
        .json(&payload)
        .send()
        .map_err(|err| format!("ollama chat failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("ollama chat status {}", response.status()));
    }
    response
        .json()
        .map_err(|err| format!("ollama chat was not json: {err}"))
}

fn ollama_get(api: &str, path: &str) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| err.to_string())?;
    client
        .get(format!("{api}{path}"))
        .send()
        .map_err(|err| err.to_string())?
        .json()
        .map_err(|err| err.to_string())
}

/// The operator's chat model and loopback Ollama origin, from the home config.
fn local_model(model: Option<String>) -> Result<(String, String), String> {
    let cfg = Home::resolve(None)
        .load_config()
        .map_err(|err| format!("home config did not load: {err}"))?;
    let api = crate::llm::ollama::native_base_url(&cfg.str_or("models.local_llm.base_url", ""))
        .ok_or("models.local_llm.base_url is not a loopback Ollama endpoint")?;
    let model = model.unwrap_or_else(|| cfg.str_or("models.local_llm.model", "").trim().to_string());
    if model.is_empty() {
        return Err("models.local_llm.model is empty".into());
    }
    Ok((model, api))
}

fn coerce_args(value: Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str(&text).unwrap_or(json!({})),
        other => other,
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Enter submits a GET search; a password field hands off; Enter in a POST
/// form and a button whose script POSTs are both held.
pub fn run_enter_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let steps = [
        json!({"action":"type","ref_id":"@Query","text":"github cgfixit"}),
        json!({"action":"press_enter","ref_id":"@Query"}),
        json!({"action":"type","ref_id":"@Password","text":"hunter2","expect":"handoff"}),
        json!({"action":"type","ref_id":"@Amount","text":"500"}),
        json!({"action":"press_enter","ref_id":"@Amount","expect":"held"}),
        json!({"action":"click","ref_id":"@Save","expect":"held"}),
    ];
    run_fixture(
        SEARCH_PAGE,
        &steps,
        |text| text.contains("submitted github cgfixit"),
        sidecar,
        headless,
    )
}

pub fn browse_cli(args: &[OsString]) -> u8 {
    dispatch(args, true)
}

pub fn verify_cli(args: &[OsString]) -> u8 {
    dispatch(args, false)
}

/// Exit codes: 0 passed, 2 failed, 3 bad arguments or config. Options are
/// single `--key=value` elements, so free text is always one argv element.
fn dispatch(args: &[OsString], browse: bool) -> u8 {
    let mut sidecar = PathBuf::from("browser/sidecar.mjs");
    let mut headless = true;
    let mut model = None;
    let mut mode: Option<String> = None;
    let mut task = None;
    let mut origins = Vec::new();
    let mut max_turns = 12usize;
    for raw in args {
        let arg = raw.to_string_lossy();
        let (key, value) = arg.split_once('=').map_or((arg.as_ref(), None), |(k, v)| (k, Some(v)));
        match (key, value) {
            ("--headed", None) => headless = false,
            // Only general browsing runs model tasks; browser-verify keeps PR grants.
            ("qwen" | "enter" | "run", None) if browse && mode.is_none() => mode = Some(key.to_string()),
            ("--model", Some(v)) if !v.is_empty() => model = Some(v.to_string()),
            ("--sidecar", Some(v)) if !v.is_empty() => sidecar = PathBuf::from(v),
            ("--task", Some(v)) if (1..=MAX_TASK_CHARS).contains(&v.chars().count()) => task = Some(v.to_string()),
            ("--origin", Some(v)) => origins.push(v.to_string()),
            ("--max-turns", Some(v)) if v.parse::<usize>().is_ok_and(|n| (1..=40).contains(&n)) => {
                max_turns = v.parse().unwrap_or(max_turns)
            }
            _ => {
                eprintln!("cgagentharness: unexpected or out-of-range browser argument {key}");
                return 3;
            }
        }
    }
    let local = |model| match local_model(model) {
        Ok(pair) => Ok(pair),
        Err(err) => {
            eprintln!("cgagentharness: {err}");
            Err(3u8)
        }
    };
    let result = match (mode.as_deref(), browse) {
        (Some("run"), _) => {
            let (Some(task), false) = (task, origins.is_empty()) else {
                eprintln!("cgagentharness: browser run needs --task and at least one --origin");
                return 3;
            };
            let (model, api) = match local(model) {
                Ok(pair) => pair,
                Err(code) => return code,
            };
            match run_task(&sidecar, headless, (&model, &api), &task, &origins, max_turns, false) {
                Ok(report) => {
                    println!("{report}");
                    return if report["finished"] == true { 0 } else { 2 };
                }
                Err(err) => Err(err),
            }
        }
        (Some("qwen"), _) => match local(model) {
            Ok((model, api)) => run_qwen_fixture(&sidecar, headless, &model, &api),
            Err(code) => return code,
        },
        (Some(_), _) => run_enter_demo(&sidecar, headless),
        (None, true) => run_browse_demo(&sidecar, headless),
        (None, false) => run_verify_demo(&sidecar, headless),
    };
    match result {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("cgagentharness: {err}");
            2
        }
    }
}

const SEARCH_PAGE: &str = r#"
<form id="find">
<label>Query <input id="q" role="searchbox" aria-label="Query"></label>
<p id="result">waiting</p>
</form>
<label>Password <input type="password" aria-label="Password"></label>
<form method="post" action="/send"><label>Amount <input aria-label="Amount"></label></form>
<button onclick="fetch('/save', {method: 'POST'})">Save</button>
<script>
document.getElementById('find').addEventListener('submit', (event) => {
  event.preventDefault();
  document.getElementById('result').textContent = 'submitted ' + document.getElementById('q').value;
});
</script>
"#;

fn board_complete(text: &str) -> bool {
    text.contains("showing open") && text.contains("filter open")
}

const FIXTURE_PAGE: &str = r#"
<button role="tab" id="all">All</button>
<button role="tab" id="open">Open</button>
<label>Filter <input id="filter" role="textbox" aria-label="Filter"></label>
<p id="result">showing all no filter</p>
<script>
let tab = 'all';
let filter = '';
function render() {
  const tabText = tab === 'open' ? 'showing open' : 'showing all';
  const filterText = filter ? ('filter ' + filter) : 'no filter';
  document.getElementById('result').textContent = tabText + ' ' + filterText;
}
document.getElementById('open').onclick = () => { tab = 'open'; render(); };
document.getElementById('all').onclick = () => { tab = 'all'; render(); };
document.getElementById('filter').oninput = (e) => { filter = e.target.value; render(); };
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_list_checks_the_granted_origin_and_fails_closed() {
        use crate::server::web_policy::{Policy, Rule, BLOCKED_GROUP};
        let mut policy = Policy::parse(br#"{"version":1,"rules":[]}"#, 64).unwrap();
        policy
            .blocked
            .push(Rule::new("https://blocked.example/*", BLOCKED_GROUP, &[]).unwrap());
        // An encoded path separator fails canonical_url but still grants the whole origin.
        let sneaky = Session::general(["https://blocked.example/%2F"]).unwrap();
        assert!(blocked_origin(&policy, &sneaky).is_some());
        let fine = Session::general(["https://fine.example/docs"]).unwrap();
        assert!(blocked_origin(&policy, &fine).is_none());
    }
}
