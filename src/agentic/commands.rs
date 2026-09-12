//! Subcommand bodies for the agentic CLI, port of `agentic/cli.py`'s `cmd_*`.
//! Exit codes: 0 ok, 2 failed, 3 env/config, 4 write refused.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::llm::backend::resolve_reasoning_effort;

use super::cli::{
    deepagent_disabled_noop, disabled_noop, exit_code_for, Opts, EXIT_ENV, EXIT_FAIL, EXIT_OK, EXIT_REFUSED,
};
use super::cloud_proposer::{cloud_key_available, settings_for, CloudProposerClient};
use super::context::{self, blocking_context_findings, describe_findings};
use super::ctx::AgenticCtx;
use super::executor::{manifest, Check};
use super::gh_client::check_gh_version;
use super::governance::inspect_candidate_text;
use super::proposer::{LocalProposerClient, ProposerClient};
use super::real_repo_loop::{finalize_real_repo_change, run_real_repo_loop, FinalizeParams, LoopParams};
use super::registry::{SkillRegistry, SkillSpec};
use super::run_store::{
    load_run, new_run_id, require_approved_for_push, require_pending_decision, require_pushed_for_publish, save_run,
    RealRepoRunRecord, PENDING_DECISION,
};
use super::unslop::UnslopProbe;
use super::workspace::RepoWorkspace;
use super::writer::{execute_write, plan_write, DEFAULT_WRITE_TIMEOUT_SEC};

const MAX_LOOP_CONTEXT_CHARS: usize = 8_000;
const MAX_STATUS_DIFF_CHARS: usize = 20_000;

fn err(msg: &str) {
    eprintln!("  [error] {msg}");
}

fn heading(t: &str) {
    println!("== {t} ==");
}

