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
    AssertionRecord, AssertionStatus, Decision, ElementDraft, ExpectationSource, InputKind, Session, VerifyReport,
};
use super::home::Home;

/// Model turns for the fixture. The board needs about four tool calls.
const MAX_TURNS: usize = 6;
/// Refused or failed actions in a row before a task stops.
const MAX_REFUSALS_IN_A_ROW: usize = 3;
/// Held requests and handoffs a task report lists for the operator.
const MAX_FOR_OPERATOR: usize = 20;
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
    if let Some(origins) = result
        .get("not_granted")
        .filter(|origins| origins.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["not_granted"] = origins.clone();
    }
    if let Some(note) = page_note(wire, &result) {
        outcome["more"] = json!(note);
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
                        let fact = session.fact(&el.ref_id);
                        if let Some(href) = fact.and_then(|fact| fact.href.as_deref()) {
                            row["href"] = json!(href);
                        }
                        // A select box's option labels are page text for `select`, not refs.
                        if let Some(options) = fact
                            .and_then(|fact| fact.snapshot_ref.as_deref())
                            .and_then(|snapshot| elements.iter().find(|raw| raw["ref"] == snapshot))
                            .and_then(|raw| raw.get("options"))
                        {
                            row["options"] = options.clone();
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

/// What the model should know about an observation that is one page of a
/// longer one, or that came back empty.
fn page_note(wire: &Value, result: &Value) -> Option<String> {
    let (page, pages) = (result["page"].as_u64()?, result["pages"].as_u64()?);
    let empty =
        result["text"].as_str().is_none_or(str::is_empty) && result["elements"].as_array().is_none_or(Vec::is_empty);
    if page < pages {
        Some(format!(
            "page {page} of {pages}: observe with page {} continues, and observe with find lists only what contains a word",
            page + 1
        ))
    } else if page > pages {
        Some(format!("there is no page {page}; the last one is {pages}"))
    } else if empty && wire.get("find").is_some_and(Value::is_string) {
        Some("nothing on this page contains that text".into())
    } else {
        None
    }
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
/// it to the operator), `held` (the sidecar must hold a form submission) or
/// `shows <text>` (the step's outcome must contain it). Any other refusal or
/// sidecar error fails the run.
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
            Some(shows)
                if shows
                    .strip_prefix("shows ")
                    .is_some_and(|want| outcome.to_string().contains(want)) => {}
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
        "observe" => Ok(json!({"op":"observe","page": wire.get("page"),"find": wire.get("find")})),
        "select_tab" => Ok(json!({"op":"select_tab","tab": wire["tab"]})),
        "wait" => Ok(json!({"op":"wait","timeout_ms": wire["timeout_ms"]})),
        "scroll" => Ok(json!({"op":"scroll","dy": wire["dy"]})),
        "click" | "follow_link" | "type" | "press_enter" | "select" => {
            let id = wire.get("ref_id").and_then(Value::as_str).unwrap_or("");
            let target = session
                .fact(id)
                .and_then(|fact| fact.snapshot_ref.clone())
                .ok_or_else(|| format!("stale or unknown ref {id}"))?;
            Ok(json!({"op": action, "target": {"ref": target}, "text": wire.get("text"), "value": wire.get("value")}))
        }
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
        json!({"role":"system","content": format!("You drive a browser with these tools. Granted origins: {}. Start with navigate. Element refs expire after every observation. You may click buttons, menus, options and tabs, and choose a select box's listed options. A long page comes in pages: when an observation has more, observe with page or find reaches the rest. A refusal is not success. A Handoff, or held_for_operator, means only the operator can do that step: say so and stop. not_granted lists sites the page asked for that this task may not reach. Call finish with your answer once the page shows it.", origins.join(", "))}),
        json!({"role":"user","content": task}),
    ];
    let mut calls = 0usize;
    let mut answer = None;
    let mut last_text = String::new();
    let mut last_url = Value::Null;
    let mut transcript = Vec::new();
    let mut refused_in_a_row = 0;
    let mut stopped = None;
    // What only the operator can do: held requests and handed-off elements.
    let mut for_operator: Vec<Value> = Vec::new();
    // Origins a page asked for that the request did not grant.
    let mut not_granted = std::collections::BTreeSet::new();
    'turns: for turn in 0..max_turns {
        // Permission checked at launch is stale after a wait (AGENTS.md). A
        // fixture session reaches only the loopback page this binary serves,
        // which the public-URL block list cannot check and would always refuse.
        if let Some(reason) = (!allow_private).then(|| policy_refusal_now(&session)).flatten() {
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
        // A finish later in this turn must not rest on results the model never read.
        let mut failed_this_turn = false;
        for call in tool_calls {
            calls += 1;
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = coerce_args(call.pointer("/function/arguments").cloned().unwrap_or(json!({})));
            let mut finish_refusal = None;
            if name == "finish" {
                transcript.push(format!("finish {}", truncate(&args.to_string(), 160)));
                match finish_verdict(&args, failed_this_turn) {
                    Ok(text) => {
                        answer = Some(text);
                        break 'turns;
                    }
                    Err(reason) => finish_refusal = Some(reason),
                }
            }
            let wire = model_wire(&name, &args);
            // Decided before the step, which re-mints refs: a handoff names its element.
            let handoff = wire.as_ref().and_then(|wire| match session.decide(wire) {
                Decision::Handoff { reason } => Some(json!({
                    "handoff": name,
                    "element": wire["ref_id"].as_str().and_then(|id| session.fact(id)).map(|fact| fact.name.clone()),
                    "reason": reason,
                })),
                _ => None,
            });
            let outcome = match (&wire, finish_refusal) {
                (_, Some(reason)) => json!({ "refused": reason }),
                (Some(wire), None) => step(&mut session, &mut sidecar, wire),
                (None, None) => json!({"refused":"malformed browser action"}),
            };
            note_for_operator(&mut for_operator, handoff, &outcome);
            for origin in outcome["not_granted"].as_array().into_iter().flatten() {
                if let (Some(origin), true) = (origin.as_str(), not_granted.len() < MAX_FOR_OPERATOR) {
                    not_granted.insert(origin.to_string());
                }
            }
            if let Some(text) = outcome.get("text").and_then(Value::as_str) {
                last_text = text.to_string();
                last_url = outcome["url"].clone();
            }
            transcript.push(format!("{name} {}", truncate(&outcome.to_string(), 240)));
            if outcome.get("elements").is_some() {
                retire_observations(&mut messages);
            }
            let failed = action_failed(&outcome);
            failed_this_turn |= failed;
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
        "for_operator": for_operator,
        "not_granted": not_granted,
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

/// Whether a tool outcome was refused or failed: a policy decision, an error
/// or a refusal. A handoff is a decision too.
fn action_failed(outcome: &Value) -> bool {
    ["decision", "error", "refused"]
        .iter()
        .any(|key| outcome.get(*key).is_some())
}

/// The answer a `finish` call ends the task with, or why it is refused. A
/// finish sent after a refused or failed call in the same turn is refused: the
/// model sent it before it could read that result, so its answer cannot rest
/// on the page. A finish first in its turn, or after only successful calls,
/// ends the task when it carries a non-blank answer.
fn finish_verdict(args: &Value, failed_earlier_this_turn: bool) -> Result<String, &'static str> {
    if failed_earlier_this_turn {
        return Err("an action in this turn was refused: read the results, then finish");
    }
    args["answer"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .ok_or("finish takes answer: your final answer as text")
}

/// Requests held and elements handed off during a task, at most
/// `MAX_FOR_OPERATOR` rows. The console lists them; nothing here sends them.
fn note_for_operator(rows: &mut Vec<Value>, handoff: Option<Value>, outcome: &Value) {
    let held = outcome["held_for_operator"].as_array().into_iter().flatten();
    for row in held.map(|request| json!({"held": request})).chain(handoff) {
        if rows.len() < MAX_FOR_OPERATOR && !rows.contains(&row) {
            rows.push(row);
        }
    }
}

fn model_wire(name: &str, args: &Value) -> Option<Value> {
    Some(match name {
        "navigate" => json!({"action":"navigate","url": args["url"]}),
        "observe" => {
            // Models send "" or null for an option they leave out, and numbers as text.
            let mut wire = json!({"action":"observe"});
            if let Some(find) = args["find"].as_str().filter(|text| !text.trim().is_empty()) {
                wire["find"] = json!(find);
            }
            match &args["page"] {
                Value::Null => {}
                Value::String(text) => {
                    wire["page"] = text.trim().parse::<u32>().map_or(json!(text), |page| json!(page))
                }
                other => wire["page"] = other.clone(),
            }
            wire
        }
        "select_tab" => json!({"action":"select_tab","tab": args["tab"]}),
        "type" => json!({"action":"type","ref_id": args["ref_id"],"text": args["text"]}),
        "select" => json!({"action":"select","ref_id": args["ref_id"],"value": args["option"]}),
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
        {"type":"function","function":{"name":"observe","description":"Read the current page and mint fresh element refs. One observation holds 40 elements and 4000 characters. On a longer page, pass page (2, 3, ...) for the next part, or find to list only the elements and text that contain a word.","parameters":{"type":"object","properties":{"page":{"type":"integer"},"find":{"type":"string"}}}}},
        {"type":"function","function":{"name":"select_tab","description":"Select a tab by its visible name.","parameters":{"type":"object","properties":{"tab":{"type":"string"}},"required":["tab"]}}},
        {"type":"function","function":{"name":"click","description":"Click a button, menu item, option, tab or link by its ref from the latest observation. A link that opens a new tab replaces this page. Requests that would send data are held for the operator.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"}},"required":["ref_id"]}}},
        {"type":"function","function":{"name":"select","description":"Choose one of the options a select box ref lists in the latest observation, by its label.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"},"option":{"type":"string"}},"required":["ref_id","option"]}}},
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
    let response = ollama_client(Duration::from_secs(180))?
        .post(format!("{api}/api/chat"))
        .json(&payload)
        .send()
        .map_err(|err| format!("ollama chat failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("ollama chat status {}", response.status()));
    }
    bounded_json(response)
}

fn ollama_get(api: &str, path: &str) -> Result<Value, String> {
    let response = ollama_client(Duration::from_secs(5))?
        .get(format!("{api}{path}"))
        .send()
        .map_err(|err| err.to_string())?;
    bounded_json(response)
}

/// The same posture as the async local-model clients (`llm::ollama`): no proxy,
/// no redirect, and a resolver that refuses any non-loopback answer. A page's
/// text rides in `messages`, so a redirect must not re-post it elsewhere.
fn ollama_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .dns_resolver(std::sync::Arc::new(crate::llm::backend::LoopbackOnlyDns))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|err| err.to_string())
}

