//! `/api/github/status` and the `/api/agent/*` routes: check profiles, runs,
//! detached jobs and schedules. Port of `harness/agent_routes.py`. Pipeline
//! work crosses the shim; a non-zero CLI exit is a successful shim call
//! (HTTP 200 with `ok=false`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::common::authz::Capability;
use crate::common::tool_broker::assert_allowed;
use crate::server::agent_policy::{available_profiles, resolve_check_profiles, run_id_re};
use crate::server::errors::{session_status, ApiError, ApiResult};
use crate::server::schemas::*;
use crate::server::state::{AppState, AGENT_RUN_TOOL};
use crate::shim::{self, OpsRequest, ShimError, REAL_REPO_RUN_MAX_TIMEOUT_SEC};

const MAX_ECHOED_RUN_ID_LEN: usize = 64;

fn validated_run_id(run_id: &str) -> ApiResult<String> {
    if !run_id_re().is_match(run_id) {
        return Err(
            ApiError::bad_request("INVALID_RUN_ID", "run_id must be 32 lowercase hex characters")
                .details(json!({"run_id": crate::common::clip_chars(run_id, MAX_ECHOED_RUN_ID_LEN)})),
        );
    }
    Ok(run_id.to_string())
}

/// One mapping for every shim failure: a spawn or temp-file failure is the
/// shim's, not the request's (502); a budget overrun is a typed 504.
fn shim_error(state: &AppState, action: &str, e: ShimError) -> ApiError {
    match e {
        ShimError::Ops(msg) => ApiError::bad_request("AGENTIC_ERROR", state.audit.redact(&msg)),
        ShimError::Timeout { action, timeout_sec } => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "AGENTIC_TIMEOUT",
            format!("agentic {action} exceeded its {timeout_sec}s budget"),
        )
        .details(json!({"action": action, "timeout_sec": timeout_sec})),
        ShimError::Io(msg) => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "SHIM_IO_ERROR",
            format!("{action} shim failed: {}", state.audit.redact(&msg)),
        ),
    }
}

async fn agentic_call(state: &AppState, req: OpsRequest) -> ApiResult<Json<Value>> {
    agentic_call_with(state, req, None).await
}

/// `prepare` reshapes the child's result before the envelope redaction. It may
/// hash (review tags), so it runs on the blocking pool, never the async worker.
type Prepare = fn(&mut shim::OpsResult, &crate::common::audit::Redactors);

async fn agentic_call_with(state: &AppState, req: OpsRequest, prepare: Option<Prepare>) -> ApiResult<Json<Value>> {
    let action = req.action.clone();
    // The child appends to the audit file directly. Land the server's queued
    // lines first, such as the broker decision that authorized this run.
    state.audit.flush_async().await;
    let result = shim::run_agentic_op(&state.shim, &req)
        .await
        .map_err(|e| shim_error(state, &action, e))?;
    if result.is_disabled_layer_success() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "AGENTIC_DISABLED",
            "agentic layer is disabled; nothing was executed",
        )
        .details(json!({"action": action})));
    }
    let result = match prepare {
        None => result,
        Some(prepare) => {
            let redactors = state.audit.redactors().clone();
            super::structured_memory::off_worker(move || {
                let mut result = result;
                prepare(&mut result, &redactors);
                result
            })
            .await
        }
    };
    Ok(Json(result.to_json(state.audit.redactors())))
}

/// Per-process key for review-diff redaction tags. It never leaves memory, so
/// a tag reveals nothing, and it is fixed for the server's life so repeated
/// status reads of one run show the same tags.
fn review_tag_key() -> &'static [u8; 32] {
    static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    KEY.get_or_init(rand::random)
}

/// The review diff is redacted like every response, but each redacted span is
/// tagged by value, so a change inside one (`127.0.0.1` to `0.0.0.0`) shows as
/// a changed tag instead of an identical line. `diff_redacted_spans` tells the
/// console to say so and where the unredacted clone is on this machine.
fn tag_review_diff(result: &mut shim::OpsResult, redactors: &crate::common::audit::Redactors) {
    let Some(parsed) = result.parsed.as_mut() else { return };
    let Some(diff) = parsed.get("diff").and_then(Value::as_str) else {
        return;
    };
    let (tagged, spans) = redactors.redact_tagged(diff, review_tag_key());
    parsed["diff"] = json!(tagged);
    parsed["diff_redacted_spans"] = json!(spans);
}

pub async fn github_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    state.audit.flush_async().await;
    let result = shim::run_agentic_op(&state.shim, &OpsRequest::new("status"))
        .await
        .map_err(|e| shim_error(&state, "status", e))?;
    Ok(Json(result.to_json(state.audit.redactors())))
}

