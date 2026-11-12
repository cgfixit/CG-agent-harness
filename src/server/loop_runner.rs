//! Server-side `/loop` runner: turns keep running after the browser disconnects.
//!
//! I6: this module lives entirely in `src/server` and reaches the model only
//! through the same [`crate::server::routes::core::chat_inner`] path the
//! console's loop turns use (`loop_turn: true`), with the same guards
//! (`claim_loop_inflight`, `loop_rate_limiter`, `loop_tool_allowlist`).
//! Every lookup, stream and stop is bound to the starting owner, and each turn
//! re-reads that account first, so a detached run never keeps authority its
//! owner has lost.
//! Stop conditions mirror the console's `runLoopTurns`: the configured turn
//! budget, the aggregate completion-token budget, consecutive failures,
//! `GOAL_DONE`, a repeated reply, and web codes that refuse the same way on
//! every later turn. A changed session goal stops the run before its next turn.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::common::errors::{HarnessError, Result};
use crate::server::config_reload::LoopLimits;
use crate::server::routes::core::chat_inner;
use crate::server::schemas::ChatRequest;

/// Mirror of the console `LOOP_GOAL_REMINDER_CHARS`: the goal restated next to
/// every turn's reply, so a small model cannot lose it behind long history.
const LOOP_GOAL_REMINDER_CHARS: usize = 2000;
/// Mirror of `LOOP_STALL_SIMILARITY`.
const LOOP_STALL_SIMILARITY: f64 = 0.9;
/// Mirror of `LOOP_STALL_MAX_WORDS`.
const LOOP_STALL_MAX_WORDS: usize = 2000;

/// Longest a turn waits for another generation to release the single local
/// model: twice the longest deadline one local call may have, which is the most
/// a web turn can take. `/loop stop` ends the wait at once.
const MODEL_BUSY_WAIT: std::time::Duration =
    std::time::Duration::from_secs(2 * crate::server::model_limits::MAX_TIMEOUT_SEC);
const MODEL_BUSY_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// Turns in a row that may lose the model between that wait and their own
/// claim before a busy model counts as a failed turn after all.
const MAX_BUSY_RETRIES: u32 = 3;

/// Why [`wait_for_model`] gave up.
enum BusyWait {
    Stopped,
    TimedOut,
}

/// Waits while another generation holds the model: the operator's own chat
/// turn, a memory consolidation, the tuning sample. A loop turn that found the
/// model busy used to fail with `CHAT_BUSY`, and two of those two seconds apart
/// (the shipped cooldown and failure limit) aborted an unattended run.
async fn wait_for_model(
    state: &crate::server::state::AppState,
    cancel: &CancellationToken,
    limit: std::time::Duration,
) -> std::result::Result<(), BusyWait> {
    let started = tokio::time::Instant::now();
    while state.generation_gate.is_held() {
        if started.elapsed() >= limit {
            return Err(BusyWait::TimedOut);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(BusyWait::Stopped),
            _ = tokio::time::sleep(MODEL_BUSY_POLL) => {}
        }
    }
    Ok(())
}

/// Web tool codes that refuse the same way on every later turn (console
/// `LOOP_BLOCKING_WEB_CODES`). Only these stop an unattended run.
pub const LOOP_BLOCKING_WEB_CODES: [&str; 5] = [
    "WEB_ALLOWLIST_EMPTY",
    "WEB_GOOGLE_PERMISSION",
    "WEB_TOOL_DENIED",
    "WEB_POLICY_UNAVAILABLE",
    "WEB_DISABLED",
];