/// JSON from at most `MAX_RESPONSE_BYTES` of body.
fn bounded_json(response: reqwest::blocking::Response) -> Result<Value, String> {
    use std::io::Read;
    let max = crate::llm::MAX_RESPONSE_BYTES;
    let mut body = Vec::new();
    response
        .take(max as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|err| format!("ollama response failed: {err}"))?;
    if body.len() > max {
        return Err("ollama response is too large".into());
    }
    serde_json::from_slice(&body).map_err(|err| format!("ollama response was not json: {err}"))
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

/// A target=_blank link replaces the page; Enter submits a GET search; a
/// select box takes a listed option; a password field hands off; Enter in a
/// POST form and a button whose script POSTs are both held; and the page's
/// WebRTC sends nothing to a UDP port the session never granted.
pub fn run_enter_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let stun = std::net::UdpSocket::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    stun.set_nonblocking(true).map_err(|err| err.to_string())?;
    let stun_port = stun.local_addr().map_err(|err| err.to_string())?.port();
    let steps = [
        json!({"action":"follow_link","ref_id":"@Open in a new tab"}),
        json!({"action":"type","ref_id":"@Query","text":"github cgfixit"}),
        json!({"action":"press_enter","ref_id":"@Query"}),
        json!({"action":"select","ref_id":"@Size","value":"Large"}),
        // Observations cut labels to 80 characters; the listed text must still select.
        json!({"action":"select","ref_id":"@Shipping","value": LONG_OPTION.chars().take(80).collect::<String>()}),
        json!({"action":"type","ref_id":"@Password","text":"hunter2","expect":"handoff"}),
        json!({"action":"type","ref_id":"@Amount","text":"500"}),
        json!({"action":"press_enter","ref_id":"@Amount","expect":"held"}),
        json!({"action":"click","ref_id":"@Save","expect":"held"}),
    ];
    let log = run_fixture(
        &SEARCH_PAGE
            .replace("STUN_PORT", &stun_port.to_string())
            .replace("LONG_OPTION", LONG_OPTION),
        &steps,
        |text| {
            text.contains("second tab")
                && text.contains("submitted github cgfixit")
                && text.contains("picked l")
                && text.contains("shipped f")
        },
        sidecar,
        headless,
    )?;
    // Both tabs ran the probe during the steps; one datagram means UDP got past the proxy.
    if stun.recv_from(&mut [0u8; 512]).is_ok() {
        return Err(format!("the page's WebRTC sent UDP around the proxy:\n{log}"));
    }
    Ok(log)
}