fn kv(k: &str, v: impl std::fmt::Display) {
    println!("  {k:<18} {v}");
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

pub fn dispatch(action: &str, cfg: &AppConfig, config_path: &Path, opts: &Opts) -> Result<u8> {
    let ctx = AgenticCtx::new(cfg.clone(), config_path)?;
    match action {
        "status" => cmd_status(&ctx),
        "test" => cmd_test(&ctx),
        "context" => cmd_context(&ctx, opts),
        "propose-skill" => cmd_propose_skill(&ctx, opts),
        "apply-skill" => cmd_apply_skill(&ctx, opts),
        "real-repo-run" => cmd_real_repo_run(&ctx, opts),
        "real-repo-runs" => cmd_real_repo_runs(&ctx),
        "real-repo-run-status" => cmd_real_repo_run_status(&ctx, opts),
        "real-repo-run-decide" => cmd_real_repo_run_decide(&ctx, opts),
        "real-repo-run-push" => cmd_real_repo_run_push(&ctx, opts),
        "real-repo-run-publish" => cmd_real_repo_run_publish(&ctx, opts),
        "real-repo-run-discard" => cmd_real_repo_run_discard(&ctx, opts),
        other => Err(HarnessError::agentic(format!("unknown subcommand: {other}"))),
    }
}

fn cmd_status(ctx: &AgenticCtx) -> Result<u8> {
    heading("CGagentHarness Agentic Status");
    kv("enabled", ctx.acfg.enabled);
    kv("repo", &ctx.acfg.repo);
    kv("mode", &ctx.acfg.mode);
    kv("writes_enabled", ctx.acfg.writes_enabled);
    kv(
        "gh_min_version",
        format!(
            "{}.{}.{}",
            ctx.acfg.gh_min_version.0, ctx.acfg.gh_min_version.1, ctx.acfg.gh_min_version.2
        ),
    );
    kv("registry_path", ctx.acfg.registry_path.display());
    kv("allowed_read_ops", ctx.acfg.allowed_read_ops.join(", "));
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    match check_gh_version(ctx.acfg.gh_min_version) {
        Ok(v) => println!("  [ok] gh {}.{}.{}", v.0, v.1, v.2),
        Err(e) => err(&e.message),
    }
    match SkillRegistry::open(ctx) {
        Ok(reg) => {
            kv("registry_version", reg.version());
            let skills = reg.list_skills();
            kv(
                "skills",
                if skills.is_empty() {
                    "(none)".to_string()
                } else {
                    skills.join(", ")
                },
            );
        }
        Err(e) => err(&format!("Registry: {}", e.message)),
    }
    Ok(EXIT_OK)
}

fn cmd_test(ctx: &AgenticCtx) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let mut passed = 0;
    let mut total = 0;
    let mut lines = Vec::new();
    let mut check = |name: &str, ok: bool, detail: &str| {
        total += 1;
        if ok {
            passed += 1;
        }
        lines.push(format!(
            "  [{}] {name}{}",
            if ok { "ok" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    };
    check("config parsed", true, &ctx.config_path.display().to_string());
    check(
        "workspace_root under data/",
        ctx.acfg
            .deepagent
            .workspace_root
            .starts_with(ctx.home_root.join("data")),
        "",
    );
    match check_gh_version(ctx.acfg.gh_min_version) {
        Ok(v) => check("gh version", true, &format!("{}.{}.{}", v.0, v.1, v.2)),
        Err(e) => check("gh version", false, &e.message),
    }
    check("git on PATH", crate::common::process::which("git").is_some(), "");
    match super::executor::production_sandbox() {
        Ok(s) => check("hard sandbox", true, s.name()),
        Err(e) => check("hard sandbox", false, &e.message),
    }
    check(
        "injection scanner",
        !ctx.scanner.is_empty(),
        &format!("{} patterns", ctx.scanner.len()),
    );
    heading(&format!("Self-test: {passed}/{total} passed"));
    for l in lines {
        println!("{l}");
    }
    Ok(if passed == total { EXIT_OK } else { EXIT_FAIL })
}

fn fetch_bundle(ctx: &AgenticCtx, opts: &Opts, include_diff: bool) -> Result<Value> {
    if let Some(pr) = opts.get("pr") {
        let n: i64 = pr
            .parse()
            .map_err(|_| HarnessError::agentic("--pr must be an integer"))?;
        context::fetch_pr_context(ctx, n, include_diff)
    } else if let Some(issue) = opts.get("issue") {
        let n: i64 = issue
            .parse()
            .map_err(|_| HarnessError::agentic("--issue must be an integer"))?;
        context::fetch_issue_context(ctx, n)
    } else {
        context::fetch_repo_context(ctx)
    }
}

fn cmd_context(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let bundle = fetch_bundle(ctx, opts, !opts.flag("no-diff"))?;
    print_json(&bundle);
    Ok(EXIT_OK)
}

fn read_body(opts: &Opts) -> Result<String> {
    if let Some(p) = opts.get("body-file") {
        return std::fs::read_to_string(p).map_err(|e| HarnessError::agentic(format!("cannot read --body-file: {e}")));
    }
    Ok(opts.get("body").unwrap_or("").to_string())
}

fn cmd_propose_skill(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let spec = SkillSpec {
        name: opts.require("name")?,
        description: opts.require("desc")?,
        body: read_body(opts)?,
    };
    let reg = SkillRegistry::open(ctx)?;
    let proposal = reg.propose_skill(&spec, opts.get("reason").unwrap_or(""))?;
    print_json(&proposal);
    Ok(EXIT_OK)
}

fn cmd_apply_skill(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    if !opts.flag("confirm") {
        err("apply-skill requires --confirm");
        return Ok(EXIT_REFUSED);
    }
    let spec = SkillSpec {
        name: opts.require("name")?,
        description: opts.require("desc")?,
        body: read_body(opts)?,
    };
    let mut reg = SkillRegistry::open(ctx)?;
    match reg.apply_skill(&spec, opts.get("reason").unwrap_or("")) {
        Ok(result) => {
            print_json(&result);
            Ok(EXIT_OK)
        }
        Err(e) if e.code == "PROMPT_INJECTION_BLOCKED" => {
            err(&format!("Injection blocked: {}", e.message));
            Ok(EXIT_REFUSED)
        }
        Err(e) => Err(e),
    }
}

fn bundle_context_text(bundle: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(pr) = bundle.get("pr").filter(|p| p.is_object()) {
        if let Some(t) = pr.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            parts.push(format!("PR title: {t}"));
        }
        if let Some(b) = pr.get("body").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            parts.push(format!("PR body:\n{b}"));
        }
    }
    if let Some(issue) = bundle.get("issue").filter(|p| p.is_object()) {
        if let Some(t) = issue.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            parts.push(format!("Issue title: {t}"));
        }
        if let Some(b) = issue.get("body").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
            parts.push(format!("Issue body:\n{b}"));
        }
    }
    if let Some(d) = bundle.get("diff").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) {
        parts.push(format!("Diff:\n{d}"));
    }
    if parts.is_empty() {
        return None;
    }
    let text = parts.join("\n\n");
    if text.chars().count() > MAX_LOOP_CONTEXT_CHARS {
        return Some(format!(
            "{}\n... [context truncated at {MAX_LOOP_CONTEXT_CHARS} chars]",
            crate::common::clip_chars(&text, MAX_LOOP_CONTEXT_CHARS)
        ));
    }
    Some(text)
}

