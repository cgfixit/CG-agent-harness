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

/// Same walk, but the source is handed back verbatim. The substring needles
/// need comments stripped; `syn` skips comments itself, so the parsed checks
/// take the real file and cannot be fooled by a `//` line inside a raw string.
fn read_tree_raw(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push((
                entry.path().display().to_string(),
                std::fs::read_to_string(entry.path()).unwrap(),
            ));
        }
    }
    assert!(!out.is_empty(), "no sources under {}", dir.display());
    out
}

/// Every identifier a `use` tree names, including the pre-alias left side of a
/// rename and every branch of a brace group.
///
/// This is what closes the bypass the substring needles cannot see:
/// `use crate::{agentic as pipeline};` contains none of `"crate::agentic"`,
/// `"agentic::"`, `"super::agentic"` or `"use crate::agentic"`, yet it binds
/// the pipeline into a server-side module under a new name. Here it yields
/// `["crate", "agentic", "pipeline"]` and the boundary check sees `agentic`.
fn use_tree_idents(tree: &syn::UseTree, out: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(p) => {
            out.push(p.ident.to_string());
            use_tree_idents(&p.tree, out);
        }
        syn::UseTree::Name(n) => out.push(n.ident.to_string()),
        syn::UseTree::Rename(r) => {
            // The real module first: the alias is what a later reader sees, but
            // the left side is what actually crossed the boundary.
            out.push(r.ident.to_string());
            out.push(r.rename.to_string());
        }
        syn::UseTree::Glob(_) => {}
        syn::UseTree::Group(g) => {
            for item in &g.items {
                use_tree_idents(item, out);
            }
        }
    }
}

#[derive(Default)]
struct UseIdents(Vec<String>);

impl<'ast> syn::visit::Visit<'ast> for UseIdents {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        use_tree_idents(&node.tree, &mut self.0);
    }
}

/// Idents bound by every `use` in a file, including imports nested inside inline
/// `mod` blocks and function bodies (`syn::visit` walks both).
fn use_idents(path: &str, text: &str) -> Vec<String> {
    let file = syn::parse_file(text).unwrap_or_else(|e| panic!("{path} does not parse as Rust: {e}"));
    let mut visitor = UseIdents::default();
    syn::visit::visit_file(&mut visitor, &file);
    visitor.0
}