/// The console `loopBlockingWebCode` port: blocking codes stop a run; one
/// read's failure does not. Returns the blocking code, or `WEB_TOKEN_BUDGET`
/// when a whole round was refused and nothing succeeded.
pub fn loop_blocking_web_code(events: &[Value]) -> Option<String> {
    let failed: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("ok") == Some(&Value::Bool(false)))
        .collect();
    if failed.is_empty() {
        return None;
    }
    for event in &failed {
        if let Some(code) = event.get("code").and_then(|c| c.as_str()) {
            if LOOP_BLOCKING_WEB_CODES.contains(&code) {
                return Some(code.to_string());
            }
        }
    }
    if !events.iter().any(|e| e.get("ok") == Some(&Value::Bool(true)))
        && failed.iter().all(|e| {
            e.get("code").and_then(|c| c.as_str()) == Some("WEB_TOKEN_BUDGET")
                && e.get("tool").and_then(|t| t.as_str()) == Some("web")
        })
    {
        return Some("WEB_TOKEN_BUDGET".to_string());
    }
    None
}

/// Turns on which every web read failed or the model gave no answer after its
/// web calls, after this turn (console mirror).
/// `WEB_SEARCH_COOLDOWN` and `WEB_SEARCH_PACED` are deliberate pauses the model
/// was told not to retry: a turn whose only failures are those pauses neither
/// counts nor resets the streak.
pub fn next_tool_failures(previous: u64, events: &[Value]) -> u64 {
    let failed: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("ok") == Some(&Value::Bool(false)))
        .collect();
    // WEB_FINAL_EMPTY: the model wrote no answer and the harness wrote the reply.
    // That turn counts even though a read succeeded on it.
    if failed
        .iter()
        .any(|e| e.get("code").and_then(Value::as_str) == Some("WEB_FINAL_EMPTY"))
    {
        previous + 1
    } else if failed.is_empty() || events.iter().any(|e| e.get("ok") == Some(&Value::Bool(true))) {
        0
    } else if failed.iter().all(|e| {
        matches!(
            e.get("code").and_then(Value::as_str),
            Some("WEB_SEARCH_COOLDOWN" | "WEB_SEARCH_PACED")
        )
    }) {
        previous
    } else {
        previous + 1
    }
}

/// Console `loopWords` port: lowercase words with punctuation dropped.
fn loop_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Console `repliesNearlyRepeat` port: 2·LCS/(|a|+|b|) over words, at least
/// [`LOOP_STALL_SIMILARITY`], bounded to the first [`LOOP_STALL_MAX_WORDS`]
/// words of each reply.
pub fn replies_nearly_repeat(previous: &str, next: &str) -> bool {
    let a: Vec<String> = loop_words(previous).into_iter().take(LOOP_STALL_MAX_WORDS).collect();
    let b: Vec<String> = loop_words(next).into_iter().take(LOOP_STALL_MAX_WORDS).collect();
    if a.is_empty() || b.is_empty() {
        return previous.trim() == next.trim();
    }
    if a == b {
        return true;
    }
    if a.len() < 12 || b.len() < 12 {
        return false;
    }
    let mut row = vec![0usize; b.len() + 1];
    let mut prev = vec![0usize; b.len() + 1];
    for aw in &a {
        std::mem::swap(&mut row, &mut prev);
        row[0] = 0;
        for (j, bw) in b.iter().enumerate() {
            row[j + 1] = if aw == bw { prev[j] + 1 } else { prev[j + 1].max(row[j]) };
        }
    }
    2.0 * row[b.len()] as f64 / (a.len() + b.len()) as f64 >= LOOP_STALL_SIMILARITY
}

/// One loop turn's instruction (console `loopTurnPrompt`). `total` is `None`
/// for an unlimited run. The goal reminder is code-point bounded so a cut
/// surrogate pair can never reach JSON.
pub fn loop_turn_prompt(turn: u64, total: Option<u64>, goal: &str) -> String {
    let collapsed: String = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    let reminder: String = collapsed.chars().take(LOOP_GOAL_REMINDER_CHARS).collect();
    let total_label = total.map(|n| n.to_string()).unwrap_or_else(|| "unlimited".to_string());
    format!(
        "Loop turn {turn} of {total_label}. Session goal: {reminder}\n\
         Follow every constraint in the full stored goal in system context. Do the next concrete step toward this goal; do not repeat or re-summarize earlier replies. \
         Complete only harness turn {turn}; tool calls and evidence digests do not advance the turn. Do not simulate later turns. \
         Be concise. If the goal is fully satisfied, give a short summary and end with GOAL_DONE on its own line. \
         Stay in this chat; do not request a real-repo write."
    )
}

