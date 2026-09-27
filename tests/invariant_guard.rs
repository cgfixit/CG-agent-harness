//! Invariant 6 at the source level: the server side never references the
//! agentic module, only the shim spawns a child, the agentic side never
//! references the server or the shim, and the deliberately duplicated
//! constants still agree. Also the repository's docs budget (#213).

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

/// rust-toolchain.toml overrides every CI toolchain input, so its channel is
/// the only compiler CI proves; it must stay the declared MSRV.
#[test]
fn pinned_toolchain_is_the_declared_rust_version() {
    let read = |name: &str| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap();
    let value = |text: &str, key: &str| {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
            Some(rest.strip_prefix('"')?.strip_suffix('"')?.to_string())
        })
    };
    let channel = value(&read("rust-toolchain.toml"), "channel").expect("rust-toolchain.toml channel");
    let msrv = value(&read("Cargo.toml"), "rust-version").expect("Cargo.toml rust-version");
    assert_eq!(
        channel, msrv,
        "bump both together, or add a CI job that builds with RUSTUP_TOOLCHAIN=<rust-version>"
    );
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

// ---------------------------------------------------------------------------
// Docs budget (#213). Markdown grows when nothing stops it: the #148 cut
// regrew within 72 hours. Every `.md` in the checkout, tracked or untracked,
// needs a row, and every file and group stays under its word cap. Editing a
// section stays cheap; adding a file or raising a cap is a visible row change
// that needs the operator's approval and a `// why:` comment on the row.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Repository-root entry points for people.
    Root,
    /// Topic docs under `docs/` and beside the code they describe.
    Guide,
    /// Dated acceptance, closeout and verification notes. New evidence goes in
    /// PR bodies and issue comments, so this group only shrinks.
    Evidence,
    /// What agents load: `AGENTS.md`, `.claude/`, `.codex/`, `.github/`.
    Agent,
}

use Kind::{Agent, Evidence, Guide, Root};

