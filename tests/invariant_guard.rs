//! Invariant 6 at the source level: the server side never references the
//! agentic module, only the shim spawns a child, the agentic side never
//! references the server or the shim, and the deliberately duplicated
//! constants still agree.

use std::path::Path;

/// Sources with comment lines removed: a doc comment is allowed to NAME the
/// module on the far side of the boundary (that is how the duplicated
/// constants document each other); code is not allowed to reference it.
fn strip_comment_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_tree(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("rs") {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            out.push((entry.path().display().to_string(), strip_comment_lines(&text)));
        }
    }
    assert!(!out.is_empty(), "no sources under {}", dir.display());
    out
}

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn server_side_never_references_agentic() {
    for sub in ["server", "shim", "llm", "common"] {
        for (path, text) in read_tree(&root().join(sub)) {
            for needle in ["crate::agentic", "agentic::", "super::agentic", "use crate::agentic"] {
                assert!(
                    !text.contains(needle),
                    "{path} references the agentic module via {needle:?}"
                );
            }
        }
    }
}

#[test]
fn only_the_shim_spawns_a_child_on_the_server_side() {
    for sub in ["server", "llm"] {
        for (path, text) in read_tree(&root().join(sub)) {
            assert!(
                !text.contains("process::Command"),
                "{path} spawns a process; only src/shim may"
            );
            assert!(
                !text.contains("Command::new"),
                "{path} spawns a process; only src/shim may"
            );
        }
    }
    let shim = std::fs::read_to_string(root().join("shim").join("mod.rs")).unwrap();
    assert!(shim.contains("current_exe()"), "the shim must spawn its own executable");
    assert!(
        shim.contains("\"agentic\""),
        "the shim must invoke the hidden agentic subcommand"
    );
    assert!(shim.contains("kill_on_drop(true)"));
}

#[test]
fn agentic_side_never_references_server_or_shim() {
    for (path, text) in read_tree(&root().join("agentic")) {
        for needle in ["crate::server", "crate::shim", "server::", "shim::"] {
            assert!(!text.contains(needle), "{path} references {needle:?}");
        }
    }
}

#[test]
fn duplicated_constants_still_agree() {
    assert_eq!(
        cgagentharness::server::agent_policy::RUN_ID_PATTERN,
        cgagentharness::agentic::run_store::RUN_ID_PATTERN,
        "RUN_ID_RE drifted between the server and the agentic side"
    );
    const _: () = assert!(
        cgagentharness::server::schemas::MAX_PLAN_CHARS >= cgagentharness::agentic::real_repo_loop::MAX_PLAN_CHARS
    );
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_FALLBACK_PLANNER_SEC,
        cgagentharness::agentic::config::DEFAULT_PLANNER_TIMEOUT_SEC
    );
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_CHECK_SEC,
        cgagentharness::agentic::executor::DEFAULT_CHECK_TIMEOUT_SEC
    );
    for (name, _) in cgagentharness::server::agent_policy::available_profiles() {
        let resolved =
            cgagentharness::server::agent_policy::resolve_check_profiles(std::slice::from_ref(&name)).unwrap();
        assert!(
            !resolved[0]["argv"].as_array().unwrap().is_empty(),
            "{name} has an empty argv"
        );
    }
    // The shim whitelist is exactly the CLI's dispatch table (no deepagent-plan, no __sleep).
    let commands = std::fs::read_to_string(root().join("agentic").join("commands.rs")).unwrap();
    for action in cgagentharness::shim::ACTIONS {
        assert!(
            commands.contains(&format!("\"{action}\" =>")),
            "{action} missing from the CLI dispatch"
        );
    }
    assert!(cgagentharness::shim::ACTIONS.contains(&"real-repo-runs"));
    assert!(!cgagentharness::shim::ACTIONS.contains(&"deepagent-plan"));
    assert!(!cgagentharness::shim::ACTIONS.contains(&"__sleep"));
}

#[test]
fn shipped_config_keeps_every_gate_closed() {
    let cfg = cgagentharness::common::config::AppConfig::from_str(
        cgagentharness::common::config::AppConfig::embedded_default(),
        Path::new("config.yaml"),
    )
    .unwrap();
    for gate in [
        "agentic.enabled",
        "agentic.deepagent_github.enabled",
        "agentic.deepagent_github.allow_git_write_tools",
        "auth.enabled",
        "unslop.enabled",
    ] {
        assert!(!cfg.flag_is_true(gate), "{gate} must ship false");
    }
    assert_eq!(cfg.str_list("policy.prompt_filter.banned_patterns").len(), 40);
    // Armed by construction, held closed only by agentic.enabled (checked above) and
    // the disable-only env kill switch -- never by EXECUTION_ENABLED flipping itself.
    if std::env::var(cgagentharness::agentic::writer::WRITE_DISABLE_ENV).is_err() {
        assert!(cgagentharness::agentic::writer::execution_enabled());
    }

    // Source of truth: the shipped YAML uses literal booleans (quoted
    // "true" / "false" would be strings and flag_is_true would hide a mistake).
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    for needle in ["api_key_optional: true", "allow_git_write_tools: false"] {
        assert!(yaml.contains(needle), "shipped config lost {needle}");
    }
    assert!(
        !yaml.contains("allow_git_write_tools: true"),
        "allow_git_write_tools must ship false"
    );
    assert!(
        cfg.flag_is_true("security.api_key_optional"),
        "direct local use must ship without a key requirement"
    );
}