/// A page that renders after a fetch is read once it has; a tab the page opens
/// on a site the task was not granted is closed, the page stays and the site
/// is named; `observe` with `page` or `find` reaches past the first 40
/// elements and 4,000 characters; and a link whose label has inline markup
/// is listed.
pub fn run_reach_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let entries: String = (1..=60)
        .map(|n| {
            format!(
                "<a href=\"/?entry={n}\">Entry {n}</a><p>{}</p>",
                "filler words here ".repeat(8)
            )
        })
        .collect();
    let (origin, port) = serve_html(&REACH_PAGE.replace("ENTRIES", &entries))?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let steps = [
        json!({"action":"navigate","url": format!("http://127.0.0.1:{port}/"),"expect":"shows late content"}),
        json!({"action":"click","ref_id":"@Share","expect":"shows http://127.0.0.1:1"}),
        json!({"action":"observe","page":3,"expect":"shows RELEASE-MARKER"}),
        // Inline markup in a label: the link is named by its text.
        json!({"action":"observe","find":"changelog","expect":"shows Changelog of releases"}),
        json!({"action":"follow_link","ref_id":"@Changelog of releases"}),
    ];
    let log = drive(session, sidecar, &steps, headless)?;
    let last = log.lines().next_back().unwrap_or("");
    if !(last.contains("changelog open") && last.contains("late content")) {
        return Err(format!("fixture text was not visible:\n{log}"));
    }
    Ok(log)
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
            ("qwen" | "enter" | "reach" | "run", None) if browse && mode.is_none() => mode = Some(key.to_string()),
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
        (Some("reach"), _) => run_reach_demo(&sidecar, headless),
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

