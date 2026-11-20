//! `POST /api/browser/run`: one model-driven browser task for an administrator.
//!
//! Gated by literal `browser.enabled` (ships true, restart-only) and literal
//! `auth.enabled`: without accounts the admin check does not run. The request
//! names the task and the origins it may reach; the page and the model cannot
//! add one. The work runs in a fixed-argv `cgagentharness browser run` child
//! with the browser environment allowlist, so the server spawns nothing itself
//! and a timeout kills the child's whole tree. Non-public IP-literal origins
//! and origins on the shared block list are refused; the block list is read
//! again before each model turn (in the child) and before delivery. The child
//! is told chat's local model selection, so `/model use` applies to it, and a
//! deadline inside `browser.timeout_sec`, so it reports before it is stopped.
//! A tuned model is loaded and its window read before the child starts.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::browser::Session;
use crate::common::browser_host::{blocked_origin, child_environment, MAX_TASK_CHARS, TASK_PROMPT_TOKENS};
use crate::server::browser_jobs::{CancelHandle, Outcome, VerdictRefusal, JOB_RUNNING};
use crate::server::errors::{ApiError, ApiResult};
use crate::server::schemas::{ValidJson, Validate};
use crate::server::state::AppState;
use crate::shim::ShimError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRunRequest {
    pub task: String,
    pub origins: Vec<String>,
}

impl Validate for BrowserRunRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !(1..=MAX_TASK_CHARS).contains(&self.task.chars().count()) {
            bad.push("task".into());
        }
        if self.origins.is_empty() || Session::general(&self.origins).is_err() {
            bad.push("origins".into());
        }
        bad
    }
}

/// One checked request, ready to start: every refusal has been made, the
/// model gate is held and the child's argv is built. `/api/browser/run` and
/// `/api/browser/jobs` both start from here, so neither skips a check.
struct Prepared {
    session: Session,
    grants: Vec<String>,
    model: Option<String>,
    owner: String,
    task_sha256: String,
    argv: Vec<String>,
    cwd: std::path::PathBuf,
    timeout: Duration,
    /// The task text (0600), removed when this is dropped.
    task_file: tempfile::NamedTempFile,
    /// Held until this is dropped: one model operation at a time.
    gate: crate::server::generation_gate::GateGuard,
}