/// `(path, kind, word cap)`, sorted by path. Caps started at each file's size
/// on 2026-09-27, rounded up to the next 100 words. Lower them when a fold lands.
const DOCS_BUDGET: &[(&str, Kind, usize)] = &[
    (".claude/CLAUDE.md", Agent, 400),
    (".claude/commands/cgagentharness-config-guard.md", Agent, 100),
    (".claude/commands/cgagentharness-gotchas.md", Agent, 100),
    (".claude/commands/cgagentharness-optimize.md", Agent, 100),
    (".claude/commands/cgagentharness-project-guidance.md", Agent, 100),
    (".claude/commands/cgagentharness-runtime-invariant-check.md", Agent, 100),
    (".claude/commands/cgagentharness-verify-deps.md", Agent, 100),
    (".claude/commands/cgagentharness-write-policy-redteam.md", Agent, 100),
    (".claude/commands/dep-sync.md", Agent, 100),
    (".claude/commands/fable-protocol.md", Agent, 100),
    (".claude/commands/verification-specialist.md", Agent, 100),
    (".claude/skills/cgagentharness-config-guard/SKILL.md", Agent, 700),
    (".claude/skills/cgagentharness-doc-sync/SKILL.md", Agent, 500),
    (".claude/skills/cgagentharness-gotchas/SKILL.md", Agent, 2000),
    (".claude/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 600),
    (".claude/skills/cgagentharness-optimize/SKILL.md", Agent, 1400),
    (".claude/skills/cgagentharness-parity/SKILL.md", Agent, 500),
    (".claude/skills/cgagentharness-project-guidance/SKILL.md", Agent, 800),
    (
        ".claude/skills/cgagentharness-runtime-invariant-check/SKILL.md",
        Agent,
        500,
    ),
    (".claude/skills/cgagentharness-verify-deps/SKILL.md", Agent, 400),
    (
        ".claude/skills/cgagentharness-write-policy-redteam/SKILL.md",
        Agent,
        800,
    ),
    (".claude/skills/dep-sync/SKILL.md", Agent, 800),
    (".claude/skills/doc-sync/SKILL.md", Agent, 800),
    (".claude/skills/fable-protocol/SKILL.md", Agent, 2400),
    (".claude/skills/run-cg-agent-harness/SKILL.md", Agent, 400),
    (".claude/skills/verification-specialist/SKILL.md", Agent, 600),
    (".codex/README.md", Agent, 200),
    (".codex/skills/cgagentharness-config-guard/SKILL.md", Agent, 700),
    (".codex/skills/cgagentharness-gotchas/SKILL.md", Agent, 2000),
    (".codex/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 600),
    (".codex/skills/cgagentharness-optimize/SKILL.md", Agent, 500),
    (".codex/skills/cgagentharness-parity/SKILL.md", Agent, 500),
    (".codex/skills/cgagentharness-project-guidance/SKILL.md", Agent, 800),
    (".codex/skills/cgagentharness-release/SKILL.md", Agent, 400),
    (".codex/skills/cgagentharness-verify/SKILL.md", Agent, 300),
    (".codex/skills/cgagentharness-write-policy-redteam/SKILL.md", Agent, 800),
    (".codex/skills/fable-protocol/SKILL.md", Agent, 500),
    (".codex/skills/verification-specialist/SKILL.md", Agent, 600),
    (".github/PULL_REQUEST_TEMPLATE.md", Agent, 1200),
    (".github/skills/README.md", Agent, 0),
    (".github/skills/repo-optimize/SKILL.md", Agent, 200),
    ("AGENTS.md", Agent, 2000),
    ("INVARIANTS.md", Root, 5200),
    ("README.md", Root, 1700),
    ("SECURITY.md", Root, 500),
    ("docs/ACCOUNTS.md", Guide, 600),
    ("docs/ANALYTICS.md", Guide, 900),
    ("docs/API_ROUTES.md", Guide, 2000),
    ("docs/BOUNDED_EDITS.md", Guide, 600),
    ("docs/CHAT_STREAMING.md", Guide, 400),
    ("docs/CHAT_WORKFLOWS.md", Guide, 2600),
    ("docs/CODING_PIPELINE.md", Guide, 2300),
    ("docs/COMPACTION_ACCEPTANCE.md", Evidence, 900),
    ("docs/CONFIG_RELOAD.md", Guide, 600),
    ("docs/CONSOLE.md", Guide, 6100),
    ("docs/CONSOLE_JOBS.md", Guide, 1400),
    ("docs/DEPENDENCIES.md", Guide, 1400),
    ("docs/DESKTOP.md", Guide, 2800),
    ("docs/DESKTOP_ACCEPTANCE.md", Evidence, 3000),
    ("docs/FINETUNE.md", Guide, 900),
    ("docs/GIT_APPROVAL.md", Guide, 800),
    ("docs/GROK_ACP_PHASE0.md", Evidence, 800),
    ("docs/INSTALL.md", Guide, 5500),
    ("docs/ISSUE_102_ACCEPTANCE.md", Evidence, 600),
    ("docs/ISSUE_148_CLOSEOUT.md", Evidence, 1100),
    ("docs/MAC_ACCEPTANCE.md", Evidence, 1000),
    ("docs/MCP_CLIENT.md", Guide, 1000),
    ("docs/MCP_SERVER.md", Guide, 1200),
    ("docs/MEMORY_BENCHMARK_PLAN.md", Evidence, 1100),
    ("docs/MEMORY_GUIDE.md", Guide, 1700),
    ("docs/MEMORY_SETUP.md", Guide, 2100),
    ("docs/MODELS.md", Guide, 900),
    ("docs/OFFLINE_CARGO.md", Guide, 600),
    ("docs/PORT_PARITY.md", Evidence, 2900),
    ("docs/PROCESS_LIFECYCLE.md", Guide, 1300),
    ("docs/RELEASING.md", Guide, 700),
    ("docs/SECURE_RESEARCH.md", Guide, 3900),
    ("docs/SPEND_AND_NOTIFICATIONS.md", Guide, 2100),
    ("docs/STRUCTURED_MEMORY.md", Guide, 3600),
    ("docs/TROUBLESHOOTING.md", Guide, 1500),
    ("docs/USER_MANUAL.md", Guide, 2300),
    ("docs/WEB.md", Guide, 1100),
    ("docs/guides/mlx-qlora-finetune.md", Guide, 800),
    ("docs/memory/ISSUE_87_CLOSEOUT.md", Evidence, 700),
    ("docs/parity/CONTRACTS.md", Guide, 700),
    ("docs/parity/STATUS.md", Guide, 1300),
    ("docs/parity/WORK.md", Guide, 500),
    ("docs/screenshots/README.md", Evidence, 200),
    ("docs/screenshots/analytics.md", Evidence, 400),
    ("docs/screenshots/issue-102-deliveries.md", Evidence, 300),
    ("docs/screenshots/issue-102-final-acceptance.md", Evidence, 500),
    ("docs/screenshots/issue-102-gateway.md", Evidence, 300),
    ("docs/screenshots/issue-102-ownership.md", Evidence, 300),
    ("docs/screenshots/issue-102-scheduling.md", Evidence, 200),
    ("docs/screenshots/issue-102.md", Evidence, 300),
    ("docs/screenshots/session-tokens-verification.md", Evidence, 1000),
    ("docs/screenshots/web-research-help-verification.md", Evidence, 1600),
    ("finetune/README.md", Guide, 600),
    ("screenshots/slash-single-line/README.md", Evidence, 300),
    ("setup-guide.md", Root, 900),
];

/// Each group's cap started at its words rounded up to the next 500.
const DOCS_GROUP_CAPS: &[(Kind, usize)] = &[(Root, 8_500), (Guide, 55_000), (Evidence, 16_500), (Agent, 24_000)];

/// A group cap this far above its words fails too, so a deletion locks in
/// instead of leaving room to regrow.
const DOCS_GROUP_SLACK: usize = 1_000;

/// Already under a root `screenshots/`; captures belong in `docs/screenshots/`.
/// Nothing may join these, and the list only shrinks.
const ROOT_SCREENSHOTS: &[&str] = &[
    "screenshots/slash-single-line/....txt",
    "screenshots/slash-single-line/01-native-command-guide.png",
    "screenshots/slash-single-line/02-native-staged-request.png",
    "screenshots/slash-single-line/03-native-multiline-agent-refused.png",
    "screenshots/slash-single-line/04-native-exact-agent-cancel.png",
    "screenshots/slash-single-line/README.md",
];

/// PDFs already under `docs/`. Nothing may join these, and the list only shrinks.
const DOCS_PDFS: &[&str] = &[
    "docs/learning/AI for code complexity optimization A hybrid perspective.pdf",
    "docs/learning/Guide_to_AI_Software_Design_From_Con.pdf",
    "docs/learning/Intro-to-Rust-from-Python-PowerShell.pdf",
    "docs/learning/comprehensive-rust.pdf",
    "docs/learning/rust_book.pdf",
    "docs/learning/rust_cheat_sheet_a4.pdf",
];

/// Files over [`LARGE_FILE_BYTES`] already in the tree. A new one needs a row
/// and a `// why:`.
const LARGE_FILES: &[&str] = &[
    "desktop/icons/icon.png",
    "docs/learning/AI for code complexity optimization A hybrid perspective.pdf",
    "docs/learning/comprehensive-rust.pdf",
    "docs/learning/rust_book.pdf",
    "docs/learning/rust_cheat_sheet_a4.pdf",
];

const LARGE_FILE_BYTES: u64 = 1024 * 1024;

/// Words as the budget counts them: whitespace-separated runs, like `wc -w`.
fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".markdown", ".mdx"].iter().any(|ext| lower.ends_with(ext))
}