fn cargo_toml_github_slug() -> String {
    let cargo = include_str!("../Cargo.toml");
    for line in cargo.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("repository") else {
            continue;
        };
        let url = rest.trim().trim_start_matches('=').trim().trim_matches('"');
        let mut parts = url.trim_end_matches(".git").rsplit('/');
        let name = parts.next().expect("repository URL has a repo name");
        let owner = parts.next().expect("repository URL has an owner");
        return format!("{owner}/{name}");
    }
    panic!("Cargo.toml missing repository = \"https://github.com/owner/name\"");
}

#[test]
fn shipped_defaults_protect_agents_md() {
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    assert!(
        yaml.contains("- \"AGENTS.md\""),
        "shipped config.default.yaml must list AGENTS.md in protected_write_paths"
    );
    assert!(
        cgagentharness::agentic::config::DEFAULT_PROTECTED_WRITE_PATH_PREFIXES.contains(&"AGENTS.md"),
        "Rust DEFAULT_PROTECTED_WRITE_PATH_PREFIXES must protect AGENTS.md"
    );
}

#[test]
fn default_repo_matches_cargo_toml_repository() {
    let expected = cargo_toml_github_slug();
    assert_eq!(
        expected, "cgfixit/CG-agent-harness",
        "Cargo.toml repository owner/name is the source of truth"
    );
    assert_eq!(
        cgagentharness::agentic::config::DEFAULT_REPO,
        expected,
        "DEFAULT_REPO must match Cargo.toml repository owner/name"
    );
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    assert!(
        yaml.contains(&format!("repo: \"{expected}\"")),
        "embedded YAML agentic.repo must equal {expected}"
    );
}

#[test]
fn readme_does_not_link_to_missing_claude_md() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = include_str!("../README.md");
    assert!(
        !readme.contains("(CLAUDE.md)"),
        "README must not link to CLAUDE.md after the rename to AGENTS.md"
    );
    assert!(
        readme.contains("(AGENTS.md)"),
        "README must point operators at AGENTS.md"
    );
    assert!(manifest.join("AGENTS.md").is_file(), "AGENTS.md must exist");
    assert!(
        !manifest.join("CLAUDE.md").exists(),
        "CLAUDE.md was renamed; do not resurrect the old filename"
    );
}

#[test]
fn writer_kill_switch_is_and_not_or() {
    let writer = include_str!("../src/agentic/writer.rs");
    assert!(
        writer.contains("EXECUTION_ENABLED && !disabled_by_env()"),
        "execution_enabled must AND the disable-only env kill switch, never OR it"
    );
    assert!(writer.contains("CGAGENTHARNESS_AGENTIC_WRITE_DISABLE"));
}

#[test]
fn agent_run_and_jobs_share_prepare_run() {
    let src = include_str!("../src/server/routes/agent.rs");
    assert!(
        src.contains("fn prepare_run("),
        "prepare_run is the shared validation/budget/broker/gate path"
    );
    let run_idx = src.find("pub async fn agent_run(").expect("agent_run must exist");
    let job_idx = src
        .find("pub async fn agent_job_create(")
        .expect("agent_job_create must exist");
    let run_slice = &src[run_idx..job_idx];
    let job_slice = &src[job_idx..];
    assert!(
        run_slice.contains("prepare_run(&state, &req)?"),
        "/api/agent/run must go through prepare_run"
    );
    assert!(
        job_slice.contains("prepare_run(&state, &req)?"),
        "/api/agent/jobs must go through prepare_run"
    );
    // Neither route builds an OpsRequest by hand — that would let them drift.
    assert!(
        !run_slice.contains("OpsRequest {"),
        "agent_run must not construct OpsRequest itself"
    );
    assert!(
        !job_slice.contains("OpsRequest {"),
        "agent_job_create must not construct OpsRequest itself"
    );
}

#[test]
fn console_asset_is_verbatim_with_both_placeholders() {
    let html = cgagentharness::server::console::HARNESS_HTML;
    assert!(html.contains("__CYCLAW_CSRF_TOKEN__"));
    assert!(html.contains("__CYCLAW_CSP_NONCE__"));
    assert!(html.contains("/static/auth_admin.js"));
    // The asset mentions localStorage once, in a comment saying it never uses it; the API call shape is what matters.
    for api in ["localStorage.", "sessionStorage.", "document.cookie"] {
        assert!(
            !html.contains(api),
            "the console keeps the API key out of storage ({api})"
        );
    }
}