fn refuse_if_injected(ctx: &AgenticCtx, text: &str, field: &str, verb: &str) -> Option<u8> {
    let findings = inspect_candidate_text(&ctx.scanner, text);
    if let Some(f) = findings.first() {
        ctx.audit.log(json!({"event": "agentic_real_repo_operator_text_injection_blocked", "field": field, "code": f.code, "repo": ctx.acfg.repo, "command": verb}));
        err(&format!(
            "refusing to {verb}: --{} matches a governed injection pattern ({})",
            field.replace('_', "-"),
            f.code
        ));
        return Some(EXIT_FAIL);
    }
    None
}

fn load_checks_file(path: &str) -> Result<Vec<Check>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| HarnessError::agentic_config(format!("cannot read checks file: {e}")).detail("path", path))?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|e| HarnessError::agentic_config(format!("cannot read checks file: {e}")).detail("path", path))?;
    let list = data
        .get("checks")
        .and_then(|c| c.as_array())
        .cloned()
        .or_else(|| data.as_array().cloned())
        .unwrap_or_default();
    if list.is_empty() {
        return Err(
            HarnessError::agentic_config("checks file must contain a non-empty JSON list").detail("path", path),
        );
    }
    let mut checks = Vec::new();
    for entry in list {
        let name = entry
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| HarnessError::agentic_config("each check entry needs 'name' and 'argv'"))?;
        let argv: Vec<String> = entry
            .get("argv")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(|x| x.to_string())).collect())
            .unwrap_or_default();
        if argv.is_empty() || entry["argv"].as_array().map(|a| a.len()).unwrap_or(0) != argv.len() {
            return Err(
                HarnessError::agentic_config("check 'argv' must be a non-empty list of strings").detail("path", path),
            );
        }
        let timeout = match entry.get("timeout_sec") {
            None => super::executor::DEFAULT_CHECK_TIMEOUT_SEC,
            Some(t) => t
                .as_u64()
                .ok_or_else(|| HarnessError::agentic_config("invalid check entry: timeout_sec must be an integer"))?,
        };
        checks.push(Check::with_timeout(name, argv, timeout).map_err(|e| HarnessError::agentic_config(e.message))?);
    }
    Ok(checks)
}

fn local_reasoning_effort(ctx: &AgenticCtx) -> Option<String> {
    if ctx.acfg.deepagent.provider != "ollama" {
        return None;
    }
    resolve_reasoning_effort(&ctx.cfg).ok().flatten()
}

