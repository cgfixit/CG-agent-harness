//! Server-side `/loop` runner: turns keep running after the browser disconnects.
//!
//! I6: this module lives entirely in `src/server` and reaches the model only
//! through the same [`crate::server::routes::core::chat_inner`] path the
//! console's loop turns use (`loop_turn: true`), with the same guards
//! (`claim_loop_inflight`, `loop_rate_limiter`, `loop_tool_allowlist`).
//! Stop conditions mirror the console's `runLoopTurns`: the configured turn
//! budget, the aggregate completion-token budget, consecutive failures,
//! `GOAL_DONE`, a repeated reply, and web codes that refuse the same way on
//! every later turn.

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

/// Mirror of the console `MAX_GOAL_CHARS`.
pub const MAX_GOAL_CHARS: usize = 2000;
/// Mirror of the console `LOOP_GOAL_REMINDER_CHARS`: the goal restated next to
/// every turn's reply, so a small model cannot lose it behind long history.
const LOOP_GOAL_REMINDER_CHARS: usize = 2000;
/// Mirror of `LOOP_STALL_SIMILARITY`.
const LOOP_STALL_SIMILARITY: f64 = 0.9;
/// Mirror of `LOOP_STALL_MAX_WORDS`.
const LOOP_STALL_MAX_WORDS: usize = 2000;

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
         Be concise. If the goal is fully satisfied, give a short summary and end with GOAL_DONE on its own line. \
         Stay in this chat; do not request a real-repo write."
    )
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoopRunState {
    Running,
    Paused,
    Stopped,
    Failed,
    Done,
}

impl LoopRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    pub fn terminal(self) -> bool {
        !matches!(self, Self::Running | Self::Paused)
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
    /// The account that started the run; its turns carry its owner context.
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
}

/// Active and recently finished runs, plus the persistence directory.
pub struct LoopManager {
    runs: Mutex<HashMap<String, LoopEntry>>,
    dir: PathBuf,
}

impl LoopManager {
    pub fn new(home: &std::path::Path) -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            dir: home.join("loops"),
        }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// A live (non-terminal) run for `session_id`, when one exists.
    pub fn active_for_session(&self, session_id: &str) -> Option<LoopRun> {
        let runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        runs.values()
            .find(|entry| entry.run.session_id == session_id && !entry.run.state.terminal())
            .map(|entry| entry.run.clone())
    }

    pub fn get(&self, run_id: &str) -> Option<LoopRun> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .map(|entry| entry.run.clone())
    }

    /// Record `run` as live and spawn its turn task. Fails when a live run for
    /// the same session already exists (`LOOP_IN_FLIGHT` semantics).
    pub fn start(
        &self,
        state: Arc<crate::server::state::AppState>,
        run: LoopRun,
        user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ) -> Result<LoopRun> {
        if self.active_for_session(&run.session_id).is_some() {
            return Err(HarnessError::new(
                "LOOP_IN_FLIGHT",
                "a loop run is already active for this session",
            ));
        }
        let (events, _) = tokio::sync::broadcast::channel(64);
        let entry = LoopEntry {
            run: run.clone(),
            cancel: CancellationToken::new(),
            events,
            user,
        };
        let cancel = entry.cancel.clone();
        let sender = entry.events.clone();
        let run_id = run.run_id.clone();
        {
            let mut runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
            // Re-check under the write lock: two starts raced.
            if runs
                .values()
                .any(|e| e.run.session_id == run.session_id && !e.run.state.terminal())
            {
                return Err(HarnessError::new(
                    "LOOP_IN_FLIGHT",
                    "a loop run is already active for this session",
                ));
            }
            runs.insert(run_id.clone(), entry);
        }
        let dir = self.dir.clone();
        tokio::spawn(async move {
            run_turns(state, run_id, cancel, sender, dir).await;
        });
        Ok(run)
    }

    /// Ask the turn task to stop; it records `Stopped` and emits the terminal event.
    pub fn stop(&self, run_id: &str) -> Option<LoopRun> {
        let runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        let entry = runs.get(run_id)?;
        entry.cancel.cancel();
        Some(entry.run.clone())
    }

    pub fn subscribe(&self, run_id: &str) -> Option<tokio::sync::broadcast::Receiver<Value>> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .map(|entry| entry.events.subscribe())
    }
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
        turn += 1;
        let framed = loop_turn_prompt(
            turn,
            (run.config.max_turns > 0).then_some(run.config.max_turns),
            &run.goal,
        );
        let req = ChatRequest {
            message: framed,
            session_id: Some(run.session_id.clone()),
            model: None,
            loop_turn: true,
            selected_facts: None,
            retrieve: false,
            retrieve_query: None,
            attachment_ids: Vec::new(),
        };
        let result = chat_inner(state.clone(), peer, user, req, None).await;
        let mut terminal: Option<(LoopRunState, String)> = None;
        match result {
            Ok(axum::Json(value)) => {
                let reply = value["reply"].as_str().unwrap_or_default().to_string();
                let completion = value["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                let web_events: Vec<Value> = value["web_tools"].as_array().cloned().unwrap_or_default();
                run.stats.turns_completed += 1;
                run.stats.total_completion_tokens = run.stats.total_completion_tokens.saturating_add(completion);
                run.stats.consecutive_failures = 0;
                run.stats.last_reply_preview = reply.chars().take(200).collect();
                let failed = web_events
                    .iter()
                    .filter(|e| e.get("ok") == Some(&Value::Bool(false)))
                    .count();
                if failed > 0 {
                    if web_events.iter().any(|e| e.get("ok") == Some(&Value::Bool(true))) {
                        run.stats.tool_failures = 0;
                    } else {
                        run.stats.tool_failures += 1;
                    }
                } else {
                    run.stats.tool_failures = 0;
                }
                if let Some(blocking) = loop_blocking_web_code(&web_events) {
                    terminal = Some((
                        LoopRunState::Done,
                        format!("loop stopped — {blocking}; the same refusal repeats on every later turn"),
                    ));
                } else if run.stats.tool_failures >= run.config.max_failures {
                    terminal = Some((
                        LoopRunState::Done,
                        format!(
                            "loop stopped — every web read failed on {} turns in a row",
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
            Err(error) => {
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
    fn turn_prompt_mirrors_console_framing() {
        let prompt = loop_turn_prompt(2, Some(3), "Write   a\nparser");
        assert!(
            prompt.starts_with("Loop turn 2 of 3. Session goal: Write a parser\n"),
            "{prompt}"
        );
        assert!(prompt.contains("GOAL_DONE"));
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