/// Open: a static allow-list listing, spawns nothing.
pub async fn agent_checks(State(state): State<Arc<AppState>>) -> Json<Value> {
    let profiles: Vec<Value> = available_profiles()
        .into_iter()
        .map(|(n, d)| json!({"name": n, "description": d}))
        .collect();
    Json(json!({
        "profiles": profiles,
        "default_profile": crate::server::agent_policy::DEFAULT_CHECK_PROFILE,
        "poll_interval_ms": state.cfg.limit("app.agent_job_poll_ms"),
        "planner_model": state.cfg.str_or("agentic.deepagent_github.model", ""),
        // Read-only: lets the console refuse a branch at /agent run instead of at confirm.
        "branch_prefixes": crate::common::identity::identity().map(|i| i.allowed_prefixes.clone()).unwrap_or_default(),
        "capabilities": {"jobs": true, "streaming": false, "job_recovery": "durable_interrupted", "descendant_stop_guaranteed": false},
    }))
}

/// The planner timeout an agent run budgets and passes to its child: the one
/// `auto_tune` derived when the planner runs the tuned chat model on the very
/// endpoint chat measured, else the configured value. Stricter than the shared
/// generation-gate check: IPv4 and IPv6 loopback can be different listeners,
/// and a timeout measured on one must not cut off a slower planner on the other.
pub(crate) fn planner_timeout_sec(state: &AppState) -> Option<u64> {
    let planner_model = state.cfg.str_or("agentic.deepagent_github.model", "");
    // Sparse configs must resolve to the same endpoint as the planner child.
    let planner_url = state
        .cfg
        .str_or("agentic.deepagent_github.base_url", "http://localhost:11434/v1");
    if !same_endpoint(&planner_url, &state.backend.base_url) {
        return None;
    }
    state
        .tuning_for(&state.current_model())
        .filter(|tuning| crate::llm::profile::same_model(&tuning.model, &planner_model))
        .and_then(|tuning| tuning.planner_timeout_sec)
}

/// Two base URLs name one endpoint: parsed, so only the scheme and host are
/// case-folded and a path keeps its case (two reverse-proxy routes that differ
/// in case stay different). Unparseable URLs never match.
fn same_endpoint(a: &str, b: &str) -> bool {
    let parse = |raw: &str| url::Url::parse(raw.trim().trim_end_matches('/')).ok();
    matches!((parse(a), parse(b)), (Some(a), Some(b)) if a == b)
}

/// True when the run's planner and /api/chat target the same backend. An
/// unset or unparseable planner `base_url` counts as shared.
fn agent_run_shares_chat_backend(state: &AppState) -> bool {
    let deep = state
        .cfg
        .str_or("agentic.deepagent_github.base_url", "")
        .trim()
        .to_string();
    if deep.is_empty() {
        return true;
    }
    match (
        canonical_backend_key(&state.backend.base_url),
        canonical_backend_key(&deep),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

fn canonical_backend_key(raw: &str) -> Option<(String, String, Option<u16>, String)> {
    let u = url::Url::parse(raw).ok()?;
    let mut host = u.host_str().unwrap_or("").to_lowercase();
    if crate::llm::backend::LOOPBACK_HOSTS.contains(&host.as_str()) {
        host = "127.0.0.1".into();
    }
    Some((
        u.scheme().to_string(),
        host,
        u.port_or_known_default(),
        u.path().trim_end_matches('/').to_string(),
    ))
}

/// Everything `/api/agent/run` decides BEFORE a child exists: profile names ->
/// fixed argv, the wall-clock budget check, the tool-broker allowlist, and the
/// two gates. Shared verbatim by the synchronous route and the detached job
/// route so the browser can never reach the shim by a weaker path.
struct PreparedRun {
    ops: OpsRequest,
    _run_gate: crate::server::generation_gate::GateGuard,
    _chat_gate: Option<crate::server::generation_gate::GateGuard>,
}

/// Resolve the check profiles and size the run budget: the shape checks every
/// entry point shares. `/api/agent/run` and `/api/agent/jobs` reach it through
/// `prepare_run_inner`; schedule preview and create call it before storing a
/// request, so a shape the run route refuses is refused at creation rather than
/// at every occurrence. Returns the check manifest and the tuned planner timeout.
fn resolve_run_shape(state: &AppState, req: &AgentRunRequest) -> ApiResult<(Vec<Value>, Option<u64>)> {
    let requested: Vec<String> = req
        .checks
        .clone()
        .unwrap_or_else(|| vec![crate::server::agent_policy::DEFAULT_CHECK_PROFILE.to_string()]);
    let checks = resolve_check_profiles(&requested).map_err(|e| {
        ApiError::bad_request("UNKNOWN_CHECK_PROFILE", e.message).details(json!({"requested": requested}))
    })?;
    // A cloud planner uses the configured `planner_timeout_sec` as its HTTP
    // bound, so a speed-derived local timeout would mis-size its budget.
    let tuned_planner = if req.provider.is_some() {
        None
    } else {
        planner_timeout_sec(state)
    };
    let planner = tuned_planner.unwrap_or(state.shim.planner_timeout_sec);
    let estimated = shim::real_repo_run_shape_budget_sec(&state.shim, planner, req.max_iterations, checks.len());
    if estimated > REAL_REPO_RUN_MAX_TIMEOUT_SEC {
        let max_fitting = (1..=MAX_ITERATIONS_CEILING as i64)
            .filter(|n| {
                shim::real_repo_run_shape_budget_sec(&state.shim, planner, Some(*n), checks.len())
                    <= REAL_REPO_RUN_MAX_TIMEOUT_SEC
            })
            .max()
            .unwrap_or(0);
        let remedy = if max_fitting > 0 {
            format!(
                "at {} check profile(s) the most that fits is max_iterations={max_fitting}",
                checks.len()
            )
        } else {
            format!(
                "no iteration count fits with {} check profile(s) -- select fewer checks",
                checks.len()
            )
        };
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "AGENTIC_BUDGET_EXCEEDED",
            format!(
                "requested shape budgets ~{estimated}s but the synchronous run cap is {REAL_REPO_RUN_MAX_TIMEOUT_SEC}s, so the run would be killed mid-flight; {remedy}"
            ),
        )
        .details(json!({
            "estimated_sec": estimated,
            "cap_sec": REAL_REPO_RUN_MAX_TIMEOUT_SEC,
            "max_iterations": req.max_iterations,
            "check_count": checks.len(),
            "max_iterations_that_fit": max_fitting,
        })));
    }
    Ok((checks, tuned_planner))
}