/// What the session stores for one server-run loop turn. The model receives
/// [`loop_turn_prompt`]; storing that too put the goal reminder (up to 2000
/// characters, already in the system prompt) into the history once per turn,
/// where it was sent again on every later turn and took half of what the
/// compaction summarizer read.
pub fn loop_turn_stored(turn: u64, total: Option<u64>) -> String {
    let total_label = total.map(|n| n.to_string()).unwrap_or_else(|| "unlimited".to_string());
    format!("Loop turn {turn} of {total_label}.")
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoopRunState {
    Running,
    Stopped,
    Failed,
    Done,
}

impl LoopRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    pub fn terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct LoopStats {
    pub turns_completed: u64,
    pub total_completion_tokens: u64,
    pub consecutive_failures: u64,
    pub tool_failures: u64,
    pub last_reply_preview: String,
}

/// One server-side `/loop` run. Persisted to `$CGAGENTHARNESS_HOME/loops/<run_id>.json`
/// after every turn so a run's progress survives a restart (a running run does
/// not resume; the file is the operator's record).
#[derive(Debug, Clone, Serialize)]
pub struct LoopRun {
    pub run_id: String,
    pub session_id: String,
    pub goal: String,
    pub web_mode: String,
    pub config: LoopLimits,
    pub state: LoopRunState,
    pub reason: String,
    pub stats: LoopStats,
}

struct LoopEntry {
    run: LoopRun,
    cancel: CancellationToken,
    /// Per-turn progress; every SSE subscriber gets its own receiver.
    events: tokio::sync::broadcast::Sender<Value>,
    /// The session owner that started the run (`local` without accounts).
    /// Every lookup, stream and stop is bound to it; a foreign run reads as
    /// not found.
    owner: String,
    /// The account that started the run; its turns carry its owner context
    /// after [`current_authority`] re-reads it before each turn.
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    web_evidence: super::chat_web::evidence::RunEvidence,
}

/// Active and recently finished runs, plus the persistence directory.
pub struct LoopManager {
    runs: Mutex<HashMap<String, LoopEntry>>,
    dir: PathBuf,
}

impl LoopManager {
    pub(crate) fn web_evidence(&self, owner: &str, run_id: &str, goal: &str) -> super::chat_web::evidence::RunEvidence {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .filter(|e| e.owner == owner && e.run.goal == goal && !e.run.state.terminal() && !e.cancel.is_cancelled())
            .map(|e| e.web_evidence.clone())
            .unwrap_or_default()
    }

    pub(crate) fn save_web_evidence(
        &self,
        owner: &str,
        run_id: &str,
        goal: &str,
        evidence: super::chat_web::evidence::RunEvidence,
    ) {
        if let Some(entry) = self
            .runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(run_id)
            .filter(|e| e.owner == owner && e.run.goal == goal && !e.run.state.terminal() && !e.cancel.is_cancelled())
        {
            entry.web_evidence = evidence;
        }
    }
    pub fn new(home: &std::path::Path) -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            dir: home.join("loops"),
        }
    }

    /// The owner's live (non-terminal) run for `session_id`, when one exists.
    /// A console that reopens the session uses it to reattach.
    pub fn active_for_session(&self, owner: &str, session_id: &str) -> Option<LoopRun> {
        let runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        runs.values()
            .find(|entry| entry.owner == owner && entry.run.session_id == session_id && !entry.run.state.terminal())
            .map(|entry| entry.run.clone())
    }

    pub fn get(&self, owner: &str, run_id: &str) -> Option<LoopRun> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .filter(|entry| entry.owner == owner)
            .map(|entry| entry.run.clone())
    }

    /// Record `run` as live and spawn its turn task. Fails when a live run for
    /// the same session already exists (`LOOP_IN_FLIGHT` semantics).
    pub fn start(
        &self,
        state: Arc<crate::server::state::AppState>,
        run: LoopRun,
        owner: String,
        user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ) -> Result<LoopRun> {
        let (events, _) = tokio::sync::broadcast::channel(64);
        let entry = LoopEntry {
            run: run.clone(),
            cancel: CancellationToken::new(),
            events,
            owner,
            user,
            web_evidence: Default::default(),
        };
        let cancel = entry.cancel.clone();
        let sender = entry.events.clone();
        let run_id = run.run_id.clone();
        {
            let mut runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
            // Checked under the write lock so two racing starts cannot both win.
            if runs
                .values()
                .any(|e| e.run.session_id == run.session_id && !e.run.state.terminal())
            {
                return Err(HarnessError::new(
                    "LOOP_IN_FLIGHT",
                    "a loop run is already active for this session",
                ));
            }
            // A session keeps only its newest run in memory, so the map is
            // bounded by sessions rather than growing with every run; a
            // finished run's record stays in loops/<run_id>.json.
            runs.retain(|_, e| e.run.session_id != run.session_id);
            runs.insert(run_id.clone(), entry);
        }
        let dir = self.dir.clone();
        tokio::spawn(async move {
            run_turns(state, run_id, cancel, sender, dir).await;
        });
        Ok(run)
    }

    /// Ask the turn task to stop; it records `Stopped` and emits the terminal event.
    pub fn stop(&self, owner: &str, run_id: &str) -> Option<LoopRun> {
        let runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        let entry = runs.get(run_id).filter(|entry| entry.owner == owner)?;
        entry.cancel.cancel();
        Some(entry.run.clone())
    }

    /// The owner's run snapshot and a receiver for its later events, taken
    /// under one lock so no event falls between them.
    pub fn subscribe(&self, owner: &str, run_id: &str) -> Option<(LoopRun, tokio::sync::broadcast::Receiver<Value>)> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .filter(|entry| entry.owner == owner)
            .map(|entry| (entry.run.clone(), entry.events.subscribe()))
    }
}