async fn prepare(
    state: &Arc<AppState>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    req: &BrowserRunRequest,
) -> ApiResult<Prepared> {
    let cfg = state.cfg.clone();
    if !cfg.flag_is_true("browser.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_DISABLED",
            "Browser sessions are off. Set browser.enabled: true in config.yaml and restart.",
        ));
    }
    // Without accounts the admin check in guards.rs does not run, so any
    // client holding the console's CSRF token could drive Chrome (#945).
    if !cfg.flag_is_true("auth.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_AUTH_REQUIRED",
            "Browser tasks need accounts: set auth.enabled: true in config.yaml and restart, then sign in as an administrator.",
        ));
    }
    let session = Session::general(&req.origins)
        .map_err(|_| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "BROWSER_BAD_ORIGIN", "origins"))?;
    // A name is checked at connect time by the sidecar; an IP literal skips DNS,
    // so loopback and private literals are refused here (fixtures are test-only).
    let fixture = fixture_origins();
    if !fixture && session.grant_strings().iter().any(|origin| non_public_literal(origin)) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "BROWSER_PRIVATE_ADDRESS",
            "Browser tasks reach public addresses only; a loopback or private IP origin is refused.",
        ));
    }
    // shortcut: only origin-level block rules are enforced; a path-level rule needs the proxy to see it.
    if !fixture && blocked_origin(&current_policy(state)?, &session).is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "BROWSER_ORIGIN_BLOCKED",
            "An origin is on the shared web block list, or could not be checked; /web status lists the rules.",
        ));
    }
    // One model operation at a time: browser tasks share the local backend.
    // Refused at once, never queued; the refusal says what holds the model.
    let gate = state.generation_gate.claim_or_busy_owner("browser").map_err(|holder| {
        ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_BUSY",
            format!(
                "The local model is busy: {}. Nothing was started; send /browse again when it finishes.",
                busy_reason(&holder)
            ),
        )
    })?;
    // Under the gate: a tuned model that is not loaded (keep_alive ran out) is
    // loaded here, outside the task's time, and a window too small for a task's
    // prompt is refused; Ollama would cut the prompt without saying so.
    let model = task_model(&state.backend.source, state.local_memory_model());
    if let Some(model) = &model {
        if state
            .ensure_window_allows(model, TASK_PROMPT_TOKENS, 0, false)
            .await
            .is_err()
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "BROWSER_WINDOW",
                "Ollama does not report the selected model loaded with a window that fits a browser task; \
                 nothing was started. /model profile shows the window.",
            ));
        }
    }
    let owner = super::auth::context_owner(user);
    let exe = std::env::current_exe()
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "BROWSER_FAILED", "harness binary not found"))?;
    let cwd = std::env::current_dir().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "working directory unreadable",
        )
    })?;
    // The task goes in a private temporary file (0600, removed with `Prepared`),
    // not in argv, which other local accounts can list.
    let task_file = write_task_file(&req.task)?;
    // Canonical origins only: a requested URL's path or query (a token, say)
    // never reaches argv, the audit log or the report.
    let grants = session.grant_strings();
    let mut argv = vec![
        exe.to_string_lossy().into_owned(),
        "browser".into(),
        "run".into(),
        format!("--task-file={}", task_file.path().display()),
        format!("--sidecar={}", cfg.str_or("browser.sidecar", "browser/sidecar.mjs")),
        format!("--max-turns={}", cfg.limit("browser.max_turns")),
    ];
    let timeout_sec = cfg.limit("browser.timeout_sec");
    argv.push(format!("--deadline-sec={}", child_deadline_sec(timeout_sec)));
    argv.extend(model.iter().map(|model| format!("--model={model}")));
    argv.extend(grants.iter().map(|origin| format!("--origin={origin}")));
    if fixture {
        argv.push("--allow-private".into());
    }
    Ok(Prepared {
        session,
        grants,
        model,
        owner,
        task_sha256: crate::common::sha256_hex(&req.task),
        argv,
        cwd,
        timeout: Duration::from_secs(timeout_sec),
        task_file,
        gate,
    })
}

/// What the task was allowed and who asked: all `deliver` needs once the
/// child is gone.
struct Asked {
    session: Session,
    grants: Vec<String>,
    model: Option<String>,
    owner: String,
    task_sha256: String,
    job: Option<String>,
}

type ChildResult = Result<(i32, String, String), ShimError>;

/// Audit the run, recheck the block list, and turn the child's output into
/// the response body. A job's stdout has progress lines before the report, so
/// the report is the last line.
fn deliver(state: &AppState, asked: &Asked, result: ChildResult) -> ApiResult<Value> {
    let exit = result.as_ref().map(|(code, _, _)| *code).ok();
    state.audit.log(json!({
        "event": "browser.run",
        "owner": asked.owner,
        "origins": asked.grants,
        "model": asked.model,
        "task_sha256": asked.task_sha256,
        "job": asked.job,
        "exit": exit,
    }));
    let (code, stdout, stderr) = result.map_err(|err| match err {
        ShimError::Timeout { .. } => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "BROWSER_TIMEOUT",
            "The browser task did not finish within browser.timeout_sec; its processes were stopped.",
        ),
        _ => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "The browser child could not run.",
        ),
    })?;
    // Permission checked at launch is stale after the run; deliver nothing if
    // the block list now covers a granted origin.
    if policy_now_blocks(state, &asked.session) {
        return Err(policy_changed());
    }
    let report = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|report| report.get("event").is_none());
    // #901: composed navigations refused and typed text held, by origin and
    // count only. The child reports origins, never a refused URL's path or query.
    if let Some(report) = &report {
        let typed_held = report["for_operator"].as_array().map_or(0, |rows| {
            rows.iter()
                .filter(|row| row["reason"] == crate::common::browser::TYPED_TEXT_HOLD)
                .count()
        });
        let refused = report["refused_composed"].as_array().filter(|rows| !rows.is_empty());
        if refused.is_some() || typed_held > 0 {
            state.audit.log(json!({
                "event": "browser.provenance",
                "owner": asked.owner,
                "refused_composed": refused,
                "typed_held": typed_held,
            }));
        }
    }
    let mut body = json!({"finished": code == 0, "exit_code": code, "report": report});
    if body["report"].is_null() {
        // Only the child's own `cgagentharness:` lines; Node and page output stay out.
        let reason = stderr
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("cgagentharness: "))
            .unwrap_or("the browser child printed no report");
        body["reason"] = json!(reason.chars().take(300).collect::<String>());
    }
    Ok(body)
}

