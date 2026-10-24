//! Invariant 6 at the source level: the server side never references the
//! agentic module, only the shim spawns a child, the agentic side never
//! references the server or the shim, and the deliberately duplicated
//! constants still agree. Also the repository's docs budget (#213).

use std::path::Path;
use syn::ext::IdentExt;

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
            out.push(p.ident.unraw().to_string());
            use_tree_idents(&p.tree, out);
        }
        syn::UseTree::Name(n) => out.push(n.ident.unraw().to_string()),
        syn::UseTree::Rename(r) => {
            // The real module first: the alias is what a later reader sees, but
            // the left side is what actually crossed the boundary.
            out.push(r.ident.unraw().to_string());
            out.push(r.rename.unraw().to_string());
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
        let syntax = boundary_syntax(&path, &text);
        for name in forbidden {
            assert!(
                !idents.iter().chain(syntax.module_refs.iter()).any(|i| i == name),
                "{path} imports {name:?} across the I6 boundary \
                 (a renamed or grouped `use` still crosses it)"
            );
        }
        assert!(
            !syntax.includes_code,
            "{path}: include! bypasses the source boundary scan"
        );
        let relative = Path::new(&path)
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for target in syntax.module_paths {
            assert!(
                matches!(
                    (relative.as_str(), target.as_str()),
                    ("common/auth_store.rs", "auth_sqlite.rs")
                        | ("common/credential_store.rs", "credential_file_writer.rs")
                ),
                "{path}: unregistered module path {target:?} bypasses the source boundary scan"
            );
        }
    }
}

#[derive(Default)]
struct BoundarySyntax {
    production_only: bool,
    paths: Vec<String>,
    module_refs: Vec<String>,
    module_paths: Vec<String>,
    includes_code: bool,
}

impl<'ast> syn::visit::Visit<'ast> for BoundarySyntax {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if self.production_only && test_only(&module.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, module);
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if self.production_only && test_only(&function.attrs) {
            return;
        }
        syn::visit::visit_item_fn(self, function);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        use_tree_idents(&item.tree, &mut self.paths);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.paths
            .extend(path.segments.iter().map(|s| s.ident.unraw().to_string()));
        self.module_refs.extend(
            path.segments
                .iter()
                .take(path.segments.len().saturating_sub(1))
                .map(|s| s.ident.unraw().to_string()),
        );
        syn::visit::visit_path(self, path);
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if path_is(attr.path(), "path") {
            let target = match &attr.meta {
                syn::Meta::NameValue(value) => match &value.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    }) => value.value(),
                    _ => "<nonliteral>".into(),
                },
                _ => "<invalid>".into(),
            };
            self.module_paths.push(target);
        } else if path_is(attr.path(), "cfg_attr") {
            if let syn::Meta::List(list) = &attr.meta {
                if list
                    .tokens
                    .to_string()
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|word| word == "path")
                {
                    self.module_paths.push("<conditional module path>".into());
                }
            }
        }
        syn::visit::visit_attribute(self, attr);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.includes_code |= node.path.segments.last().is_some_and(|s| s.ident.unraw() == "include");
        let tokens = syn::buffer::TokenBuffer::new2(node.tokens.clone());
        self.macro_tokens(tokens.begin());
        syn::visit::visit_macro(self, node);
    }
}

impl BoundarySyntax {
    // syn deliberately does not parse macro bodies. Inspect token trees as code,
    // excluding literal content (including doc text and include_str!/bytes! data).
    fn macro_tokens(&mut self, mut cursor: syn::buffer::Cursor<'_>) {
        while !cursor.eof() {
            if let Some((ident, next)) = cursor.ident() {
                let name = ident.unraw().to_string();
                self.paths.push(name.clone());
                self.module_refs.push(name.clone());
                self.includes_code |= name == "include" && next.punct().is_some_and(|(p, _)| p.as_char() == '!');
                cursor = next;
            } else if let Some((punct, next)) = cursor.punct() {
                if punct.as_char() == '#' {
                    if let Some((body, _, _, _)) = next.any_group() {
                        if let Ok(meta) = syn::parse2::<syn::Meta>(body.token_stream()) {
                            if path_is(meta.path(), "path")
                                || path_is(meta.path(), "cfg_attr")
                                    && body
                                        .token_stream()
                                        .to_string()
                                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                                        .any(|word| word == "path")
                            {
                                self.module_paths.push("<macro module path>".into());
                            }
                        }
                    }
                }
                cursor = next;
            } else if let Some((body, _, _, next)) = cursor.any_group() {
                self.macro_tokens(body);
                cursor = next;
            } else {
                cursor = cursor.token_tree().expect("nonempty token cursor").1;
            }
        }
    }
}

fn path_is(path: &syn::Path, name: &str) -> bool {
    path.get_ident().is_some_and(|ident| ident.unraw() == name)
}

fn boundary_syntax(path: &str, text: &str) -> BoundarySyntax {
    scan_syntax(path, text, false)
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || attr.path().is_ident("cfg") && attr.parse_args::<syn::Path>().is_ok_and(|path| path.is_ident("test"))
    })
}