/// An option label longer than the 80 characters an observation lists.
const LONG_OPTION: &str =
    "Freight delivery to the loading dock with a scheduled two hour window and a signature on receipt";

const SEARCH_PAGE: &str = r#"
<p id="tab">first tab</p>
<a href="/?tab=2" target="_blank">Open in a new tab</a>
<form id="find">
<label>Query <input id="q" role="searchbox" aria-label="Query"></label>
<p id="result">waiting</p>
</form>
<label>Password <input type="password" aria-label="Password"></label>
<form method="post" action="/send"><label>Amount <input aria-label="Amount"></label></form>
<button onclick="fetch('/save', {method: 'POST'})">Save</button>
<label>Size <select aria-label="Size" onchange="document.getElementById('picked').textContent = 'picked ' + this.value"><option value="s">Small</option><option value="l">Large</option></select></label>
<p id="picked">nothing picked</p>
<label>Shipping <select aria-label="Shipping" onchange="document.getElementById('shipped').textContent = 'shipped ' + this.value"><option value="s">Standard</option><option value="f">LONG_OPTION</option></select></label>
<p id="shipped">not shipped</p>
<script>
if (location.search === '?tab=2') document.getElementById('tab').textContent = 'second tab';
try {
  const probe = new RTCPeerConnection({iceServers: [{urls: 'stun:127.0.0.1:STUN_PORT'}]});
  probe.createDataChannel('probe');
  probe.createOffer().then((offer) => probe.setLocalDescription(offer));
} catch (err) {}
document.getElementById('find').addEventListener('submit', (event) => {
  event.preventDefault();
  document.getElementById('result').textContent = 'submitted ' + document.getElementById('q').value;
});
</script>
"#;