fn prepare_run(state: &AppState, req: &AgentRunRequest, owner: &str) -> ApiResult<PreparedRun> {
    prepare_run_inner(state, req, owner, false)
}

fn prepare_run_inner(state: &AppState, req: &AgentRunRequest, owner: &str, recurring: bool) -> ApiResult<PreparedRun> {
    if recurring {
        super::goals::validate_binding_recurring(state, req, owner)?;
    } else {
        super::goals::validate_binding(state, req, owner)?;
    }
    if recurring {
        refuse_scheduled_cloud_planner(req)?;
    }
    let (checks, tuned_planner) = resolve_run_shape(state, req)?;
    assert_allowed(
        AGENT_RUN_TOOL,
        &["real-repo-run".into()],
        &state.agent_run_tool_allowlist(),
        &state.audit,
    )
    .map_err(|e| ApiError::from_err(StatusCode::FORBIDDEN, &e))?;
    let Some(_run_gate) = state.agent_run_gate.claim("agent") else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "AGENT_RUN_BUSY",
            "another agent run is already in progress",
        ));
    };
    let _chat_gate = if agent_run_shares_chat_backend(state) {
        match state.generation_gate.claim("agent") {
            Some(g) => Some(g),
            None => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "AGENT_RUN_BUSY",
                    "a local model chat turn is already running",
                ))
            }
        }
    } else {
        None
    };
    let ops = OpsRequest {
        action: "real-repo-run".into(),
        instruction: Some(req.instruction.clone()),
        checks: Some(checks),
        branch: Some(req.branch.clone()),
        commit_message: Some(req.commit_message.clone()),
        reason: Some(req.reason.clone()),
        confirm: req.confirm,
        max_iterations: req.max_iterations,
        plan: req.plan.clone(),
        read_files: req.canonical_read_files(),
        pr: req.pr,
        issue: req.issue,
        planner_timeout_sec: tuned_planner,
        provider: req.provider.map(|p| p.as_str().to_string()),
        confirm_online: req.confirm_online,
        planner_identity: Some(
            state
                .cfg
                .planner_identity()
                .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?,
        ),
        ..Default::default()
    };
    Ok(PreparedRun {
        ops,
        _run_gate,
        _chat_gate,
    })
}

/// A schedule fires with nobody present, so it cannot carry the per-run consent
/// a cloud planner's repository egress needs.
fn refuse_scheduled_cloud_planner(req: &AgentRunRequest) -> ApiResult<()> {
    if req.provider.is_some() || req.confirm_online {
        return Err(ApiError::bad_request(
            "SCHEDULED_CLOUD_PLANNER",
            "a scheduled run uses the local planner; a cloud planner needs a run started from the console",
        ));
    }
    Ok(())
}