/// Re-read the starting account before a turn: a detached run never keeps
/// more authority than its owner holds now. The account must still exist with
/// the same id, be enabled, need no password change, and hold a role that may
/// start a loop (`operator` or `admin`). Without accounts there is nothing to
/// revoke.
fn current_authority(
    state: &crate::server::state::AppState,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
) -> std::result::Result<Option<axum::Extension<crate::common::auth_store::UserSummary>>, String> {
    let (Some(auth), Some(started)) = (state.auth.as_ref(), user.as_ref()) else {
        return Ok(user);
    };
    let Some(current) = auth.get_user(&started.username) else {
        return Err("the account that started this loop no longer exists".into());
    };
    if current.user_id != started.user_id {
        return Err("the account that started this loop no longer exists".into());
    }
    if current.disabled {
        return Err("the account that started this loop was disabled".into());
    }
    if current.must_change_password {
        return Err("the account that started this loop must change its password".into());
    }
    if !matches!(current.role.as_str(), "operator" | "admin") {
        return Err(format!(
            "the account that started this loop now has role {} and cannot run loop turns",
            current.role
        ));
    }
    Ok(Some(axum::Extension(current)))
}

fn persist(dir: &std::path::Path, run: &LoopRun) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(error = %e, "loop run persistence directory could not be created");
        return;
    }
    let path = dir.join(format!("{}.json", run.run_id));
    let Ok(text) = serde_json::to_string_pretty(run) else {
        return;
    };
    // A torn write must never leave a run file the operator cannot read.
    let tmp = dir.join(format!(".{}.json.tmp", run.run_id));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