/// Port 1 on loopback is an origin no fixture session is granted.
const REACH_PAGE: &str = r#"
<p id="late">loading</p>
<p id="where">start</p>
<button onclick="window.open('http://127.0.0.1:1/away')">Share</button>
ENTRIES
<p>RELEASE-MARKER 9.9.9</p>
<a href="/?changelog"><code>Changelog</code> of releases</a>
<script>
fetch('/data').then(() => setTimeout(() => { document.getElementById('late').textContent = 'late content'; }, 300));
if (location.search === '?changelog') document.getElementById('where').textContent = 'changelog open';
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

    /// A loopback listener that answers every connection with `response` and
    /// counts them. Returns its origin and the counter.
    fn canned(response: Vec<u8>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Read;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap()); // DevSkim: ignore DS137138 because loopback test fixture
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.read(&mut [0u8; 8192]);
                let _ = stream.write_all(&response);
            }
        });
        (origin, hits)
    }

    #[test]
    fn ollama_requests_follow_no_redirect_and_read_a_bounded_body() {
        use std::sync::atomic::Ordering;
        let ok =
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        let (elsewhere, leaked) = canned(ok.to_vec());
        let moved = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {elsewhere}/api/chat\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        let (api, asked) = canned(moved.into_bytes());
        // The page text in `messages` must not be re-posted to the redirect target.
        let err = ollama_chat(&api, "m", &[json!({"role":"user","content":"page text"})], &json!([])).unwrap_err();
        assert!(err.contains("307"), "{err}");
        assert!(ollama_get(&api, "/api/ps").is_err());
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(leaked.load(Ordering::SeqCst), 0, "a redirect was followed");

        let mut big = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\"".to_vec();
        big.resize(big.len() + crate::llm::MAX_RESPONSE_BYTES, b'a');
        big.push(b'"');
        let (api, _) = canned(big);
        let err = ollama_chat(&api, "m", &[], &json!([])).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        // A name goes through the loopback-only resolver; a literal does not.
        assert!(ollama_get(&elsewhere, "/api/version").is_ok());
        assert!(ollama_get(&elsewhere.replace("127.0.0.1", "localhost"), "/api/version").is_ok());
    }

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

    #[test]
    fn observe_arguments_are_normalised_and_a_long_page_says_so() {
        let wire = |args| model_wire("observe", &args).unwrap();
        assert_eq!(wire(json!({"find": "", "page": null})), json!({"action":"observe"}));
        assert_eq!(
            wire(json!({"find": "Download", "page": "2"})),
            json!({"action":"observe","find":"Download","page":2})
        );
        // Text that is not a number stays text, and the policy refuses it.
        let session = Session::general(["https://example.com"]).unwrap();
        assert!(session.decide(&wire(json!({"page": 2}))).is_allow());
        assert!(!session.decide(&wire(json!({"page": "two"}))).is_allow());

        let plain = json!({"action":"observe"});
        let seen = |page, pages| json!({"page": page, "pages": pages, "text": "t", "elements": []});
        assert!(page_note(&plain, &seen(1, 3)).unwrap().contains("page 1 of 3"));
        assert!(page_note(&plain, &seen(4, 3)).unwrap().contains("no page 4"));
        assert_eq!(page_note(&plain, &seen(3, 3)), None);
        let miss = json!({"page": 1, "pages": 1, "text": "", "elements": []});
        assert!(page_note(&json!({"action":"observe","find":"x"}), &miss).is_some());
        assert_eq!(page_note(&plain, &miss), None);
    }

    #[test]
    fn finish_is_refused_after_a_failed_call_in_the_same_turn() {
        let done = json!({"answer": "zipfile — Work with ZIP archives"});
        let observed = json!({"url": "https://docs.python.org/3/", "text": "Library index", "elements": []});
        let malformed = json!({"refused": "malformed browser action"});
        let stale = json!({"decision": "Refuse { reason: \"stale ref\" }"});
        let verdict = |earlier: &[&Value]| finish_verdict(&done, earlier.iter().any(|outcome| action_failed(outcome)));
        // First in its turn, or after only successful calls: accepted.
        assert_eq!(verdict(&[]).as_deref(), Ok("zipfile — Work with ZIP archives"));
        assert_eq!(
            verdict(&[&observed, &observed]).as_deref(),
            Ok("zipfile — Work with ZIP archives")
        );
        // After a refused or failed call, even one followed by a success: refused.
        for earlier in [vec![&malformed], vec![&observed, &stale], vec![&stale, &observed]] {
            let refused = verdict(&earlier).unwrap_err();
            assert!(refused.contains("read the results, then finish"), "{refused}");
        }
        // A blank or missing answer stays refused either way.
        for args in [json!({}), json!({"answer": "  "}), json!({"find": "zipfile"})] {
            assert!(finish_verdict(&args, false)
                .unwrap_err()
                .starts_with("finish takes answer"));
        }
    }

    #[test]
    fn operator_rows_list_holds_and_handoffs_once_within_the_cap() {
        let mut rows = Vec::new();
        let held = json!({"held_for_operator": ["POST https://shop.example/cart"]});
        note_for_operator(&mut rows, None, &held);
        note_for_operator(&mut rows, None, &held);
        let handoff = json!({"handoff": "type", "element": "Password", "reason": "secret"});
        note_for_operator(&mut rows, Some(handoff.clone()), &json!({"decision": "Handoff"}));
        assert_eq!(rows, vec![json!({"held": "POST https://shop.example/cart"}), handoff]);
        for n in 0..MAX_FOR_OPERATOR {
            note_for_operator(&mut rows, None, &json!({"held_for_operator": [format!("POST /{n}")]}));
        }
        assert_eq!(rows.len(), MAX_FOR_OPERATOR);
    }
}