fn policy_now_blocks(state: &AppState, session: &Session) -> bool {
    // The public block list cannot name a fixture's loopback origin.
    !fixture_origins() && current_policy(state).map_or(true, |policy| blocked_origin(&policy, session).is_some())
}

fn policy_changed() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "WEB_POLICY_CHANGED",
        "The URL policy changed during this browser task; nothing was delivered.",
    )
}

pub async fn run(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<BrowserRunRequest>,
) -> ApiResult<Json<Value>> {
    let Prepared {
        session,
        grants,
        model,
        owner,
        task_sha256,
        argv,
        cwd,
        timeout,
        task_file,
        gate: _gate,
    } = prepare(&state, user, &req).await?;
    let result = crate::shim::run_argv_with_env(&argv, &cwd, timeout, child_environment()).await;
    drop(task_file);
    let asked = Asked {
        session,
        grants,
        model,
        owner,
        task_sha256,
        job: None,
    };
    deliver(&state, &asked, result).map(Json)
}

/// How long a cancelled job's child gets to stop by itself (it stops before
/// its next model turn) before its process tree is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// `POST /api/browser/jobs`: the same checked request as `/api/browser/run`,
/// detached. Every refusal is still synchronous; 202 means the task holds the
/// model gate and its child is starting. The job's task owns the gate, the
/// task file and the child, so a closed tab ends none of them.
pub async fn job_start(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<BrowserRunRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if cfg!(not(unix)) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_JOBS_UNSUPPORTED",
            "Browser jobs run on macOS and Linux only; /browse without a job still works.",
        ));
    }
    let Prepared {
        session,
        grants,
        model,
        owner,
        task_sha256,
        mut argv,
        cwd,
        timeout,
        task_file,
        gate,
    } = prepare(&state, user, &req).await?;
    argv.push("--job".into());
    // Literal true only: a job started without it holds every request, as before.
    let takeover = state.cfg.flag_is_true("browser.takeover.enabled");
    if takeover {
        argv.push(format!(
            "--takeover-wait-sec={}",
            state.cfg.limit("browser.takeover.wait_sec")
        ));
    }
    let job_id = crate::common::random_hex(16);
    let (lines, mut printed) = tokio::sync::mpsc::unbounded_channel();
    let (input, child_input) = std::sync::mpsc::channel();
    let token = tokio_util::sync::CancellationToken::new();
    state.browser_jobs.insert(
        &job_id,
        &owner,
        &grants,
        CancelHandle {
            input,
            token: token.clone(),
        },
    );
    let asked = Asked {
        session,
        grants,
        model,
        owner,
        task_sha256,
        job: Some(job_id.clone()),
    };
    let (id, task_state) = (job_id.clone(), state.clone());
    let work = tokio::spawn(async move {
        let state = task_state;
        // Held by this task until it ends, however it ends.
        let (_gate, _task_file) = (gate, task_file);
        let channel = crate::shim::ChildChannel {
            lines,
            input: child_input,
        };
        let child = crate::shim::run_argv_streaming(&argv, &cwd, timeout, child_environment(), channel);
        let overdue = async {
            token.cancelled().await;
            tokio::time::sleep(CANCEL_GRACE).await;
        };
        tokio::pin!(child, overdue);
        let result = loop {
            tokio::select! {
                result = &mut child => break Some(result),
                Some(line) = printed.recv() => state.browser_jobs.note_line(&id, &line),
                // Leaving this scope drops the child future, which kills its process tree.
                () = &mut overdue => break None,
            }
        };
        match result {
            None => {
                state.audit.log(json!({
                    "event": "browser.run",
                    "owner": asked.owner,
                    "origins": asked.grants,
                    "model": asked.model,
                    "task_sha256": asked.task_sha256,
                    "job": asked.job,
                    "exit": Value::Null,
                    "killed_after_cancel": true,
                }));
                Outcome::Killed
            }
            Some(result) => match deliver(&state, &asked, result) {
                Ok(body) => Outcome::Report(body),
                Err(err) => Outcome::Failed {
                    code: err.code,
                    message: err.message,
                },
            },
        }
    });
    // A supervising task records the end even if the job's own task panics.
    let (id, jobs) = (job_id.clone(), state.browser_jobs.clone());
    tokio::spawn(async move {
        let outcome = work.await.unwrap_or_else(|_| Outcome::Failed {
            code: "BROWSER_FAILED".into(),
            message: "the browser job ended unexpectedly".into(),
        });
        jobs.finish(&id, outcome);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job_id": job_id, "status": JOB_RUNNING, "takeover": takeover})),
    ))
}