async fn run_turns(
    state: Arc<crate::server::state::AppState>,
    run_id: String,
    cancel: CancellationToken,
    events: tokio::sync::broadcast::Sender<Value>,
    dir: PathBuf,
) {
    let peer: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 0));
    let mut last_reply: Option<String> = None;
    let mut turn: u64 = 0;
    let mut busy_retries: u32 = 0;
    loop {
        let (mut run, user) = {
            let runs = state.loops.runs.lock().unwrap_or_else(|p| p.into_inner());
            let Some(entry) = runs.get(&run_id) else { return };
            (entry.run.clone(), entry.user.clone())
        };
        if cancel.is_cancelled() {
            finish(
                &state,
                &run_id,
                run,
                LoopRunState::Stopped,
                "stopped by operator".into(),
                &events,
                &dir,
            );
            return;
        }
        if run.config.max_turns > 0 && run.stats.turns_completed >= run.config.max_turns {
            let reason = format!("loop budget exhausted ({} turn(s))", run.stats.turns_completed);
            finish(&state, &run_id, run, LoopRunState::Done, reason, &events, &dir);
            return;
        }
        if run.config.max_total_completion_tokens > 0
            && run.stats.total_completion_tokens >= run.config.max_total_completion_tokens
        {
            let reason = format!(
                "loop aggregate token budget exhausted ({} total completion tokens)",
                run.stats.total_completion_tokens
            );
            finish(&state, &run_id, run, LoopRunState::Done, reason, &events, &dir);
            return;
        }
        match wait_for_model(&state, &cancel, MODEL_BUSY_WAIT).await {
            Ok(()) => {}
            Err(BusyWait::Stopped) => {
                finish(
                    &state,
                    &run_id,
                    run,
                    LoopRunState::Stopped,
                    "stopped by operator".into(),
                    &events,
                    &dir,
                );
                return;
            }
            Err(BusyWait::TimedOut) => {
                let reason = format!(
                    "loop aborted — the local model stayed busy for {} minutes",
                    MODEL_BUSY_WAIT.as_secs() / 60
                );
                finish(&state, &run_id, run, LoopRunState::Failed, reason, &events, &dir);
                return;
            }
        }
        // After the wait, never before it: the wait can last as long as a
        // background model job, and an account disabled or demoted meanwhile
        // must not get the turn that was queued for it.
        let user = match current_authority(&state, user) {
            Ok(user) => user,
            Err(reason) => {
                finish(
                    &state,
                    &run_id,
                    run,
                    LoopRunState::Failed,
                    format!("loop aborted — {reason}"),
                    &events,
                    &dir,
                );
                return;
            }
        };
        turn += 1;
        let total = (run.config.max_turns > 0).then_some(run.config.max_turns);
        let framed = loop_turn_prompt(turn, total, &run.goal);
        let req = ChatRequest {
            message: framed.clone(),
            session_id: Some(run.session_id.clone()),
            model: None,
            loop_turn: true,
            selected_facts: None,
            retrieve: false,
            retrieve_query: None,
            attachment_ids: Vec::new(),
            stored_message: Some(loop_turn_stored(turn, total)),
        };
        let result = chat_inner(state.clone(), peer, user, req, None, Some(&run.goal)).await;
        let mut terminal: Option<(LoopRunState, String)> = None;
        match result {
            Ok(axum::Json(value)) => {
                let reply = value["reply"].as_str().unwrap_or_default().to_string();
                // The turn's own prompt and reply, so a watching console renders
                // the transcript as the browser-driven loop does. Subscribers
                // are bound to the run's owner (see LoopManager::subscribe).
                let _ = events.send(json!({
                    "type": "exchange",
                    "prompt": framed,
                    "reply": reply,
                    "model": value["model"],
                    "usage": value["usage"],
                    "tally": value["tally"],
                    "web_tools": value["web_tools"],
                }));
                let completion = value["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                let web_events: Vec<Value> = value["web_tools"].as_array().cloned().unwrap_or_default();
                busy_retries = 0;
                run.stats.turns_completed += 1;
                run.stats.total_completion_tokens = run.stats.total_completion_tokens.saturating_add(completion);
                run.stats.consecutive_failures = 0;
                run.stats.last_reply_preview = reply.chars().take(200).collect();
                run.stats.tool_failures = next_tool_failures(run.stats.tool_failures, &web_events);
                if let Some(blocking) = loop_blocking_web_code(&web_events) {
                    terminal = Some((
                        LoopRunState::Done,
                        format!("loop stopped — {blocking}; the same refusal repeats on every later turn"),
                    ));
                } else if run.stats.tool_failures >= run.config.max_failures {
                    terminal = Some((
                        LoopRunState::Done,
                        format!(
                            "loop stopped — web reads failed or the model gave no answer on {} turns in a row",
                            run.config.max_failures
                        ),
                    ));
                } else if crate::server::routes::core::reply_has_goal_done(&reply) {
                    terminal = Some((
                        LoopRunState::Done,
                        "model reported GOAL_DONE (unverified) — loop stopped".into(),
                    ));
                } else if last_reply.as_deref().is_some_and(|p| replies_nearly_repeat(p, &reply)) {
                    terminal = Some((
                        LoopRunState::Done,
                        "loop stopped — model repeated the previous reply (stall)".into(),
                    ));
                } else {
                    last_reply = Some(reply);
                }
            }
            Err(error) if error.code == "LOOP_GOAL_CHANGED" => {
                terminal = Some((LoopRunState::Stopped, error.message));
            }
            // Another generation took the model between the wait above and
            // this turn's own claim. Nothing was sent: the turn is not failed
            // and not counted, and the next pass waits again.
            Err(error) if error.code == "CHAT_BUSY" && busy_retries < MAX_BUSY_RETRIES => {
                busy_retries += 1;
                turn -= 1;
            }
            Err(error) => {
                busy_retries = 0;
                // Failed inference still spends tokens; missing usage contributes no known count.
                let completion = error.details["known_completion_tokens"]
                    .as_u64()
                    .or_else(|| error.details["completion_tokens"].as_u64())
                    .unwrap_or(0);
                run.stats.total_completion_tokens = run.stats.total_completion_tokens.saturating_add(completion);
                run.stats.consecutive_failures += 1;
                run.reason = format!("turn {turn} failed: {} ({})", error.code, error.message);
                if run.stats.consecutive_failures >= run.config.max_failures {
                    terminal = Some((
                        LoopRunState::Failed,
                        format!("loop aborted — {} consecutive chat failures", run.config.max_failures),
                    ));
                }
            }
        }
        let Some((final_state, reason)) = terminal else {
            // Cool down between turns so /loop stop can land and the model
            // server breathes; a stop lands immediately.
            {
                let mut runs = state.loops.runs.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(entry) = runs.get_mut(&run_id) {
                    entry.run = run.clone();
                }
            }
            persist(&dir, &run);
            let _ = events.send(json!({"type": "turn", "run": run}));
            if run.config.cooldown_ms > 0 {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        finish(&state, &run_id, run, LoopRunState::Stopped, "stopped by operator".into(), &events, &dir);
                        return;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(run.config.cooldown_ms)) => {}
                }
            }
            continue;
        };
        finish(&state, &run_id, run, final_state, reason, &events, &dir);
        return;
    }
}

