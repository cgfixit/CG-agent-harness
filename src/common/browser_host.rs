//! One browser session process. It owns the Playwright sidecar for the whole
//! session. The coding check sandbox is not involved, and this is not an MCP call.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

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
/// Largest sidecar reply line. The sidecar caps every field (40 elements,
/// hrefs and the URL at 8,192 characters, text at 4,000), which stays far
/// under this even with JSON escaping; only a broken sidecar reaches it.
const MAX_REPLY_BYTES: u64 = 4 * 1024 * 1024;
/// Longest wait for one sidecar reply. The slowest op (navigate: 10 s load,
/// up to 14 s to settle, then an observation with 5 s steps) fits well inside,
/// and three failures in a row still end before the default 300 s task deadline.
const SIDECAR_CALL_TIMEOUT: Duration = Duration::from_secs(75);
/// One model call when no task deadline was given (a direct CLI run).
const MODEL_CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Why a task with a deadline stopped once the deadline passed.
const OUT_OF_TIME: &str = "the task's time budget ran out";
/// Prompt tokens a task needs room for, by the bytes/4 estimate: the tool list,
/// the system text, a `MAX_TASK_CHARS` task, 12 turns of calls and one live
/// observation at the sidecar's caps (older ones are retired). An estimate.
pub const TASK_PROMPT_TOKENS: u64 = 12_000;
/// Characters of a link target an observation shows the model.
const MAX_SHOWN_HREF_CHARS: usize = 300;
/// Page dialogs one observation shows the model, and the characters of each.
const MAX_SHOWN_DIALOGS: usize = 5;
const MAX_SHOWN_DIALOG_CHARS: usize = 320;
/// Pixels one model `scroll` moves the page: about two screens, inside the
/// policy kernel's 2,000.
const SCROLL_STEP_PX: i64 = 1_500;
/// Longest model `wait`, matching the policy kernel's 5,000 ms.
const MAX_WAIT_SECONDS: u64 = 5;

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

/// Prefix of a task's Chrome profile in the temporary directory. The sidecar
/// names it `cgah-browser-<its pid>-<random>` (`freshProfile` in sidecar.mjs).
const PROFILE_PREFIX: &str = "cgah-browser-";

/// Removes the Chrome profiles of browser tasks whose sidecar is gone, and
/// returns how many. A task killed at its deadline cannot remove its own, and
/// it holds that task's cookies and cache. The sidecar does the same when a
/// task starts; the server calls this at start, so a profile does not wait for
/// the next task (#894).
pub fn sweep_stale_profiles() -> usize {
    sweep_profiles_in(&std::env::temp_dir(), &super::process::pid_alive)
}

/// Only a real directory named `cgah-browser-<digits>-...` whose pid is dead is
/// removed. A name without a pid is left alone, whatever it holds: nothing
/// says who owns it, and such a directory has held a harness home.
fn sweep_profiles_in(dir: &Path, alive: &dyn Fn(u32) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(profile_owner) else {
            continue;
        };
        // file_type does not follow a symlink, so a link is never followed.
        if alive(pid) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The sidecar pid in a profile directory's name, or `None` for any other name.
fn profile_owner(name: &str) -> Option<u32> {
    let (pid, rest) = name.strip_prefix(PROFILE_PREFIX)?.split_once('-')?;
    (!rest.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
        .then(|| pid.parse().ok())
        .flatten()
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

/// Where a session's request verdicts come from.
enum PolicySource {
    /// A fixture session on a loopback page, which the public block list
    /// cannot name: nothing is refused.
    Open,
    /// The shared policy file and its rule cap.
    File(PathBuf, usize),
    /// The home or its config could not be read: every request is refused.
    Unreadable,
}

/// Answers the sidecar about each URL Chrome is about to request, so a
/// path-scoped block rule holds inside a granted origin (#899). The file is
/// re-read for every question and parsed again only when its bytes changed,
/// so an edit applies to the next request, not the next model turn. An
/// unreadable or invalid policy refuses every request.
struct RequestPolicy {
    source: PolicySource,
    snapshot: Option<(Vec<u8>, crate::server::web_policy::Policy)>,
    /// Fixtures only: a loopback origin and the public-looking name the block
    /// rules use for it, because a rule cannot name a private address.
    alias: Option<(String, String)>,
}

impl RequestPolicy {
    fn open() -> Self {
        Self::from(PolicySource::Open)
    }

    /// The operator's saved policy; unreadable when the home or config is.
    fn shared() -> Self {
        let home = Home::resolve(None);
        let file = home
            .load_config()
            .ok()
            .and_then(|cfg| crate::server::web_search::WebTool::new(&home.tools_dir(), &cfg).ok())
            .map(|web| web.policy_file());
        Self::from(file.map_or(PolicySource::Unreadable, |(path, max_rules)| {
            PolicySource::File(path, max_rules)
        }))
    }

    fn from(source: PolicySource) -> Self {
        Self {
            source,
            snapshot: None,
            alias: None,
        }
    }

    fn refuses(&mut self, raw: &str) -> bool {
        let (path, max_rules) = match &self.source {
            PolicySource::Open => return false,
            PolicySource::Unreadable => return true,
            PolicySource::File(path, max_rules) => (path, *max_rules),
        };
        let Ok(bytes) = crate::server::web_policy::Policy::read(path, max_rules) else {
            self.snapshot = None;
            return true;
        };
        if self.snapshot.as_ref().is_none_or(|(seen, _)| *seen != bytes) {
            self.snapshot = crate::server::web_policy::Policy::parse(&bytes, max_rules)
                .ok()
                .map(|policy| (bytes, policy));
        }
        let renamed = self
            .alias
            .as_ref()
            .and_then(|(from, to)| raw.strip_prefix(from.as_str()).map(|rest| format!("{to}{rest}")));
        self.snapshot
            .as_ref()
            .is_none_or(|(_, policy)| policy.blocks_request(renamed.as_deref().unwrap_or(raw)))
    }
}

/// Decides one request the sidecar has paused for the operator (#995). It is
/// given the sidecar's question: method, whole URL, content type, size, the
/// body's SHA-256 and a text preview. All of it but the hash is page-written.
/// It may block for as long as the session's wait; `true` sends the request.
pub type Approver = std::sync::Arc<dyn Fn(&Value) -> bool + Send + Sync>;

/// Takeover for one session: how long a paused request waits, and who decides.
#[derive(Clone)]
pub struct Takeover {
    pub wait: Duration,
    pub approver: Approver,
}

/// The longest wait a session may ask the sidecar for; it clamps to the same.
pub const MAX_TAKEOVER_WAIT: Duration = Duration::from_secs(600);

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
    /// Shared with the reader thread, which writes request verdicts.
    stdin: std::sync::Arc<std::sync::Mutex<Option<ChildStdin>>>,
    /// Reply lines from a reader thread, so a call can stop waiting.
    replies: Receiver<Result<String, String>>,
    next_id: u64,
    call_timeout: Duration,
    /// The task deadline: no call waits past it, held or not.
    deadline: Option<Instant>,
    /// The sidecar proxy's loopback port, from its `start` reply. Only the
    /// guard fixture uses it, to probe the proxy without the credential.
    proxy_port: Option<u16>,
}

impl Sidecar {
    /// Takes the child's stdin and stdout. A thread reads one bounded line at
    /// a time and stops at end of file, an error or an oversized line. It
    /// answers the sidecar's `check` lines itself, from `policy`, so a page's
    /// requests get a verdict while the task waits on the model too. An
    /// `approve` line is answered on its own thread by `approver`, so block-list
    /// questions keep being answered while the operator decides. Without an
    /// approver, or while one decision is already open, the answer is no.
    fn attach(
        mut child: Child,
        call_timeout: Duration,
        mut policy: RequestPolicy,
        approver: Option<Approver>,
    ) -> Result<Self, String> {
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("sidecar pipes missing".into());
        };
        let stdin = std::sync::Arc::new(std::sync::Mutex::new(Some(stdin)));
        let verdicts = stdin.clone();
        let deciding = std::sync::Arc::new(AtomicBool::new(false));
        let (send, replies) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                // A page cannot make this process buffer without limit.
                let reply = match (&mut reader).take(MAX_REPLY_BYTES + 1).read_line(&mut line) {
                    Ok(0) => break,
                    Ok(read) if read as u64 > MAX_REPLY_BYTES => {
                        Err(format!("sidecar reply exceeded {MAX_REPLY_BYTES} bytes"))
                    }
                    Ok(_) => {
                        if let Some(verdict) = request_verdict(&line, &mut policy) {
                            // A lost verdict is a refusal: the sidecar stops waiting.
                            if let Some(stdin) = verdicts.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
                                let _ = writeln!(stdin, "{verdict}").and_then(|()| stdin.flush());
                            }
                            continue;
                        }
                        if let Some(question) = approval_question(&line) {
                            answer_approval(question, approver.clone(), deciding.clone(), verdicts.clone());
                            continue;
                        }
                        Ok(line)
                    }
                    Err(err) => Err(err.to_string()),
                };
                let last = reply.is_err();
                if send.send(reply).is_err() || last {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            replies,
            next_id: 0,
            call_timeout,
            deadline: None,
            proxy_port: None,
        })
    }

    /// The sidecar is mid-line or unresponsive: stop it so every later call
    /// fails at once instead of waiting.
    fn stop(&mut self, why: String) -> String {
        *self.stdin.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let _ = self.child.kill();
        format!("{why}; the session was stopped")
    }

    /// `allow_private` lets IP-literal grants reach loopback and private
    /// addresses; only test-support fixtures pass true.
    fn start(
        script: &Path,
        session: &Session,
        headless: bool,
        allow_private: bool,
        policy: RequestPolicy,
    ) -> Result<Self, String> {
        Self::start_with(script, session, headless, allow_private, policy, None)
    }

    /// `start`, with takeover when `takeover` is given: a request held as the
    /// operator's step stays paused for its verdict instead of failing at once.
    fn start_with(
        script: &Path,
        session: &Session,
        headless: bool,
        allow_private: bool,
        policy: RequestPolicy,
        takeover: Option<Takeover>,
    ) -> Result<Self, String> {
        if !script
            .parent()
            .is_some_and(|dir| dir.join("node_modules/playwright").exists())
        {
            return Err("playwright is not prepared. From browser/, run PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --ignore-scripts. A session does not install it.".into());
        }
        let child = Command::new("node")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env_clear()
            .envs(child_environment())
            .env("PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD", "1")
            .spawn()
            .map_err(|err| format!("node sidecar failed to start: {err}"))?;
        let wait = takeover
            .as_ref()
            .map_or(Duration::ZERO, |takeover| takeover.wait.min(MAX_TAKEOVER_WAIT));
        // An op whose request is paused answers only after the verdict or the wait.
        let call_timeout = SIDECAR_CALL_TIMEOUT + wait;
        let mut sidecar = Self::attach(child, call_timeout, policy, takeover.map(|takeover| takeover.approver))?;
        let started = sidecar.call(json!({
            "op": "start",
            "grants": session.grant_strings(),
            "headless": headless,
            "allow_private": allow_private,
            "takeover_wait_ms": wait.as_millis() as u64,
        }))?;
        if started.get("ok") != Some(&Value::Bool(true)) {
            return Err(format!("sidecar refused start: {started}"));
        }
        sidecar.proxy_port = started["result"]["proxy_port"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok());
        Ok(sidecar)
    }
}