fn refuse_bound_session_pins(state: &AppState, owner: &str, req: &AgentRunRequest) -> ApiResult<()> {
    // A staged /diag excerpt is local-chat-only context: refuse rather than drop it.
    state
        .diag
        .for_turn(owner, "", crate::server::diag_reader::Surface::Agent)
        .map_err(|e| super::diag::diag_error(&e))?;
    refuse_session_pins(state, owner, req)
}

/// The persistent half of `refuse_bound_session_pins`: a goal session that still
/// pins a live attachment. Schedule preview and create apply it too, because
/// every firing would refuse the same session; the staged `/diag` excerpt is
/// transient, so only firings check it.
fn refuse_session_pins(state: &AppState, owner: &str, req: &AgentRunRequest) -> ApiResult<()> {
    let Some(binding) = &req.goal_stage else {
        return Ok(());
    };
    let session = state
        .store
        .for_owner(owner)
        .get(&binding.session_id)
        .map_err(|e| ApiError::from_err(session_status(&e), &e))?;
    let live_blobs = state.attachments.owned_blobs(owner, &session.pinned_ids_for(owner));
    let live_pins: Vec<String> = session
        .pinned_ids_for(owner)
        .into_iter()
        .filter(|id| live_blobs.contains_key(id))
        .collect();
    crate::server::retrieval::refuse_forbidden_attachments(
        crate::server::retrieval::RetrievalSurface::Agent,
        &[],
        &live_pins,
    )
    .map_err(|e| crate::server::attachments::upload_error(&e))
}

/// Synchronous: blocks until the child finishes (the verbatim console expects this).
pub async fn agent_run(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<AgentRunRequest>,
) -> ApiResult<Json<Value>> {
    if req.goal_stage.is_some() {
        return Err(ApiError::bad_request(
            "GOAL_REQUIRES_JOB",
            "Goal-linked work uses the durable /api/agent/jobs workflow",
        ));
    }
    let account = user.as_ref().map(|user| user.0.clone());
    let owner = super::auth::context_owner(user);
    // The revocation count as the run is asked, read before any wait, as
    // `/browse` reads it: a count read later would already include a reset.
    let account = asking_account(&state, account).await;
    refuse_bound_session_pins(&state, &owner, &req)?;
    let prepared = prepare_run(&state, &req, &owner)?;
    // The run lasts as long as the child, and the account was checked only
    // when the request arrived. A disable, deletion, loss of `RunCoding` or
    // revoked sign-in (password reset or change) while it runs drops the
    // child (`kill_on_drop`) and the gates; one that lands after the child
    // exits withholds its result.
    let outcome = tokio::select! {
        outcome = agentic_call(&state, prepared.ops) => outcome,
        () = coding_authority_lost(&state, &owner, &account) => return Err(run_withheld(&state, &owner, "stopped")),
    };
    if !owner_still_may_run(&state, &owner, &account).await {
        return Err(run_withheld(&state, &owner, "withheld"));
    }
    let Json(mut result) = outcome?;
    // The receipt stages an episode in the memory store: off the async worker.
    let result = super::structured_memory::off_worker(move || {
        result["memory_suggestion"] =
            crate::server::structured_memory_suggest::coding_completed(&state, &owner, &req.instruction, &result);
        result
    })
    .await;
    Ok(Json(result))
}

/// How often a synchronous run rechecks its account while its child runs.
const RUN_ACCOUNT_RECHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// The signed-in account that asked for a synchronous run and its
/// `sign_in_revocations` count then; `None` without accounts.
type AskingAccount = Option<(crate::common::auth_store::UserSummary, u64)>;

/// Reads the asking account's revocation count on the blocking pool (the
/// account store's lock is held across its SQLite rewrite).
async fn asking_account(
    state: &Arc<AppState>,
    account: Option<crate::common::auth_store::UserSummary>,
) -> AskingAccount {
    let account = account?;
    let state = state.clone();
    super::structured_memory::off_worker(move || {
        let auth = state.auth.as_ref()?;
        let seen = auth.sign_in_revocations(&account.username);
        Some((account, seen))
    })
    .await
}

/// Whether `owner` may still run coding and its sign-ins have not been
/// revoked since it asked, read on the blocking pool. Always true without
/// accounts: the one local user cannot be disabled.
async fn owner_still_may_run(state: &Arc<AppState>, owner: &str, account: &AskingAccount) -> bool {
    if state.auth.is_none() {
        return true;
    }
    let (state, owner, account) = (state.clone(), owner.to_string(), account.clone());
    super::structured_memory::off_worker(move || {
        schedule_owner_authorized(&state, &owner)
            && match (state.auth.as_ref(), &account) {
                (Some(auth), Some((user, seen))) => auth.account_unchanged(user, Some(*seen)),
                _ => true,
            }
    })
    .await
}