fn finish(
    state: &Arc<crate::server::state::AppState>,
    run_id: &str,
    mut run: LoopRun,
    final_state: LoopRunState,
    reason: String,
    events: &tokio::sync::broadcast::Sender<Value>,
    dir: &std::path::Path,
) {
    run.state = final_state;
    run.reason = reason.clone();
    {
        let mut runs = state.loops.runs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = runs.get_mut(run_id) {
            entry.run = run.clone();
            entry.web_evidence = Default::default();
        }
    }
    persist(dir, &run);
    state.audit.log(json!({
        "event": "loop_run_finished",
        "run_id": run.run_id,
        "session_id": run.session_id,
        "state": run.state.as_str(),
        "reason": reason,
        "turns_completed": run.stats.turns_completed,
        "total_completion_tokens": run.stats.total_completion_tokens,
    }));
    let _ = events.send(json!({"type": "state", "run": run}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_references_belong_only_to_the_active_owner_run_and_goal() {
        let dir = tempfile::tempdir().unwrap();
        let manager = LoopManager::new(dir.path());
        let (events, _) = tokio::sync::broadcast::channel(1);
        let run = LoopRun {
            run_id: "run".into(),
            session_id: "session".into(),
            goal: "goal".into(),
            web_mode: "all".into(),
            config: LoopLimits {
                max_turns: 2,
                max_total_completion_tokens: 4096,
                max_failures: 2,
                cooldown_ms: 0,
            },
            state: LoopRunState::Running,
            reason: String::new(),
            stats: Default::default(),
        };
        manager.runs.lock().unwrap().insert(
            "run".into(),
            LoopEntry {
                run,
                cancel: CancellationToken::new(),
                events,
                owner: "alice".into(),
                user: None,
                web_evidence: Default::default(),
            },
        );
        let mut evidence = super::super::chat_web::evidence::RunEvidence::default();
        evidence.searched("question");
        // Search-result read grants ride with the run and end with it.
        evidence.granted = vec!["https://listed.example/page".into()];
        manager.save_web_evidence("alice", "run", "goal", evidence.clone());
        assert_eq!(manager.web_evidence("alice", "run", "goal").queries, vec!["question"]);
        assert_eq!(manager.web_evidence("alice", "run", "goal").granted.len(), 1);
        for (owner, run, goal) in [
            ("bob", "run", "goal"),
            ("alice", "different", "goal"),
            ("alice", "run", "changed"),
        ] {
            assert!(manager.web_evidence(owner, run, goal).queries.is_empty());
            manager.save_web_evidence(owner, run, goal, Default::default());
        }
        assert_eq!(manager.web_evidence("alice", "run", "goal").queries.len(), 1);
        manager.runs.lock().unwrap().get_mut("run").unwrap().cancel.cancel();
        assert!(manager.web_evidence("alice", "run", "goal").queries.is_empty());
        assert!(manager.web_evidence("alice", "run", "goal").granted.is_empty());
        manager.save_web_evidence("alice", "run", "goal", evidence);
    }

    #[test]
    fn search_cooldown_turns_do_not_count_toward_max_failures() {
        let fail = |code: &str| json!({"tool":"web_search","ok":false,"code":code});
        let ok = json!({"tool":"web_fetch","ok":true});
        assert_eq!(next_tool_failures(1, &[fail("WEB_SEARCH_COOLDOWN")]), 1);
        assert_eq!(next_tool_failures(0, &[fail("WEB_SEARCH_COOLDOWN")]), 0);
        assert_eq!(next_tool_failures(1, &[fail("WEB_FETCH_FAILED")]), 2);
        assert_eq!(
            next_tool_failures(1, &[fail("WEB_SEARCH_COOLDOWN"), fail("WEB_TIMEOUT")]),
            2
        );
        assert_eq!(next_tool_failures(3, &[fail("WEB_SEARCH_COOLDOWN"), ok.clone()]), 0);
        assert_eq!(next_tool_failures(3, &[]), 0);
        // A turn the harness had to answer counts, even with a read that worked.
        assert_eq!(next_tool_failures(1, &[ok.clone(), fail("WEB_FINAL_EMPTY")]), 2);
        assert_eq!(loop_blocking_web_code(&[ok.clone(), fail("WEB_FINAL_EMPTY")]), None);
        assert_eq!(loop_blocking_web_code(&[fail("WEB_SEARCH_COOLDOWN")]), None);
    }

    #[test]
    fn search_paced_turns_do_not_count_toward_max_failures() {
        let fail = |code: &str| json!({"tool":"web_search","ok":false,"code":code});
        let ok = json!({"tool":"web_fetch","ok":true});
        assert_eq!(next_tool_failures(1, &[fail("WEB_SEARCH_PACED")]), 1);
        assert_eq!(next_tool_failures(0, &[fail("WEB_SEARCH_PACED")]), 0);
        assert_eq!(
            next_tool_failures(1, &[fail("WEB_SEARCH_PACED"), fail("WEB_SEARCH_COOLDOWN")]),
            1
        );
        assert_eq!(
            next_tool_failures(1, &[fail("WEB_SEARCH_PACED"), fail("WEB_TIMEOUT")]),
            2
        );
        assert_eq!(next_tool_failures(3, &[fail("WEB_SEARCH_PACED"), ok]), 0);
        assert_eq!(loop_blocking_web_code(&[fail("WEB_SEARCH_PACED")]), None);
    }

    #[test]
    fn the_stored_turn_is_a_short_label_without_the_goal() {
        let goal = "g ".repeat(900);
        let sent = loop_turn_prompt(3, Some(50), &goal);
        let stored = loop_turn_stored(3, Some(50));
        assert_eq!(stored, "Loop turn 3 of 50.");
        assert!(
            sent.starts_with(&stored),
            "the stored label opens the prompt that was sent"
        );
        assert!(sent.len() > 1_800 && stored.len() < 32);
        assert_eq!(loop_turn_stored(1, None), "Loop turn 1 of unlimited.");
    }

    #[test]
    fn turn_prompt_mirrors_console_framing() {
        let prompt = loop_turn_prompt(2, Some(3), "Write   a\nparser");
        assert!(
            prompt.starts_with("Loop turn 2 of 3. Session goal: Write a parser\n"),
            "{prompt}"
        );
        assert!(prompt.contains("GOAL_DONE"));
        assert!(
            prompt.contains("Complete only harness turn 2; tool calls and evidence digests do not advance the turn.")
        );
        let unlimited = loop_turn_prompt(1, None, "g");
        assert!(
            unlimited.starts_with("Loop turn 1 of unlimited. Session goal: g\n"),
            "{unlimited}"
        );
        let long = loop_turn_prompt(1, Some(1), &"x".repeat(2001));
        assert!(
            long.contains(&"x".repeat(2000)),
            "the goal reminder is code-point bounded"
        );
        assert!(!long.contains(&"x".repeat(2001)));
    }

    #[test]
    fn stall_and_blocking_ports_match_console_cases() {
        assert!(replies_nearly_repeat("same words here", "same words here"));
        assert!(!replies_nearly_repeat(
            "a completely different answer",
            "same words here"
        ));
        assert!(replies_nearly_repeat(
            &"word ".repeat(30),
            &format!("{}changed", "word ".repeat(29))
        ));
        let page = json!({"tool": "web_fetch", "ok": true});
        assert_eq!(
            loop_blocking_web_code(&[
                page.clone(),
                json!({"tool": "web_fetch", "ok": false, "code": "WEB_HOST_DENIED"})
            ]),
            None
        );
        assert_eq!(
            loop_blocking_web_code(&[json!({"tool": "web", "ok": false, "code": "WEB_TOKEN_BUDGET"})]),
            Some("WEB_TOKEN_BUDGET".into())
        );
        assert_eq!(
            loop_blocking_web_code(&[
                page,
                json!({"tool": "web_search", "ok": false, "code": "WEB_ALLOWLIST_EMPTY"})
            ]),
            Some("WEB_ALLOWLIST_EMPTY".into())
        );
        assert_eq!(loop_blocking_web_code(&[]), None);
    }
}