fn checked_job_id(job_id: &str) -> ApiResult<()> {
    if crate::common::is_lower_hex(job_id, 32) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "INVALID_JOB_ID",
        "job_id must be 32 lowercase hex characters",
    ))
}

fn no_such_job() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "BROWSER_JOB_NOT_FOUND", "no such browser job")
}

/// `GET /api/browser/jobs/{job_id}`: one of the caller's jobs. A stored report
/// is rechecked against the block list on every read, as the synchronous
/// route does before it delivers; a blocked origin withholds the report.
pub async fn job_get(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> ApiResult<Json<Value>> {
    checked_job_id(&job_id)?;
    let owner = super::auth::context_owner(user);
    let mut job = state.browser_jobs.get(&owner, &job_id).ok_or_else(no_such_job)?;
    if job.get("result").is_some() {
        let origins: Vec<String> = serde_json::from_value(job["origins"].clone()).unwrap_or_default();
        let blocked = Session::general(&origins).map_or(true, |session| policy_now_blocks(&state, &session));
        if blocked {
            let changed = policy_changed();
            if let Some(fields) = job.as_object_mut() {
                fields.remove("result");
                fields.insert(
                    "error".into(),
                    json!({"code": changed.code, "message": changed.message}),
                );
            }
        }
    }
    Ok(Json(job))
}

/// `POST /api/browser/jobs/{job_id}/cancel`: ask the caller's job to stop. The
/// child stops before its next model turn and reports what it did; one that
/// has not stopped after `CANCEL_GRACE` is killed with its process tree.
pub async fn job_cancel(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> ApiResult<Json<Value>> {
    checked_job_id(&job_id)?;
    let owner = super::auth::context_owner(user);
    let job = state.browser_jobs.cancel(&owner, &job_id).ok_or_else(no_such_job)?;
    state.audit.log(json!({
        "event": "browser.job_cancel",
        "owner": owner,
        "job": job_id,
        "status": job["status"],
    }));
    Ok(Json(job))
}

/// The operator's yes to the one request waiting on their job. `request` and
/// `sha256` are the ones the job showed, so the yes cannot land on a request
/// the operator did not see. `confirm` and `reason` have no default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserApproveRequest {
    pub request: u64,
    pub sha256: String,
    pub confirm: bool,
    pub reason: String,
}

impl Validate for BrowserApproveRequest {
    fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if !crate::common::is_lower_hex(&self.sha256, 64) {
            bad.push("sha256".into());
        }
        if !self.confirm {
            bad.push("confirm".into());
        }
        if crate::common::is_visibly_blank(&self.reason)
            || !(1..=MAX_APPROVAL_REASON_CHARS).contains(&self.reason.trim().chars().count())
        {
            bad.push("reason".into());
        }
        bad
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserDenyRequest {
    pub request: u64,
}

impl Validate for BrowserDenyRequest {
    fn validate(&self) -> Vec<String> {
        Vec::new()
    }
}

const MAX_APPROVAL_REASON_CHARS: usize = 500;

fn verdict_refused(refusal: VerdictRefusal) -> ApiError {
    match refusal {
        VerdictRefusal::NoJob => no_such_job(),
        VerdictRefusal::NothingWaiting => ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_NOTHING_WAITING",
            "No request is waiting on this job: it was decided, its wait ran out, or the job ended.",
        ),
        VerdictRefusal::Stale => ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_APPROVAL_STALE",
            "That is not the request waiting now. Read the job again and decide on what it shows.",
        ),
    }
}