/// Fail if any `use` in `dir` names a module on the far side of the I6 boundary,
/// under any spelling: direct, grouped, nested, glob-suffixed or renamed.
fn assert_no_use_crosses(dir: &Path, forbidden: &[&str]) {
    for (path, text) in read_tree_raw(dir) {
        let idents = use_idents(&path, &text);
        for name in forbidden {
            assert!(
                !idents.iter().any(|i| i == name),
                "{path} imports {name:?} across the I6 boundary \
                 (a renamed or grouped `use` still crosses it)"
            );
        }
    }
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
        // The needles above miss every aliased spelling; this does not.
        assert_no_use_crosses(&root().join(sub), &["agentic"]);
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
    // Reverse direction, same bypass: `use crate::{server as console};`.
    assert_no_use_crosses(&root().join("agentic"), &["server", "shim"]);
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
fn shipped_config_enforces_accounts_tls_and_keeps_execution_gates_closed() {
    let cfg = cgagentharness::common::config::AppConfig::from_str(
        cgagentharness::common::config::AppConfig::embedded_default(),
        Path::new("config.yaml"),
    )
    .unwrap();
    for gate in [
        "agentic.enabled",
        "agentic.deepagent_github.enabled",
        "agentic.deepagent_github.allow_git_write_tools",
        "unslop.enabled",
        "mcp.enabled",
        "mcp.sse_allow_loopback",
        "mcp.server.enabled",
    ] {
        assert!(!cfg.flag_is_true(gate), "{gate} must ship false");
    }
    assert!(
        cfg.str_list("mcp.server.tools").is_empty(),
        "listener enablement must not grant tools"
    );
    for gate in [
        "memory.enabled",
        "structured_memory.enabled",
        "structured_memory.episode_capture",
        "structured_memory.explicit_recall",
        "structured_memory.retrieval",
        "structured_memory.auto_retrieval",
        "structured_memory.consolidation",
        "structured_memory.auto_consolidation",
        "structured_memory.auto_suggest_chat",
        "structured_memory.auto_suggest_coding",
    ] {
        assert!(cfg.flag_is_true(gate), "{gate} must ship true");
    }
    for boundary in ["auth.enabled", "tls.enabled", "tls.auto_generate"] {
        assert!(cfg.flag_is_true(boundary), "{boundary} must ship true");
    }
    assert_eq!(cfg.str_list("policy.prompt_filter.banned_patterns").len(), 40);
    // Armed by construction, held closed only by agentic.enabled (checked above) and
    // the disable-only env kill switch -- never by EXECUTION_ENABLED flipping itself.
    // Isolate the operator shell: cargo test must not inherit a dogfood
    // CGAGENTHARNESS_AGENTIC_WRITE_DISABLE and skip this assert.
    std::env::remove_var(cgagentharness::agentic::writer::WRITE_DISABLE_ENV);
    assert!(
        cgagentharness::agentic::writer::execution_enabled(),
        "EXECUTION_ENABLED ships true; tests must not inherit the operator kill switch"
    );

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
fn shipped_defaults_deny_sensitive_read_basenames() {
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    assert!(
        yaml.contains("denied_read_basenames:"),
        "shipped config.default.yaml must seed denied_read_basenames"
    );
    for needle in [
        "- \".env\"",
        "- \".env.*\"",
        "- \"*.pem\"",
        "- \"id_rsa\"",
        "- \"id_rsa*\"",
        "- \"id_ed25519*\"",
        "- \"credentials*\"",
        "- \".npmrc\"",
        "- \".netrc\"",
        "- \"*.p12\"",
    ] {
        assert!(yaml.contains(needle), "shipped denied_read_basenames lost {needle}");
    }
    let defaults = cgagentharness::agentic::config::DEFAULT_DENIED_READ_BASENAMES;
    for required in [
        ".env",
        ".env.*",
        "*.pem",
        "id_rsa",
        "id_ed25519*",
        ".npmrc",
        ".netrc",
        "*.p12",
    ] {
        assert!(
            defaults.contains(&required),
            "DEFAULT_DENIED_READ_BASENAMES must include {required}"
        );
    }
}

#[test]
fn fresh_chat_does_not_select_a_repository() {
    assert_eq!(cgagentharness::agentic::config::DEFAULT_REPO, "");
    let cfg = cgagentharness::common::config::AppConfig::from_str(
        cgagentharness::common::config::AppConfig::embedded_default(),
        Path::new("config.yaml"),
    )
    .unwrap();
    assert_eq!(cfg.str_or("agentic.repo", "unexpected"), "");
    assert!(!cfg.flag_is_true("agentic.enabled"));
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
    let start_idx = src.find("pub(crate) fn start_job(").expect("start_job must exist");
    let run_slice = &src[run_idx..job_idx];
    let job_slice = &src[job_idx..start_idx];
    let start_slice = &src[start_idx..];
    assert!(
        run_slice.contains("prepare_run(&state, &req, &owner)?"),
        "/api/agent/run must go through prepare_run"
    );
    assert!(
        job_slice.contains("start_job("),
        "/api/agent/jobs must go through start_job"
    );
    assert!(
        start_slice.contains("prepare_run_inner(&state, &req, &owner, schedule_id.is_some())?"),
        "detached jobs and schedules must go through prepare_run_inner"
    );
    assert!(
        src.contains("prepare_run_inner(state, req, owner, false)"),
        "prepare_run must be the non-recurring wrapper around prepare_run_inner"
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
    assert!(
        !start_slice.contains("OpsRequest {"),
        "start_job must not construct OpsRequest itself"
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

/// The bypass this guard exists to close, pinned as a fixture.
///
/// Every source below is a spelling that imports `agentic` into a server-side
/// module. The four legacy substring needles miss the aliased and grouped ones
/// entirely — `assert_alias_is_invisible_to_needles` proves that rather than
/// asserting it in prose — so the parsed check is the only thing standing
/// between `use crate::{agentic as pipeline};` and a green build.
#[test]
fn aliased_and_grouped_imports_across_the_boundary_are_caught() {
    const LEGACY_NEEDLES: [&str; 4] = ["crate::agentic", "agentic::", "super::agentic", "use crate::agentic"];

    // Sources that must be rejected, and whether the legacy needles see them.
    let crossings: [(&str, &str); 7] = [
        ("renamed in a group", "use crate::{agentic as pipeline};"),
        ("renamed directly", "use crate::agentic as pipeline;"),
        ("grouped with a sibling", "use crate::{agentic as p, common};"),
        ("nested group", "use crate::{agentic::{writer as w}};"),
        ("glob under a rename", "use crate::agentic::writer::*;"),
        (
            "inside an inline module",
            "mod inner { use crate::{agentic as pipeline}; }",
        ),
        ("inside a function body", "fn f() { use crate::{agentic as pipeline}; }"),
    ];
    for (label, src) in crossings {
        let idents = use_idents(label, src);
        assert!(
            idents.iter().any(|i| i == "agentic"),
            "{label}: {src:?} crosses the boundary but the parsed check missed it"
        );
    }

    // The reverse direction has the same shape.
    for src in ["use crate::{server as console};", "use crate::{shim as edge};"] {
        let idents = use_idents("reverse", src);
        assert!(
            idents.iter().any(|i| i == "server" || i == "shim"),
            "{src:?} crosses the boundary but the parsed check missed it"
        );
    }

    // A doc or line comment may still NAME the far side: that is how the
    // deliberately duplicated constants document each other.
    for src in [
        "//! see crate::agentic::run_store::RUN_ID_PATTERN\nfn f() {}",
        "/// mirrors crate::agentic::config::DEFAULT_PLANNER_TIMEOUT_SEC\npub const X: u64 = 1;",
    ] {
        assert!(
            use_idents("comment", src).is_empty(),
            "a comment naming the far side must stay legal: {src:?}"
        );
    }

    // Do not let this test rot into a tautology: if the legacy needles ever grow
    // to cover the grouped rename, this assertion is the one that should be
    // updated, deliberately, rather than the parser being quietly dropped.
    let aliased = "use crate::{agentic as pipeline};";
    assert!(
        !LEGACY_NEEDLES.iter().any(|n| aliased.contains(n)),
        "the legacy substring needles now cover {aliased:?}; \
         re-check whether the parsed guard is still the load-bearing one"
    );
}