/// Resolves once `owner` may no longer run coding or its sign-ins are
/// revoked, checked every `RUN_ACCOUNT_RECHECK`; never resolves without
/// accounts.
async fn coding_authority_lost(state: &Arc<AppState>, owner: &str, account: &AskingAccount) {
    if state.auth.is_none() {
        return std::future::pending().await;
    }
    loop {
        tokio::time::sleep(RUN_ACCOUNT_RECHECK).await;
        if !owner_still_may_run(state, owner, account).await {
            return;
        }
    }
}

/// The refusal for a synchronous run whose account changed: `stopped` while
/// the child ran, `withheld` after it exited. Audited; the result never leaves.
fn run_withheld(state: &AppState, owner: &str, outcome: &str) -> ApiError {
    state.audit.log(json!({
        "event": "agent_run_withheld",
        "owner": owner,
        "outcome": outcome,
        "reason": "account_changed",
    }));
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
        "The account was disabled, deleted, lost coding access or had its sign-ins revoked during this run; its result was not delivered.",
    )
}

// ---------------------------------------------------------------- detached jobs

fn validated_job_id(job_id: &str) -> ApiResult<String> {
    if !run_id_re().is_match(job_id) {
        return Err(
            ApiError::bad_request("INVALID_JOB_ID", "job_id must be 32 lowercase hex characters")
                .details(json!({"job_id": crate::common::clip_chars(job_id, MAX_ECHOED_RUN_ID_LEN)})),
        );
    }
    Ok(job_id.to_string())
}

/// Same validation and gates as `/api/agent/run`, then returns 202 with a job
/// id while the child runs in a detached task that owns the gates.
pub async fn agent_job_create(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::common::auth_store::UserSummary>>,
    ValidJson(req): ValidJson<AgentRunRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let owner = super::auth::context_owner(user);
    // `start_job` reads the session, claims the run gates, persists the job
    // store and claims the goal stage. All of it runs, unchanged and in order,
    // in one blocking-pool closure: splitting it would put an await between
    // job registration and the stage claim, where a dropped request could
    // strand a registered job whose task never starts.
    super::structured_memory::off_worker(move || start_job(state, req, owner, None)).await
}