/// One `browser.approval` line: who, which job, and the request by method,
/// origin, size and hash. Never the path, the query or the body.
fn audit_approval(state: &AppState, owner: &str, job_id: &str, shown: &Value, decision: &str, reason: Option<&str>) {
    let origin = shown["url"]
        .as_str()
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.origin().ascii_serialization());
    state.audit.log(json!({
        "event": "browser.approval",
        "owner": owner,
        "job": job_id,
        "method": shown["method"],
        "origin": origin,
        "size": shown["size"],
        "sha256": shown["sha256"],
        "decision": decision,
        "reason": reason,
    }));
}

/// `POST /api/browser/jobs/{job_id}/approve`: let the one request waiting on
/// the caller's job be sent, once, as shown. Everything is checked again
/// now, after the wait: the feature and `/browse` are still on, the block
/// list still allows that URL and its origin, and that request is still the
/// one waiting. The sidecar's network gate then binds the pass to the
/// request's method, URL and body hash and asks the block list once more.
pub async fn job_approve(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    ValidJson(req): ValidJson<BrowserApproveRequest>,
) -> ApiResult<Json<Value>> {
    checked_job_id(&job_id)?;
    let cfg = state.cfg.clone();
    if !cfg.flag_is_true("browser.enabled") || !cfg.flag_is_true("auth.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_DISABLED",
            "Browser sessions are off, or accounts are; nothing was approved.",
        ));
    }
    if !cfg.flag_is_true("browser.takeover.enabled") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "BROWSER_TAKEOVER_DISABLED",
            "Approving a held request is off. Set browser.takeover.enabled: true in config.yaml and restart.",
        ));
    }
    let owner = super::auth::context_owner(user);
    let pending = state.browser_jobs.pending(&owner, &job_id).map_err(verdict_refused)?;
    // Reload policy after the wait and before the send (AGENTS.md).
    // The policy file is read on the blocking pool, never on an async worker.
    // The verdict below names the request again, so one that was decided or
    // replaced while the file was read is refused there.
    let url = pending["url"].as_str().unwrap_or("").to_string();
    let reader = state.clone();
    let blocked = !fixture_origins()
        && super::structured_memory::off_worker(move || {
            current_policy(&reader).map_or(true, |policy| {
                policy.blocks_request(&url)
                    || url::Url::parse(&url).map_or(true, |parsed| {
                        Session::general([parsed.origin().ascii_serialization()])
                            .map_or(true, |session| blocked_origin(&policy, &session).is_some())
                    })
            })
        })
        .await;
    if blocked {
        // The owner's no, so the child stops waiting and the request is held.
        if let Ok(shown) = state.browser_jobs.verdict(&owner, &job_id, (req.request, None), false) {
            audit_approval(&state, &owner, &job_id, &shown, "refused_by_policy", None);
        }
        return Err(policy_changed());
    }
    if pending["expires_at"]
        .as_f64()
        .is_none_or(|until| crate::common::now_ts() >= until)
    {
        return Err(verdict_refused(VerdictRefusal::NothingWaiting));
    }
    let shown = state
        .browser_jobs
        .verdict(&owner, &job_id, (req.request, Some(&req.sha256)), true)
        .map_err(verdict_refused)?;
    audit_approval(&state, &owner, &job_id, &shown, "approved", Some(req.reason.trim()));
    Ok(Json(
        json!({"job_id": job_id, "decision": "approved", "request": req.request}),
    ))
}

/// `POST /api/browser/jobs/{job_id}/deny`: the waiting request is held, as
/// every held request was before, and the task reports it.
pub async fn job_deny(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    ValidJson(req): ValidJson<BrowserDenyRequest>,
) -> ApiResult<Json<Value>> {
    checked_job_id(&job_id)?;
    let owner = super::auth::context_owner(user);
    let shown = state
        .browser_jobs
        .verdict(&owner, &job_id, (req.request, None), false)
        .map_err(verdict_refused)?;
    audit_approval(&state, &owner, &job_id, &shown, "denied", None);
    Ok(Json(
        json!({"job_id": job_id, "decision": "denied", "request": req.request}),
    ))
}

/// True only in a `test-support` build whose server was started with
/// `CGAH_TEST_PRIVATE_BROWSER_ORIGINS` set: a task may then name a loopback
/// origin, so the whole job path can be run against a local fixture page.
/// Release builds have no such switch.
fn fixture_origins() -> bool {
    cfg!(feature = "test-support") && std::env::var_os("CGAH_TEST_PRIVATE_BROWSER_ORIGINS").is_some()
}