/// Gates 3-6 for a provider-driven command. `None` => allowed.
fn cloud_gates(ctx: &AgenticCtx, provider: &str, confirm_online: bool) -> Option<u8> {
    if ctx.acfg.deepagent.cloud_provider(provider).is_none() {
        err(&format!("cloud provider '{provider}' is not enabled (gates 3/4)"));
        return Some(EXIT_ENV);
    }
    if !cloud_key_available(provider) {
        err(&format!("cloud provider '{provider}' has no API key set (gate 5)"));
        return Some(EXIT_ENV);
    }
    if !confirm_online {
        err(&format!(
            "--confirm-online is required to drive the loop with '{provider}' (gate 6)"
        ));
        return Some(EXIT_REFUSED);
    }
    ctx.audit
        .log(json!({"event": "agentic_deepagent_cloud_confirmed", "provider": provider}));
    None
}

fn cmd_real_repo_run(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    if !ctx.acfg.deepagent.enabled {
        return Ok(deepagent_disabled_noop());
    }
    let instruction = opts.require("instruction").map_err(HarnessError::agentic)?;
    let checks_file = opts.require("checks-file").map_err(HarnessError::agentic)?;
    let branch = opts.require("branch").map_err(HarnessError::agentic)?;
    let commit_message = opts.require("commit-message").map_err(HarnessError::agentic)?;
    let reason = opts.get("reason").unwrap_or("").to_string();
    let max_iterations: u64 = match opts.get("max-iterations") {
        None => 3,
        Some(v) => v
            .parse()
            .map_err(|_| HarnessError::agentic("--max-iterations must be an integer"))?,
    };
    let provider = opts.get("provider").map(|s| s.to_string());
    if let Some(p) = &provider {
        if let Some(code) = cloud_gates(ctx, p, opts.flag("confirm-online")) {
            return Ok(code);
        }
    } else if ctx.acfg.deepagent.model.trim().is_empty() {
        err("agentic.deepagent_github.model must be configured before real-repo-run");
        return Ok(EXIT_ENV);
    }
    super::writer::current_repository_policy(ctx, "run", &reason, opts.flag("confirm"))?;
    if let Some(code) = refuse_if_injected(ctx, &instruction, "instruction", "run") {
        return Ok(code);
    }
    let bundle = match fetch_bundle(ctx, opts, true) {
        Ok(b) => b,
        Err(e) => {
            err(&e.message);
            return Ok(exit_code_for(&e));
        }
    };
    let blocking = blocking_context_findings(&bundle);
    if !blocking.is_empty() {
        err(&format!(
            "refusing to run: {} injection finding(s) in the fetched context ({})",
            blocking.len(),
            describe_findings(&blocking)
        ));
        return Ok(EXIT_FAIL);
    }
    let context_text = bundle_context_text(&bundle);
    let checks = match load_checks_file(&checks_file) {
        Ok(c) => c,
        Err(e) => {
            err(&e.message);
            return Ok(EXIT_ENV);
        }
    };
    let mut plan = String::new();
    if let Some(pf) = opts.get("plan-file") {
        plan = match std::fs::read_to_string(pf) {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                err(&format!("could not read --plan-file: {e}"));
                return Ok(EXIT_ENV);
            }
        };
        if plan.is_empty() {
            err("--plan-file is empty; omit the flag to run without a plan");
            return Ok(EXIT_ENV);
        }
        if let Some(code) = refuse_if_injected(ctx, &plan, "plan_file", "run") {
            return Ok(code);
        }
    }
    let plan_sha = if plan.is_empty() {
        None
    } else {
        Some(crate::common::sha256_hex(&plan))
    };
    let runs_dir = ctx.runs_dir();
    let run_id = new_run_id();
    let tools = match RepoWorkspace::clone(ctx) {
        Ok(t) => t,
        Err(e) => {
            err(&e.message);
            return Ok(exit_code_for(&e));
        }
    };
    let dest = tools.worktree().display().to_string();
    let _run_lease = super::run_store::acquire_run_lease(&runs_dir, &run_id, true)?;
    // Persisted BEFORE the loop runs so a killed process still leaves a record.
    let mut record = RealRepoRunRecord::new(&run_id, &ctx.acfg.repo, &dest, "running");
    record.origin_url = Some(tools.origin_url()?);
    record.provider = provider.clone();
    record.plan_sha256 = plan_sha.clone();
    save_run(&runs_dir, &mut record)?;

    let local_client;
    let cloud_client;
    let client: &dyn ProposerClient = match &provider {
        Some(p) => {
            let settings = settings_for(&ctx.acfg.deepagent, p)?;
            cloud_client =
                CloudProposerClient::new(settings, &ctx.audit, &ctx.scanner, ctx.redactors(), ctx.spend_file())?;
            &cloud_client
        }
        None => {
            local_client = LocalProposerClient::new(
                &ctx.audit,
                &ctx.acfg.deepagent.base_url,
                &ctx.acfg.deepagent.model,
                ctx.acfg.deepagent.planner_timeout_sec,
                &std::env::var("DEEPAGENT_API_KEY").unwrap_or_default(),
                local_reasoning_effort(ctx),
            )?;
            &local_client
        }
    };
    let unslop = if provider.is_none() {
        UnslopProbe::from_config(&ctx.cfg, &ctx.home_root)
    } else {
        None
    };
    let read_paths = opts.all("read-file");
    let outcome = run_real_repo_loop(
        ctx,
        &tools,
        client,
        &LoopParams {
            instruction: &instruction,
            checks: &checks,
            branch_name: &branch,
            commit_message: &commit_message,
            max_iterations,
            reason: &reason,
            confirm: true,
            max_tokens: ctx.acfg.deepagent.planner_max_tokens,
            context: context_text.as_deref(),
            read_paths: &read_paths,
            protected_write_paths: &ctx.acfg.deepagent.protected_write_paths,
            max_write_budget_bytes: Some(ctx.acfg.deepagent.max_write_budget_bytes),
            scan_code_shape: ctx.acfg.deepagent.scan_code_shape,
            plan: &plan,
            unslop: unslop.as_ref(),
            sandbox: None,
        },
    );
    let result = match outcome {
        Ok(r) => r,
        Err(e) => {
            record.status = "failed".into();
            record.error = Some(e.message.clone());
            save_run(&runs_dir, &mut record)?;
            tools.close_after_loop(Some(&e));
            err(&e.message);
            return Ok(exit_code_for(&e));
        }
    };
    if result.accepted {
        let changed = result.changed_files();
        let base_head = manifest::git_head(tools.worktree())?;
        let (_, digest) = manifest::build_manifest(tools.worktree(), &changed, &run_id, &base_head)?;
        ctx.audit.log(json!({"event": "agentic_real_repo_manifest_built", "run_id": run_id, "acceptance_digest": digest, "files": changed.len()}));
        record.status = PENDING_DECISION.into();
        record.branch_name = result.branch_name.clone();
        record.commit_message = result.commit_message.clone();
        record.changed_files = changed;
        record.iterations = result.iterations.len() as u64;
        record.acceptance_digest = Some(digest);
        record.acceptance_base_head = Some(base_head);
        tools.release();
    } else {
        record.status = "exhausted".into();
        record.iterations = result.iterations.len() as u64;
        tools.close();
    }
    save_run(&runs_dir, &mut record)?;
    print_json(&record.to_json());
    Ok(EXIT_OK)
}