pub(crate) fn start_job(
    state: Arc<AppState>,
    req: AgentRunRequest,
    owner: String,
    schedule_id: Option<String>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // Refuse before the broker decision and gate claims, like agent_run.
    refuse_bound_session_pins(&state, &owner, &req)?;
    let prepared = prepare_run_inner(&state, &req, &owner, schedule_id.is_some())?;
    let job_id = crate::common::random_hex(16);
    let action = prepared.ops.action.clone();
    let task_state = state.clone();
    let task_job = job_id.clone();
    let instruction = req.instruction.clone();
    let task_schedule = schedule_id.clone();
    let task_owner = owner.clone();
    let (registered, ready) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        if ready.await.is_err() {
            return;
        }
        // The account was checked when the request arrived. An account
        // disabled, deleted or demoted out of `RunCoding` before this job was
        // registered missed `JobStore::cancel_owner`, so it is checked again
        // now that the job is in the store: any later change finds it there.
        let (check_state, check_owner) = (task_state.clone(), task_owner.clone());
        let still_allowed =
            super::structured_memory::off_worker(move || schedule_owner_authorized(&check_state, &check_owner)).await;
        // Both gate guards are released before the job reads as settled: a
        // caller that sees it settle may start the next run at once.
        let PreparedRun {
            ops,
            _run_gate: run_gate,
            _chat_gate: chat_gate,
        } = prepared;
        if !still_allowed {
            drop((run_gate, chat_gate));
            let (refuse_state, refuse_owner, refuse_job) = (task_state.clone(), task_owner.clone(), task_job.clone());
            super::structured_memory::off_worker(move || {
                refuse_state.audit.log(json!({
                    "event": "agent_job_refused",
                    "job_id": refuse_job,
                    "owner": refuse_owner,
                    "reason": "account_changed",
                }));
                refuse_state.jobs.finish(
                    &refuse_job,
                    Err(ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "AUTH_REQUIRED",
                        "The account was disabled, deleted or lost coding access before this job started; nothing ran.",
                    )),
                );
            })
            .await;
            return;
        }
        let outcome = agentic_call(&task_state, ops).await.map(|Json(v)| v);
        drop((run_gate, chat_gate));
        // Saving the outcome (fsync) and enqueueing its notification block. A
        // cancel that wins the job lock first still makes this finish a no-op.
        let (finish_state, finish_job) = (task_state.clone(), task_job.clone());
        super::structured_memory::off_worker(move || finish_state.jobs.finish(&finish_job, outcome)).await;
        // The receipt stages an episode in the memory store: off the async worker.
        // `finish` released this task's handle, so no cancel can abort it here.
        let (receipt_state, receipt_job, receipt_owner) = (task_state.clone(), task_job.clone(), task_owner.clone());
        // The same closure reads the job back: the job lock is held across the
        // job file's fsync by its writers, so no read of it runs on an async worker.
        let finished = super::structured_memory::off_worker(move || {
            receipt_state.jobs.annotate_completion(&receipt_job, |result| {
                result["memory_suggestion"] = crate::server::structured_memory_suggest::coding_completed(
                    &receipt_state,
                    &receipt_owner,
                    &instruction,
                    result,
                );
            });
            receipt_state.jobs.get(&receipt_owner, &receipt_job)
        })
        .await;
        if let Some(job) = finished {
            if let Some(sid) = task_schedule {
                let event = if job["status"] == crate::server::agent_jobs::FINISHED {
                    "agent_schedule_completed"
                } else {
                    "agent_schedule_failed"
                };
                task_state.audit.log(json!({
                    "event": event,
                    "schedule_id": sid,
                    "job_id": task_job,
                    "status": job["status"],
                }));
            }
        }
    });
    state
        .jobs
        .try_insert_running(&owner, &job_id, &action, handle)
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "JOB_PERSIST_FAILED",
                "Cannot durably register work; no worker was started",
            )
        })?;
    if let Some(binding) = &req.goal_stage {
        let claimed = if schedule_id.is_some() {
            state
                .store
                .for_owner(&owner)
                .rebind_goal_stage(&binding.session_id, &binding.stage_id, &req, &job_id)
        } else {
            state
                .store
                .for_owner(&owner)
                .claim_goal_stage(&binding.session_id, &binding.stage_id, &req, &job_id)
        };
        // Refused before it started: the job leaves no record and sends no
        // completion notification (a cancel would report work that never ran).
        if let Err(error) = claimed {
            state.jobs.discard_unstarted(&owner, &job_id);
            return Err(ApiError::from_err(StatusCode::CONFLICT, &error));
        }
    }
    let _ = registered.send(());
    state
        .audit
        .log(json!({"event": "agent_job_started", "job_id": job_id, "action": action, "schedule_id": schedule_id}));
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job_id": job_id, "action": action, "status": crate::server::agent_jobs::RUNNING})),
    ))
}

/// What a firing would refuse about the request itself, checked before a
/// schedule is previewed or stored. The binding and pin checks read the session
/// file, so the whole check runs on the blocking pool.
async fn refuse_unschedulable(state: &Arc<AppState>, req: &AgentRunRequest, owner: &str) -> ApiResult<()> {
    let (state, req, owner) = (state.clone(), req.clone(), owner.to_string());
    refuse_scheduled_cloud_planner(&req)?;
    super::structured_memory::off_worker(move || {
        super::goals::validate_binding(&state, &req, &owner)?;
        refuse_session_pins(&state, &owner, &req)?;
        resolve_run_shape(&state, &req).map(|_| ())
    })
    .await
}

pub async fn schedule_preview(
    State(state): State<Arc<AppState>>,
    user: Caller,
    ValidJson(body): ValidJson<AgentScheduleCreate>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    refuse_unschedulable(&state, &body.request, &owner).await?;
    let spec = body
        .spec()
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    let request = serde_json::to_value(&body.request)
        .map_err(|_| ApiError::bad_request("INVALID_SCHEDULE_REQUEST", "request could not be previewed"))?;
    // Activation holds the preview lock across the schedule file's fsync.
    let result = super::structured_memory::off_worker(move || {
        state.schedules.preview(spec, request, &owner, crate::common::now_ts())
    })
    .await
    .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(result))
}