/// Every docs-budget violation, given `(path, words)` for each Markdown file
/// that needs a row.
fn docs_budget_violations(
    markdown: &[(String, usize)],
    budget: &[(&str, Kind, usize)],
    group_caps: &[(Kind, usize)],
) -> Vec<String> {
    let mut out = Vec::new();
    for pair in budget.windows(2) {
        if pair[0].0 >= pair[1].0 {
            out.push(format!(
                "DOCS_BUDGET must stay sorted and unique: {} follows {}",
                pair[1].0, pair[0].0
            ));
        }
    }
    let mut totals: Vec<(Kind, usize)> = group_caps.iter().map(|&(kind, _)| (kind, 0)).collect();
    for (path, words) in markdown {
        let Some(&(_, kind, cap)) = budget.iter().find(|row| row.0 == path) else {
            out.push(format!(
                "{path} ({words} words) has no DOCS_BUDGET row. Edit the section that owns this topic \
                 instead of adding a file; a new doc needs the operator's approval, a row and a `// why:`."
            ));
            continue;
        };
        if *words > cap {
            out.push(format!(
                "{path} has {words} words, over its cap of {cap}. Cut or link rather than restate; \
                 raising the cap needs a `// why:`."
            ));
        }
        match totals.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, total)) => *total += words,
            None => out.push(format!("{path}: {kind:?} has no DOCS_GROUP_CAPS entry")),
        }
    }
    for (path, _, _) in budget {
        if !markdown.iter().any(|(p, _)| p == path) {
            out.push(format!(
                "DOCS_BUDGET lists {path}, which does not exist; delete its row."
            ));
        }
    }
    for (&(kind, cap), &(_, total)) in group_caps.iter().zip(&totals) {
        if total > cap {
            out.push(format!(
                "{kind:?} docs total {total} words, over the group cap of {cap}. Cut or link rather than \
                 restate; raising the cap needs a `// why:`."
            ));
        } else if cap - total >= DOCS_GROUP_SLACK {
            out.push(format!(
                "{kind:?} docs total {total} words, {} under the group cap of {cap}; lower it to {} so the \
                 cut stays cut.",
                cap - total,
                total.div_ceil(500) * 500
            ));
        }
    }
    out
}