/// The model tag the child is told to use: chat's `/model use` selection when
/// it is local, else the configured local model. `None` leaves the child on
/// `models.local_llm.model`. The child talks only to the configured primary
/// endpoint, so a server on its fallback backend holds no tag valid there.
fn task_model(backend_source: &str, model: String) -> Option<String> {
    (backend_source == "primary" && !model.trim().is_empty()).then_some(model)
}

/// Seconds the child gets of `browser.timeout_sec`: the rest is for closing
/// Chrome and printing the report before the runner stops the process tree.
fn child_deadline_sec(timeout_sec: u64) -> u64 {
    timeout_sec.saturating_sub(TEARDOWN_SEC).clamp(1, 1800)
}

/// The sidecar gets 5 s to exit after its pipe closes.
const TEARDOWN_SEC: u64 = 20;

fn write_task_file(task: &str) -> ApiResult<tempfile::NamedTempFile> {
    use std::io::Write;
    let failed = |_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "BROWSER_FAILED",
            "the task file could not be written",
        )
    };
    let mut file = tempfile::Builder::new()
        .prefix("cgah-browse-task-")
        .tempfile()
        .map_err(failed)?;
    file.write_all(task.as_bytes()).map_err(failed)?;
    file.flush().map_err(failed)?;
    Ok(file)
}

fn current_policy(state: &AppState) -> ApiResult<crate::server::web_policy::Policy> {
    state.web_snapshot().policy().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "WEB_POLICY_UNAVAILABLE",
            "The shared URL policy file is missing or unreadable, so browser tasks are refused. \
             An administrator's /web allow creates a missing one; an unreadable file is never reset.",
        )
    })
}

/// What holds the model gate, in the operator's words. The gate's labels are
/// internal, so one this list does not know is described, not printed.
fn busy_reason(holder: &str) -> &'static str {
    use crate::server::{model_limits::TUNE_OWNER, structured_memory_suggest::GATE_OWNER};
    match holder {
        TUNE_OWNER => "its speed is being measured after /model use, which takes a few seconds",
        "chat" => "a chat reply is being written",
        "browser" => "another browser task is running",
        "agent" => "a coding run is using it",
        "consolidation" | GATE_OWNER => "memory is being updated",
        _ => "another model operation is running",
    }
}

/// True for a loopback, private or other non-public IP-literal origin.
fn non_public_literal(origin: &str) -> bool {
    match url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host().map(|host| host.to_owned()))
    {
        Some(url::Host::Ipv4(ip)) => !crate::server::web_policy::is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => !crate::server::web_policy::is_public_ip(ip.into()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #894: the busy refusal names what holds the model, in fixed words.
    #[test]
    fn a_busy_refusal_names_the_holder_without_printing_its_label() {
        let tune = busy_reason(crate::server::model_limits::TUNE_OWNER);
        assert!(tune.contains("/model use"), "{tune}");
        assert_eq!(busy_reason("browser"), "another browser task is running");
        for holder in ["attachment_unpin", "", "anything-new"] {
            assert_eq!(busy_reason(holder), "another model operation is running");
        }
    }

    #[test]
    fn only_a_primary_backend_names_the_task_model() {
        assert_eq!(
            task_model("primary", "gemma4:26b".into()).as_deref(),
            Some("gemma4:26b")
        );
        assert_eq!(task_model("fallback", "lmstudio-tag".into()), None);
        assert_eq!(task_model("primary", " ".into()), None);
    }

    #[test]
    fn the_child_deadline_stays_inside_the_task_timeout() {
        assert_eq!(child_deadline_sec(300), 280);
        assert_eq!(child_deadline_sec(30), 10);
        assert_eq!(child_deadline_sec(1800), 1780);
        assert_eq!(child_deadline_sec(5), 1);
    }

    #[test]
    fn literal_origins_must_be_public() {
        for origin in [
            "https://127.0.0.1:8080",
            "https://10.0.0.5:443",
            "https://[::1]:443",
            "https://192.168.1.1:443",
        ] {
            assert!(non_public_literal(origin), "{origin}");
        }
        for origin in [
            "https://example.com:443",
            "https://8.8.8.8:443",
            "https://localhost:443",
        ] {
            assert!(!non_public_literal(origin), "{origin}");
        }
    }
}