fn render_pending_diff(ctx: &AgenticCtx, dest: &str, changed_files: &[String]) -> String {
    let tools = match RepoWorkspace::attach(ctx, Path::new(dest)) {
        Ok(t) => t,
        Err(e) => return format!("[diff unavailable: {}]", e.message),
    };
    let mut parts = Vec::new();
    match tools.diff(false) {
        Ok(d) if !d.is_empty() => parts.push(d),
        Ok(_) => {}
        Err(e) => return format!("[diff unavailable: {}]", e.message),
    }
    let untracked = match tools.untracked_files() {
        Ok(u) => u,
        Err(e) => return format!("[diff unavailable: {}]", e.message),
    };
    let mut new_files: Vec<&String> = untracked.iter().filter(|u| changed_files.contains(u)).collect();
    new_files.sort();
    for path in new_files {
        match tools.read_file(path) {
            Ok(content) => parts.push(format!("--- new file: {path} ---\n{content}")),
            Err(e) => return format!("[diff unavailable: {}]", e.message),
        }
    }
    let text = parts.join("\n\n");
    if text.is_empty() {
        return "[no diff to show -- the candidate reported changed files, but none were tracked or new]".into();
    }
    if text.chars().count() > MAX_STATUS_DIFF_CHARS {
        return format!(
            "{}\n... [diff truncated at {MAX_STATUS_DIFF_CHARS} chars]",
            crate::common::clip_chars(&text, MAX_STATUS_DIFF_CHARS)
        );
    }
    text
}