/// Every tree-rule violation, given `(path, bytes)` for each file in the checkout.
fn tree_violations(
    files: &[(String, u64)],
    root_screenshots: &[&str],
    docs_pdfs: &[&str],
    large_files: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    for (path, bytes) in files {
        let lower = path.to_ascii_lowercase();
        if lower.starts_with("screenshots/") && !root_screenshots.contains(&path.as_str()) {
            out.push(format!(
                "{path} is under a root screenshots/; captures go in docs/screenshots/, notes in the PR body."
            ));
        }
        if lower.starts_with("docs/") && lower.ends_with(".pdf") && !docs_pdfs.contains(&path.as_str()) {
            out.push(format!(
                "{path} is a PDF under docs/; link to it instead of committing it."
            ));
        }
        if *bytes > LARGE_FILE_BYTES && !large_files.contains(&path.as_str()) {
            out.push(format!(
                "{path} is {bytes} bytes, over 1 MiB; a large file needs a LARGE_FILES row and a `// why:`."
            ));
        }
    }
    for (name, list) in [
        ("ROOT_SCREENSHOTS", root_screenshots),
        ("DOCS_PDFS", docs_pdfs),
        ("LARGE_FILES", large_files),
    ] {
        for listed in list {
            if !files.iter().any(|(p, _)| p == listed) {
                out.push(format!(
                    "{name} lists {listed}, which does not exist; delete the entry."
                ));
            }
        }
    }
    out
}

/// Every file git would see in the checkout: tracked plus untracked, minus
/// ignored. An agent's uncommitted scratch `.md` counts.
fn checkout_files(manifest: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .current_dir(manifest)
        .output()
        .unwrap_or_else(|e| panic!("the docs budget lists the checkout with git: {e}"));
    assert!(
        out.status.success(),
        "git ls-files failed ({}), so the docs budget cannot see the checkout: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        // A tracked file deleted from the working tree is gone for this check too.
        .filter(|p| manifest.join(p).is_file())
        .collect();
    files.sort();
    files.dedup();
    assert!(
        files.iter().any(|p| p == "AGENTS.md"),
        "git listed no AGENTS.md under {}",
        manifest.display()
    );
    files
}

/// Markdown the binary compiles in with `include_str!` (styles, the default
/// soul, bundled skills): product assets rather than docs, so they need no row.
fn runtime_markdown(manifest: &Path) -> Vec<String> {
    let checkout = std::fs::canonicalize(manifest).unwrap();
    let mut out = Vec::new();
    for (path, text) in read_tree_raw(&manifest.join("src")) {
        let dir = Path::new(&path).parent().unwrap();
        for chunk in text.split("include_str!(\"").skip(1) {
            let literal = chunk.split('"').next().unwrap_or_default();
            if !is_markdown(literal) {
                continue;
            }
            let target = std::fs::canonicalize(dir.join(literal))
                .unwrap_or_else(|e| panic!("{path}: include_str!({literal:?}) does not resolve: {e}"));
            let relative = target
                .strip_prefix(&checkout)
                .unwrap_or_else(|_| panic!("{path}: include_str!({literal:?}) is outside the checkout"));
            let parts: Vec<_> = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect();
            out.push(parts.join("/"));
        }
    }
    out
}

#[test]
fn markdown_stays_on_the_docs_budget() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = checkout_files(manifest);
    let runtime = runtime_markdown(manifest);
    assert!(
        runtime.iter().any(|p| p == "assets/soul.default.md"),
        "include_str! scan found no runtime Markdown: {runtime:?}"
    );
    let markdown: Vec<(String, usize)> = files
        .iter()
        .filter(|p| is_markdown(p) && !runtime.contains(p))
        .map(|p| {
            let bytes = std::fs::read(manifest.join(p)).unwrap();
            (p.clone(), word_count(&String::from_utf8_lossy(&bytes)))
        })
        .collect();
    let sized: Vec<(String, u64)> = files
        .iter()
        .map(|p| (p.clone(), std::fs::metadata(manifest.join(p)).unwrap().len()))
        .collect();
    let mut violations = docs_budget_violations(&markdown, DOCS_BUDGET, DOCS_GROUP_CAPS);
    violations.extend(tree_violations(&sized, ROOT_SCREENSHOTS, DOCS_PDFS, LARGE_FILES));
    assert!(
        violations.is_empty(),
        "docs budget (#213): edit the section that owns the topic.\n  {}",
        violations.join("\n  ")
    );
}