pub async fn schedule_create(
    State(state): State<Arc<AppState>>,
    user: Caller,
    ValidJson(body): ValidJson<AgentScheduleCreate>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let owner = super::auth::context_owner(user);
    refuse_unschedulable(&state, &body.request, &owner).await?;
    let spec = body
        .spec()
        .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    let request = serde_json::to_value(&body.request)
        .map_err(|_| ApiError::bad_request("INVALID_SCHEDULE_REQUEST", "request could not be stored"))?;
    // Activation persists the schedule store (fsync) under its mutex.
    let (activate_state, preview_id) = (state.clone(), body.preview_id.unwrap_or_default());
    let created = super::structured_memory::off_worker(move || {
        activate_state
            .schedules
            .activate(&preview_id, spec, request, &owner, crate::common::now_ts())
    })
    .await
    .map_err(|e| ApiError::from_err(StatusCode::BAD_REQUEST, &e))?;
    state
        .audit
        .log(json!({"event":"agent_schedule_created","schedule_id":created.schedule_id,"schema_version":1}));
    Ok((StatusCode::CREATED, Json(created.to_json())))
}

pub async fn schedules_list(State(state): State<Arc<AppState>>, user: Caller) -> Json<Value> {
    let owner = super::auth::context_owner(user);
    // Writers hold the schedule lock across the schedule file's fsync, so a
    // read waits for it on the blocking pool, as every read below does.
    let schedules = super::structured_memory::off_worker(move || state.schedules.list(&owner)).await;
    Json(json!({
        "schedules": schedules.into_iter().map(|s| s.to_json()).collect::<Vec<_>>(),
    }))
}

fn validated_schedule_id(schedule_id: &str) -> ApiResult<String> {
    if !run_id_re().is_match(schedule_id) {
        return Err(
            ApiError::bad_request("INVALID_SCHEDULE_ID", "schedule_id must be 32 lowercase hex characters")
                .details(json!({"schedule_id": crate::common::clip_chars(schedule_id, MAX_ECHOED_RUN_ID_LEN)})),
        );
    }
    Ok(schedule_id.to_string())
}

pub async fn schedule_get(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(schedule_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let id = validated_schedule_id(&schedule_id)?;
    super::structured_memory::off_worker(move || state.schedules.get(&owner, &id))
        .await
        .map(|s| Json(s.to_json()))
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "SCHEDULE_NOT_FOUND", "no such schedule"))
}

pub async fn schedule_cancel(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(schedule_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let id = validated_schedule_id(&schedule_id)?;
    // Saving the cancellation (fsync) blocks, as for jobs.
    let (cancel_state, cancel_id) = (state.clone(), id.clone());
    let v = super::structured_memory::off_worker(move || cancel_state.schedules.cancel(&owner, &cancel_id))
        .await
        .map_err(|e| ApiError::from_err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "SCHEDULE_NOT_FOUND", "no such schedule"))?;
    state
        .audit
        .log(json!({"event": "agent_schedule_cancelled", "schedule_id": id}));
    Ok(Json(v.to_json()))
}

/// Test seam: `start_job` for `owner` without the HTTP gate, standing in for
/// a request that passed the gate before its account changed.
#[cfg(any(test, feature = "test-support"))]
pub fn start_job_for_owner(
    state: Arc<AppState>,
    req: AgentRunRequest,
    owner: String,
) -> ApiResult<(StatusCode, Json<Value>)> {
    start_job(state, req, owner, None)
}

/// Whether `owner` may run coding now: an enabled account holding
/// `RunCoding` (or `local` without accounts). Rechecked at every schedule
/// occurrence, including legacy rows, and by every job once it is registered.
fn schedule_owner_authorized(state: &AppState, owner: &str) -> bool {
    match &state.auth {
        Some(auth) => auth.owner_permits(owner, Capability::RunCoding),
        None => owner == "local",
    }
}

/// Evaluate due schedules with a caller-supplied clock (tests inject `now`).
/// Each occurrence persists the schedule store and runs `start_job` (session
/// reads, job-store and goal-stage writes), so the whole tick runs on the
/// blocking pool, as `agent_job_create` runs `start_job`.
pub async fn tick_schedules(state: &Arc<AppState>, now: f64) {
    let state = state.clone();
    super::structured_memory::off_worker(move || tick_schedules_blocking(&state, now)).await
}