fn cmd_real_repo_runs(ctx: &AgenticCtx) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let directory = ctx.runs_dir();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            print_json(&json!({"runs":[],"truncated":false}));
            return Ok(EXIT_OK);
        }
        Err(e) => return Err(e.into()),
    };
    let mut paths = Vec::new();
    let mut scanned = 0;
    for entry in entries.take(4097) {
        scanned += 1;
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let id = path.file_stem().and_then(|v| v.to_str()).unwrap_or("");
        if super::run_store::run_id_re().is_match(id) {
            paths.push((entry.metadata()?.modified()?, id.to_string()));
        }
    }
    paths.sort_by_key(|a| std::cmp::Reverse(a.0));
    let truncated = scanned > 4096 || paths.len() > 128;
    let mut records = Vec::new();
    for (_, id) in paths.into_iter().take(128) {
        match load_run(&directory, &id) {
            Ok(mut record) => {
                super::run_store::reconcile_run(&directory, &mut record)?;
                records.push(json!({"run_id":id,"status":record.status,"updated_at":record.updated_at,"error":record.error}));
            }
            Err(_) => records.push(json!({"run_id":id,"status":"unreadable","error":"Retained record is unreadable; no action was taken."})),
        }
    }
    print_json(&json!({"runs":records,"truncated":truncated}));
    Ok(EXIT_OK)
}

fn cmd_real_repo_run_status(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let run_id = opts.require("run-id").map_err(HarnessError::agentic)?;
    let mut record = load_run(&ctx.runs_dir(), &run_id)?;
    super::run_store::reconcile_run(&ctx.runs_dir(), &mut record)?;
    let mut payload = record.to_json();
    if record.status == PENDING_DECISION || record.status == "interrupted" {
        payload["diff"] = json!(render_pending_diff(ctx, &record.dest, &record.changed_files));
    }
    print_json(&payload);
    Ok(EXIT_OK)
}

fn push_record(
    tools: &RepoWorkspace<'_>,
    record: &mut RealRepoRunRecord,
    runs_dir: &Path,
    reason: &str,
    confirm: bool,
) -> Result<u8> {
    let branch = record
        .branch_name
        .clone()
        .ok_or_else(|| HarnessError::agentic("run record has no branch_name to push"))?;
    match tools.push_approved(
        &branch,
        record.approved_commit.as_deref().unwrap_or(""),
        record.origin_url.as_deref().unwrap_or(""),
        reason,
        confirm,
    ) {
        Ok(_) => {
            record.pushed = true;
            Ok(EXIT_OK)
        }
        Err(e) => {
            save_run(runs_dir, record)?;
            err(&format!(
                "push {}: {}",
                if e.code == "AGENTIC_WRITE_REFUSED" {
                    "refused"
                } else {
                    "failed"
                },
                e.message
            ));
            print_json(&record.to_json());
            Ok(exit_code_for(&e))
        }
    }
}