impl Driver for Sidecar {
    fn call(&mut self, mut message: Value) -> Result<Value, String> {
        self.next_id += 1;
        message["id"] = json!(self.next_id);
        {
            let mut stdin = self.stdin.lock().unwrap_or_else(|p| p.into_inner());
            let stdin = stdin.as_mut().ok_or("sidecar stdin closed")?;
            writeln!(stdin, "{message}").map_err(|err| err.to_string())?;
            stdin.flush().map_err(|err| err.to_string())?;
        }
        // A stuck renderer fails this call; the task's report still comes back.
        let timeout = self.deadline.map_or(self.call_timeout, |deadline| {
            self.call_timeout
                .min(deadline.saturating_duration_since(Instant::now()))
        });
        let line = match self.replies.recv_timeout(timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(why)) => return Err(self.stop(why)),
            Err(RecvTimeoutError::Timeout) if out_of_time(self.deadline).is_some() => {
                return Err(self.stop(OUT_OF_TIME.to_string()));
            }
            Err(RecvTimeoutError::Timeout) => {
                let secs = self.call_timeout.as_secs();
                return Err(self.stop(format!("sidecar did not answer within {secs} s")));
            }
            Err(RecvTimeoutError::Disconnected) => return Err("sidecar closed the pipe".into()),
        };
        serde_json::from_str(&line)
            .map_err(|err| format!("sidecar reply was not json: {err}: {}", truncate(&line, 300)))
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        drop(self.stdin.lock().unwrap_or_else(|p| p.into_inner()).take());
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

/// The answer to one sidecar `check` line, or `None` when `line` is an
/// ordinary reply. The sidecar writes `check` first on those lines and `id`
/// first on replies, so page text inside a reply cannot pass for a question.
/// A question without a URL is refused.
fn request_verdict(line: &str, policy: &mut RequestPolicy) -> Option<Value> {
    if !line.starts_with("{\"check\":") {
        return None;
    }
    let question: Value = serde_json::from_str(line).ok()?;
    let allow = question["url"].as_str().is_some_and(|url| !policy.refuses(url));
    Some(json!({"verdict": question["check"], "allow": allow}))
}

/// The sidecar's question about one paused request, or `None` when `line` is
/// anything else. As with `check`, the sidecar writes `approve` first on these
/// lines and `id` first on replies, so page text inside a reply cannot pass
/// for a question.
fn approval_question(line: &str) -> Option<Value> {
    if !line.starts_with("{\"approve\":") {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Answer one approval question off the reader thread. One decision is open
/// at a time; a second question, or any question with no approver, is refused
/// at once. A lost answer is a refusal: the sidecar stops waiting by itself.
fn answer_approval(
    question: Value,
    approver: Option<Approver>,
    deciding: std::sync::Arc<AtomicBool>,
    stdin: std::sync::Arc<std::sync::Mutex<Option<ChildStdin>>>,
) {
    let write = |allow: bool, id: &Value, stdin: &std::sync::Mutex<Option<ChildStdin>>| {
        let verdict = json!({"verdict": id, "allow": allow});
        if let Some(stdin) = stdin.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            let _ = writeln!(stdin, "{verdict}").and_then(|()| stdin.flush());
        }
    };
    let Some(approver) = approver.filter(|_| !deciding.swap(true, Ordering::AcqRel)) else {
        write(false, &question["approve"], &stdin);
        return;
    };
    thread::spawn(move || {
        let allow = approver(&question);
        deciding.store(false, Ordering::Release);
        write(allow, &question["approve"], &stdin);
    });
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
        if let Some(origin) = reply["redirect_origin"].as_str() {
            return json!({"error": "Page redirect requires a separate browser grant", "redirect_origin": origin});
        }
        return json!({"error": reply.get("error").cloned().unwrap_or(reply)});
    }
    let result = reply.get("result").cloned().unwrap_or(json!({}));
    // What the page showed is what a model-driven task may later navigate to
    // or type (#901): its URL, text, element names and option labels.
    let raw_elements = result["elements"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let shown: Vec<&str> = result["text"]
        .as_str()
        .into_iter()
        .chain(raw_elements.iter().filter_map(|el| el["name"].as_str()))
        .chain(
            raw_elements
                .iter()
                .flat_map(|el| el["options"].as_array().into_iter().flatten())
                .filter_map(Value::as_str),
        )
        .collect();
    session.saw_page(result["url"].as_str(), &shown);
    let mut outcome = json!({"url": result.get("url"), "text": result.get("text")});
    if let Some(held) = result
        .get("held")
        .filter(|held| held.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["held_for_operator"] = held.clone();
    }
    // Requests a page sent on its own: never sent, listed, and the task goes on.
    if let Some(held) = result
        .get("held_background")
        .filter(|held| held.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["held_in_background"] = held.clone();
    }
    // Requests the operator approved and the gate then sent.
    if let Some(sent) = result
        .get("approved")
        .filter(|sent| sent.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["approved_by_operator"] = sent.clone();
    }
    if let Some(origins) = result
        .get("not_granted")
        .filter(|origins| origins.as_array().is_some_and(|rows| !rows.is_empty()))
    {
        outcome["not_granted"] = origins.clone();
    }
    if result["blocked"].as_u64().is_some_and(|count| count > 0) {
        outcome["blocked_by_policy"] = result["blocked"].clone();
    }
    // Page-written, like the text: shown to the model, never obeyed by the host.
    if let Some(dialogs) = result
        .get("dialogs")
        .and_then(Value::as_array)
        .filter(|rows| !rows.is_empty())
    {
        let shown: Vec<String> = dialogs
            .iter()
            .filter_map(Value::as_str)
            .take(MAX_SHOWN_DIALOGS)
            .map(|text| truncate(text, MAX_SHOWN_DIALOG_CHARS))
            .collect();
        outcome["dialogs"] = json!(shown);
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
                        // The full target stays in the fact that follow_link uses.
                        if let Some(href) = fact.and_then(|fact| fact.href.as_deref()) {
                            row["href"] = json!(shown_href(href));
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
/// it to the operator), `held` (the sidecar must hold a request as the
/// operator's step), `background` (it must hold one as the page's own, and
/// none as the operator's) or `shows <text>` (the step's outcome must contain
/// it). Any other refusal or sidecar error fails the run.
fn drive(mut session: Session, script: &Path, steps: &[Value], headless: bool) -> Result<String, String> {
    // Fixtures serve on 127.0.0.1: a private grant needs a test-support build.
    let mut sidecar = Sidecar::start(
        script,
        &session,
        headless,
        cfg!(feature = "test-support"),
        RequestPolicy::open(),
    )?;
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
            Some("handoff") if handed_off(&outcome) => continue,
            Some("held") if outcome.get("held_for_operator").is_some() => {}
            Some("background")
                if outcome.get("held_in_background").is_some() && outcome.get("held_for_operator").is_none() => {}
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
/// without a tool call; a spent turn budget is not success. With a `deadline`,
/// a model call gets the time left, not a fixed limit, and no turn starts
/// after it, so the report is printed before the server stops the process.
pub fn run_task(
    script: &Path,
    headless: bool,
    (model, api, deadline): (&str, &str, Option<Instant>),
    task: &str,
    origins: &[String],
    max_turns: usize,
    allow_private: bool,
) -> Result<Value, String> {
    let mut session = Session::general(origins).map_err(|err| format!("{err:?}"))?;
    // Origins only: a requested URL's path or query (a token, say) never reaches
    // the prompt or the report.
    let grants = session.grant_strings();
    // A fixture session reaches only the loopback page this binary serves,
    // which the public-URL block list cannot check and would always refuse.
    let policy = if allow_private {
        RequestPolicy::open()
    } else {
        RequestPolicy::shared()
    };
    // Only a job started with a wait has someone to ask (#995).
    let takeover = JOB_TAKEOVER.get().cloned();
    let mut sidecar = Sidecar::start_with(script, &session, headless, allow_private, policy, takeover)?;
    sidecar.deadline = deadline;
    let tools = browser_tools();
    let mut chat = |messages: &[Value]| {
        ollama_chat(api, model, messages, &tools, call_timeout(deadline))
            .map_err(|err| out_of_time(deadline).unwrap_or(err))
    };
    let report = model_loop(
        &mut session,
        &mut sidecar,
        &mut chat,
        &|session: &Session| {
            job_turn()
                .or_else(|| out_of_time(deadline))
                .or_else(|| (!allow_private).then(|| policy_refusal_now(session)).flatten())
        },
        task,
        &grants,
        max_turns,
    );
    drop(sidecar);
    let mut report = report?;
    report["model"] = json!(model);
    Ok(report)
}

/// Why a job's task stopped once its server asked it to, or went away.
pub const JOB_CANCELLED_REASON: &str = "cancelled by the operator";

/// True in a `browser run --job` child: a server holds its stdin and reads
/// its stdout line by line.
static JOB_MODE: AtomicBool = AtomicBool::new(false);
/// Set once that server sent `cancel` or closed the pipe.
static JOB_CANCELLED: AtomicBool = AtomicBool::new(false);
static JOB_TURNS: AtomicUsize = AtomicUsize::new(0);

/// Set by `browser run --job --takeover-wait-sec=N`: the job's server decides
/// held requests, through this process's stdout and stdin.
static JOB_TAKEOVER: std::sync::OnceLock<Takeover> = std::sync::OnceLock::new();

/// One line of a job's stdin: stop, or the operator's answer to the request
/// with this id. Anything else is ignored.
#[derive(Debug, PartialEq)]
enum JobInput {
    Cancel,
    Verdict { id: u64, allow: bool },
}

fn job_input(line: &str) -> Option<JobInput> {
    let line = line.trim();
    if line == "cancel" {
        return Some(JobInput::Cancel);
    }
    let message: Value = serde_json::from_str(line).ok()?;
    let allow = match message["verdict"].as_str()? {
        "approve" => true,
        "deny" => false,
        _ => return None,
    };
    Some(JobInput::Verdict {
        id: message["id"].as_u64()?,
        allow,
    })
}

/// When a held request stops waiting: after `wait`, or at the task deadline.
fn hold_until(now: Instant, wait: Duration, deadline: Option<Instant>) -> Instant {
    deadline.map_or(now + wait, |deadline| deadline.min(now + wait))
}

/// Job mode for this process: read stdin on a thread until a `cancel` line
/// or end of input. Either ends the task before its next model turn, so a
/// task whose server is gone does not keep driving Chrome. With `wait`, a
/// request held as the operator's step is announced on stdout and waits that
/// long for a verdict line, or until the task `deadline`; cancel, end of input
/// and silence all mean no.
fn watch_job_input(wait: Option<Duration>, deadline: Option<Instant>) {
    JOB_MODE.store(true, Ordering::Release);
    let (verdicts, answers) = mpsc::channel::<(u64, bool)>();
    if let Some(wait) = wait {
        let answers = std::sync::Mutex::new(answers);
        let approver: Approver = std::sync::Arc::new(move |question: &Value| {
            let Some(id) = question["approve"].as_u64() else {
                return false;
            };
            let now = Instant::now();
            let until = hold_until(now, wait, deadline);
            let wait_sec = until.saturating_duration_since(now).as_secs();
            job_event(json!({"event": "waiting", "wait_sec": wait_sec, "question": question}));
            let answers = answers.lock().unwrap_or_else(|p| p.into_inner());
            let decision = loop {
                match answers.recv_timeout(until.saturating_duration_since(Instant::now())) {
                    // An answer to an earlier question is not this one's.
                    Ok((answered, _)) if answered != id => continue,
                    Ok((_, true)) => break "approved",
                    Ok((_, false)) => break "denied",
                    Err(RecvTimeoutError::Timeout) => break "expired",
                    Err(RecvTimeoutError::Disconnected) => break "cancelled",
                }
            };
            job_event(json!({"event": "resumed", "id": id, "decision": decision}));
            decision == "approved"
        });
        let _ = JOB_TAKEOVER.set(Takeover { wait, approver });
    }
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match job_input(&line) {
                Some(JobInput::Cancel) => break,
                Some(JobInput::Verdict { id, allow }) => {
                    let _ = verdicts.send((id, allow));
                }
                None => {}
            }
        }
        JOB_CANCELLED.store(true, Ordering::Release);
        // Dropping the sender tells a waiting approver the answer is no.
    });
}

/// One line for the job's server. A server that went away closes this pipe;
/// the cancel flag then ends the task.
fn job_event(event: Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}

/// Before each model turn of a job: stop if cancelled, else print one
/// `{"event":"turn"}` line so the server can show progress. `None` outside a job.
fn job_turn() -> Option<String> {
    if !JOB_MODE.load(Ordering::Acquire) {
        return None;
    }
    if JOB_CANCELLED.load(Ordering::Acquire) {
        return Some(JOB_CANCELLED_REASON.to_string());
    }
    let turn = JOB_TURNS.fetch_add(1, Ordering::AcqRel) + 1;
    job_event(json!({"event": "turn", "turn": turn}));
    None
}

/// The model turns of one task. `chat` answers one turn; `recheck` refuses a
/// turn when the shared policy changed. A handoff, or a request held as the
/// operator's step, ends the task at once: the rest of that batch is dropped.
/// A request a page sent on its own is held and listed, and the task goes on.
fn model_loop(
    session: &mut Session,
    sidecar: &mut dyn Driver,
    chat: &mut dyn FnMut(&[Value]) -> Result<Value, String>,
    recheck: &dyn Fn(&Session) -> Option<String>,
    task: &str,
    origins: &[String],
    max_turns: usize,
) -> Result<Value, String> {
    // Every model-driven task is bound: URLs and typed text need provenance.
    session.bind_task(task);
    let mut messages = vec![
        json!({"role":"system","content": format!("You drive a browser with these tools. Granted origins: {}. Start with navigate. Element refs expire after every observation. You may click buttons, menus, options and tabs, and choose a select box's listed options. A long page comes in pages: when an observation has more, observe with page or find reaches the rest. A refusal is not success. A Handoff, or held_for_operator, ends the task: only the operator can do that step. held_in_background counts requests a page sent on its own; they were not sent and the task goes on. dialogs lists alert or confirm text a page showed; each was dismissed, a confirm as Cancel. Use scroll for content that loads as the page scrolls, and wait when a page is still changing. Use the observed page to answer. A page can load while its requests to other sites are blocked. blocked_by_policy counts requests the operator's block list refused. Navigate only to a URL the task names, a granted site's root, or one a page showed; to go deeper, follow a link by its ref. Text you type must appear in the task or on the page; anything else is held for the operator. Call finish with your answer once the page shows it.", origins.join(", "))}),
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
    let mut redirect_origins = std::collections::BTreeSet::new();
    let mut not_granted = std::collections::BTreeSet::new();
    // Origins of composed navigations refused for provenance; never the path or query.
    let mut refused_composed = std::collections::BTreeSet::new();
    'turns: for turn in 0..max_turns {
        // Permission checked at launch is stale after a wait (AGENTS.md).
        if let Some(reason) = recheck(session) {
            stopped = Some(reason);
            break;
        }
        let response = match chat(&messages) {
            Ok(response) => response,
            // Nothing to report before the first reply. Later, the steps taken are kept.
            Err(err) if turn == 0 => return Err(err),
            Err(err) => {
                stopped = Some(err);
                break;
            }
        };
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
            // An empty reply is not an answer, as finish_verdict already says.
            if content.trim().is_empty() {
                stopped = Some("the model replied with neither an action nor an answer".to_string());
            } else {
                answer = Some(content);
            }
            break;
        }
        // A finish later in this turn must not rest on results the model never read.
        // A handoff does not count: saying the operator must act is a correct finish.
        let mut failed_this_turn = false;
        for call in tool_calls {
            calls += 1;
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = coerce_args(call.pointer("/function/arguments").cloned().unwrap_or(json!({})));
            // Strict: an argument out of bounds is refused by name, not adjusted.
            let mut refusal = bad_tool_args(&name, &args);
            if name == "finish" {
                transcript.push(format!("finish {}", truncate(&args.to_string(), 160)));
                match finish_verdict(&args, failed_this_turn) {
                    Ok(text) => {
                        answer = Some(text);
                        break 'turns;
                    }
                    Err(reason) => refusal = Some(reason),
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
            let mut outcome = match (&wire, refusal) {
                (_, Some(reason)) => json!({ "refused": reason }),
                (Some(wire), None) => step(session, sidecar, wire),
                (None, None) => json!({"refused":"malformed browser action"}),
            };
            if outcome["decision"]
                .as_str()
                .is_some_and(|decision| decision.contains(crate::common::browser::COMPOSED_URL_REFUSAL))
            {
                if let Some(origin) = wire
                    .as_ref()
                    .and_then(|wire| wire["url"].as_str())
                    .and_then(|url| url::Url::parse(url).ok())
                    .map(|url| url.origin().ascii_serialization())
                {
                    if refused_composed.len() < MAX_FOR_OPERATOR {
                        refused_composed.insert(origin);
                    }
                }
            }
            let needs_operator = handoff.is_some() || outcome.get("held_for_operator").is_some();
            note_for_operator(&mut for_operator, handoff, &outcome);
            // The operator's rows keep each request. The model gets the count:
            // a page writes those URLs, and nothing it must act on is in them.
            if let Some(count) = outcome["held_in_background"].as_array().map(Vec::len) {
                outcome["held_in_background"] = json!(count);
            }
            if let Some(count) = outcome["approved_by_operator"].as_array().map(Vec::len) {
                outcome["approved_by_operator"] = json!(count);
            }
            if let Some(origin) = outcome["redirect_origin"].as_str() {
                if redirect_origins.len() < MAX_FOR_OPERATOR {
                    redirect_origins.insert(origin.to_owned());
                }
            }
            // Page-requested origins belong in the report. The model has no action to take on them.
            if let Some(Value::Array(origins)) = outcome.as_object_mut().and_then(|obj| obj.remove("not_granted")) {
                for origin in origins {
                    if let (Some(origin), true) = (origin.as_str(), not_granted.len() < MAX_FOR_OPERATOR) {
                        not_granted.insert(origin.to_owned());
                    }
                }
            }
            if let Some(text) = outcome.get("text").and_then(Value::as_str) {
                last_text = text.to_string();
                last_url = outcome["url"].clone();
            }
            transcript.push(format!("{name} {}", truncate(&outcome.to_string(), 240)));
            // The prompt asks the model to stop; the host makes sure it does.
            if needs_operator {
                stopped = Some("needs the operator: a step was handed off or a request was held".to_string());
                break 'turns;
            }
            if outcome.get("elements").is_some() {
                retire_observations(&mut messages);
            }
            let failed = action_failed(&outcome);
            failed_this_turn |= blocks_finish(&outcome);
            refused_in_a_row = if failed { refused_in_a_row + 1 } else { 0 };
            messages.push(json!({"role":"tool","tool_name": name,"content": outcome.to_string()}));
            // A model that keeps retrying refused actions only spends the deadline.
            if refused_in_a_row >= MAX_REFUSALS_IN_A_ROW {
                stopped = Some("three refused or failed actions in a row".to_string());
                break 'turns;
            }
        }
    }
    Ok(json!({
        "finished": answer.is_some(),
        "answer": answer,
        "stopped": stopped,
        "origins": origins,
        "model_calls_with_tools": calls,
        "for_operator": for_operator,
        "not_granted": not_granted,
        "redirect_origins": redirect_origins,
        "refused_composed": refused_composed,
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
        (model, api, None),
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

/// Whether a tool outcome is a handoff: only the operator can do that step.
/// It still counts as failed toward `MAX_REFUSALS_IN_A_ROW`.
fn handed_off(outcome: &Value) -> bool {
    outcome["decision"]
        .as_str()
        .is_some_and(|decision| decision.starts_with("Handoff"))
}

/// Whether an outcome makes a later `finish` in the same turn premature: a
/// refusal or failure the model has not read yet. A handoff does not, because
/// the system prompt tells the model to say so and stop.
fn blocks_finish(outcome: &Value) -> bool {
    action_failed(outcome) && !handed_off(outcome)
}

/// The answer a `finish` call ends the task with, or why it is refused. A
/// finish sent after a refused or failed call in the same turn is refused: the
/// model sent it before it could read that result, so its answer cannot rest
/// on the page. `blocks_finish` sets that flag and exempts handoffs. A finish
/// first in its turn, or after only successful calls or handoffs, ends the task
/// when it carries a non-blank answer.
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
/// `held_background` rows are requests a page sent on its own; `approved` rows
/// are requests the operator let through, which were sent. At the cap, a hold,
/// handoff or approval replaces the newest background row.
fn note_for_operator(rows: &mut Vec<Value>, handoff: Option<Value>, outcome: &Value) {
    let held = outcome["held_for_operator"].as_array().into_iter().flatten();
    // Each approval sent a request, so each is listed, repeats included.
    let approved = outcome["approved_by_operator"].as_array().into_iter().flatten();
    for row in held
        .map(|request| json!({"held": request}))
        .chain(handoff)
        .chain(approved.map(|request| json!({"approved": request})))
    {
        if row.get("approved").is_none() && rows.contains(&row) {
            continue;
        }
        if rows.len() >= MAX_FOR_OPERATOR {
            let Some(newest) = rows.iter().rposition(|shown| shown.get("held_background").is_some()) else {
                continue;
            };
            rows.remove(newest);
        }
        rows.push(row);
    }
    let background = outcome["held_in_background"].as_array().into_iter().flatten();
    for row in background.map(|request| json!({"held_background": request})) {
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
        "scroll" => {
            json!({"action":"scroll","dy": if args["direction"] == "up" { -SCROLL_STEP_PX } else { SCROLL_STEP_PX }})
        }
        "wait" => json!({"action":"wait","timeout_ms": wait_seconds(args).unwrap_or(0) * 1_000}),
        "type" => json!({"action":"type","ref_id": args["ref_id"],"text": args["text"]}),
        "select" => json!({"action":"select","ref_id": args["ref_id"],"value": args["option"]}),
        "press_enter" | "click" | "follow_link" => json!({"action": name,"ref_id": args["ref_id"]}),
        _ => return None,
    })
}

/// The whole seconds a `wait` call asks for. Models send numbers as text.
fn wait_seconds(args: &Value) -> Option<u64> {
    match &args["seconds"] {
        Value::String(text) => text.trim().parse().ok(),
        other => other.as_u64(),
    }
}

/// Why a `scroll` or `wait` call is refused before it reaches the page, naming
/// the argument. Other tools are judged by the policy kernel.
fn bad_tool_args(name: &str, args: &Value) -> Option<&'static str> {
    match name {
        "scroll" if !matches!(args["direction"].as_str(), Some("down" | "up")) => {
            Some("scroll takes direction: down or up")
        }
        "wait" if !wait_seconds(args).is_some_and(|secs| (1..=MAX_WAIT_SECONDS).contains(&secs)) => {
            Some("wait takes seconds: a whole number from 1 to 5")
        }
        _ => None,
    }
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
        {"type":"function","function":{"name":"scroll","description":"Scroll the page down or up, for content that loads as the page scrolls. Returns a fresh observation.","parameters":{"type":"object","properties":{"direction":{"type":"string","enum":["down","up"]}},"required":["direction"]}}},
        {"type":"function","function":{"name":"wait","description":"Wait 1 to 5 seconds while the page is still changing, then read it again.","parameters":{"type":"object","properties":{"seconds":{"type":"integer"}},"required":["seconds"]}}},
        {"type":"function","function":{"name":"follow_link","description":"Follow a link ref from the latest observation.","parameters":{"type":"object","properties":{"ref_id":{"type":"string"}},"required":["ref_id"]}}},
        {"type":"function","function":{"name":"finish","description":"Stop after the visible paragraph is correct.","parameters":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}}}
    ])
}

/// The time one model call gets: what is left of the task, or the fixed limit.
fn call_timeout(deadline: Option<Instant>) -> Duration {
    deadline.map_or(MODEL_CALL_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_secs(1))
    })
}

fn out_of_time(deadline: Option<Instant>) -> Option<String> {
    deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
        .then(|| OUT_OF_TIME.to_string())
}

fn ollama_chat(api: &str, model: &str, messages: &[Value], tools: &Value, timeout: Duration) -> Result<Value, String> {
    // No options block: context length is server-side, so num_ctx is never sent.
    // think stays off whatever reasoning_effort says: a reasoning pass on each
    // of up to 40 turns does not fit a task deadline on a local model.
    let payload = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": false,
        "think": false,
    });
    let response = ollama_client(timeout)?
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
    // A signed-in Ollama forwards a cloud tag's chat, the task and every page
    // observation, to ollama.com. Every model call here uses this model.
    if crate::llm::ollama::cloud_tag(&model) {
        return Err(format!(
            "{model} is an Ollama cloud model (a :cloud or -cloud tag); browser tasks keep prompts on this machine"
        ));
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

/// A link target as the model sees it. Tracking links run to kilobytes, and
/// forty of them would fill the 32K window; a cut one ends in `…`.
fn shown_href(href: &str) -> String {
    if href.chars().count() <= MAX_SHOWN_HREF_CHARS {
        href.to_string()
    } else {
        format!("{}…", truncate(href, MAX_SHOWN_HREF_CHARS))
    }
}

/// A POST the page sends as it loads is held as the page's own; a
/// target=_blank link replaces the page; Enter submits a GET search; a select
/// box takes a listed option; a password field hands off; Enter in a POST
/// form and a button whose script POSTs are both held as the operator's
/// step; and the page's WebRTC sends nothing to a UDP port the session never
/// granted.
pub fn run_enter_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let stun = std::net::UdpSocket::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    stun.set_nonblocking(true).map_err(|err| err.to_string())?;
    let stun_port = stun.local_addr().map_err(|err| err.to_string())?.port();
    let page = SEARCH_PAGE
        .replace("STUN_PORT", &stun_port.to_string())
        .replace("LONG_OPTION", LONG_OPTION);
    let (origin, port) = serve_html(&page)?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let steps = [
        // The page POSTs as it loads. That is the page's own request, whether
        // the model navigated, followed a link or opened a tab: held, listed
        // and not the operator's step.
        json!({"action":"navigate","url": format!("http://127.0.0.1:{port}/"),"expect":"background"}),
        json!({"action":"follow_link","ref_id":"@Load again","expect":"background"}),
        json!({"action":"follow_link","ref_id":"@Open in a new tab","expect":"background"}),
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
    let log = drive(session, sidecar, &steps, headless)?;
    let last = log.lines().next_back().unwrap_or("");
    if !["second tab", "submitted github cgfixit", "picked l", "shipped f"]
        .iter()
        .all(|want| last.contains(want))
    {
        return Err(format!("fixture text was not visible:\n{log}"));
    }
    // Both tabs ran the probe during the steps; one datagram means UDP got past the proxy.
    if stun.recv_from(&mut [0u8; 512]).is_ok() {
        return Err(format!("the page's WebRTC sent UDP around the proxy:\n{log}"));
    }
    Ok(log)
}

/// A page that renders after a fetch is read once it has; a tab the page opens
/// on a site the task was not granted is closed, the page stays and the site
/// is named; `scroll` brings content that loads on scroll, `wait` content that
/// arrives late, and an alert's text is reported; `observe` with `page` or
/// `find` reaches past the first 40 elements and 4,000 characters; and a link
/// whose label has inline markup is listed.
pub fn run_reach_demo(sidecar: &Path, headless: bool) -> Result<String, String> {
    let entries: String = (1..=60)
        .map(|n| {
            format!(
                "<a href=\"/?entry={n}\">Entry {n}</a><p>{}</p>",
                "filler words here ".repeat(8)
            )
        })
        .collect();
    // "İ" (U+0130) lowercases to two UTF-16 units, so 200 of them put a
    // lowercased index 200 units past the same place in the page text.
    let page = REACH_PAGE
        .replace("ENTRIES", &entries)
        .replace("DOTTED_CAPITALS", &"&#304;".repeat(200));
    let (origin, port) = serve_html(&page)?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let steps = [
        json!({"action":"navigate","url": format!("http://127.0.0.1:{port}/"),"expect":"shows late content"}),
        json!({"action":"click","ref_id":"@Share","expect":"shows http://127.0.0.1:1"}),
        // Content that appears on scroll, content that appears after a delay,
        // and an alert's text: each shows in the step's own observation.
        json!({"action":"scroll","dy": SCROLL_STEP_PX,"expect":"shows LAZY-LOADED"}),
        json!({"action":"click","ref_id":"@Start timer","expect":"shows timer idle"}),
        json!({"action":"wait","timeout_ms": 2_500,"expect":"shows TIMER-DONE"}),
        json!({"action":"click","ref_id":"@Notify","expect":"shows alert: Saved draft 7"}),
        json!({"action":"observe","page":3,"expect":"shows RELEASE-MARKER"}),
        // The excerpt around a match is cut from the page text as written.
        json!({"action":"observe","find":"dotted-capital","expect":"shows DOTTED-CAPITAL marker"}),
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

/// One raw sidecar op for the guard fixture: its result and the reply's size.
fn guard_call(sidecar: &mut Sidecar, lines: &mut Vec<String>, op: Value) -> Result<(Value, usize), String> {
    let reply = sidecar.call(op.clone())?;
    let size = reply.to_string().len();
    if reply["ok"] != true {
        return Err(format!("sidecar refused {op}: {}", truncate(&reply.to_string(), 300)));
    }
    lines.push(format!("{} -> {} bytes", op["op"], size));
    Ok((reply["result"].clone(), size))
}

/// Request lines the guard fixture's server has received, as `METHOD /path`.
type Received = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// What the guard fixture's loopback server answers for one path.
enum Reply {
    Html(String),
    /// HTML sent without `content-length` (the connection's close ends it),
    /// so the sidecar learns the size only by reading.
    Unsized(String),
    Script(&'static str),
    Redirect(&'static str),
}

/// A loopback server that answers fixed routes and records `METHOD /path` for
/// every request it receives, so a guard is checked by what reached the
/// server, not by an error the browser shows.
fn serve_recording(routes: Vec<(&'static str, Reply)>) -> Result<(String, u16, Received), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for _ in 0..256 {
            let Ok((mut socket, _)) = listener.accept() else { break };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
            // One connection at a time: a body the browser stops reading (the
            // refused `/huge`) must not stall the routes after it.
            let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
            let mut head = Vec::new();
            let mut buf = [0u8; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
                match socket.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            let line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
            let mut parts = line.split(' ');
            let method = parts.next().unwrap_or("").to_string();
            let target = parts.next().unwrap_or("");
            let path = target.split('?').next().unwrap_or("").to_string();
            log.lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("{method} {path}"));
            let (status, kind, body, extra) = match routes.iter().find(|(route, _)| *route == path).map(|(_, r)| r) {
                Some(Reply::Html(body)) => (
                    "200 OK",
                    "text/html",
                    format!("<!doctype html><html><body>{body}</body></html>"),
                    String::new(),
                ),
                Some(Reply::Unsized(body)) => {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nconnection: close\r\n\r\n";
                    let _ =
                        socket.write_all(format!("{head}<!doctype html><html><body>{body}</body></html>").as_bytes());
                    continue;
                }
                Some(Reply::Script(body)) => ("200 OK", "text/javascript", (*body).to_string(), String::new()),
                Some(Reply::Redirect(to)) => (
                    "301 Moved Permanently",
                    "text/plain",
                    String::new(),
                    format!("location: {to}\r\n"),
                ),
                None => ("404 Not Found", "text/plain", String::new(), String::new()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
        }
    });
    Ok((format!("http://127.0.0.1:{port}"), port, seen))
}

/// The status line the sidecar proxy gives a request sent without this
/// session's credential.
fn unauthenticated_proxy_status(proxy_port: u16, request: &str) -> Result<String, String> {
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", proxy_port)).map_err(|err| err.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| err.to_string())?;
    socket.write_all(request.as_bytes()).map_err(|err| err.to_string())?;
    let mut reply = Vec::new();
    let mut buf = [0u8; 512];
    while !reply.contains(&b'\n') && reply.len() < 4096 {
        match socket.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => reply.extend_from_slice(&buf[..n]),
        }
    }
    Ok(String::from_utf8_lossy(&reply).lines().next().unwrap_or("").to_string())
}

/// Characters of filler text on the caps page; a reply that carried it all
/// would be over a megabyte.
const BIG_TEXT_CHARS: usize = 1_140_000;
/// Length of the caps page's long link target, over the sidecar's 8,192.
const LONG_HREF_CHARS: usize = 20_000;
/// A capped observation (40 elements, 8,192-character hrefs, 4,000 characters
/// of text) stays well under this; the uncapped caps page is over a megabyte.
const MAX_GUARDED_REPLY_BYTES: usize = 400_000;

/// The sidecar guards from #921, #922 and #933 (#945). Each is checked by what
/// the loopback fixture server received or by the sidecar's raw reply, so a
/// guard that stops working fails here even when nothing looks wrong:
/// - no dedicated, shared or blob worker starts, so none of their requests or
///   WebSockets reach the server (`worker-src 'none'`);
/// - a same-origin redirect lands on its target (the network gate lets
///   redirects through to be checked again);
/// - an observation of a 1.14 MB page with a 20 KB link stays small, its text
///   is cut at 4,000 characters and the long href is dropped;
/// - the sidecar proxy answers 407 to an unauthenticated CONNECT and to
///   absolute-form GETs, and the sidecar keeps answering afterwards;
/// - a block rule on a path holds inside the granted origin (#899): see
///   `guard_path_denials`;
/// - a held POST never reaches the server, whether the page sent it on its
///   own or the model's click caused it (#894): see `guard_holds`.
///
/// The session's verdicts come from a policy file through `RequestPolicy`, as
/// a task's do; only the loopback origin's name is swapped for the rules.
pub fn run_guards_demo(script: &Path, headless: bool) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("cgah-guards-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let result = guards_with_policy(script, headless, &dir.join("web_allowlist.json"));
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The name block rules use for the guard fixture's loopback origin.
const GUARD_RULE_HOST: &str = "guards.fixture.example";

/// Replaces the fixture's policy file in one rename with a denylist that
/// blocks `patterns`.
fn write_guard_policy(file: &Path, patterns: &[String]) -> Result<(), String> {
    use crate::server::web_policy::{Mode, Policy};
    let mut policy = Policy::empty();
    policy.mode = Mode::Denylist;
    for pattern in patterns {
        policy
            .blocked
            .push(Policy::block_rule(pattern).map_err(|err| err.message)?);
    }
    let bytes = serde_json::to_vec(&policy).map_err(|err| err.to_string())?;
    let staged = file.with_extension("staged");
    std::fs::write(&staged, bytes).map_err(|err| err.to_string())?;
    std::fs::rename(&staged, file).map_err(|err| err.to_string())
}

/// A page inside the granted origin that asks for blocked paths every way a
/// page can without the model: an image, a redirected image, a fetch, a
/// redirected fetch, and a fetch every 300 ms for the policy-edit check.
const GUARD_DENIALS_PAGE: &str = r#"
<p>DOCS-OK</p>
<a href="/private/page">Private link</a>
<img src="/private/pixel"><img src="/to-private">
<script>
fetch("/private/api").catch(() => {});
fetch("/to-private").catch(() => {});
setInterval(() => fetch("/later/ping").catch(() => {}), 300);
</script>
"#;

/// Path-level block rules inside a granted origin (#899), by what the server
/// received:
/// - no request reaches `/private/` from a page's own subresources, from a
///   redirect to it, from a navigation or from a redirect chain, while the
///   rest of the origin still loads;
/// - a rule added while a page is open stops that page's next requests with
///   no model turn in between;
/// - with the policy file gone nothing new is sent, and browsing resumes when
///   it is back.
fn guard_path_denials(
    sidecar: &mut Sidecar,
    lines: &mut Vec<String>,
    (base, rules, file): (&str, &str, &Path),
    seen: &Received,
) -> Result<(), String> {
    let received = || seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let count = |row: &str| received().iter().filter(|seen| *seen == row).count();
    let private = || -> Vec<String> {
        received()
            .into_iter()
            .filter(|row| row.contains(" /private/"))
            .collect()
    };
    let none_private = |after: &str| match private() {
        leaked if leaked.is_empty() => Ok(()),
        leaked => Err(format!("a blocked path was requested {after}: {leaked:?}")),
    };
    let refused = |sidecar: &mut Sidecar, path: &str| -> Result<String, String> {
        let reply = sidecar.call(json!({"op":"navigate","url": format!("{base}{path}")}))?;
        match reply["ok"] == true {
            true => Err(format!("navigating to {path} was not refused")),
            false => Ok(reply["error"].as_str().unwrap_or("").to_string()),
        }
    };
    let only_private = [format!("{rules}/private/*")];

    let (page, _) = guard_call(sidecar, lines, json!({"op":"navigate","url": format!("{base}/docs")}))?;
    if !page["text"].as_str().unwrap_or("").contains("DOCS-OK") || page["blocked"].as_u64().unwrap_or(0) == 0 {
        return Err(format!(
            "the docs page did not load, or reported no blocked request: {page}"
        ));
    }
    guard_call(sidecar, lines, json!({"op":"wait","timeout_ms": 1000}))?;
    none_private("by a page's subresources or their redirects")?;
    if count("GET /to-private") == 0 || count("GET /later/ping") == 0 {
        return Err(format!(
            "allowed paths beside a blocked one stopped loading: {:?}",
            received()
        ));
    }

    let why = refused(sidecar, "/private/page")?;
    let chain = refused(sidecar, "/hop")?;
    if !why.contains("block list") || !chain.contains("block list") {
        return Err(format!("a blocked navigation did not say why: {why} / {chain}"));
    }
    none_private("by a navigation or a redirect chain")?;
    lines.push(format!("blocked navigate -> {why}"));

    // A rule added while the page is open: no op runs during the sleep, so
    // the page's requests are judged with no model turn in between.
    guard_call(sidecar, lines, json!({"op":"navigate","url": format!("{base}/docs")}))?;
    write_guard_policy(file, &[only_private[0].clone(), format!("{rules}/later/*")])?;
    thread::sleep(Duration::from_millis(700));
    let pings = count("GET /later/ping");
    thread::sleep(Duration::from_millis(1500));
    if count("GET /later/ping") != pings {
        return Err("a path blocked while the page was open was still requested".into());
    }
    lines.push(format!("rule added mid-page -> /later/ping stayed at {pings} requests"));

    // Unreadable policy: nothing new is sent. Then it is back, and so is browsing.
    std::fs::remove_file(file).map_err(|err| err.to_string())?;
    let landed = count("GET /new");
    refused(sidecar, "/new")?;
    if count("GET /new") != landed {
        return Err("a request was sent while the policy could not be read".into());
    }
    write_guard_policy(file, &only_private)?;
    let (page, _) = guard_call(sidecar, lines, json!({"op":"navigate","url": format!("{base}/new")}))?;
    if !page["text"].as_str().unwrap_or("").contains("REDIRECT-LANDED") {
        return Err(format!("browsing did not resume with the policy back: {page}"));
    }
    none_private("after the policy edits")
}

/// A page that POSTs as it loads, with a button whose click POSTs.
const GUARD_HOLDS_PAGE: &str = r#"
<p>HOLDS-PAGE</p>
<button onclick="fetch('/save', {method: 'POST', body: 'order'}).catch(() => {})">Save</button>
<script>fetch('/autosave', {method: 'POST', body: 'draft'}).catch(() => {});</script>
"#;

/// The two kinds of held request (#894), by what the server received: a POST
/// the page sends on its own is listed as background, a POST the model's click
/// causes is listed as the operator's step, and neither reaches the server.
fn guard_holds(sidecar: &mut Sidecar, lines: &mut Vec<String>, base: &str, seen: &Received) -> Result<(), String> {
    let rows = |page: &Value, key: &str| -> Vec<String> {
        page[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row.as_str().map(str::to_string))
            .collect()
    };
    let (page, _) = guard_call(sidecar, lines, json!({"op":"navigate","url": format!("{base}/holds")}))?;
    if rows(&page, "held_background") != [format!("POST {base}/autosave")] || !rows(&page, "held").is_empty() {
        return Err(format!("a page's own POST was not held as background: {page}"));
    }
    let save = page["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|el| el["name"] == "Save")
        .and_then(|el| el["ref"].as_str())
        .ok_or("the Save button was not listed")?
        .to_string();
    let (page, _) = guard_call(sidecar, lines, json!({"op":"click","target":{"ref": save}}))?;
    if rows(&page, "held") != [format!("POST {base}/save")] || !rows(&page, "held_background").is_empty() {
        return Err(format!("a click's POST was not held as the operator's step: {page}"));
    }
    let sent: Vec<String> = seen
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|row| !row.starts_with("GET "))
        .cloned()
        .collect();
    if !sent.is_empty() {
        return Err(format!("a held request reached the server: {sent:?}"));
    }
    lines.push("holds -> page's own POST listed as background, click's POST as the operator's; neither sent".into());
    Ok(())
}

/// A shop page for the takeover fixture: a button whose click POSTs, one that
/// sends the identical POST again once the first is answered, and a form.
const TAKEOVER_PAGE: &str = r#"
<p>SHOP</p>
<button onclick="save()">Save</button>
<button onclick="saveTwice()">Save twice</button>
<form method="post" action="/api/order"><input name="item" value="tea"><button>Order</button></form>
<p id="state">idle</p>
<script>
const show = (text) => { document.getElementById('state').textContent = text; };
const post = () => fetch('/api/save', {method: 'POST', body: 'qty=2'});
function save() { post().then((r) => show('saved ' + r.status)).catch(() => show('not saved')); }
function saveTwice() { post().then(() => post()).then(() => show('saved twice')).catch(() => show('second not saved')); }
</script>
"#;

/// What an approved order loads: a page that POSTs on its own as it loads.
const TAKEOVER_RESULT_PAGE: &str =
    "<p>ORDER-PLACED</p><script>fetch('/api/beacon', {method: 'POST', body: 'seen'}).catch(() => {});</script>";

/// What the takeover fixture's operator does with the next question.
enum Verdict {
    Allow,
    Deny,
    /// Answer yes, but only after this long.
    AllowAfter(Duration),
    /// Block the request's path in the policy file, then answer yes.
    BlockThenAllow,
}

/// The fixture's operator: answers each question from `plan` in order (no
/// plan left means no) and keeps every question it was asked.
struct ScriptedOperator {
    plan: std::sync::Mutex<std::collections::VecDeque<Verdict>>,
    asked: std::sync::Mutex<Vec<Value>>,
    block: Box<dyn Fn() + Send + Sync>,
}

impl ScriptedOperator {
    fn decide(&self, question: &Value) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(question.clone());
        let next = self.plan.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
        match next {
            Some(Verdict::Allow) => true,
            Some(Verdict::AllowAfter(delay)) => {
                thread::sleep(delay);
                true
            }
            Some(Verdict::BlockThenAllow) => {
                (self.block)();
                true
            }
            Some(Verdict::Deny) | None => false,
        }
    }

    fn plan(&self, verdicts: impl IntoIterator<Item = Verdict>) {
        *self.plan.lock().unwrap_or_else(|p| p.into_inner()) = verdicts.into_iter().collect();
    }

    fn asked(&self) -> Vec<Value> {
        self.asked.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// One-shot approval of a held request (#995), by what the loopback server
/// received:
/// - an approved POST reaches the server once, and the page gets its answer;
/// - the identical POST sent again is a new question, and refused it is held;
/// - a denied POST and one whose answer comes after the wait never arrive;
/// - a block rule added while the operator decides stops an approved POST;
/// - an approved form POST loads its result, a denied one leaves the page.
///
/// The operator is asked with the method, whole URL, size, SHA-256 and text
/// of the body that would be sent.
pub fn run_takeover_demo(script: &Path, headless: bool) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("cgah-takeover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let result = takeover_with_policy(script, headless, &dir.join("web_allowlist.json"));
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn takeover_with_policy(script: &Path, headless: bool, policy_file: &Path) -> Result<String, String> {
    let (origin, port, seen) = serve_recording(vec![
        ("/shop", Reply::Html(TAKEOVER_PAGE.to_string())),
        ("/api/save", Reply::Html("SAVED".to_string())),
        ("/api/order", Reply::Html(TAKEOVER_RESULT_PAGE.to_string())),
        ("/api/beacon", Reply::Html(String::new())),
    ])?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let base = format!("http://127.0.0.1:{port}");
    let rules = format!("http://{GUARD_RULE_HOST}:{port}"); // DevSkim: ignore DS137138 because it renames a loopback test fixture
    write_guard_policy(policy_file, &[])?;
    let policy = || {
        let mut policy = RequestPolicy::from(PolicySource::File(policy_file.to_path_buf(), 64));
        policy.alias = Some((base.clone(), rules.clone()));
        policy
    };
    let operator = std::sync::Arc::new(ScriptedOperator {
        plan: Default::default(),
        asked: Default::default(),
        block: {
            let (file, rule) = (policy_file.to_path_buf(), format!("{rules}/api/*"));
            Box::new(move || {
                let _ = write_guard_policy(&file, std::slice::from_ref(&rule));
            })
        },
    });
    let takeover = |wait: Duration| {
        let operator = operator.clone();
        Takeover {
            wait,
            approver: std::sync::Arc::new(move |question: &Value| operator.decide(question)),
        }
    };
    let count = |row: &str| {
        seen.lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|seen| *seen == row)
            .count()
    };
    let rows = |page: &Value, key: &str| -> Vec<String> {
        page[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row.as_str().map(str::to_string))
            .collect()
    };
    let mut lines = Vec::new();
    // Load the shop page, then click the button with this name.
    let click = |sidecar: &mut Sidecar, lines: &mut Vec<String>, name: &str| -> Result<Value, String> {
        let (page, _) = guard_call(sidecar, lines, json!({"op":"navigate","url": format!("{base}/shop")}))?;
        let target = page["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|el| el["name"] == name)
            .and_then(|el| el["ref"].as_str())
            .ok_or_else(|| format!("the {name} button was not listed"))?
            .to_string();
        Ok(guard_call(sidecar, lines, json!({"op":"click","target":{"ref": target}}))?.0)
    };
    let save = format!("POST {base}/api/save");
    let order = format!("POST {base}/api/order");
    // Fixtures serve on 127.0.0.1: a private grant needs a test-support build.
    let private = cfg!(feature = "test-support");
    let mut sidecar = Sidecar::start_with(
        script,
        &session,
        headless,
        private,
        policy(),
        Some(takeover(Duration::from_secs(20))),
    )?;

    // Approved once: sent once, and the page gets the server's answer.
    operator.plan([Verdict::Allow]);
    let page = click(&mut sidecar, &mut lines, "Save")?;
    if rows(&page, "approved") != [save.clone()] || !rows(&page, "held").is_empty() || count("POST /api/save") != 1 {
        return Err(format!("an approved POST was not sent exactly once: {page}"));
    }
    if !page["text"].as_str().unwrap_or("").contains("saved 200") {
        return Err(format!("the page did not get the approved request's answer: {page}"));
    }
    let asked = operator.asked();
    let question = asked.last().ok_or("the operator was not asked")?;
    let expected = json!({
        "method": "POST",
        "url": format!("{base}/api/save"),
        "size": 5,
        "sha256": crate::common::sha256_hex("qty=2"),
        "preview": "qty=2",
    });
    if expected
        .as_object()
        .into_iter()
        .flatten()
        .any(|(key, value)| &question[key] != value)
    {
        return Err(format!("the operator was not shown the request as sent: {question}"));
    }
    lines.push("approved -> asked with method, URL, size, hash and body; sent once".into());

    // The identical request again is a new question; refused, it is held.
    operator.plan([Verdict::Allow, Verdict::Deny]);
    let page = click(&mut sidecar, &mut lines, "Save twice")?;
    if operator.asked().len() != 3 || count("POST /api/save") != 2 || rows(&page, "held") != [save.clone()] {
        return Err(format!(
            "an identical second POST passed on the first one's approval: {page}"
        ));
    }
    lines.push("identical second POST -> asked again, refused, held".into());

    // Denied: never sent.
    operator.plan([Verdict::Deny]);
    let page = click(&mut sidecar, &mut lines, "Save")?;
    if count("POST /api/save") != 2 || rows(&page, "held") != [save.clone()] || !rows(&page, "approved").is_empty() {
        return Err(format!("a denied POST was sent, or not listed as held: {page}"));
    }
    lines.push("denied -> held, not sent".into());

    // A form POST: approved it loads its result, denied the page stays.
    operator.plan([Verdict::Deny]);
    let page = click(&mut sidecar, &mut lines, "Order")?;
    if count("POST /api/order") != 0 || rows(&page, "held") != [order.clone()] {
        return Err(format!("a denied form POST was sent, or not listed as held: {page}"));
    }
    // The yes comes later than a click waits for a page load: the click is
    // not a failure, and the wait is the operator's.
    operator.plan([Verdict::AllowAfter(Duration::from_secs(6))]);
    let asked_before = operator.asked().len();
    let page = click(&mut sidecar, &mut lines, "Order")?;
    if count("POST /api/order") != 1 || !page["text"].as_str().unwrap_or("").contains("ORDER-PLACED") {
        return Err(format!("an approved form POST did not load its result: {page}"));
    }
    // The page that loaded POSTs on its own: held as background, never asked about.
    if operator.asked().len() != asked_before + 1
        || rows(&page, "held_background") != [format!("POST {base}/api/beacon")]
        || !rows(&page, "held").is_empty()
        || count("POST /api/beacon") != 0
    {
        return Err(format!(
            "the approved page's own POST was asked about, sent, or held as the operator's: {page}"
        ));
    }
    lines.push(
        "form POST -> denied stays on the page; approved after 6 s loads the result once, its own POST held as background"
            .into(),
    );

    // A block rule added while the operator decides: approved, and still not sent.
    operator.plan([Verdict::BlockThenAllow]);
    let page = click(&mut sidecar, &mut lines, "Save")?;
    if count("POST /api/save") != 2 || !rows(&page, "approved").is_empty() {
        return Err(format!("an approved POST was sent after its path was blocked: {page}"));
    }
    lines.push("rule added while waiting -> approved, not sent".into());
    write_guard_policy(policy_file, &[])?;
    drop(sidecar);

    // An answer after the wait is too late: held, and the late yes sends nothing.
    let mut sidecar = Sidecar::start_with(
        script,
        &session,
        headless,
        private,
        policy(),
        Some(takeover(Duration::from_secs(1))),
    )?;
    operator.plan([Verdict::AllowAfter(Duration::from_secs(3))]);
    let page = click(&mut sidecar, &mut lines, "Save")?;
    if rows(&page, "held") != [save] {
        return Err(format!("a POST whose wait ran out was not held: {page}"));
    }
    thread::sleep(Duration::from_secs(3));
    guard_call(&mut sidecar, &mut lines, json!({"op":"observe"}))?;
    if count("POST /api/save") != 2 {
        return Err("a yes that came after the wait sent the request".into());
    }
    lines.push("expired -> held; the late yes sent nothing".into());

    // Without takeover nothing is asked and nothing waits.
    drop(sidecar);
    let before = operator.asked().len();
    let mut sidecar = Sidecar::start(script, &session, headless, private, policy())?;
    let page = click(&mut sidecar, &mut lines, "Save")?;
    if operator.asked().len() != before || rows(&page, "held").len() != 1 || count("POST /api/save") != 2 {
        return Err(format!("a session without takeover asked or sent: {page}"));
    }
    lines.push(format!(
        "fixture server received: {:?}",
        seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    ));
    Ok(lines.join("\n"))
}

fn guards_with_policy(script: &Path, headless: bool, policy_file: &Path) -> Result<String, String> {
    let big = format!(
        "<p>{}</p><a href=\"/?q={}\">Long link</a><a href=\"/new\">Short link</a>",
        "filler ".repeat(BIG_TEXT_CHARS / 7),
        "a".repeat(LONG_HREF_CHARS)
    );
    // Past the sidecar's MAX_DOCUMENT_BYTES (16 MiB): refused, never rendered.
    let huge = format!("<p>HUGE-DOCUMENT-RENDERED</p>{}", "x".repeat(17 * 1024 * 1024));
    let (origin, port, seen) = serve_recording(vec![
        ("/workers", Reply::Html(GUARD_WORKERS_PAGE.to_string())),
        ("/w.js", Reply::Script(GUARD_WORKER_SCRIPT)),
        ("/sw.js", Reply::Script(GUARD_WORKER_SCRIPT)),
        ("/old", Reply::Redirect("/new")),
        ("/outside", Reply::Redirect("https://outside.invalid/private?from=page")),
        ("/new", Reply::Html("<p>REDIRECT-LANDED</p>".to_string())),
        ("/big", Reply::Html(big)),
        ("/huge", Reply::Unsized(huge)),
        ("/holds", Reply::Html(GUARD_HOLDS_PAGE.to_string())),
        ("/docs", Reply::Html(GUARD_DENIALS_PAGE.to_string())),
        ("/to-private", Reply::Redirect("/private/landing")),
        ("/hop", Reply::Redirect("/to-private")),
        ("/later/ping", Reply::Html(String::new())),
        ("/private/page", Reply::Html("<p>PRIVATE</p>".to_string())),
        ("/private/landing", Reply::Html("<p>PRIVATE</p>".to_string())),
        ("/private/pixel", Reply::Html(String::new())),
        ("/private/api", Reply::Html(String::new())),
    ])?;
    let session = Session::general([&origin]).map_err(|err| format!("{err:?}"))?;
    let base = format!("http://127.0.0.1:{port}");
    let rules = format!("http://{GUARD_RULE_HOST}:{port}"); // DevSkim: ignore DS137138 because it renames a loopback test fixture
    write_guard_policy(policy_file, &[format!("{rules}/private/*")])?;
    let mut policy = RequestPolicy::from(PolicySource::File(policy_file.to_path_buf(), 64));
    policy.alias = Some((base.clone(), rules.clone()));
    // Fixtures serve on 127.0.0.1: a private grant needs a test-support build.
    let mut sidecar = Sidecar::start(script, &session, headless, cfg!(feature = "test-support"), policy)?;
    let mut lines = Vec::new();
    let received = |seen: &Received| seen.lock().unwrap_or_else(|p| p.into_inner()).clone();

    // Workers: give them time to start and call out, then read the server's log.
    let (page, _) = guard_call(
        &mut sidecar,
        &mut lines,
        json!({"op":"navigate","url": format!("{base}/workers")}),
    )?;
    if !page["text"].as_str().unwrap_or("").contains("workers tried") {
        return Err(format!("the workers page did not render: {page}"));
    }
    guard_call(&mut sidecar, &mut lines, json!({"op":"wait","timeout_ms": 2000}))?;
    let leaked: Vec<String> = received(&seen)
        .into_iter()
        .filter(|row| row.contains("/w.js") || row.contains("/sw.js") || row.contains("/beacon"))
        .collect();
    if !leaked.is_empty() {
        return Err(format!(
            "a worker started or called out (worker-src 'none' missing?): {leaked:?}"
        ));
    }

    // Redirect: the 301 is followed and its target is loaded and shown.
    let (page, _) = guard_call(
        &mut sidecar,
        &mut lines,
        json!({"op":"navigate","url": format!("{base}/old")}),
    )?;
    if !page["text"].as_str().unwrap_or("").contains("REDIRECT-LANDED")
        || !received(&seen).contains(&"GET /new".to_string())
    {
        return Err(format!("a same-origin redirect did not land on its target: {page}"));
    }

    let outside = sidecar.call(json!({"op":"navigate","url":format!("{base}/outside")}))?;
    if outside["ok"] != false || outside["redirect_origin"] != "https://outside.invalid:443" {
        return Err("a refused page redirect did not report its origin alone".into());
    }

    guard_holds(&mut sidecar, &mut lines, &base, &seen)?;
    guard_path_denials(&mut sidecar, &mut lines, (&base, &rules, policy_file), &seen)?;

    // Caps: the reply, the text and each href stay within the sidecar's limits.
    let (page, size) = guard_call(
        &mut sidecar,
        &mut lines,
        json!({"op":"navigate","url": format!("{base}/big")}),
    )?;
    if size > MAX_GUARDED_REPLY_BYTES {
        return Err(format!(
            "an observation reply was {size} bytes; the caps should keep it under {MAX_GUARDED_REPLY_BYTES}"
        ));
    }
    if page["text"].as_str().unwrap_or("").chars().count() > 4_000 {
        return Err("an observation's text was not cut at 4,000 characters".into());
    }
    let elements = page["elements"].as_array().cloned().unwrap_or_default();
    let named = |name: &str| elements.iter().find(|el| el["name"] == name).cloned();
    let long = named("Long link").ok_or("the long link was not listed")?;
    if long["href"].as_str().is_some_and(|href| href.len() > 8_192) {
        return Err(format!("a {LONG_HREF_CHARS}-character href was listed whole"));
    }
    if named("Short link").and_then(|el| el["href"].as_str().map(str::to_string)) != Some(format!("{base}/new")) {
        return Err("an ordinary href was not listed".into());
    }

    // A document past the read cap is refused whole, and the session goes on.
    let huge = sidecar.call(json!({"op":"navigate","url": format!("{base}/huge")}))?;
    if huge.to_string().contains("HUGE-DOCUMENT-RENDERED") {
        return Err("a document past the 16 MiB read cap was rendered".into());
    }
    if huge["ok"] == true || !huge.to_string().contains("MiB a browser task reads") {
        return Err(format!(
            "a document past the read cap was not refused by name: {}",
            truncate(&huge.to_string(), 300)
        ));
    }
    lines.push("a 17 MiB document -> refused at the read cap".into());
    // Back to the page the proxy-probe check below observes.
    guard_call(
        &mut sidecar,
        &mut lines,
        json!({"op":"navigate","url": format!("{base}/big")}),
    )?;

    // Proxy: without the session credential every form of request gets 407.
    let proxy = sidecar.proxy_port.ok_or("the sidecar did not report its proxy port")?;
    for request in [
        format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        format!("GET https://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        format!("GET {base}/new HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
    ] {
        let status = unauthenticated_proxy_status(proxy, &request)?;
        let first = request.lines().next().unwrap_or("");
        if !status.contains(" 407") {
            return Err(format!(
                "the sidecar proxy answered {status:?} to an unauthenticated {first:?}"
            ));
        }
        lines.push(format!("unauthenticated {first} -> {status}"));
    }
    // The sidecar survived those and still observes the page.
    let (page, _) = guard_call(&mut sidecar, &mut lines, json!({"op":"observe"}))?;
    if page["elements"].as_array().is_none_or(Vec::is_empty) {
        return Err("the sidecar stopped answering after the proxy probes".into());
    }
    lines.push(format!("fixture server received: {:?}", received(&seen)));
    Ok(lines.join("\n"))
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
    let mut job = false;
    let mut takeover_wait = None;
    let mut allow_private = false;
    let started = Instant::now();
    let mut deadline = None;
    for raw in args {
        let arg = raw.to_string_lossy();
        let (key, value) = arg.split_once('=').map_or((arg.as_ref(), None), |(k, v)| (k, Some(v)));
        match (key, value) {
            ("--headed", None) => headless = false,
            // The server's job runner: progress lines out, `cancel` in.
            ("--job", None) => job = true,
            // Fixture runs only: the server passes it for a loopback origin.
            ("--allow-private", None) if cfg!(feature = "test-support") => allow_private = true,
            // How long a held request waits for the job's server to decide.
            ("--takeover-wait-sec", Some(v)) if v.parse::<u64>().is_ok_and(|n| (1..=600).contains(&n)) => {
                takeover_wait = v.parse().ok().map(Duration::from_secs)
            }
            // Only general browsing runs model tasks; browser-verify keeps PR grants.
            ("qwen" | "enter" | "reach" | "guards" | "takeover" | "run", None) if browse && mode.is_none() => {
                mode = Some(key.to_string())
            }
            ("--model", Some(v)) if !v.is_empty() => model = Some(v.to_string()),
            ("--sidecar", Some(v)) if !v.is_empty() => sidecar = PathBuf::from(v),
            ("--task", Some(v)) if (1..=MAX_TASK_CHARS).contains(&v.chars().count()) => task = Some(v.to_string()),
            // The server passes the task this way so it never shows in a process listing.
            ("--task-file", Some(v)) if !v.is_empty() => {
                match read_task_file(Path::new(v)) {
                    Some(text) => task = Some(text),
                    None => {
                        eprintln!("cgagentharness: --task-file is unreadable, empty or longer than {MAX_TASK_CHARS} characters");
                        return 3;
                    }
                }
            }
            ("--origin", Some(v)) => origins.push(v.to_string()),
            ("--max-turns", Some(v)) if v.parse::<usize>().is_ok_and(|n| (1..=40).contains(&n)) => {
                max_turns = v.parse().unwrap_or(max_turns)
            }
            // The server's task limit, counted from this process's start.
            ("--deadline-sec", Some(v)) if v.parse::<u64>().is_ok_and(|n| (1..=1800).contains(&n)) => {
                deadline = v.parse().ok().map(|n| started + Duration::from_secs(n))
            }
            _ => {
                eprintln!("cgagentharness: unexpected or out-of-range browser argument {key}");
                return 3;
            }
        }
    }
    if job && mode.as_deref() != Some("run") {
        eprintln!("cgagentharness: --job is for browser run only");
        return 3;
    }
    if takeover_wait.is_some() && !job {
        eprintln!("cgagentharness: --takeover-wait-sec needs --job");
        return 3;
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
            if job {
                watch_job_input(takeover_wait, deadline);
            }
            match run_task(
                &sidecar,
                headless,
                (&model, &api, deadline),
                &task,
                &origins,
                max_turns,
                allow_private,
            ) {
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
        (Some("guards"), _) => run_guards_demo(&sidecar, headless),
        (Some("takeover"), _) => run_takeover_demo(&sidecar, headless),
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

/// The task text in `path`: UTF-8, 1 to `MAX_TASK_CHARS` characters. Larger
/// files are refused before they are read whole.
fn read_task_file(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take((MAX_TASK_CHARS * 4 + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    (1..=MAX_TASK_CHARS).contains(&text.chars().count()).then_some(text)
}

/// Starts a dedicated, a shared and a blob worker. Each would fetch a beacon
/// and open a WebSocket on the fixture server if it ran. The blob worker's
/// base is its `blob:` URL, so its beacon names the page's origin outright.
const GUARD_WORKERS_PAGE: &str = r#"
<p>workers tried</p>
<script>
const beacon = JSON.stringify(location.origin + "/beacon-blob");
const blob = URL.createObjectURL(new Blob(["fetch(" + beacon + ");"], {type: "text/javascript"}));
for (const make of [() => new Worker("/w.js"), () => new SharedWorker("/sw.js"), () => new Worker(blob)]) {
  try { make(); } catch (e) {}
}
</script>
"#;

/// What a fixture worker would do: a GET the page route allows, a POST and a
/// WebSocket, all to the fixture server.
const GUARD_WORKER_SCRIPT: &str =
    "fetch('/beacon-get'); fetch('/beacon-post', {method: 'POST', body: 'x'}); new WebSocket('ws://' + location.host + '/beacon-ws');";

/// An option label longer than the 80 characters an observation lists.
const LONG_OPTION: &str =
    "Freight delivery to the loading dock with a scheduled two hour window and a signature on receipt";

const SEARCH_PAGE: &str = r#"
<p id="tab">first tab</p>
<a href="/?tab=2" target="_blank">Open in a new tab</a>
<a href="/?again">Load again</a>
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
fetch('/loaded', {method: 'POST'}).catch(() => {});
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
<p id="lazy">lazy pending</p>
<button onclick="setTimeout(() => { document.getElementById('timed').textContent = 'TIMER-DONE'; }, 1500)">Start timer</button>
<p id="timed">timer idle</p>
<button onclick="alert('Saved draft 7')">Notify</button>
ENTRIES
<p>RELEASE-MARKER 9.9.9</p>
<a href="/?changelog"><code>Changelog</code> of releases</a>
<p>DOTTED_CAPITALS DOTTED-CAPITAL marker</p>
<script>
fetch('/data').then(() => setTimeout(() => { document.getElementById('late').textContent = 'late content'; }, 300));
addEventListener('scroll', () => { document.getElementById('lazy').textContent = 'LAZY-LOADED'; }, {once: true});
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

    #[test]
    fn a_reply_cut_inside_an_emoji_parses_once_the_sidecar_replaces_the_lone_half() {
        // What `JSON.stringify` wrote for a title cut between an emoji's halves,
        // before and after `wellFormed` in browser/sidecar.mjs.
        let lone = r#"{"id":1,"ok":true,"result":{"title":"ab\ud83d"}}"#;
        assert!(serde_json::from_str::<Value>(lone).is_err());
        let fixed: Value =
            serde_json::from_str("{\"id\":1,\"ok\":true,\"result\":{\"title\":\"ab\u{FFFD}\"}}").unwrap();
        assert_eq!(fixed["result"]["title"], "ab\u{FFFD}");
        let approve = "{\"approve\":7,\"preview\":\"x\u{FFFD}\"}";
        assert_eq!(approval_question(approve).unwrap()["approve"], 7);
        assert!(approval_question(r#"{"approve":7,"preview":"x\ud83d"}"#).is_none());
    }

    #[test]
    fn long_link_targets_are_cut_for_the_model_only() {
        let short = "https://docs.example/a?b=c";
        assert_eq!(shown_href(short), short);
        let long = format!("https://docs.example/?t={}", "x".repeat(4000));
        let shown = shown_href(&long);
        assert_eq!(shown.chars().count(), MAX_SHOWN_HREF_CHARS + 1);
        assert!(shown.ends_with('…') && long.starts_with(shown.trim_end_matches('…')));
    }

    #[test]
    fn a_task_file_must_be_utf8_and_within_the_task_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task");
        std::fs::write(&path, "list the games behind Start").unwrap();
        assert_eq!(read_task_file(&path).as_deref(), Some("list the games behind Start"));
        for bad in [
            Vec::new(),
            "x".repeat(MAX_TASK_CHARS + 1).into_bytes(),
            vec![0xff, 0xfe],
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(read_task_file(&path), None);
        }
        std::fs::write(&path, "é".repeat(MAX_TASK_CHARS)).unwrap();
        assert_eq!(read_task_file(&path).map(|t| t.chars().count()), Some(MAX_TASK_CHARS));
        assert_eq!(read_task_file(&dir.path().join("missing")), None);
    }

    /// A reply line past `MAX_REPLY_BYTES` stops the sidecar instead of being
    /// buffered, and the next call fails rather than reading the rest of it.
    #[cfg(unix)]
    #[test]
    fn an_oversized_sidecar_reply_stops_the_session() {
        let child = Command::new("sh")
            .args(["-c", "read _; head -c 5000000 /dev/zero | tr '\\0' x; echo"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut sidecar = Sidecar::attach(child, Duration::from_secs(30), RequestPolicy::open(), None).unwrap();
        let err = sidecar.call(json!({"op":"observe"})).unwrap_err();
        assert!(err.contains("exceeded"), "{err}");
        assert_eq!(
            sidecar.call(json!({"op":"observe"})).unwrap_err(),
            "sidecar stdin closed"
        );
    }

    /// A sidecar that never answers fails the call within its timeout instead
    /// of holding the task until browser.timeout_sec, and is stopped.
    #[cfg(unix)]
    #[test]
    fn a_silent_sidecar_fails_the_call_within_its_timeout() {
        let child = Command::new("sh")
            .args(["-c", "read _; sleep 30"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut sidecar = Sidecar::attach(child, Duration::from_millis(300), RequestPolicy::open(), None).unwrap();
        let started = std::time::Instant::now();
        let err = sidecar.call(json!({"op":"observe"})).unwrap_err();
        assert!(err.contains("did not answer within"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            sidecar.call(json!({"op":"observe"})).unwrap_err(),
            "sidecar stdin closed"
        );
    }

    /// A paused op cannot outlast the task deadline, or the child is killed
    /// at browser.timeout_sec before it prints its report.
    #[cfg(unix)]
    #[test]
    fn a_sidecar_call_ends_at_the_task_deadline() {
        let child = Command::new("sh")
            .args(["-c", "read _; sleep 30"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut sidecar = Sidecar::attach(child, Duration::from_secs(30), RequestPolicy::open(), None).unwrap();
        sidecar.deadline = Some(Instant::now() + Duration::from_millis(300));
        let started = Instant::now();
        let err = sidecar.call(json!({"op":"observe"})).unwrap_err();
        assert!(err.contains(OUT_OF_TIME), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_held_request_waits_no_later_than_the_task_deadline() {
        let now = Instant::now();
        let wait = Duration::from_secs(120);
        assert_eq!(hold_until(now, wait, None), now + wait);
        // Shipped defaults: a hold starting 200 s into a 280 s child deadline.
        let deadline = now + Duration::from_secs(80);
        assert_eq!(hold_until(now, wait, Some(deadline)), deadline);
        assert_eq!(hold_until(now, wait, Some(now + Duration::from_secs(200))), now + wait);
    }

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
        let err = ollama_chat(
            &api,
            "m",
            &[json!({"role":"user","content":"page text"})],
            &json!([]),
            MODEL_CALL_TIMEOUT,
        )
        .unwrap_err();
        assert!(err.contains("307"), "{err}");
        assert!(ollama_get(&api, "/api/ps").is_err());
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(leaked.load(Ordering::SeqCst), 0, "a redirect was followed");

        let mut big = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\"".to_vec();
        big.resize(big.len() + crate::llm::MAX_RESPONSE_BYTES, b'a');
        big.push(b'"');
        let (api, _) = canned(big);
        let err = ollama_chat(&api, "m", &[], &json!([]), MODEL_CALL_TIMEOUT).unwrap_err();
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
    fn request_verdicts_follow_the_policy_file_and_fail_closed() {
        use crate::server::web_policy::{Mode, Policy};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web_allowlist.json");
        let save = |patterns: &[&str]| {
            let mut policy = Policy::empty();
            policy.mode = Mode::Denylist;
            for pattern in patterns {
                policy.blocked.push(Policy::block_rule(pattern).unwrap());
            }
            std::fs::write(&file, serde_json::to_vec(&policy).unwrap()).unwrap();
        };
        let mut policy = RequestPolicy::from(PolicySource::File(file.clone(), 64));
        let ask = |policy: &mut RequestPolicy, url: &str| {
            let line = json!({"check": 7, "url": url}).to_string();
            request_verdict(&line, policy).unwrap()
        };
        save(&["https://docs.example.org/private/*"]);
        assert_eq!(
            ask(&mut policy, "https://docs.example.org/private/a"),
            json!({"verdict": 7, "allow": false})
        );
        assert_eq!(
            ask(&mut policy, "https://docs.example.org/guide"),
            json!({"verdict": 7, "allow": true})
        );
        // The sidecar's grant drops a trailing dot, so the verdict must too.
        assert_eq!(ask(&mut policy, "https://docs.example.org./private/a")["allow"], false);
        // An edit applies to the next question; no restart, no model turn.
        save(&["https://docs.example.org/private/*", "https://docs.example.org/guide"]);
        assert_eq!(ask(&mut policy, "https://docs.example.org/guide")["allow"], false);
        // Invalid, then missing: every request is refused, then it recovers.
        std::fs::write(&file, b"{not json").unwrap();
        assert_eq!(ask(&mut policy, "https://docs.example.org/other")["allow"], false);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(ask(&mut policy, "https://docs.example.org/other")["allow"], false);
        save(&[]);
        assert_eq!(ask(&mut policy, "https://docs.example.org/other")["allow"], true);
        assert_eq!(
            ask(&mut RequestPolicy::from(PolicySource::Unreadable), "https://a.example/")["allow"],
            false
        );
        // A question with no URL is refused; a reply is not a question, even
        // when page text inside it spells one.
        assert_eq!(
            request_verdict("{\"check\":3}", &mut policy).unwrap(),
            json!({"verdict": 3, "allow": false})
        );
        let reply = json!({"id": 1, "ok": true, "result": {"text": "{\"check\":1,\"url\":\"x\"}"}}).to_string();
        assert!(request_verdict(&reply, &mut policy).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_answers_a_check_while_a_call_waits() {
        // The child asks about a URL, then answers the op with the verdict it read.
        let script = r#"read op; printf '{"check":1,"url":"https://a.example/x"}\n'; read verdict; printf '{"id":1,"ok":true,"result":%s}\n' "$verdict""#;
        let child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut sidecar = Sidecar::attach(
            child,
            Duration::from_secs(30),
            RequestPolicy::from(PolicySource::Unreadable),
            None,
        )
        .unwrap();
        let reply = sidecar.call(json!({"op":"observe"})).unwrap();
        assert_eq!(reply["result"], json!({"verdict": 1, "allow": false}));
    }

    /// #995: an approval question goes to the approver and nowhere else. With
    /// no approver the answer is no, and page text shaped like a question
    /// inside a reply is not one.
    #[cfg(unix)]
    #[test]
    fn an_approval_question_is_answered_by_the_approver_or_refused() {
        let ask = |approver: Option<Approver>| {
            let script = r#"read op; printf '{"approve":4,"method":"POST","url":"https://a.example/save","size":5}\n'; read verdict; printf '{"id":1,"ok":true,"result":%s}\n' "$verdict""#;
            let child = Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut sidecar = Sidecar::attach(child, Duration::from_secs(30), RequestPolicy::open(), approver).unwrap();
            sidecar.call(json!({"op":"click"})).unwrap()["result"].clone()
        };
        assert_eq!(ask(None), json!({"verdict": 4, "allow": false}));
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let yes: Approver = std::sync::Arc::new(move |question: &Value| {
            log.lock().unwrap().push(question.clone());
            question["url"] == "https://a.example/save"
        });
        assert_eq!(ask(Some(yes)), json!({"verdict": 4, "allow": true}));
        assert_eq!(seen.lock().unwrap()[0]["size"], 5);

        assert!(approval_question("{\"approve\":1,\"url\":\"https://a.example/\"}").is_some());
        let reply = json!({"id": 1, "ok": true, "result": {"text": "{\"approve\":1}"}}).to_string();
        assert!(approval_question(&reply).is_none());
        assert!(approval_question("{\"approve\":").is_none());
    }

    /// One decision is open at a time: a question that arrives while the
    /// approver is still deciding is refused without reaching it.
    #[cfg(unix)]
    #[test]
    fn a_second_approval_question_is_refused_while_one_is_open() {
        // Two questions at once, then both verdicts back in the reply, in arrival order.
        let script = r#"read op; printf '{"approve":1}\n{"approve":2}\n'; read first; read second; printf '{"id":1,"ok":true,"result":[%s,%s]}\n' "$first" "$second""#;
        let child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let slow: Approver = std::sync::Arc::new(move |_: &Value| {
            counted.fetch_add(1, Ordering::AcqRel);
            thread::sleep(Duration::from_millis(400));
            true
        });
        let mut sidecar = Sidecar::attach(child, Duration::from_secs(30), RequestPolicy::open(), Some(slow)).unwrap();
        let reply = sidecar.call(json!({"op":"click"})).unwrap();
        assert_eq!(
            reply["result"],
            json!([{"verdict": 2, "allow": false}, {"verdict": 1, "allow": true}])
        );
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn a_job_reads_only_cancel_and_well_formed_verdict_lines() {
        assert_eq!(job_input(" cancel \n"), Some(JobInput::Cancel));
        assert_eq!(
            job_input(r#"{"verdict":"approve","id":7}"#),
            Some(JobInput::Verdict { id: 7, allow: true })
        );
        assert_eq!(
            job_input(r#"{"id":7,"verdict":"deny"}"#),
            Some(JobInput::Verdict { id: 7, allow: false })
        );
        for line in [
            "",
            "approve",
            r#"{"verdict":"approve"}"#,
            r#"{"verdict":"yes","id":7}"#,
            r#"{"verdict":true,"id":7}"#,
            r#"{"verdict":"approve","id":"7"}"#,
            r#"{"verdict":"approve","id":-1}"#,
        ] {
            assert_eq!(job_input(line), None, "{line}");
        }
    }

    #[test]
    fn approved_requests_are_listed_each_time_and_the_model_gets_the_count() {
        let mut rows = Vec::new();
        let sent = json!({"approved_by_operator": ["POST https://shop.example/save"]});
        note_for_operator(&mut rows, None, &sent);
        note_for_operator(&mut rows, None, &sent);
        assert_eq!(rows, vec![json!({"approved": "POST https://shop.example/save"}); 2]);
        for n in 0..MAX_FOR_OPERATOR {
            note_for_operator(
                &mut rows,
                None,
                &json!({"held_in_background": [format!("POST /beacon/{n}")]}),
            );
        }
        note_for_operator(&mut rows, None, &sent);
        assert_eq!(rows.len(), MAX_FOR_OPERATOR);
        assert_eq!(rows.iter().filter(|row| row.get("approved").is_some()).count(), 3);
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
        let handoff = json!({"decision": "Handoff { reason: \"secret field\" }"});
        let verdict = |earlier: &[&Value]| {
            let blocks = earlier.iter().any(|outcome| blocks_finish(outcome));
            finish_verdict(&done, blocks)
        };
        // First in its turn, or after only successful calls: accepted.
        assert_eq!(verdict(&[]).as_deref(), Ok("zipfile — Work with ZIP archives"));
        assert_eq!(
            verdict(&[&observed, &observed]).as_deref(),
            Ok("zipfile — Work with ZIP archives")
        );
        // After a handoff: accepted, though the handoff still counts as failed in a row.
        assert!(action_failed(&handoff) && handed_off(&handoff) && !handed_off(&stale));
        assert_eq!(
            verdict(&[&observed, &handoff]).as_deref(),
            Ok("zipfile — Work with ZIP archives")
        );
        // After a refused or failed call, even one followed by a success or a handoff: refused.
        for earlier in [
            vec![&malformed],
            vec![&observed, &stale],
            vec![&stale, &observed],
            vec![&handoff, &malformed],
        ] {
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

    #[test]
    fn a_hold_or_handoff_is_listed_even_when_background_rows_fill_the_cap() {
        let mut rows = Vec::new();
        for n in 0..MAX_FOR_OPERATOR + 5 {
            note_for_operator(
                &mut rows,
                None,
                &json!({"held_in_background": [format!("POST /beacon/{n}")]}),
            );
        }
        assert_eq!(rows.len(), MAX_FOR_OPERATOR);
        let save = json!({"held_for_operator": ["POST /save"], "held_in_background": ["POST /late"]});
        let handoff = json!({"handoff": "type", "element": "Password", "reason": "secret"});
        note_for_operator(&mut rows, Some(handoff.clone()), &save);
        assert_eq!(rows.len(), MAX_FOR_OPERATOR);
        assert!(rows.contains(&json!({"held": "POST /save"})) && rows.contains(&handoff));
        assert!(rows.contains(&json!({"held_background": "POST /beacon/0"})));
        assert!(!rows.contains(&json!({"held_background": "POST /late"})));
    }

    /// A page with a password field, a button whose POST is held, and a plain
    /// button. Records every op it is sent.
    #[derive(Default)]
    struct ScriptedPage {
        ops: Vec<String>,
    }

    impl Driver for ScriptedPage {
        fn call(&mut self, op: Value) -> Result<Value, String> {
            let target = op.pointer("/target/ref").and_then(Value::as_str).unwrap_or("");
            self.ops.push(
                format!("{} {target}", op["op"].as_str().unwrap_or(""))
                    .trim()
                    .to_string(),
            );
            let held: Vec<&str> = if target == "save" {
                vec!["POST https://example.com/save"]
            } else {
                vec![]
            };
            let background: Vec<&str> = if target == "auto" {
                vec!["POST https://example.com/autosave?draft=page-private-text"]
            } else {
                vec![]
            };
            let dialogs: Vec<String> = if op["op"] == "wait" {
                vec!["confirm: Delete the draft?".into(), "z".repeat(1_000)]
            } else {
                vec![]
            };
            Ok(json!({"ok": true, "result": {
                "url": "https://example.com/", "text": "page", "held": held, "held_background": background,
                "dialogs": dialogs,
                "elements": [
                    {"ref": "auto", "role": "button", "name": "Autosaves"},
                    {"ref": "pw", "role": "textbox", "name": "Password", "input": "secret"},
                    {"ref": "save", "role": "button", "name": "Save"},
                    {"ref": "after", "role": "button", "name": "After"},
                    {"ref": "filter", "role": "textbox", "name": "Filter", "input": "text"},
                    {"ref": "next", "role": "link", "name": "Next", "href": "https://example.com/next"},
                ],
            }}))
        }
    }

    /// Navigate, then send `batch` (tool name, element name) in one turn.
    fn run_batch(batch: &[(&str, &str)]) -> (Value, Vec<String>) {
        run_batch_with(batch, "task", "x")
    }

    /// `run_batch` for task text `task`, typing `typed` in every `type` call.
    fn run_batch_with(batch: &[(&str, &str)], task: &str, typed: &str) -> (Value, Vec<String>) {
        let (report, ops, _) = run_batch_seen(batch, task, typed);
        (report, ops)
    }

    /// `run_batch_with`, also returning every message the model was sent.
    fn run_batch_seen(batch: &[(&str, &str)], task: &str, typed: &str) -> (Value, Vec<String>, String) {
        let origins = vec!["https://example.com".to_string()];
        let mut session = Session::general(&origins).unwrap();
        let mut page = ScriptedPage::default();
        let mut turn = 0;
        let mut sent = String::new();
        let mut chat = |messages: &[Value]| {
            turn += 1;
            sent = json!(messages).to_string();
            let calls: Vec<Value> = match turn {
                1 => vec![json!({"function": {"name": "navigate", "arguments": {"url": "https://example.com/"}}})],
                2 => {
                    let seen: Value = messages
                        .iter()
                        .rev()
                        .find(|m| m["role"] == "tool")
                        .and_then(|m| serde_json::from_str(m["content"].as_str()?).ok())
                        .unwrap();
                    let ref_of = |name: &str| {
                        seen["elements"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|el| el["name"] == name)
                            .unwrap()["ref_id"]
                            .clone()
                    };
                    batch
                        .iter()
                        .map(|(tool, target)| match *tool {
                            "navigate" => json!({"function": {"name": "navigate", "arguments": {"url": target}}}),
                            "type" => json!({"function": {"name": "type", "arguments": {"ref_id": ref_of(target), "text": typed}}}),
                            _ => json!({"function": {"name": tool, "arguments": {"ref_id": ref_of(target)}}}),
                        })
                        .collect()
                }
                _ => vec![],
            };
            Ok(json!({"message": {"role": "assistant", "content": "done", "tool_calls": calls}}))
        };
        let report = model_loop(&mut session, &mut page, &mut chat, &|_| None, task, &origins, 6).unwrap();
        (report, page.ops, sent)
    }

    /// #901 step 2: a URL the model composed on a granted origin is refused,
    /// and the report keeps its origin only, never the path or query.
    #[test]
    fn a_composed_navigation_is_refused_and_reported_by_origin_only() {
        let (report, ops) = run_batch(&[("navigate", "https://example.com/collect?d=private-memo-text")]);
        assert_eq!(ops, ["navigate"], "the composed URL never reached the page");
        assert_eq!(report["refused_composed"], json!(["https://example.com"]));
        assert!(!report.to_string().contains("private-memo-text"), "{report}");
        assert!(!report.to_string().contains("/collect"), "{report}");
        // A link the page showed, a URL the task names and the granted root still go.
        let (_, ops) = run_batch(&[("navigate", "https://example.com/next")]);
        assert_eq!(ops, ["navigate", "navigate"]);
        // A fragment the page never showed is composed: page scripts can read it.
        let (report, ops) = run_batch(&[("navigate", "https://example.com/next#leak=private-memo-text")]);
        assert_eq!(ops, ["navigate"]);
        assert!(!report.to_string().contains("private-memo-text"), "{report}");
        let (report, ops) = run_batch_with(
            &[("navigate", "https://example.com/docs/a?b=1")],
            "Read https://example.com/docs/a?b=1, then summarize.",
            "x",
        );
        assert_eq!(ops, ["navigate", "navigate"]);
        assert_eq!(report["refused_composed"], json!([]));
    }

    /// #901 step 3: typed text must come from the task or the current page;
    /// anything else is held for the operator and ends the task.
    #[test]
    fn typed_text_needs_the_task_or_the_page_as_its_source() {
        for (task, typed) in [("Set the filter to open issues", "open issues"), ("task", "page")] {
            let (report, ops) = run_batch_with(&[("type", "Filter")], task, typed);
            assert_eq!(ops, ["navigate", "type filter"], "{typed}");
            assert!(report["for_operator"].as_array().unwrap().is_empty());
        }
        let (report, ops) = run_batch_with(
            &[("type", "Filter"), ("press_enter", "Filter")],
            "task",
            "my private memo",
        );
        assert_eq!(ops, ["navigate"], "neither the text nor Enter reached the page");
        assert_eq!(report["for_operator"][0]["element"], "Filter");
        assert_eq!(
            report["for_operator"][0]["reason"],
            crate::common::browser::TYPED_TEXT_HOLD
        );
        assert!(!report.to_string().contains("my private memo"), "{report}");
    }

    #[test]
    fn a_handoff_ends_the_task_and_drops_the_rest_of_the_batch() {
        let (report, ops) = run_batch(&[("type", "Password"), ("click", "After")]);
        assert_eq!(ops, ["navigate"], "nothing after the handoff reached the page");
        assert_eq!(report["finished"], false);
        assert!(report["stopped"].as_str().unwrap().starts_with("needs the operator"));
        assert_eq!(report["for_operator"][0]["element"], "Password");
    }

    #[test]
    fn a_held_request_ends_the_task_and_drops_the_rest_of_the_batch() {
        let (report, ops) = run_batch(&[("click", "Save"), ("navigate", "https://example.com/next")]);
        assert_eq!(ops, ["navigate", "click save"], "the navigate after the hold never ran");
        assert_eq!(report["finished"], false);
        assert!(report["stopped"].as_str().unwrap().starts_with("needs the operator"));
        assert_eq!(report["for_operator"][0]["held"], "POST https://example.com/save");
    }

    /// #894: the model's `scroll` and `wait` reach the page only with their
    /// bounded arguments, and what they send the policy kernel allows.
    #[test]
    fn scroll_and_wait_take_only_their_bounded_arguments() {
        let session = Session::general(["https://example.com"]).unwrap();
        for (name, args, wire) in [
            (
                "scroll",
                json!({"direction": "down"}),
                json!({"action": "scroll", "dy": 1500}),
            ),
            (
                "scroll",
                json!({"direction": "up"}),
                json!({"action": "scroll", "dy": -1500}),
            ),
            (
                "wait",
                json!({"seconds": 1}),
                json!({"action": "wait", "timeout_ms": 1000}),
            ),
            (
                "wait",
                json!({"seconds": "5"}),
                json!({"action": "wait", "timeout_ms": 5000}),
            ),
        ] {
            assert_eq!(bad_tool_args(name, &args), None, "{name} {args}");
            assert_eq!(model_wire(name, &args), Some(wire.clone()), "{name} {args}");
            assert!(session.decide(&wire).is_allow(), "{wire}");
        }
        for (name, args) in [
            ("scroll", json!({})),
            ("scroll", json!({"direction": "left"})),
            ("scroll", json!({"direction": 3000})),
            ("wait", json!({})),
            ("wait", json!({"seconds": 0})),
            ("wait", json!({"seconds": 6})),
            ("wait", json!({"seconds": -1})),
            ("wait", json!({"seconds": 2.5})),
            ("wait", json!({"seconds": "soon"})),
        ] {
            assert!(
                bad_tool_args(name, &args).is_some_and(|why| why.starts_with(name)),
                "{name} {args}"
            );
        }
        assert_eq!(bad_tool_args("click", &json!({})), None);
        let names: Vec<String> = browser_tools()
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names
            .iter()
            .all(|name| name == "finish" || model_wire(name, &json!({})).is_some()));
        assert!(names.contains(&"scroll".to_string()) && names.contains(&"wait".to_string()));
    }

    /// A refused `wait` never reaches the page, and a page dialog's text is
    /// shown to the model within its caps.
    #[test]
    fn a_bad_wait_is_refused_and_dialog_text_is_shown() {
        let origins = vec!["https://example.com".to_string()];
        let mut session = Session::general(&origins).unwrap();
        let mut page = ScriptedPage::default();
        let mut turn = 0;
        let mut sent = String::new();
        let mut chat = |messages: &[Value]| {
            turn += 1;
            sent = json!(messages).to_string();
            let calls = match turn {
                1 => vec![json!({"function": {"name": "navigate", "arguments": {"url": "https://example.com/"}}})],
                2 => vec![
                    json!({"function": {"name": "wait", "arguments": {"seconds": 60}}}),
                    json!({"function": {"name": "wait", "arguments": {"seconds": 2}}}),
                ],
                _ => vec![],
            };
            Ok(json!({"message": {"role": "assistant", "content": "done", "tool_calls": calls}}))
        };
        let report = model_loop(&mut session, &mut page, &mut chat, &|_| None, "task", &origins, 6).unwrap();
        assert_eq!(page.ops, ["navigate", "wait"], "only the bounded wait reached the page");
        assert_eq!(report["finished"], true);
        assert!(sent.contains("wait takes seconds"), "{sent}");
        assert!(sent.contains("confirm: Delete the draft?"), "{sent}");
        assert!(
            !sent.contains(&"z".repeat(MAX_SHOWN_DIALOG_CHARS + 1)),
            "a long dialog is cut"
        );
    }

    /// #894: only a profile whose name carries a dead sidecar's pid is removed.
    #[test]
    fn the_profile_sweep_removes_only_dead_pid_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let make = |name: &str| {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(dir.join("Default")).unwrap();
            std::fs::write(dir.join("Default/Cookies"), "c").unwrap();
            dir
        };
        let dead = make("cgah-browser-4242-AbC123");
        let live = make("cgah-browser-7-xYz789");
        // No pid in the name: a hand-made test home or another tool's profile.
        let pidless = [
            make("cgah-browser-AbCdEf12"),
            make("cgah-browser-123456"),
            make("cgah-browser--x"),
            make("cgah-browser-12x-y"),
            make("cgah-browser-4242-"),
            make("cgah-chat-browser-4242-x"),
        ];
        let file = tmp.path().join("cgah-browser-4242-file");
        std::fs::write(&file, "not a directory").unwrap();
        #[cfg(unix)]
        let link = {
            let target = make("kept-target");
            let link = tmp.path().join("cgah-browser-4242-link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            (link, target)
        };
        assert_eq!(sweep_profiles_in(tmp.path(), &|pid| pid == 7), 1);
        assert!(!dead.exists());
        assert!(live.join("Default/Cookies").exists());
        assert!(pidless.iter().all(|dir| dir.join("Default/Cookies").exists()));
        assert!(file.exists());
        #[cfg(unix)]
        assert!(link.0.exists() && link.1.join("Default/Cookies").exists());
        assert_eq!(sweep_profiles_in(tmp.path(), &|pid| pid == 7), 0);
        assert_eq!(sweep_profiles_in(&tmp.path().join("missing"), &|_| false), 0);
        for (name, pid) in [
            ("cgah-browser-4242-AbC123", Some(4242)),
            ("cgah-browser-99999999999-x", None),
        ] {
            assert_eq!(profile_owner(name), pid, "{name}");
        }
    }

    /// #894: a request the page sent on its own is listed for the operator
    /// and the task goes on; the model is told how many, never the URL.
    #[test]
    fn a_background_hold_is_listed_and_the_task_goes_on() {
        let (report, ops, sent) = run_batch_seen(&[("click", "Autosaves")], "task", "x");
        assert_eq!(ops, ["navigate", "click auto"]);
        assert_eq!(report["finished"], true);
        assert!(report["stopped"].is_null());
        assert_eq!(
            report["for_operator"],
            json!([{"held_background": "POST https://example.com/autosave?draft=page-private-text"}])
        );
        assert!(
            sent.contains("\\\"held_in_background\\\":1"),
            "the model sees the count"
        );
        assert!(!sent.contains("page-private-text"), "the model never sees the held URL");
        // The rest of a batch still runs after one.
        let (_, ops) = run_batch(&[("click", "Autosaves"), ("navigate", "https://example.com/next")]);
        assert_eq!(ops, ["navigate", "click auto", "navigate"]);
    }

    /// A model call that fails after the first turn stops the task with the
    /// steps already taken; a failed first call is still an error.
    #[test]
    fn a_late_model_failure_keeps_the_report() {
        let origins = vec!["https://example.com".to_string()];
        let mut session = Session::general(&origins).unwrap();
        let mut page = ScriptedPage::default();
        let mut turn = 0;
        let mut chat = |_: &[Value]| {
            turn += 1;
            if turn > 1 {
                return Err(OUT_OF_TIME.to_string());
            }
            Ok(json!({"message": {"role": "assistant", "content": "", "tool_calls": [
                {"function": {"name": "navigate", "arguments": {"url": "https://example.com/"}}}
            ]}}))
        };
        let report = model_loop(&mut session, &mut page, &mut chat, &|_| None, "task", &origins, 6).unwrap();
        assert_eq!(report["finished"], false);
        assert_eq!(report["stopped"], OUT_OF_TIME);
        assert_eq!(report["model_calls_with_tools"], 1);
        let mut failing = |_: &[Value]| Err("ollama chat status 404".to_string());
        assert!(model_loop(&mut session, &mut page, &mut failing, &|_| None, "task", &origins, 6).is_err());
    }

    #[test]
    fn a_deadline_bounds_each_model_call() {
        assert_eq!(call_timeout(None), MODEL_CALL_TIMEOUT);
        let left = call_timeout(Some(Instant::now() + Duration::from_secs(90)));
        assert!(left > Duration::from_secs(80) && left <= Duration::from_secs(90));
        let past = Instant::now().checked_sub(Duration::from_secs(1));
        assert!(past.is_some());
        assert_eq!(call_timeout(past), Duration::from_secs(1));
        assert_eq!(out_of_time(past).as_deref(), Some(OUT_OF_TIME));
        assert_eq!(out_of_time(None), None);
    }

    #[test]
    fn an_empty_reply_is_not_a_finished_task() {
        let origins = vec!["https://example.com".to_string()];
        let mut session = Session::general(&origins).unwrap();
        let mut page = ScriptedPage::default();
        for content in ["", "  \n "] {
            let mut chat = |_: &[Value]| Ok(json!({"message": {"role": "assistant", "content": content}}));
            let report = model_loop(&mut session, &mut page, &mut chat, &|_| None, "task", &origins, 6).unwrap();
            assert_eq!(report["finished"], false, "{content:?}");
            assert!(report["answer"].is_null());
            assert!(report["stopped"]
                .as_str()
                .unwrap()
                .contains("neither an action nor an answer"));
        }
        let mut chat = |_: &[Value]| Ok(json!({"message": {"role": "assistant", "content": "Python 3.14"}}));
        let report = model_loop(&mut session, &mut page, &mut chat, &|_| None, "task", &origins, 6).unwrap();
        assert_eq!(
            (report["finished"].clone(), report["answer"].clone()),
            (json!(true), json!("Python 3.14"))
        );
    }

    #[test]
    fn a_batch_with_nothing_for_the_operator_runs_to_the_answer() {
        let (report, ops) = run_batch(&[("click", "After"), ("navigate", "https://example.com/next")]);
        assert_eq!(ops, ["navigate", "click after", "navigate"]);
        assert_eq!(report["finished"], true);
        assert!(report["stopped"].is_null());
    }
}