fn tick_schedules_blocking(state: &Arc<AppState>, now: f64) {
    use crate::server::agent_schedules::{parse_request, DispatchOutcome};
    for due in state.schedules.due(now) {
        let owner = due.owner.clone();
        let schedule_id = due.schedule_id.clone();
        let occurrence_id = due.next_occurrence_id.clone();
        let request = due.request.clone();
        state.schedules.consume_and_dispatch(
            &due,
            now,
            || {
                if !schedule_owner_authorized(state, &owner) {
                    state.audit.log(json!({"event":"agent_schedule_refused","schedule_id":schedule_id,"code":"SCHEDULE_OWNER_REVOKED"}));
                    return DispatchOutcome::Revoked;
                }
                if let Err(error) = parse_request(&request) {
                    state.audit.log(json!({"event":"agent_schedule_failed","schedule_id":schedule_id,"code":error.code}));
                    return DispatchOutcome::Refused;
                }
                DispatchOutcome::Accepted
            },
            || {
                if !schedule_owner_authorized(state, &owner) {
                    state.audit.log(json!({"event":"agent_schedule_refused","schedule_id":schedule_id,"code":"SCHEDULE_OWNER_REVOKED"}));
                    return DispatchOutcome::Revoked;
                }
                let req = match parse_request(&request) {
                    Ok(req) => req,
                    Err(error) => {
                        state.audit.log(json!({"event":"agent_schedule_failed","schedule_id":schedule_id,"code":error.code}));
                        return DispatchOutcome::Refused;
                    }
                };
                state.audit.log(json!({"event":"agent_schedule_start","schedule_id":schedule_id,"occurrence_id":occurrence_id}));
                match start_job(state.clone(), req, owner.clone(), Some(schedule_id.clone())) {
                    Ok(_) => DispatchOutcome::Accepted,
                    Err(error) => {
                        state.audit.log(json!({"event":"agent_schedule_failed","schedule_id":schedule_id,"code":error.body()["detail"]["code"]}));
                        DispatchOutcome::Refused
                    }
                }
            },
        );
    }
}

pub async fn agent_job_get(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(job_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let id = validated_job_id(&job_id)?;
    super::structured_memory::off_worker(move || state.jobs.get(&owner, &id))
        .await
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "JOB_NOT_FOUND", "no such job"))
}

pub async fn agent_jobs_list(State(state): State<Arc<AppState>>, user: Caller) -> Json<Value> {
    let owner = super::auth::context_owner(user);
    let jobs = super::structured_memory::off_worker(move || state.jobs.list(&owner)).await;
    let running = jobs
        .iter()
        .filter(|j| j["status"] == crate::server::agent_jobs::RUNNING)
        .count();
    Json(json!({"jobs": jobs, "running": running}))
}

pub async fn agent_job_cancel(
    State(state): State<Arc<AppState>>,
    user: Caller,
    Path(job_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner = super::auth::context_owner(user);
    let id = validated_job_id(&job_id)?;
    // Saving the cancellation (fsync) and enqueueing its notification block.
    let (cancel_state, cancel_id) = (state.clone(), id.clone());
    let v = super::structured_memory::off_worker(move || cancel_state.jobs.cancel(&owner, &cancel_id))
        .await
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "JOB_NOT_FOUND", "no such job"))?;
    state.audit.log(json!({"event": "agent_job_cancelled", "job_id": id}));
    Ok(Json(v))
}

pub async fn agent_runs(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    agentic_call(&state, OpsRequest::new("real-repo-runs")).await
}

pub async fn agent_run_status(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-status");
    ops.run_id = Some(checked);
    agentic_call_with(&state, ops, Some(tag_review_diff)).await
}

pub async fn agent_run_decision(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ValidJson(req): ValidJson<AgentDecisionRequest>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-decide");
    ops.run_id = Some(checked);
    ops.decision = Some(req.decision.as_str().to_string());
    ops.reason = Some(req.reason);
    ops.confirm = req.confirm;
    agentic_call(&state, ops).await
}

pub async fn agent_run_push(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ValidJson(req): ValidJson<AgentPushRequest>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-push");
    ops.run_id = Some(checked);
    ops.reason = Some(req.reason);
    ops.confirm = req.confirm;
    agentic_call(&state, ops).await
}

pub async fn agent_run_publish(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ValidJson(req): ValidJson<AgentPublishRequest>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-publish");
    ops.run_id = Some(checked);
    ops.reason = Some(req.reason.clone());
    ops.body = req.body;
    ops.confirm = req.confirm;
    agentic_call(&state, ops).await
}

pub async fn agent_run_discard(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let checked = validated_run_id(&run_id)?;
    let mut ops = OpsRequest::new("real-repo-run-discard");
    ops.run_id = Some(checked);
    agentic_call(&state, ops).await
}

// Identity comes only from the guarded account extension.
type Caller = Option<axum::Extension<crate::common::auth_store::UserSummary>>;

#[cfg(test)]
mod tests {
    use super::same_endpoint;

    #[test]
    fn endpoints_match_only_on_the_same_listener_and_path() {
        assert!(same_endpoint("HTTP://LocalHost:11434/v1/", "http://localhost:11434/v1"));
        assert!(!same_endpoint("http://h:1/Route", "http://h:1/route"));
        assert!(!same_endpoint("http://[::1]:11434/v1", "http://127.0.0.1:11434/v1"));
        assert!(!same_endpoint("not a url", "not a url"));
    }
}