fn publish_record(
    ctx: &AgenticCtx,
    record: &mut RealRepoRunRecord,
    runs_dir: &Path,
    reason: &str,
    confirm: bool,
    body: &str,
) -> Result<u8> {
    let params = json!({
        "head": record.branch_name, "title": record.commit_message,
        "body": body,
    });
    let current = super::writer::current_repository_policy(ctx, "pr_create", reason, confirm)?;
    let outcome = plan_write(&current, &ctx.audit, "pr_create", reason, confirm, params)
        .and_then(|plan| execute_write(ctx, &plan, confirm, DEFAULT_WRITE_TIMEOUT_SEC));
    match outcome {
        Ok(result) => {
            record.pr_url = result
                .get("stdout")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            Ok(EXIT_OK)
        }
        Err(e) => {
            if e.details
                .get("indeterminate")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                record.pr_url =
                    Some("INDETERMINATE: gh pr create outcome unknown; verify on GitHub before retrying".into());
            }
            save_run(runs_dir, record)?;
            err(&format!(
                "publish {}: {}",
                if e.code == "AGENTIC_WRITE_REFUSED" {
                    "refused"
                } else {
                    "failed"
                },
                e.message
            ));
            print_json(&record.to_json());
            Ok(exit_code_for(&e))
        }
    }
}

fn cmd_real_repo_run_decide(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    let push = opts.flag("push");
    let publish = opts.flag("publish");
    let decision = opts.require("decision").map_err(HarnessError::agentic)?;
    if push || publish {
        return Err(HarnessError::write_refused("approve only commits locally; use separate real-repo-run-push and real-repo-run-publish actions with --reason and --confirm"));
    }
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    if decision != "approve" && decision != "reject" {
        return Err(HarnessError::agentic("--decision must be 'approve' or 'reject'"));
    }
    let runs_dir = ctx.runs_dir();
    let run_id = opts.require("run-id").map_err(HarnessError::agentic)?;
    let mut record = load_run(&runs_dir, &run_id)?;
    require_pending_decision(&record)?;
    let (branch, message) = match (&record.branch_name, &record.commit_message) {
        (Some(b), Some(m)) => (b.clone(), m.clone()),
        _ => {
            return Err(
                HarnessError::agentic("run record is pending decision but missing branch_name/commit_message")
                    .detail("run_id", run_id),
            )
        }
    };
    let tools = RepoWorkspace::attach(ctx, Path::new(&record.dest))?;
    let outcome = finalize_real_repo_change(
        ctx,
        &tools,
        &FinalizeParams {
            reason: opts.get("reason").unwrap_or(""),
            confirm: opts.flag("confirm"),
            branch_name: &branch,
            commit_message: &message,
            changed_files: &record.changed_files,
            decision: &decision,
            protected_write_paths: &ctx.acfg.deepagent.protected_write_paths,
            run_id: &record.run_id,
            acceptance_digest: record.acceptance_digest.as_deref(),
            acceptance_base_head: record.acceptance_base_head.as_deref(),
        },
    );
    if decision == "reject" {
        tools.close();
    }
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            err(&e.message);
            return Ok(exit_code_for(&e));
        }
    };
    record.status = outcome["status"].as_str().unwrap_or("").to_string();
    record.approved_commit = outcome["approved_commit"].as_str().map(str::to_string);
    // Record the local decision; push and publication are separate calls.
    save_run(&runs_dir, &mut record)?;
    print_json(&record.to_json());
    Ok(EXIT_OK)
}