fn scan_syntax(path: &str, text: &str, production_only: bool) -> BoundarySyntax {
    let file = syn::parse_file(text).unwrap_or_else(|error| panic!("{path} does not parse: {error}"));
    let mut visitor = BoundarySyntax {
        production_only,
        ..Default::default()
    };
    syn::visit::visit_file(&mut visitor, &file);
    visitor
}

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn server_side_never_references_agentic() {
    for sub in ["server", "shim", "llm", "common", "netconnect"] {
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
fn diag_reads_only_named_files_under_home_logs() {
    // One reader opens files, and only `<home>/logs/<allowlisted name>`; the
    // routes and prompt code never touch the filesystem or spawn anything.
    let reader = strip_comment_lines(&std::fs::read_to_string(root().join("server/diag_reader.rs")).unwrap());
    let code = &reader[..reader.find("#[cfg(test)]").expect("reader tests")];
    assert_eq!(code.matches(".open(").count(), 1, "diag_reader opens one file kind");
    assert!(
        code.contains(".open(logs_dir.join(name))"),
        "the path is logs_dir plus the allowlisted name"
    );
    assert!(
        code.contains("libc::O_NOFOLLOW"),
        "the leaf is opened without following links"
    );
    assert!(
        code.contains("file.take(max_bytes)"),
        "reads stay bounded after the metadata check"
    );
    let scan = code.find("scan_normalized").expect("output is scanned");
    let redact = code.find("redactors.redact(").expect("output is redacted");
    assert!(redact < scan, "redaction precedes the scanner");
    for forbidden in [
        "read_to_string",
        "File::open",
        "read_dir",
        "Command",
        "TcpStream",
        "canonicalize",
        "home.root",
    ] {
        assert!(!code.contains(forbidden), "diag_reader must not use {forbidden}");
    }
    let routes = strip_comment_lines(&std::fs::read_to_string(root().join("server/routes/diag.rs")).unwrap());
    for forbidden in ["std::fs", "File::", "OpenOptions", "Command", "Path::new", "PathBuf"] {
        assert!(!routes.contains(forbidden), "routes/diag.rs must not use {forbidden}");
    }
    assert!(
        routes.contains("state.diag.config.allowed(&file)"),
        "the file parameter is matched against the allowlist"
    );
    let paths = cgagentharness::server::routes::REGISTERED_PATHS;
    for path in ["/api/diag", "/api/diag/read", "/api/diag/inject", "/api/diag/forget"] {
        assert!(paths.contains(&path), "{path} must be in REGISTERED_PATHS");
    }
    let reload = std::fs::read_to_string(root().join("server/config_reload.rs")).unwrap();
    assert!(!reload.contains("\"diagnostics"), "diagnostics stays restart-only");
}

#[test]
fn self_knowledge_reads_no_file_and_never_reaches_agent_runs() {
    // The manual is compiled in; the live block renders values the caller holds.
    for file in ["server/self_manual.rs", "server/live_state.rs"] {
        let text = strip_comment_lines(&std::fs::read_to_string(root().join(file)).unwrap());
        let code = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
        for forbidden in [
            "std::fs",
            "File::",
            "OpenOptions",
            "read_to_string",
            "Command",
            "TcpStream",
            "crate::agentic",
            "create_in_dir",
            "open_in_dir",
            "MmapDirectory",
        ] {
            assert!(!code.contains(forbidden), "{file} must not use {forbidden}");
        }
    }
    let manual = std::fs::read_to_string(root().join("server/self_manual.rs")).unwrap();
    assert!(manual.contains("include_str!(\"../../assets/self/MANUAL.md\")"));
    assert!(
        manual.contains("Index::create_in_ram"),
        "the manual index is never persisted"
    );
    let agent = std::fs::read_to_string(root().join("server/routes/agent.rs")).unwrap();
    for needle in ["self_knowledge", "live_state", "self_manual"] {
        assert!(!agent.contains(needle), "/agent must not carry {needle}");
    }
    let reload = std::fs::read_to_string(root().join("server/config_reload.rs")).unwrap();
    assert!(!reload.contains("\"diagnostics"), "diagnostics stays restart-only");
}

#[test]
fn netconnect_dials_only_through_the_budgeted_dialer() {
    // Sockets and name lookups. Only connect.rs may open a socket, and
    // nothing under src/netconnect may resolve a host name.
    const SOCKETS: &[&str] = &["TcpStream", "TcpSocket", "UdpSocket", "socket2", "RawSocket"];
    const LOOKUPS: &[&str] = &["lookup_host", "ToSocketAddrs", "to_socket_addrs", "getaddrinfo"];
    for (path, text) in read_tree(&root().join("netconnect")) {
        let dialer = path.replace('\\', "/").ends_with("netconnect/connect.rs");
        for needle in LOOKUPS {
            assert!(!text.contains(needle), "{path} resolves names via {needle:?}");
        }
        if !dialer {
            for needle in SOCKETS {
                assert!(
                    !text.contains(needle),
                    "{path} opens a socket via {needle:?}; dial through netconnect::Dialer"
                );
            }
        }
    }
    let dialer = std::fs::read_to_string(root().join("netconnect").join("connect.rs")).unwrap();
    assert!(!dialer.contains("UdpSocket"), "the diagnostics dialer is TCP only");
    let body = &dialer[dialer.find("pub async fn probe_all").expect("probe_all exists")..];
    let recheck = body.find("check_target").expect("probe_all rechecks scope");
    let dial = body.find("self.connector.connect").expect("probe_all dials");
    assert!(recheck < dial, "scope recheck must precede the dial");
    assert!(
        dialer.contains("SocketAddr::V4(addr)"),
        "the live dial takes an IPv4 socket address"
    );
}

#[test]
fn only_registered_process_owners_construct_commands() {
    const OWNERS: &[&str] = &[
        "shim/mod.rs",
        "common/process.rs",
        "common/mcp.rs",
        "common/mcp_worker.rs",
        "common/windows_job.rs",
    ];
    for sub in ["server", "shim", "llm", "common", "netconnect"] {
        for (path, text) in read_tree_raw(&root().join(sub)) {
            let relative = Path::new(&path)
                .strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // Native test probes may launch fixture tools; the production
            // process owners remain the explicit list above.
            let syntax = scan_syntax(&path, &text, true);
            if syntax.paths.iter().any(|ident| ident == "Command") {
                assert!(
                    OWNERS.contains(&relative.as_str()),
                    "{path}: process Command outside registered owners"
                );
            }
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
fn direct_paths_module_loading_and_command_aliases_are_scanned() {
    let fixture = "#[cfg(test)] mod probe { use std::process::Command; fn f() { Command::new(\"x\"); } }";
    assert!(!scan_syntax("test fixture", fixture, true)
        .paths
        .iter()
        .any(|name| name == "Command"));
    assert!(scan_syntax("production", &fixture.replace("#[cfg(test)]", ""), true)
        .paths
        .iter()
        .any(|name| name == "Command"));
    for code in [
        "use crate :: r#agentic as pipeline;",
        "use crate::{r#agentic as pipeline};",
    ] {
        assert!(use_idents("raw import", code).iter().any(|name| name == "agentic"));
    }
    for code in [
        "fn f() { crate :: r#agentic :: run(); }",
        "fn f() { dbg!(crate :: agentic :: run()); }",
    ] {
        assert!(boundary_syntax("path", code)
            .module_refs
            .iter()
            .any(|name| name == "agentic"));
    }
    assert!(
        boundary_syntax(
            "macro include",
            r#"macro_rules! load { () => { include!("outside.rs"); } }"#
        )
        .includes_code
    );
    assert!(
        !boundary_syntax(
            "macro data",
            r#"macro_rules! data { () => { include_str!("outside.rs") } }"#
        )
        .includes_code
    );
    assert!(!boundary_syntax(
        "macro path",
        r#"macro_rules! load { () => { #[path="outside.rs"] mod other; } }"#
    )
    .module_paths
    .is_empty());
    assert!(
        boundary_syntax("raw command", "fn f() { std :: process :: r#Command :: new(\"x\"); }")
            .paths
            .iter()
            .any(|name| name == "Command")
    );
    assert!(boundary_syntax(
        "macro command",
        "macro_rules! spawn { () => { std :: process :: Command :: new(\"x\") } }"
    )
    .paths
    .iter()
    .any(|name| name == "Command"));
    let direct = boundary_syntax("direct", "fn f() { crate :: agentic :: run(); }");
    assert!(direct.paths.iter().any(|name| name == "agentic"));
    let path = boundary_syntax("module", "#[path = \"../agentic/mod.rs\"] mod hidden;");
    assert_eq!(path.module_paths, ["../agentic/mod.rs"]);
    assert_eq!(
        boundary_syntax("raw attribute", "#[r#path = \"../agentic/mod.rs\"] mod hidden;").module_paths,
        ["../agentic/mod.rs"]
    );
    assert!(
        boundary_syntax(
            "conditional",
            "#[cfg_attr(unix, path = \"../agentic/mod.rs\")] mod hidden;"
        )
        .module_paths
        .len()
            == 1
    );
    assert!(boundary_syntax("include", "include!(concat!(\"../agentic/\", \"mod.rs\"));").includes_code);
    assert!(!boundary_syntax("data", "const X: &str = include_str!(\"readme.txt\");").includes_code);
    assert!(use_idents("alias", "use std::{process::Command as Child};")
        .iter()
        .any(|name| name == "Command"));
    assert!(
        boundary_syntax("constructor", "fn f() { std :: process :: Command :: new(\"x\"); }")
            .paths
            .iter()
            .any(|name| name == "Command")
    );
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
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_FALLBACK_PLANNER_SEC,
        cgagentharness::agentic::config::DEFAULT_PLANNER_TIMEOUT_SEC
    );
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_CHECK_SEC,
        cgagentharness::agentic::executor::DEFAULT_CHECK_TIMEOUT_SEC
    );
    // The server derives --planner-timeout-sec up to its ceiling; the child
    // must accept every value the server can send.
    assert_eq!(
        cgagentharness::server::model_limits::MAX_TIMEOUT_SEC,
        cgagentharness::agentic::commands::MAX_PLANNER_TIMEOUT_SEC
    );
    for (name, _) in cgagentharness::server::agent_policy::available_profiles() {
        let resolved =
            cgagentharness::server::agent_policy::resolve_check_profiles(std::slice::from_ref(&name)).unwrap();
        assert!(
            !resolved[0]["argv"].as_array().unwrap().is_empty(),
            "{name} has an empty argv"
        );
    }
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_DEFAULT_ITERATIONS,
        cgagentharness::agentic::commands::DEFAULT_MAX_ITERATIONS
    );
    // The shim whitelist is exactly the CLI's dispatch table (no deepagent-plan, no __sleep),
    // in both directions: an arm missing from ACTIONS would be unreachable through the shim.
    let commands = std::fs::read_to_string(root().join("agentic").join("commands.rs")).unwrap();
    let start = commands.find("pub fn dispatch(").expect("dispatch");
    let end = start + commands[start..].find("\n}\n").expect("dispatch end");
    let arm = regex::Regex::new(r#""([a-z][a-z-]*)" =>"#).unwrap();
    let mut arms: Vec<&str> = arm
        .captures_iter(&commands[start..end])
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    arms.sort_unstable();
    let mut actions = cgagentharness::shim::ACTIONS.to_vec();
    actions.sort_unstable();
    assert_eq!(arms, actions, "CLI dispatch arms and shim::ACTIONS differ");
    for action in cgagentharness::shim::JSON_ACTIONS {
        assert!(
            cgagentharness::shim::ACTIONS.contains(&action),
            "{action} is JSON but not an action"
        );
    }
    assert!(cgagentharness::shim::ACTIONS.contains(&"real-repo-runs"));
    assert!(!cgagentharness::shim::ACTIONS.contains(&"deepagent-plan"));
    assert!(!cgagentharness::shim::ACTIONS.contains(&"__sleep"));
}

/// `deny.toml` and `desktop/deny.toml` each carry a `[bans].deny` list of
/// telemetry and analytics SDK crates (the CI half of the otel-hardening
/// skill's T5 sweep, #297). cargo-deny matches exact names and stays silent
/// about a banned crate that is absent from the graph, so a name added to one
/// file and not the other, or misspelled in one, would never surface at check
/// time. Both lists must be the same entries in the same order.
#[test]
fn deny_lists_ban_the_same_telemetry_crates() {
    fn entries(name: &str) -> Vec<String> {
        // A Git for Windows checkout with core.autocrlf=true carries CRLF, and
        // no .gitattributes pins these files to LF; the table split below must
        // not depend on the line ending (Codex P2 on #315).
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name))
            .unwrap()
            .replace("\r\n", "\n");
        let bans = text
            .split("\n[bans]\n")
            .nth(1)
            .unwrap_or_else(|| panic!("{name} has no [bans] table"));
        let bans = bans.split("\n[").next().unwrap();
        let list = bans
            .split_once("\ndeny = [")
            .unwrap_or_else(|| panic!("{name} [bans] has no deny list"))
            .1;
        let list = list.split("\n]").next().unwrap();
        list.lines()
            .map(|line| line.split('#').next().unwrap().trim().trim_end_matches(','))
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }
    let backend = entries("deny.toml");
    let desktop = entries("desktop/deny.toml");
    assert!(
        backend.iter().any(|e| e.contains("crate = \"opentelemetry\"")),
        "deny.toml [bans].deny lost the opentelemetry entry: {backend:?}"
    );
    let mut seen = std::collections::HashSet::new();
    for entry in &backend {
        assert!(seen.insert(entry), "deny.toml [bans].deny lists {entry} twice");
    }
    assert_eq!(
        backend, desktop,
        "deny.toml and desktop/deny.toml [bans].deny lists differ; add or change a crate in both files"
    );
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
        "netconnect.enabled",
        "netconnect.passive_listen",
        "netconnect.discovery",
        "netconnect.port_scan",
        "netconnect.diagnostics",
        "netconnect.throughput",
        "netconnect.anomaly_detection",
        "netconnect.home_automation",
        "notifications.enabled",
        "agentic.deepagent_github.retrieval.enabled",
        "diagnostics.logs_enabled",
        "diagnostics.model_tool",
    ] {
        assert!(!cfg.flag_is_true(gate), "{gate} must ship false");
    }
    // Context about the harness itself ships on as literal booleans; it grants no tool.
    let knowledge = cgagentharness::server::self_manual::SelfKnowledgeConfig::from_config(&cfg).unwrap();
    assert!(knowledge.self_knowledge && knowledge.live_state);
    for key in ["diagnostics.self_knowledge", "diagnostics.live_state"] {
        assert!(cfg.flag_is_true(key), "{key} ships as the literal boolean true");
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
    // D7 (#603): automatic suggestions ship restricted to listed local models;
    // the "*" opt-out is an operator choice, never a shipped default.
    let supported = cfg
        .get("structured_memory.suggestion_supported")
        .and_then(|v| v.as_sequence())
        .expect("structured_memory.suggestion_supported ships as a list");
    assert!(!supported.is_empty(), "the shipped local model is listed");
    for entry in supported {
        let model = entry.get("model").and_then(|m| m.as_str()).unwrap_or("");
        assert!(
            !model.is_empty() && model.trim() != "*",
            "shipped entries name one model: {entry:?}"
        );
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
    for needle in [
        "api_key_optional: true",
        "allow_git_write_tools: false",
        "allow_plaintext_key_file: false",
    ] {
        assert!(yaml.contains(needle), "shipped config lost {needle}");
    }
    assert!(
        !yaml.contains("allow_plaintext_key_file: true"),
        "allow_plaintext_key_file must ship false"
    );
    assert!(
        !cfg.flag_is_true("security.allow_plaintext_key_file"),
        "plaintext key file must ship false"
    );
    assert!(
        !cgagentharness::server::config_reload::RELOADABLE.contains(&"security.allow_plaintext_key_file"),
        "plaintext key file opt-in is restart-only"
    );
    assert!(
        !yaml.contains("allow_git_write_tools: true"),
        "allow_git_write_tools must ship false"
    );
    assert!(
        cfg.flag_is_true("security.api_key_optional"),
        "direct local use must ship without a key requirement"
    );
    assert!(
        cfg.str_list("netconnect.allowed_cidrs").is_empty(),
        "netconnect scope must ship empty"
    );
    let netconnect = cgagentharness::netconnect::NetconnectConfig::from_config(&cfg).unwrap();
    assert!(netconnect.scope.is_empty());
    assert!(netconnect.warnings.is_empty());
    assert!(netconnect.throughput_endpoint.is_none());
    for tier in cgagentharness::netconnect::Tier::ALL {
        assert!(!netconnect.tier_enabled(tier));
        assert!(!netconnect.tier_may_run(tier));
    }
}

#[test]
fn shipped_defaults_protect_agent_instruction_files() {
    // Both files are loaded automatically by agents, so a coding run must
    // never be able to rewrite them.
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    for file in ["AGENTS.md", "CLAUDE.md"] {
        assert!(
            yaml.contains(&format!("- \"{file}\"")),
            "shipped config.default.yaml must list {file} in protected_write_paths"
        );
        assert!(
            cgagentharness::agentic::config::DEFAULT_PROTECTED_WRITE_PATH_PREFIXES.contains(&file),
            "Rust DEFAULT_PROTECTED_WRITE_PATH_PREFIXES must protect {file}"
        );
    }
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
fn agents_md_is_the_manual_and_claude_md_its_root_summary() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = include_str!("../README.md");
    assert!(
        !readme.contains("(CLAUDE.md)"),
        "README must not link to CLAUDE.md; operators read AGENTS.md"
    );
    assert!(
        readme.contains("(AGENTS.md)"),
        "README must point operators at AGENTS.md"
    );
    assert!(manifest.join("AGENTS.md").is_file(), "AGENTS.md must exist");
    // The per-session summary lives at the root, where Claude Code loads it once.
    let claude = include_str!("../CLAUDE.md");
    assert!(
        claude.contains("`AGENTS.md`"),
        "root CLAUDE.md must defer to AGENTS.md as the full manual"
    );
    assert!(
        !manifest.join(".claude/CLAUDE.md").exists(),
        "CLAUDE.md moved to the repository root; do not resurrect the .claude/ copy"
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
    /// What agents load: `AGENTS.md`, `CLAUDE.md`, `.claude/`, `.codex/`, `.github/`.
    Agent,
}

use Kind::{Agent, Evidence, Guide, Root};

/// `(path, kind, word cap)`, sorted by path. Caps started at each file's size
/// on 2026-09-27, rounded up to the next 100 words. Lower them when a fold lands.
// why: operator-approved (2026-10-18) +200 words on every row that had under 60 words of headroom on main (45 rows).
const DOCS_BUDGET: &[(&str, Kind, usize)] = &[
    // Rows the 2026-10-13 doc-sync trimmed carry their new size plus about 2%, rounded up to the next 10.
    // why: operator-requested per-file index of the folder (one line per file); size plus about 10%, rounded up to the next 10.
    (".claude/skills/README.md", Agent, 440),
    (".claude/skills/cgagentharness-config-guard/SKILL.md", Agent, 900),
    (".claude/skills/cgagentharness-gotchas/SKILL.md", Agent, 1700),
    (".claude/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 600),
    // why: operator-approved raise from 1400 to 1776 for the optimize skill rewrite. The file is 1277 words, so
    // 499 are unused headroom; the Agent group cap, not this row, is the binding limit.
    (".claude/skills/cgagentharness-optimize/SKILL.md", Agent, 1776),
    // why: port of CyClaw's otel-hardening skill (telemetry-kill contract for a Rust tree); 1241 words, rounded up to the next 100.
    (".claude/skills/cgagentharness-otel-hardening/SKILL.md", Agent, 1500),
    (".claude/skills/cgagentharness-project-guidance/SKILL.md", Agent, 910),
    (
        ".claude/skills/cgagentharness-runtime-invariant-check/SKILL.md",
        Agent,
        500,
    ),
    (
        ".claude/skills/cgagentharness-write-policy-redteam/SKILL.md",
        Agent,
        1000,
    ),
    (".claude/skills/dep-sync/SKILL.md", Agent, 970),
    (".claude/skills/doc-sync/SKILL.md", Agent, 800),
    (".claude/skills/fable-protocol/SKILL.md", Agent, 990),
    (".claude/skills/run-cg-agent-harness/SKILL.md", Agent, 500),
    (".claude/skills/verification-specialist/SKILL.md", Agent, 740),
    (".codex/skills/cgagentharness-config-guard/SKILL.md", Agent, 500),
    (".codex/skills/cgagentharness-gotchas/SKILL.md", Agent, 700),
    (".codex/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 550),
    (".codex/skills/cgagentharness-optimize/SKILL.md", Agent, 350),
    (".codex/skills/cgagentharness-project-guidance/SKILL.md", Agent, 350),
    (".codex/skills/cgagentharness-release/SKILL.md", Agent, 400),
    (".codex/skills/cgagentharness-verify/SKILL.md", Agent, 300),
    (".codex/skills/cgagentharness-write-policy-redteam/SKILL.md", Agent, 700),
    (".codex/skills/fable-protocol/SKILL.md", Agent, 400),
    (".codex/skills/verification-specialist/SKILL.md", Agent, 600),
    // why: operator-approved (2026-10-14) raise to 2200 after the change-type, merge-order and ELI5 guidance grew it to 2162 words.
    (".github/PULL_REQUEST_TEMPLATE.md", Agent, 2400),
    (".github/skills/repo-optimize/SKILL.md", Agent, 400),
    // why: operator-requested per-file index of the folder (one line per file); size plus about 10%, rounded up to the next 10.
    (".github/workflows/README.md", Agent, 460),
    // why: netconnect passive CLI and LAN scope rule; rounded up to the next 100.
    // why: OS credential store sentence for managed provider keys; 1907 words after the main merge, rounded up to the next 100.
    // why: feature contracts folded into an INVARIANTS.md section table; 1491 words, rounded up to the next 100.
    // why: operator-approved raise (1508 words after the project-skills heading edit on main); set to 1510.
    // why: operator-approved headroom for research-job steps 8-12 (#415 review roadmap): ~45 words
    // (gate list, /loop excludes MCP, no job-long gate), plus ~10%, rounded up to the next 50.
    // why: the 2026-10-13 doc-sync cut it to 1441 words; 1490 keeps the ~50-word roadmap allowance above.
    // why: #550/#552 self-manual and red-team guidance grew it to 1459; standing rule is to raise the cap to the reported count, never trim gate text.
    ("AGENTS.md", Agent, 1459),
    // why: the per-session summary moved here from .claude/CLAUDE.md so Claude Code loads one file.
    // why: now `@AGENTS.md` plus Claude-only lines; 183 words, rounded up to the next 100.
    ("CLAUDE.md", Agent, 400),
    // why: #237 and #243 added the cloud-truncation and web-budget contracts;
    // #235 the process-global CSRF note and the I6 process map.
    // why: netconnect fail-closed LAN scope section; 5683 words, rounded up to the next 100.
    // why: read-only tool, slash, and panel rules; 5790 words, rounded up to the next 100.
    // why: credential and preemption contracts matched to runtime, then the OS credential store section; 5997 words, rounded up to the next 100.
    // why: caveat that loaded keys still reach gh and the shim child until #286; set to the reported 6026 words.
    // why: redaction claim narrowed to startup-loaded key values (#313); set to the reported
    // 6027 words after merging main 17b28d9. Whichever of #312/#313 lands second resets it exactly.
    // why: operator-requested telemetry and dependency-posture section; set to the reported 6094 words.
    // why: operator-approved headroom for research-job steps 8-12 (#415 review roadmap): ~310 words
    // (job runtime ~120, unit chain ~90, report unit ~50, /research slash rules ~35, unslop note ~15),
    // plus ~10%, rounded up to the next 50. Lower it to the reported size once those land.
    // why: operator-approved +100 (2026-10-13) for the goal web-mode default contract; file was at 6437.
    // why: operator-approved +150 (2026-10-18), shared by the four compaction follow-up PRs (#513 fixes, hard limit, reply parity, window trigger).
    // why: owner-approved (2026-10-18) cap set to the measured count with #546 + #551 together; never trim gate text to fit.
    // why: operator-approved (2026-10-19) measured count with #546 and #550 together, raised instead of trimming.
    // why: operator-set (2026-10-23) standing allowance of 10,000, not a measured count: the file was 7,099 words; the
    // Root group cap below is the binding limit, so new contracts fit without trimming gate text.
    ("INVARIANTS.md", Root, 10_000),
    // why: README refresh folded Origins into the intro and trimmed duplicates; 1443 words, rounded up to the next 100.
    // why: operator-approved (2026-10-14) macOS installer and launcher pointer in Quick start; 1489 words, rounded up to the next 50.
    // why: dependency refresh removed five stale Linux prerequisite words; keep the cut.
    ("README.md", Root, 1689),
    ("SECURITY.md", Root, 690),
    // why: operator-approved +100 (2026-10-13) for the goal web-mode default row; file was at 1728.
    ("docs/API_ROUTES.md", Guide, 1830),
    // why: #240 folded BOUNDED_EDITS.md, GIT_APPROVAL.md and OFFLINE_CARGO.md in here.
    ("docs/CODING_PIPELINE.md", Guide, 3710),
    // why: operator-approved (2026-10-23) measured count for the public Google search pace floor, merged with the #649 cooldown keys.
    ("docs/CONFIG_RELOAD.md", Guide, 608),
    // why: #246 folded CHAT_WORKFLOWS.md and CHAT_STREAMING.md in here.
    // why: read-only /net panel and slash row; 6605 words, rounded up to the next 100.
    // why: operator-approved +150 (2026-10-18), shared by the same four compaction follow-up PRs.
    // why: operator-approved (2026-10-19) measured count with #546 and #550 together, raised instead of trimming.
    ("docs/CONSOLE.md", Guide, 6187),
    // why: dependency refresh removed 72 words of retired Tantivy and Reqwest constraints; keep the cut with prior headroom.
    // why: the 2026-10-20 dep-sync folded the dated 2026-09 audit notes into Retained constraints; 944 words, keep the cut.
    // why: operator-approved +200 (2026-10-22) so dated advisory notes such as RUSTSEC-2026-0384 fit without trimming history.
    ("docs/DEPENDENCIES.md", Guide, 1150),
    // why: OS credential store replaces the dotenv-only startup note; 2839 words, rounded up to the next 100.
    ("docs/DESKTOP.md", Guide, 2655),
    // why: operator-approved explainer for the agent-neutral .githooks security gate; 1364 words, rounded up to the next 100.
    // why: hook behavior review trims duplicated reuse instructions; retain the smaller budget.
    ("docs/GITHOOKS.md", Guide, 1400),
    // why: credentials table now describes the OS store and the plaintext opt-in; 5571 words, rounded up to the next 100.
    // Setup memory guidance replaces repeated setup/verification prose; keep the cut.
    // why: operator-approved (2026-10-14) automated macOS install (section 1) and browser access (section 7); 5188 words plus about 1%, rounded up to the next 50.
    // why: operator-approved (2026-10-14) raise to 7500 for room to grow the install guide after #491 and #496 together hit 5250; the Guide group cap still bounds it.
    ("docs/INSTALL.md", Guide, 7474),
    ("docs/MCP_CLIENT.md", Guide, 1090),
    ("docs/MCP_SERVER.md", Guide, 1230),
    // why: #244 folded MEMORY_SETUP.md and the user manual's memory sections in here.
    ("docs/MEMORY_GUIDE.md", Guide, 1950),
    // why: #237 documented refused truncated cloud replies.
    ("docs/MODELS.md", Guide, 1163),
    ("docs/PROCESS_LIFECYCLE.md", Guide, 1360),
    // why: operator-requested per-file index of docs/ (3-5 sentences each) replaces the 150-word folder stub; 1103 words, rounded up to the next 100.
    // why: operator-requested reading paths plus an owning-code and tests line per doc; size plus about 5%, rounded up to the next 50.
    // why: README audit removes 44 words; retain the existing headroom.
    ("docs/README.md", Guide, 1656),
    ("docs/RELEASING.md", Guide, 900),
    // why: #245 folded WEB.md and ACCOUNTS.md in here.
    // why: API Keys and SerpAPI now name the OS credential store; 4014 words, rounded up to the next 100.
    // why: operator-approved (2026-10-24) raise to 4000 so #649 and #651 fit together without trimming contract text.
    ("docs/SECURE_RESEARCH.md", Guide, 4000),
    // why: operator-approved (2026-10-14) fold of ANALYTICS.md in here; 2157 words, cap set to 2750 for room to grow.
    ("docs/SPEND_AND_NOTIFICATIONS.md", Guide, 2750),
    // why: #244 made this the one memory contract: a gate table, and the #87
    // closeout facts it lacked.
    ("docs/STRUCTURED_MEMORY.md", Guide, 3640),
    // why: operator-approved (2026-10-19) measured count with #546 and #550 together, raised instead of trimming.
    ("docs/TROUBLESHOOTING.md", Guide, 1555),
    // why: operator netconnect user guide; 3293 words after the doc-sync trim and the diag tier, rounded up to the next 100.
    ("docs/netconnect.md", Guide, 3491),
    ("docs/screenshots/README.md", Evidence, 387),
    // why: the seven folder-README rows below are operator-requested per-file indexes (one line per file);
    // each is its size plus about 10%, rounded up to the next 10.
    // why: operator-approved (2026-10-14) install-macos.sh and start-macos.sh lines, then the operator-requested
    // Terminal-walkthrough, user-guide and init.sh lines; 564 words plus about 2%, rounded up to the next 10.
    ("scripts/README.md", Guide, 780),
    ("src/agentic/README.md", Guide, 550),
    ("src/common/README.md", Guide, 600),
    // why: README audit removes three words; retain the existing headroom.
    ("src/netconnect/README.md", Guide, 387),
    ("src/server/README.md", Guide, 740),
    ("src/server/routes/README.md", Guide, 390),
    ("tests/README.md", Guide, 930),
];

/// Each group's cap started at its words rounded up to the next 500.
// why: #249 documents Spend under Analytics and the conditional Job webhooks button (46,536 words, rounded up to the next 500).
// why: PR template evidence guidance raised the Agent total to 22841, rounded up to the next 500; then
// trimming AGENTS.md and CLAUDE.md (moved to the root) brought it to 22186, rounded up to the next 500.
// why: netconnect LAN-scope bullet in AGENTS.md; Agent total 22541 words, rounded up to the next 500.
// why: read-only netconnect panel and slash rules; Root total 7506 words, rounded up to the next 500.
// why: #305 docs refresh plus its restored SECURE_RESEARCH and MCP_SERVER caveats, then #313's CODING_PIPELINE redaction wording; set to the reported Guide total of 45275 words.
// why: README refresh (#284) plus the OS credential store section and its #286 caveat; set to the reported Root total of 8145 words.
// why: console accessibility port (keyboard, zoom and live-log note; Attach files wording); Guide is 45340 words with the other open PRs (#353, #354, #359) and before the sign-in docs cut.
// why: the cgagentharness-otel-hardening skill brought Agent to 23,860 words; rounded up to the next 500.
// why: docs/GITHOOKS.md (.githooks security gate explainer, 1364 words) raised Guide from 45,311 to 46,675; set to 46,700.
// why: CONSOLE_JOBS, DESKTOP_ACCEPTANCE, FINETUNE and docs/parity left the tree and the folder
// READMEs arrived; set to the reported totals rounded up to the next 500 (Root 7921, Guide 43766,
// Evidence 139, Agent 22768).
// why: operator-approved raise from 8,000 to 8,200 for the INVARIANTS.md telemetry/dependency section and SECURITY.md posture bullets.
// why: operator-approved Root headroom matching the INVARIANTS.md row above (+356); rounded up to the next 500, under the slack rule.
// why: #432 CI/release trust docs put Guide at 44,006 words; operator-approved raise from 44,000 to 45,000 (under the slack rule).
// why: the docs/README.md per-file index put Guide at 45,059 words; operator-requested raise from 45,000 to 45,500 (under the slack rule).
// why: the 2026-10-13 doc-sync cut Guide to 44,132 words with that index; lowered to the next 500 so the cut stays cut.
// why: operator-requested per-file folder indexes (src, tests, scripts, desktop, assets, data) and the docs/README.md
// owning-code map put Guide at 47,658 and the .claude/skills and workflows indexes put Agent at 23,299; each rounded
// up to the next 500 under the slack rule.
// why: operator-approved (2026-10-14) macOS installer docs put Guide at 48,597 words; rounded up to the next 500.
// why: the skills review trimmed fable-protocol and gotchas; Agent is 21322 words, rounded up to the next 500.
// why: operator-approved (2026-10-14) Guide cap so docs/INSTALL.md can reach its 7,500-word row (Guide at 47,827 after
// ANALYTICS.md and USER_MANUAL.md were folded in; 64,500 keeps the gap under the 17,000 override); DOCS_GROUP_SLACK_OVERRIDES
// lets this one group sit that far above its words.
// why: operator-approved (2026-10-18) +200 Root words for the #494 first-run session contract in INVARIANTS.md.
// why: operator-approved (2026-10-18) +200 per raised DOCS_BUDGET row, added to its group (Root 3, Guide 21, Evidence 1, Agent 20).
// why: the Codex skill and AGENTS trim lowers Agent row caps by 3,590 words; retain that reduction in the group cap.
// why: operator-set (2026-10-23) Root cap of 10,000 with the INVARIANTS.md row; Root was at 9,216 words, so the
// 1,000-word DOCS_GROUP_SLACK still forces this cap down if the group shrinks by more than 216.
const DOCS_GROUP_CAPS: &[(Kind, usize)] = &[(Root, 10_000), (Guide, 68_700), (Evidence, 700), (Agent, 21_910)];

/// A group cap this far above its words fails too, so a deletion locks in
/// instead of leaving room to regrow.
const DOCS_GROUP_SLACK: usize = 1_000;

/// Groups the operator allowed to sit further above their words than
/// `DOCS_GROUP_SLACK`; every other group keeps the default.
// why: operator-approved (2026-10-14) Guide headroom for the 65,000 cap; 17,000 covers 64,500 - 47,827.
// why: operator-approved (2026-10-18) +200-per-row raise: Guide 19,500 covers 68,700 - 49,361 and Agent 4,500
// covers 25,500 - 21,466, each rounded up to the next 500.
const DOCS_GROUP_SLACK_OVERRIDES: &[(Kind, usize)] = &[(Guide, 19_500), (Agent, 4_500)];

/// The slack a group cap may keep above its words before it must be lowered.
fn group_slack(kind: Kind) -> usize {
    DOCS_GROUP_SLACK_OVERRIDES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(DOCS_GROUP_SLACK, |&(_, slack)| slack)
}

/// Word cap for a folder `README.md` with no `DOCS_BUDGET` row: an index of
/// what the folder holds and where the full doc lives, never a second copy.
/// A row, when present, wins over this fallback.
const FOLDER_README_WORDS: usize = 150;

/// The budget row a folder `README.md` gets when `DOCS_BUDGET` has none.
fn folder_readme_row(path: &str) -> Option<(Kind, usize)> {
    if !path.ends_with("/README.md") {
        return None;
    }
    let agent = [".claude/", ".codex/", ".github/"].iter().any(|p| path.starts_with(p));
    Some((if agent { Agent } else { Guide }, FOLDER_README_WORDS))
}

/// All historical root captures were relocated to `docs/screenshots/`.
/// Nothing may join this grandfather list.
const ROOT_SCREENSHOTS: &[&str] = &[];

/// PDFs already under `docs/`. Nothing may join these, and the list only shrinks.
const DOCS_PDFS: &[&str] = &[
    "docs/learning/AI for code complexity optimization A hybrid perspective.pdf",
    "docs/learning/Guide_to_AI_Software_Design_From_Con.pdf",
    "docs/learning/Intro-to-Rust-from-Python-PowerShell.pdf",
    "docs/learning/rust_cheat_sheet_a4.pdf",
];

/// Files over [`LARGE_FILE_BYTES`] already in the tree. A new one needs a row
/// and a `// why:`.
const LARGE_FILES: &[&str] = &[
    "desktop/icons/icon.png",
    "docs/learning/AI for code complexity optimization A hybrid perspective.pdf",
    "docs/learning/rust_cheat_sheet_a4.pdf",
];

const LARGE_FILE_BYTES: u64 = 1024 * 1024;

/// A demo video under `docs/screenshots/` may reach [`VIDEO_FILE_BYTES`]
/// without a `LARGE_FILES` row; every other file keeps the 1 MiB cap.
// why: the operator approved 8 MiB for screen-recorded demos (2026-10-15); a
// smooth 30 fps console recording runs several MB.
const VIDEO_FILE_BYTES: u64 = 8 * 1024 * 1024;
const VIDEO_EXTENSIONS: &[&str] = &[".mp4", ".webm", ".mov"];

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
        let row = budget
            .iter()
            .find(|row| row.0 == path)
            .map(|&(_, kind, cap)| (kind, cap))
            .or_else(|| folder_readme_row(path));
        let Some((kind, cap)) = row else {
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
        } else if cap - total >= group_slack(kind) {
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
        let video = lower.starts_with("docs/screenshots/") && VIDEO_EXTENSIONS.iter().any(|e| lower.ends_with(e));
        if video && *bytes > VIDEO_FILE_BYTES && !large_files.contains(&path.as_str()) {
            out.push(format!(
                "{path} is {bytes} bytes, over the 8 MiB demo-video cap; shorten or re-encode it."
            ));
        } else if !video && *bytes > LARGE_FILE_BYTES && !large_files.contains(&path.as_str()) {
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

/// Exact checkout paths exempt from the new-file rule and the group word cap.
/// Entries need owner approval. No wildcards and no environment switch.
/// An agent must not add a path.
const DOCS_BUDGET_OWNER_EXEMPT: &[(&str, &str)] = &[
    // why: owner approved 2026-10-10. Measured bake-off report from one ordered pass. It picks no winner.
    (
        "docs/bakeoff/local-model-bakeoff-2026-10-10.md",
        "owner approved 2026-10-10: measured bake-off report, one ordered pass, picks no winner",
    ),
];

#[test]
fn markdown_stays_on_the_docs_budget() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = checkout_files(manifest);
    let runtime = runtime_markdown(manifest);
    assert!(
        runtime.iter().any(|p| p == "assets/soul.default.md"),
        "include_str! scan found no runtime Markdown: {runtime:?}"
    );
    for (path, reason) in DOCS_BUDGET_OWNER_EXEMPT {
        assert!(
            !path.contains(['*', '?', '[']),
            "docs budget exempt {path} must be an exact path; entries need owner approval"
        );
        assert!(
            !reason.is_empty(),
            "docs budget exempt {path} needs its one-line reason"
        );
        assert!(
            files.iter().any(|p| p == path),
            "docs budget exempt {path} is not in the checkout; entries need owner approval"
        );
    }
    let markdown: Vec<(String, usize)> = files
        .iter()
        .filter(|p| {
            is_markdown(p) && !runtime.contains(p) && !DOCS_BUDGET_OWNER_EXEMPT.iter().any(|(exempt, _)| exempt == p)
        })
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
    // A folder README needs no row but stays under FOLDER_README_WORDS and counts
    // toward its group; an explicit row still wins.
    assert!(docs_budget_violations(
        &md(&[("docs/A.md", 50), ("docs/B.md", 1), ("src/README.md", 90)]),
        budget,
        caps
    )
    .is_empty());
    expect(
        docs_budget_violations(
            &md(&[
                ("docs/A.md", 1),
                ("docs/B.md", 1),
                ("src/README.md", FOLDER_README_WORDS + 1),
            ]),
            budget,
            caps,
        ),
        "src/README.md has 151 words, over its cap of 150",
    );
    expect(
        docs_budget_violations(
            &md(&[("docs/A.md", 1), ("docs/B.md", 1), (".claude/README.md", 1)]),
            budget,
            caps,
        ),
        "Agent has no DOCS_GROUP_CAPS entry",
    );
    assert_eq!(folder_readme_row("README.md"), None);
    assert_eq!(folder_readme_row("docs/x.md"), None);
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
    let root_budget: &[(&str, Kind, usize)] = &[("docs/A.md", Root, 100), ("docs/B.md", Root, 100)];
    expect(
        docs_budget_violations(
            &md(&[("docs/A.md", 10), ("docs/B.md", 10)]),
            root_budget,
            &[(Root, 1_500)],
        ),
        "lower it to 500",
    );
    // An operator-approved override keeps that group's headroom.
    assert!(docs_budget_violations(&md(&[("docs/A.md", 10), ("docs/B.md", 10)]), budget, &[(Guide, 1_500)]).is_empty());
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
    assert!(tree_violations(
        &sized(&[("docs/screenshots/demo/a.mp4", VIDEO_FILE_BYTES)]),
        &[],
        &[],
        &[]
    )
    .is_empty());
    expect(
        tree_violations(
            &sized(&[("docs/screenshots/demo/a.MP4", VIDEO_FILE_BYTES + 1)]),
            &[],
            &[],
            &[],
        ),
        "over the 8 MiB demo-video cap",
    );
    expect(
        tree_violations(
            &sized(&[("docs/screenshots/a.png", LARGE_FILE_BYTES + 1)]),
            &[],
            &[],
            &[],
        ),
        "over 1 MiB",
    );
    expect(
        tree_violations(&sized(&[("assets/demo.mp4", LARGE_FILE_BYTES + 1)]), &[], &[], &[]),
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