/// The rules pinned on a synthetic tree, so a refactor cannot quietly turn the
/// tripwire into a tautology.
#[test]
fn docs_budget_rejects_unlisted_oversized_and_misplaced_files() {
    fn expect(violations: Vec<String>, needle: &str) {
        assert!(
            violations.iter().any(|v| v.contains(needle)),
            "expected a violation containing {needle:?}, got {violations:#?}"
        );
    }
    let md = |files: &[(&str, usize)]| files.iter().map(|&(p, w)| (p.to_string(), w)).collect::<Vec<_>>();
    let budget: &[(&str, Kind, usize)] = &[("docs/A.md", Guide, 100), ("docs/B.md", Guide, 100)];
    let caps: &[(Kind, usize)] = &[(Guide, 150)];

    assert!(docs_budget_violations(&md(&[("docs/A.md", 100), ("docs/B.md", 50)]), budget, caps).is_empty());
    // A new file fails however small it is.
    expect(
        docs_budget_violations(
            &md(&[("docs/A.md", 1), ("docs/B.md", 1), ("docs/NOTES.md", 1)]),
            budget,
            caps,
        ),
        "docs/NOTES.md (1 words) has no DOCS_BUDGET row",
    );
    // So does growing past a file cap, a group cap, or deleting a file without its row.
    expect(
        docs_budget_violations(&md(&[("docs/A.md", 101), ("docs/B.md", 1)]), budget, caps),
        "docs/A.md has 101 words, over its cap of 100",
    );
    expect(
        docs_budget_violations(&md(&[("docs/A.md", 100), ("docs/B.md", 60)]), budget, caps),
        "Guide docs total 160 words, over the group cap of 150",
    );
    expect(
        docs_budget_violations(&md(&[("docs/A.md", 1)]), budget, caps),
        "DOCS_BUDGET lists docs/B.md, which does not exist",
    );
    // A cut locks in: a group cap left far above its words fails until lowered.
    expect(
        docs_budget_violations(&md(&[("docs/A.md", 10), ("docs/B.md", 10)]), budget, &[(Guide, 1_500)]),
        "lower it to 500",
    );
    expect(
        docs_budget_violations(
            &md(&[("docs/A.md", 1), ("docs/B.md", 1)]),
            &[budget[1], budget[0]],
            caps,
        ),
        "must stay sorted and unique",
    );
    assert!(is_markdown("docs/NOTES.MD") && is_markdown("x.markdown") && !is_markdown("notes.txt"));
    assert_eq!(word_count("one  two\n\tthree "), 3);

    let sized = |files: &[(&str, u64)]| files.iter().map(|&(p, b)| (p.to_string(), b)).collect::<Vec<_>>();
    expect(
        tree_violations(&sized(&[("screenshots/new.png", 10)]), &[], &[], &[]),
        "under a root screenshots/",
    );
    expect(
        tree_violations(&sized(&[("docs/learning/new.PDF", 10)]), &[], &[], &[]),
        "is a PDF under docs/",
    );
    expect(
        tree_violations(&sized(&[("assets/big.bin", LARGE_FILE_BYTES + 1)]), &[], &[], &[]),
        "over 1 MiB",
    );
    expect(
        tree_violations(&sized(&[]), &["screenshots/old.png"], &[], &[]),
        "ROOT_SCREENSHOTS lists screenshots/old.png",
    );
    let grandfathered = sized(&[("screenshots/old.png", 10), ("docs/old.pdf", LARGE_FILE_BYTES + 1)]);
    assert!(tree_violations(
        &grandfathered,
        &["screenshots/old.png"],
        &["docs/old.pdf"],
        &["docs/old.pdf"]
    )
    .is_empty());
}