fn attach_approved(
    ctx: &AgenticCtx,
    opts: &Opts,
    require: &str,
) -> Result<std::result::Result<(RealRepoRunRecord, PathBuf), u8>> {
    if !ctx.acfg.enabled {
        return Ok(Err(disabled_noop()));
    }
    let run_id = opts.require("run-id").map_err(HarnessError::agentic)?;
    let runs_dir = ctx.runs_dir();
    let record = load_run(&runs_dir, &run_id)?;
    if require == "push" {
        require_approved_for_push(&record)?;
    } else {
        require_pushed_for_publish(&record)?;
    }
    if record.branch_name.is_none() {
        return Err(HarnessError::agentic("run record is approved but missing branch_name").detail("run_id", run_id));
    }
    Ok(Ok((record, runs_dir)))
}

fn cmd_real_repo_run_push(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    let (mut record, runs_dir) = match attach_approved(ctx, opts, "push")? {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };
    let tools = RepoWorkspace::attach(ctx, Path::new(&record.dest))?;
    let code = push_record(
        &tools,
        &mut record,
        &runs_dir,
        opts.get("reason").unwrap_or(""),
        opts.flag("confirm"),
    )?;
    if code != EXIT_OK {
        return Ok(code);
    }
    save_run(&runs_dir, &mut record)?;
    print_json(&record.to_json());
    Ok(EXIT_OK)
}

fn cmd_real_repo_run_publish(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !opts.flag("confirm") {
        err("--confirm is required to open a pull request");
        return Ok(EXIT_REFUSED);
    }
    let (mut record, runs_dir) = match attach_approved(ctx, opts, "publish")? {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };
    let tools = RepoWorkspace::attach(ctx, Path::new(&record.dest))?;
    super::writer::current_repository_policy(ctx, "pr_create", opts.get("reason").unwrap_or(""), true)?;
    use std::io::Read;
    let mut body = String::new();
    if let Some(path) = opts.get("body-file") {
        std::fs::File::open(path)?
            .take((crate::common::MAX_PR_BODY_BYTES + 1) as u64)
            .read_to_string(&mut body)?;
    } else if let Some(value) = opts.get("body") {
        body = value.to_string();
    }
    if body.trim().is_empty() || body.len() > crate::common::MAX_PR_BODY_BYTES {
        return Err(HarnessError::agentic(
            "publication requires a reviewed --body-file (or --body) of 1..65536 bytes; use the repository PR template",
        ));
    }
    tools.verify_published_source(
        record.branch_name.as_deref().unwrap_or(""),
        record.approved_commit.as_deref().unwrap_or(""),
        record.origin_url.as_deref().unwrap_or(""),
    )?;
    let code = publish_record(
        ctx,
        &mut record,
        &runs_dir,
        opts.get("reason").unwrap_or(""),
        true,
        &body,
    )?;
    if code != EXIT_OK {
        return Ok(code);
    }
    save_run(&runs_dir, &mut record)?;
    print_json(&record.to_json());
    Ok(EXIT_OK)
}

fn cmd_real_repo_run_discard(ctx: &AgenticCtx, opts: &Opts) -> Result<u8> {
    if !ctx.acfg.enabled {
        return Ok(disabled_noop());
    }
    let run_id = opts.require("run-id").map_err(HarnessError::agentic)?;
    let runs_dir = ctx.runs_dir();
    let mut record = load_run(&runs_dir, &run_id)?;
    super::run_store::reconcile_run(&runs_dir, &mut record)?;
    // A running legacy record has no reliable ownership evidence. A current
    // worker holds its lease, so neither can be discarded as stale by a guess.
    if record.status == "running" {
        return Err(HarnessError::agentic(
            "run is live or ownership is unknown; discard refused",
        ));
    }
    if record.status == PENDING_DECISION {
        err(&format!(
            "run {run_id} is still pending a decision -- run real-repo-run-decide first"
        ));
        return Ok(EXIT_FAIL);
    }
    let dest = Path::new(&record.dest);
    if dest.is_dir() {
        let tools = RepoWorkspace::attach(ctx, dest)?;
        tools.close();
    }
    record.status = "discarded".into();
    save_run(&runs_dir, &mut record)?;
    print_json(&record.to_json());
    Ok(EXIT_OK)
}
