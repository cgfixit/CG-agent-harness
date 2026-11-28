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
        assert_file_does_not_cross(&path, &text, forbidden);
    }
}

fn assert_file_does_not_cross(path: &str, text: &str, forbidden: &[&str]) {
    let idents = use_idents(path, text);
    let syntax = boundary_syntax(path, text);
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
    let relative = Path::new(path)
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

/// Top-level modules on the console side of the I6 boundary. Every entry under
/// `src/` is one of these, `agentic/`, or a crate-root file with its own check
/// (`every_top_level_source_is_on_a_scanned_side`), so none escapes the scans.
const SERVER_SIDE: [&str; 5] = ["server", "shim", "llm", "common", "netconnect"];

#[test]
fn server_side_never_references_agentic() {
    for sub in SERVER_SIDE {
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

/// A `MISSING_MEANS_ON` switch reads as on when its key is absent, so the
/// startup drift report skips it. That is only true while the code reads it
/// through `flag_missing_on`: one read through `flag_is_true` would treat the
/// same absent key as off with no warning. No other key may use the accessor,
/// because it must never open a gate that has to stay closed until an
/// operator opens it.
#[test]
fn missing_means_on_switches_are_read_only_through_their_accessor() {
    use cgagentharness::common::config::MISSING_MEANS_ON;
    let mut reads = Vec::new();
    for (path, text) in read_tree(&root()) {
        for key in MISSING_MEANS_ON {
            assert!(
                !text.contains(&format!("flag_is_true(\"{key}\")")),
                "{path} reads {key} through flag_is_true"
            );
        }
        let mut rest = text.as_str();
        while let Some(at) = rest.find(".flag_missing_on(") {
            let after = &rest[at + ".flag_missing_on(".len()..];
            let key = after
                .strip_prefix('"')
                .and_then(|s| s.split_once('"'))
                .map(|(key, _)| key.to_string());
            match key {
                Some(key) => reads.push(key),
                // The accessor's own unit test passes each listed key by name.
                None => assert!(
                    path.replace('\\', "/").ends_with("common/config.rs"),
                    "{path} calls flag_missing_on without a literal key"
                ),
            }
            rest = after;
        }
    }
    reads.sort();
    reads.dedup();
    let mut listed: Vec<String> = MISSING_MEANS_ON.iter().map(|k| k.to_string()).collect();
    listed.sort();
    assert_eq!(reads, listed, "flag_missing_on reads exactly the MISSING_MEANS_ON keys");
}

/// Every path a file names outside test-only items, each as its segments:
/// expression and type paths, `use` trees (a group yields one path per leaf)
/// and `a::b` runs inside macro bodies, which syn leaves unparsed.
#[derive(Default)]
struct ProductionPaths(Vec<Vec<String>>);

impl ProductionPaths {
    fn use_tree(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.unraw().to_string());
                self.use_tree(&path.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(name) => {
                let mut full = prefix.clone();
                full.push(name.ident.unraw().to_string());
                self.0.push(full);
            }
            // `as` and `*` are not identifiers, so no real path has these
            // segments: a rule can tell `use crate as root;` and `use crate::*;`
            // from the bare `crate` of `pub(crate)`.
            syn::UseTree::Rename(rename) => {
                let mut full = prefix.clone();
                full.push(rename.ident.unraw().to_string());
                full.push("as".into());
                full.push(rename.rename.unraw().to_string());
                self.0.push(full);
            }
            syn::UseTree::Glob(_) => {
                let mut full = prefix.clone();
                full.push("*".into());
                self.0.push(full);
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.use_tree(item, prefix);
                }
            }
        }
    }

    fn macro_tokens(&mut self, mut cursor: syn::buffer::Cursor<'_>) {
        let mut run: Vec<String> = Vec::new();
        let mut colons = 0;
        while !cursor.eof() {
            if let Some((ident, next)) = cursor.ident() {
                // An ident continues the run only straight after `::`.
                if !run.is_empty() && colons != 2 {
                    self.0.push(std::mem::take(&mut run));
                }
                run.push(ident.unraw().to_string());
                colons = 0;
                cursor = next;
            } else if let Some((punct, next)) = cursor.punct().filter(|(p, _)| p.as_char() == ':') {
                let _ = punct;
                colons += 1;
                cursor = next;
            } else {
                self.0.push(std::mem::take(&mut run));
                colons = 0;
                if let Some((body, _, _, next)) = cursor.any_group() {
                    self.macro_tokens(body);
                    cursor = next;
                } else {
                    cursor = cursor.token_tree().expect("nonempty token cursor").1;
                }
            }
        }
        self.0.push(run);
    }
}

impl<'ast> syn::visit::Visit<'ast> for ProductionPaths {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !test_only(&module.attrs) {
            syn::visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !test_only(&function.attrs) {
            syn::visit::visit_item_fn(self, function);
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.use_tree(&item.tree, &mut Vec::new());
    }

    /// `extern crate self as root;` names the crate root like `use crate as root;`.
    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        if test_only(&item.attrs) {
            return;
        }
        let name = item.ident.unraw().to_string();
        let mut full = vec![if name == "self" { "crate".to_string() } else { name }];
        if let Some((_, rename)) = &item.rename {
            full.push("as".into());
            full.push(rename.unraw().to_string());
        }
        self.0.push(full);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.0
            .push(path.segments.iter().map(|s| s.ident.unraw().to_string()).collect());
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let tokens = syn::buffer::TokenBuffer::new2(node.tokens.clone());
        self.macro_tokens(tokens.begin());
        syn::visit::visit_macro(self, node);
    }
}

fn production_paths(path: &str, text: &str) -> Vec<Vec<String>> {
    let file = syn::parse_file(text).unwrap_or_else(|error| panic!("{path} does not parse: {error}"));
    let mut visitor = ProductionPaths::default();
    syn::visit::visit_file(&mut visitor, &file);
    visitor.0
}

/// What a production path under src/netconnect must not name, if anything.
/// INVARIANTS "Netconnect is fail-closed": outside tests it imports only
/// `crate::common` and `crate::server::slash`, and only connect.rs opens a
/// socket. The needle lists in the test below miss both of these: another
/// crate module's HTTP client (`crate::llm::ollama::http_client` names no
/// needle and dials an IP literal), and a `libc::socket` whose type is a
/// number, not a `SOCK_*` name.
fn netconnect_path_violation(segments: &[String], dialer: bool) -> Option<&'static str> {
    let mut names: Vec<&str> = segments.iter().map(String::as_str).collect();
    // `self::super::super::llm` is `super::super::llm`.
    while names.first() == Some(&"self") {
        names.remove(0);
    }
    // In a file directly under src/netconnect, `super::super` is the crate.
    let in_crate = match names.as_slice() {
        ["crate", rest @ ..] | ["super", "super", rest @ ..] => Some(rest),
        _ => None,
    };
    match in_crate {
        // A bare `crate` is `pub(crate)` visibility, not an import.
        Some([] | ["common" | "netconnect", ..] | ["server", "slash", ..]) | None => {}
        // `use crate as root;`, `extern crate self as root;` and `use crate::*;`
        // bring every crate module into scope under a name this rule cannot follow.
        Some(["as", ..] | ["*"]) => return Some("an alias or glob of the crate root"),
        Some(_) => return Some("a crate module other than common, netconnect and server::slash"),
    }
    const LIBC_SOCKET_CALLS: &[&str] = &[
        "socket",
        "socketpair",
        "connect",
        "bind",
        "listen",
        "accept",
        "sendto",
        "sendmsg",
    ];
    match names.as_slice() {
        ["libc", call] | ["libc", call, "as", _] if !dialer && LIBC_SOCKET_CALLS.contains(call) => {
            Some("a raw libc socket call")
        }
        // `use libc::*;` makes a later bare `socket(..)` a raw call with no `libc::` on it.
        ["libc", "*"] if !dialer => Some("a glob of libc, which hides raw socket calls"),
        _ => None,
    }
}

#[test]
fn netconnect_path_rule_names_what_the_needles_miss() {
    let violations = |code: &str, dialer: bool| -> Vec<&'static str> {
        production_paths("probe", code)
            .iter()
            .filter_map(|segments| netconnect_path_violation(segments, dialer))
            .collect()
    };
    for hostile in [
        "fn f() { let _ = crate::llm::ollama::http_client(t); }",
        "use crate::llm::ollama::{http_client as c};",
        "use crate::{common::dns, llm::*};",
        "fn f() { let _ = super::super::shim::run(); }",
        "use crate::server::web_search;",
        "fn f() { log!(crate::server::state::get()); }",
        "fn f() { unsafe { libc::socket(libc::AF_INET, 1, 0) }; }",
        "use libc::connect;",
        // An alias or glob of the crate root hides the module every later
        // path names, and so does a `self::` in front of `super::super`.
        "use crate as root; fn f() { let _ = root::llm::ollama::http_client(t); }",
        "use super::super as up; fn f() { let _ = up::shim::run(); }",
        "extern crate self as root; fn f() { let _ = root::llm::ollama::http_client(t); }",
        "use crate::*; fn f() { let _ = llm::ollama::http_client(t); }",
        "use super::super::*; fn f() { let _ = llm::ollama::http_client(t); }",
        "fn f() { let _ = self::super::super::llm::ollama::http_client(t); }",
        "use self::super::super::llm;",
        "use libc::{socket as s};",
        "use libc::*; fn f() { unsafe { socket(2, 1, 0) }; }",
    ] {
        assert!(!violations(hostile, false).is_empty(), "missed: {hostile}");
    }
    for fine in [
        "use crate::common::process as p; use super::scope::*; use self::scope::Scope;",
        "pub(crate) fn f() {} pub(in crate::netconnect) fn g() {} fn h() { self::g() }",
        "use crate::common::process::run; use crate::netconnect::Scope;",
        "fn f() { let _ = crate::server::slash::preexisting_slash_names(); }",
        "fn f() { unsafe { libc::getifaddrs(p) }; }",
        "#[cfg(test)] mod tests { use crate::server::views::list_wired_tools_with; }",
        "#[test] fn t() { let _ = super::super::llm::x(); }",
    ] {
        assert!(violations(fine, false).is_empty(), "false alarm: {fine}");
    }
    // connect.rs owns the socket, so a raw call there is its own business.
    assert!(violations("fn f() { unsafe { libc::socket(2, 1, 0) }; }", true).is_empty());
}

#[test]
fn netconnect_dials_only_through_the_budgeted_dialer() {
    // Sockets and name lookups. Only connect.rs may open a socket, and
    // nothing under src/netconnect may resolve a host name.
    const SOCKETS: &[&str] = &["TcpStream", "TcpSocket", "UdpSocket", "socket2", "RawSocket"];
    // `lookup_bounded` and `dns::` are this crate's own resolver: netconnect
    // may import `crate::common`, and that path names none of the std needles.
    const LOOKUPS: &[&str] = &[
        "lookup_host",
        "ToSocketAddrs",
        "to_socket_addrs",
        "getaddrinfo",
        "lookup_bounded",
        "dns::",
    ];
    // An HTTP client resolves names and opens its own sockets without ever
    // naming a socket type, so it would pass every needle above.
    const HTTP_CLIENTS: &[&str] = &["reqwest", "pinned_client_builder"];
    const LISTENERS: &[&str] = &[
        "TcpListener",
        "UnixListener",
        "UnixStream",
        "UnixDatagram",
        "SOCK_RAW",
        "SOCK_DGRAM",
        "SOCK_STREAM",
    ];
    for (path, text) in read_tree(&root().join("netconnect")) {
        let dialer = path.replace('\\', "/").ends_with("netconnect/connect.rs");
        for needle in LOOKUPS {
            assert!(!text.contains(needle), "{path} resolves names via {needle:?}");
        }
        for needle in HTTP_CLIENTS {
            assert!(
                !text.contains(needle),
                "{path} reaches the network via {needle:?}; dial through netconnect::Dialer"
            );
        }
        for segments in production_paths(&path, &text) {
            if let Some(what) = netconnect_path_violation(&segments, dialer) {
                panic!("{path} names {what}: {}", segments.join("::"));
            }
        }
        // The needles miss a renamed import (`use crate::common::{dns as d};`).
        let imports = use_idents(&path, &text);
        for name in ["dns", "reqwest"] {
            assert!(
                !imports.iter().any(|i| i == name),
                "{path} imports {name:?}; dial through netconnect::Dialer"
            );
        }
        if !dialer {
            for needle in SOCKETS {
                assert!(
                    !text.contains(needle),
                    "{path} opens a socket via {needle:?}; dial through netconnect::Dialer"
                );
            }
            // Listeners and `SOCK_*` names (`netconnect_path_violation` covers a
            // `libc::socket` with a numeric type). Production syntax only:
            // cli.rs tests bind fixtures.
            let production = scan_syntax(&path, &text, true);
            for needle in LISTENERS {
                assert!(
                    !production.paths.iter().any(|p| p == needle),
                    "{path} opens a socket via {needle:?}; only connect.rs may"
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

/// Every reqwest builder names its verifier. A new `Client::builder()` with no
/// `tls_certs_only`, preconfigured trust, extra root, or `tls-verifier:` marker
/// is the platform verifier by accident. The pinned builder must pass the
/// bundled Mozilla roots and nothing else.
#[test]
fn every_reqwest_builder_names_its_tls_verifier() {
    const MARKERS: &[&str] = &[
        "tls-verifier: loopback",
        "tls-verifier: platform-public",
        "tls-verifier: platform-contrast",
    ];
    const EXPLICIT: &[&str] = &[
        "tls_certs_only(",
        "use_preconfigured_tls(",
        "tls_backend_preconfigured(",
        "add_root_certificate(",
    ];
    let repo = root().parent().unwrap().to_path_buf();
    let mut files = Vec::new();
    for dir in ["src", "desktop/src", "tests"] {
        for (path, text) in read_tree_raw(&repo.join(dir)) {
            files.push((path, text));
        }
    }
    let mut builders = 0usize;
    for (path, text) in &files {
        let lines: Vec<&str> = text.lines().collect();
        let spots: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains("Client::builder()"))
            .map(|(index, _)| index)
            .collect();
        for (nth, line) in spots.iter().copied().enumerate() {
            builders += 1;
            let back = spots.get(nth.wrapping_sub(1)).map(|p| p + 1).unwrap_or(0);
            let back = back.max(line.saturating_sub(15));
            let forward = spots.get(nth + 1).copied().unwrap_or(lines.len());
            let forward = forward.min(line + 30);
            let region = lines[back..forward].join("\n");
            let marked = MARKERS.iter().any(|marker| region.contains(marker));
            let explicit = EXPLICIT.iter().any(|token| region.contains(token));
            assert!(
                marked || explicit,
                "{path}:{} builds a reqwest client without a named TLS verifier\n{region}",
                line + 1
            );
        }
    }
    assert!(builders > 0, "the verifier scan found no reqwest builders");
    let pinned = std::fs::read_to_string(root().join("server/web_search.rs")).unwrap();
    let body = &pinned[..pinned.find("#[cfg(test)]").unwrap_or(pinned.len())];
    assert!(
        body.contains(".tls_certs_only(bundled_mozilla_roots().iter().cloned())"),
        "pinned_client_builder must verify with the bundled Mozilla roots only"
    );
    assert!(
        body.contains("webpki_root_certs::TLS_SERVER_ROOT_CERTS"),
        "bundled roots must be the webpki-root-certs Mozilla set"
    );
    assert!(
        !body.contains("tls-verifier: platform"),
        "the pinned client must not opt into the platform verifier"
    );
}

#[test]
fn only_registered_process_owners_construct_commands() {
    const OWNERS: &[&str] = &[
        "shim/mod.rs",
        "common/process.rs",
        "common/browser_host.rs",
        "common/mcp.rs",
        "common/mcp_worker.rs",
        "common/windows_job.rs",
    ];
    for sub in SERVER_SIDE {
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

/// The directory scans see only the modules they name. A new top-level module,
/// or a binary under `src/bin/`, would sit outside every one of them.
#[test]
fn every_top_level_source_is_on_a_scanned_side() {
    for entry in std::fs::read_dir(root()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        let classified = SERVER_SIDE.contains(&name.as_str())
            || matches!(name.as_str(), "agentic" | "lib.rs" | "main.rs" | "README.md");
        assert!(
            classified,
            "src/{name} is outside every I6 scan; add it to SERVER_SIDE or the agentic side"
        );
    }
}

/// `pub use agentic as pipeline;` in the crate root would let a server file call
/// `crate::pipeline::cli::main` with no `agentic` identifier in it, and
/// `pub use server as console;` would do the same in reverse. The root only
/// declares modules.
#[test]
fn crate_root_declares_modules_without_bridging_them() {
    let path = root().join("lib.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_file_does_not_cross(&path.display().to_string(), &text, &["agentic", "server", "shim"]);
}

#[derive(Default)]
struct PipelineCalls(usize);

impl<'ast> syn::visit::Visit<'ast> for PipelineCalls {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(callee) = &*call.func {
            let segments: Vec<String> = callee
                .path
                .segments
                .iter()
                .map(|s| s.ident.unraw().to_string())
                .collect();
            self.0 += usize::from(segments == ["cgagentharness", "agentic", "cli", "main"]);
        }
        syn::visit::visit_expr_call(self, call);
    }
}

/// The binary is the CLI whitelist edge: it names the pipeline exactly once, as
/// the direct call behind the hidden `agentic` subcommand. A second mention (an
/// alias, or `agentic::cli::main` handed to `serve` as a value) would run the
/// pipeline in the console's process.
#[test]
fn binary_reaches_the_pipeline_only_through_its_hidden_subcommand() {
    let path = root().join("main.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut calls = PipelineCalls::default();
    syn::visit::visit_file(&mut calls, &syn::parse_file(&text).unwrap());
    let syntax = boundary_syntax(&path.display().to_string(), &text);
    let mentions = syntax.paths.iter().filter(|ident| *ident == "agentic").count();
    assert_eq!(
        (calls.0, mentions),
        (1, 1),
        "src/main.rs must name the pipeline once, as the hidden subcommand's direct call"
    );
    assert!(
        !syntax.includes_code && syntax.module_paths.is_empty(),
        "src/main.rs must not include or remap modules"
    );
}

/// A `common::config` limit row copies a default or ceiling that another
/// module (or the agentic child, which reads the same keys) also names. The
/// row is what `AppConfig::load` enforces, so the copies must not drift.
#[test]
fn limit_rows_agree_with_the_constants_they_mirror() {
    use cgagentharness::common::config::{OutOfRange, FLOAT_LIMITS, NUMERIC_LIMITS};
    let int = |key: &str| NUMERIC_LIMITS.iter().find(|row| row.key == key).unwrap();
    let float = |key: &str| FLOAT_LIMITS.iter().find(|row| row.key == key).unwrap();
    let timeout = int("agentic.deepagent_github.planner_timeout_sec");
    assert_eq!(
        timeout.default,
        cgagentharness::agentic::config::DEFAULT_PLANNER_TIMEOUT_SEC
    );
    assert_eq!(
        timeout.default,
        cgagentharness::shim::REAL_REPO_RUN_FALLBACK_PLANNER_SEC
    );
    assert_eq!(timeout.max, cgagentharness::agentic::commands::MAX_PLANNER_TIMEOUT_SEC);
    assert_eq!(timeout.max, cgagentharness::shim::REAL_REPO_RUN_MAX_TIMEOUT_SEC);
    let tokens = int("agentic.deepagent_github.planner_max_tokens");
    assert_eq!(
        tokens.default,
        cgagentharness::agentic::config::DEFAULT_PLANNER_MAX_TOKENS
    );
    assert_eq!(tokens.max, cgagentharness::server::compaction::MAX_WINDOW);
    assert_eq!(
        int("personality.soul_max_chars").max,
        cgagentharness::server::prompts::SOUL_CHARS_HARD_CAP
    );
    let chat = float("models.local_llm.timeout_sec");
    assert_eq!(chat.default, cgagentharness::llm::openai_chat::DEFAULT_CHAT_TIMEOUT_SEC);
    assert_eq!(chat.max, cgagentharness::server::model_limits::MAX_TIMEOUT_SEC as f64);
    for row in [timeout, tokens] {
        assert_eq!(row.out_of_range, OutOfRange::Refuse, "{}", row.key);
        assert_eq!(row.min, 1, "the child's positive_int floor: {}", row.key);
    }
    for row in [chat, float("models.local_llm.temperature")] {
        assert_eq!(row.out_of_range, OutOfRange::Refuse, "{}", row.key);
    }
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
    // The child refuses gh timeout/retry settings whose pre-planner bounds
    // reach the shim's fixed setup budget; it must refuse against the same number.
    assert_eq!(
        cgagentharness::shim::REAL_REPO_RUN_OVERHEAD_SEC,
        cgagentharness::agentic::config::REAL_REPO_RUN_SETUP_BUDGET_SEC
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
    // A config without agentic.gh_min_version falls back to the code default,
    // so that default must be the shipped value.
    let shipped = cgagentharness::common::config::AppConfig::from_str(
        cgagentharness::common::config::AppConfig::embedded_default(),
        Path::new("defaults"),
    )
    .unwrap();
    let (major, minor, patch) = cgagentharness::agentic::gh_client::DEFAULT_MIN_GH;
    assert_eq!(
        shipped.str_or("agentic.gh_min_version", ""),
        format!("{major}.{minor}.{patch}")
    );
    // The console caps the goal box with its own copy of the server's ceiling.
    assert!(
        cgagentharness::server::console::HARNESS_HTML.contains(&format!(
            "const MAX_GOAL_CHARS = {};",
            cgagentharness::server::prompts::MAX_GOAL_CHARS
        )),
        "console MAX_GOAL_CHARS drifted from server::prompts::MAX_GOAL_CHARS"
    );
    // The desktop shell is a separate crate on its own toolchain: a handshake
    // version bumped on one side alone makes every packaged app refuse its own
    // bundled backend, and no CI job runs the shell against the sidecar.
    let protocol = |file: &str, pattern: &str| {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        let found = regex::Regex::new(pattern)
            .unwrap()
            .captures(&text)
            .map(|c| c[1].to_string());
        found.unwrap_or_else(|| panic!("{file} names no handshake PROTOCOL"))
    };
    assert_eq!(
        protocol("desktop/src/backend.rs", r"pub const PROTOCOL: u64 = (\d+);"),
        protocol("src/server/desktop.rs", r"pub const PROTOCOL: u32 = (\d+);"),
        "desktop/src/backend.rs PROTOCOL drifted from src/server/desktop.rs"
    );
    // `/loop` keeps private Rust mirrors of three console limits; nothing else
    // pinned them, so a changed console value left the server stalling on a
    // different threshold than the page that shows it.
    let loop_runner =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/loop_runner.rs")).unwrap();
    for name in [
        "LOOP_GOAL_REMINDER_CHARS",
        "LOOP_STALL_SIMILARITY",
        "LOOP_STALL_MAX_WORDS",
    ] {
        let value = |text: &str, pattern: String| {
            regex::Regex::new(&pattern)
                .unwrap()
                .captures(text)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| panic!("no `{name}` matched {pattern}"))
        };
        assert_eq!(
            value(&loop_runner, format!(r"const {name}: \w+ = ([0-9.]+);")),
            value(
                cgagentharness::server::console::HARNESS_HTML,
                format!(r"const {name} = ([0-9.]+);")
            ),
            "console {name} drifted from src/server/loop_runner.rs"
        );
    }
    // The setup-code dialog checks a picked file against the generator's alphabet
    // and length; a drift would refuse a real code or show a wrong one.
    let authn = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/common/authn.rs")).unwrap();
    let grab = |text: &str, pattern: &str| {
        regex::Regex::new(pattern)
            .unwrap()
            .captures(text)
            .map(|c| (c[1].to_string(), c.get(2).map(|m| m.as_str().to_string())))
            .unwrap_or_else(|| panic!("no match for {pattern}"))
    };
    let (alphabet, _) = grab(&authn, r#"const SETUP_CODE_ALPHABET: &\[u8\] = b"([^"]+)";"#);
    let (length, _) = grab(&authn, r"const SETUP_CODE_LEN: usize = (\d+);");
    assert_eq!(
        grab(
            cgagentharness::server::console::HARNESS_HTML,
            r"const SETUP_CODE_SYMBOLS = /\^\[([^\]]+)\]\{(\d+)\}\$/;"
        ),
        (alphabet, Some(length)),
        "console SETUP_CODE_SYMBOLS drifted from src/common/authn.rs"
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

/// Fixture bypasses compile only with `test-support` (AGENTS.md Traps), and
/// resolver 2 keeps the self dev-dependency's features out of release builds.
/// Nothing else may enable it, no release build line may select features, and
/// the packaged-CLI smoke keys on a hook name that only the feature compiles.
#[test]
fn release_builds_leave_out_test_support() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(repo.join("Cargo.toml")).unwrap();
    let mut section = "";
    let mut enabling = Vec::new();
    for line in manifest.lines().map(str::trim).filter(|line| !line.starts_with('#')) {
        if line.starts_with('[') {
            section = line;
        } else if line.contains("test-support") && line != "test-support = []" {
            enabling.push(section);
        }
    }
    assert_eq!(
        enabling,
        ["[dev-dependencies]"],
        "only the self dev-dependency may enable test-support"
    );
    let mut scripts: Vec<_> = std::fs::read_dir(repo.join(".github/workflows"))
        .unwrap()
        .chain(std::fs::read_dir(repo.join("scripts")).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("yml" | "sh")))
        .collect();
    scripts.sort();
    let mut release_builds = 0;
    for path in scripts {
        let text = std::fs::read_to_string(&path).unwrap().replace("\\\n", " ");
        for line in text
            .lines()
            .filter(|l| l.contains("cargo build") && l.contains("--release"))
        {
            release_builds += 1;
            assert!(
                !["--features", "--all-features", " -F "]
                    .iter()
                    .any(|flag| line.contains(flag)),
                "{}: a release build selects features: {}",
                path.display(),
                line.trim()
            );
        }
    }
    assert!(release_builds > 0, "the scan found no release build line");
    let marker = "CGAH_TEST_CLOUD_BASE_URL";
    let hook = std::fs::read_to_string(root().join("llm/cloud_chat.rs")).unwrap();
    assert!(
        hook.contains("#[cfg(feature = \"test-support\")]") && hook.contains(marker),
        "the smoke's marker must stay a test-support-only hook"
    );
    let smoke = std::fs::read_to_string(repo.join("scripts/smoke-packaged-cli.py")).unwrap();
    assert!(
        smoke.contains(marker),
        "the packaged-CLI smoke must refuse a test-support build"
    );
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

/// `desktop-check.yml` compiles the desktop crate on a pull request only when
/// its classify regex matches a changed path. The crate also reads files outside
/// `desktop/` at compile time (`#[path]` includes and `include_bytes!` /
/// `include_str!` embeds), so each such target must be in the regex: a change
/// to one that is not would skip the compile job, merge green and break the
/// next `main` push, which is the failure the workflow header describes.
#[test]
fn desktop_check_classifies_every_out_of_tree_compile_input() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/desktop-check.yml")).unwrap();
    let classifier = regex::Regex::new(r"grep -Eq '([^']+)'").unwrap();
    let pattern = classifier
        .captures(&workflow)
        .expect("desktop-check.yml classify step greps changed paths")
        .get(1)
        .unwrap()
        .as_str()
        .to_string();
    let matcher = regex::Regex::new(&pattern).unwrap();
    let include =
        regex::Regex::new(r#"(?:include_bytes!|include_str!)\("([^"]+)"\)|#\[path\s*=\s*"([^"]+)"\]"#).unwrap();
    // `read_tree_raw` hands back absolute paths; build.rs sits beside src/.
    let mut sources = read_tree_raw(&repo.join("desktop").join("src"));
    let build_rs = repo.join("desktop").join("build.rs");
    sources.push((
        build_rs.display().to_string(),
        std::fs::read_to_string(&build_rs).unwrap_or_default(),
    ));
    let mut inputs = Vec::new();
    for (path, text) in &sources {
        let dir = Path::new(path).parent().unwrap();
        for cap in include.captures_iter(text) {
            let target = cap.get(1).or_else(|| cap.get(2)).unwrap().as_str();
            let resolved = dunce::canonicalize(dir.join(target))
                .unwrap_or_else(|e| panic!("{path} includes {target}, which does not exist: {e}"));
            let relative = resolved
                .strip_prefix(dunce::canonicalize(repo).unwrap())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !relative.starts_with("desktop/") {
                inputs.push((path.clone(), target.to_string(), relative));
            }
        }
    }
    assert!(
        inputs.iter().any(|(_, _, rel)| rel == "src/common/local_tls.rs"),
        "the `#[path]` include of local_tls.rs is the case the workflow header names; the scan must find it"
    );
    for (path, target, relative) in &inputs {
        assert!(
            matcher.is_match(relative),
            "{path} compiles {target} ({relative}) but the desktop-check.yml classify regex `{pattern}` \
             does not match it, so a PR changing only that file skips the desktop compile job"
        );
    }
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
        "diagnostics.model_tool",
        // #996: native computer use ships off before any of its code exists.
        "computer.enabled",
        // #782 S2: the network-only Linux backend is an explicit operator opt-in.
        "agentic.sandbox.allow_linux_netns_fallback",
        // Opt-in egress and spend switches no test pinned before 2026-11-26: Firecrawl
        // sends URLs to api.firecrawl.dev, the budget cap must not trust bytes/4
        // estimates, and the LM Studio failover is a second local endpoint.
        "web.firecrawl.enabled",
        "models.cloud_chat.budget_on_heuristic",
        "models.local_llm.fallback.enabled",
    ] {
        // A missing key also reads as off, so `!flag_is_true` alone would stay green
        // after a gate is renamed or dropped. Require the literal shipped `false`.
        assert_eq!(cfg.bool_opt(gate), Some(false), "{gate} must ship as literal false");
    }
    // why: operator decision (2026-11-14): local repository retrieval ships on. It reads only
    // the pipeline-owned clone through the same basename refusals, and cloud proposers still
    // cannot widen their read set (INVARIANTS.md, coding pipeline).
    assert!(
        cfg.flag_is_true("agentic.deepagent_github.retrieval.enabled"),
        "agentic.deepagent_github.retrieval.enabled must ship true"
    );
    // why: operator decision (2026-11-01): a spent or failing SerpAPI falls back to public
    // Google, which costs nothing and still needs its URL rule in allowlist mode.
    assert!(
        cfg.flag_is_true("web.search.fallback_to_public_google"),
        "web.search.fallback_to_public_google must ship true"
    );
    // why: operator decision (2026-11-22, superseding #647 Phase 2's opt-in): a failed
    // Tavily search tries SerpAPI, which spends SerpAPI quota only when a key is active.
    assert!(
        cfg.flag_is_true("web.search.fallback_to_serpapi"),
        "web.search.fallback_to_serpapi must ship true"
    );
    // why: operator decision (2026-11-12): an administrator's turn may read the exact URLs
    // its own search listed, for that turn or loop run. It was added off the day before. The
    // bounds that stay (exact URL, memory only, blocked rules win, admin role only) are
    // asserted in web_policy and web_search unit tests and in tests/chat_web.rs.
    // why: operator decision (2026-11-12), later the same day: a home whose config.yaml
    // lacks the key reads it as on (`MISSING_MEANS_ON`), so an existing home gets the
    // shipped behaviour without an edit. Literal false still turns it off.
    assert!(
        cfg.flag_is_true("web.search.result_reads.enabled"),
        "web.search.result_reads.enabled must ship true"
    );
    // Browser tasks ship on but stay administrator-only and need literal auth.enabled
    // (tests/auth_guards.rs); every takeover approval needs confirm and a reason
    // (tests/secure_portal.rs). Pinned so a flipped default is a visible decision.
    for key in ["browser.enabled", "browser.takeover.enabled"] {
        assert_eq!(cfg.bool_opt(key), Some(true), "{key} must ship as literal true");
    }
    // Context about the harness itself ships on as literal booleans; it grants no tool.
    // why: 2026-11-01, new homes ship /diag log reads on. model_tool stays closed above.
    let knowledge = cgagentharness::server::self_manual::SelfKnowledgeConfig::from_config(&cfg).unwrap();
    assert!(knowledge.self_knowledge && knowledge.live_state);
    for key in [
        "diagnostics.logs_enabled",
        "diagnostics.self_knowledge",
        "diagnostics.live_state",
    ] {
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
    for needle in ["allow_git_write_tools: false", "allow_plaintext_key_file: false"] {
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

/// Account-role authority lives in exactly two places: `common/authz.rs` owns
/// the parse and the capability matrix, and the gate (`server/guards.rs`)
/// resolves the parsed role against `routes::ROUTE_CAPS`. The role string
/// constants stay in `common/authn.rs`. Every other stored-role comparison is
/// legacy surface pinned below until the follow-up consolidation; a new
/// comparison outside the pins means a handler grew its own role authority
/// instead of consulting the capability table.
/// A line is an account-role comparison when it names `role`, names an
/// account role and has a comparison shape. Message roles
/// ("user"/"assistant") never match.
fn is_role_comparison(line: &str) -> bool {
    const SHAPES: &[&str] = &[
        "==",
        "!=",
        ".eq(",
        ".ne(",
        "=>",
        "matches!(",
        ".as_str()",
        ".as_ref()",
        ".as_deref()",
        "starts_with(",
        ".contains(",
    ];
    const TOKENS: &[&str] = &[
        "\"admin\"",
        "\"operator\"",
        "\"auditor\"",
        "AUDITOR_ROLE",
        "ROLE_ADMIN",
        "ROLE_OPERATOR",
    ];
    let packed: String = line.split_whitespace().collect();
    packed.contains("role")
        && TOKENS.iter().any(|token| packed.contains(token))
        && SHAPES.iter().any(|shape| packed.contains(shape))
}

#[test]
fn account_role_comparisons_stay_pinned_to_authz() {
    /// Files that may name account roles freely: the string constants and the
    /// parser/matrix.
    const ROLE_OWNERS: &[&str] = &["common/authn.rs", "common/authz.rs"];
    /// (file, needle) pairs, each consumed once: a leftover pin is stale and
    /// a hit without a pin is a new ad-hoc role check. Empty since #965's
    /// consolidation moved every site onto `authz` (`grants_stored`/`is_role`);
    /// the scan stays so the next comparison fails the build until it is
    /// pinned here with its PR.
    const PINNED: &[(&str, &str)] = &[];
    // The shapes are matched with whitespace removed and in either operand
    // order: `"admin" == account.role`, `role.eq("operator")` and a `match` arm
    // are the same decision as `role == "admin"`.
    for comparison in [
        r#"if account.role == "admin" {"#,
        r#"if account.role=="admin" {"#,
        r#"if "admin" == account.role {"#,
        r#"if account.role.eq("operator") {"#,
        r#"if AUDITOR_ROLE != user.role {"#,
        r#"match account.role.as_ref() { "admin" => true, _ => false }"#,
        r#"matches!(user.role.as_str(), "auditor")"#,
    ] {
        assert!(is_role_comparison(comparison), "{comparison}");
    }
    for other in [
        r#"mgr.set_role("alice", "operator").unwrap();"#,
        r#"assert_eq!(user.role, "admin");"#,
        r#"json!({"role": "user", "content": text})"#,
        r#"if message.role == "assistant" {"#,
        r#"let name = if username == "admin" { 1 } else { 2 };"#,
    ] {
        assert!(!is_role_comparison(other), "{other}");
    }
    let mut pins: Vec<(&str, &str)> = PINNED.to_vec();
    for (path, text) in read_tree(&root()) {
        let rel = path.replace('\\', "/");
        if ROLE_OWNERS.iter().any(|owner| rel.ends_with(owner)) {
            continue;
        }
        for line in text.lines() {
            if !is_role_comparison(line) {
                continue;
            }
            let hit = pins
                .iter()
                .position(|(file, needle)| rel.ends_with(file) && line.contains(needle));
            match hit {
                Some(index) => {
                    pins.swap_remove(index);
                }
                None => panic!(
                    "{rel}: `{}` is an unpinned account-role comparison. Role authority belongs \
                     in common/authz.rs and routes::ROUTE_CAPS; consolidating a pinned site means \
                     deleting its pin, and a genuinely new comparison needs the pin list updated \
                     with the PR",
                    line.trim()
                ),
            }
        }
    }
    assert!(pins.is_empty(), "stale role-comparison pins (site removed?): {pins:?}");
    // The parse itself runs once, at the gate: no other production code turns a
    // stored role string into a `Role`.
    for (path, text) in read_tree(&root()) {
        let rel = path.replace('\\', "/");
        if rel.ends_with("common/authz.rs") || rel.ends_with("server/guards.rs") {
            continue;
        }
        let code = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
        assert!(
            !code.contains("Role::parse"),
            "{rel} parses account roles outside the gate; the gate is the single parse site"
        );
    }
}

/// `security.api_key_optional` is an inert compatibility key (INVARIANTS.md,
/// "Signals weaker than their name"): account login and the direct-loopback
/// check never consult it. Its only allowed mention is the JSON compatibility
/// field emitted as the literal `true`. Reading the key, or deriving that field
/// from anything, would turn a dead switch into an authentication control, so
/// it has to arrive as a contract change, not a refactor.
#[test]
fn api_key_optional_stays_an_unread_compatibility_key() {
    const FIELD: &str = "\"api_key_optional\":true";
    let desktop = Path::new(env!("CARGO_MANIFEST_DIR")).join("desktop/src");
    for dir in [root(), desktop] {
        for (path, text) in read_tree(&dir) {
            for line in text.lines().filter(|line| line.contains("api_key_optional")) {
                let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                assert!(
                    !compact.replace(FIELD, "").contains("api_key_optional"),
                    "{path}: `{}` reads or derives api_key_optional. It is an inert \
                     compatibility key; wiring it changes the account contract in INVARIANTS.md",
                    line.trim()
                );
            }
        }
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
fn shipped_defaults_protect_rust_check_configuration() {
    // The default cargo profiles run inside the clone, so a candidate that could
    // write these files could make its own checks pass (a target runner of
    // `true`, formatting disabled, lints allowed).
    let yaml = cgagentharness::common::config::AppConfig::embedded_default();
    let defaults: Vec<String> = cgagentharness::agentic::config::DEFAULT_PROTECTED_WRITE_PATH_PREFIXES
        .iter()
        .map(|p| p.to_string())
        .collect();
    for entry in [
        ".cargo/",
        "rustfmt.toml",
        ".rustfmt.toml",
        "clippy.toml",
        ".clippy.toml",
        "Cargo.toml",
    ] {
        assert!(
            yaml.contains(&format!("- \"{entry}\"")),
            "shipped config.default.yaml must list {entry} in protected_write_paths"
        );
        assert!(
            defaults.iter().any(|p| p == entry),
            "Rust defaults must protect {entry}"
        );
    }
    for path in [
        ".cargo/config.toml",
        ".cargo/config",
        ".rustfmt.toml",
        "crates/a/clippy.toml",
        ".clippy.toml",
    ] {
        assert!(
            cgagentharness::agentic::real_repo_loop::matches_protected_path(path, &defaults),
            "{path} must be refused by the default protected list"
        );
    }
    assert!(!cgagentharness::agentic::real_repo_loop::matches_protected_path(
        "src/cargo/mod.rs",
        &defaults
    ));
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

const NUMERIC_ACCESSORS: &[&str] = &[
    "u64_or",
    "u64_in",
    "u64_strict",
    "f64_or",
    "f64_in",
    "limit",
    "limit_f64",
];

/// Files whose numeric accessor takes a key computed from a literal table in
/// the same file, which the loader still covers by the full path.
const COMPUTED_NUMERIC_KEYS: &[&str] = &[
    "server/config_reload.rs",
    "server/model_limits.rs",
    "server/web_search.rs",
];

/// What a numeric accessor or `AppConfig::from_str` call names as its key.
#[derive(Debug)]
enum KeyArg {
    Literal(String),
    /// A `format!` template such as `{prefix}.window_seconds`.
    Template(String),
    Computed,
}

/// Production numeric config reads and `AppConfig::from_str` inputs, including
/// those inside macro bodies (`json!`, `format!`), which syn leaves unparsed.
#[derive(Default)]
struct ConfigReads {
    reads: Vec<(String, KeyArg)>,
    parses: Vec<KeyArg>,
    /// `u64_or(..).clamp(..)` and `f64_or(..).clamp(..)`: a range kept at the
    /// read site instead of in `common::config::NUMERIC_LIMITS`.
    lenient_clamps: Vec<KeyArg>,
    /// `let name = "literal";` and `const NAME: &str = "literal";` in this file.
    bindings: std::collections::BTreeMap<String, String>,
}

fn key_arg(expr: &syn::Expr) -> KeyArg {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) => KeyArg::Literal(value.value()),
        syn::Expr::Reference(reference) => key_arg(&reference.expr),
        syn::Expr::Macro(mac) if path_is_last(&mac.mac.path, "format") => {
            key_tokens(&syn::buffer::TokenBuffer::new2(mac.mac.tokens.clone()), true)
        }
        syn::Expr::Call(call) => match &*call.func {
            syn::Expr::Path(path) if path_is_last(&path.path, "embedded_default") => {
                KeyArg::Literal("<embedded default>".into())
            }
            _ => KeyArg::Computed,
        },
        _ => KeyArg::Computed,
    }
}

fn path_is_last(path: &syn::Path, name: &str) -> bool {
    path.segments.last().is_some_and(|s| s.ident.unraw() == name)
}

/// The first argument of a macro-body call, read from its tokens.
fn key_tokens(buffer: &syn::buffer::TokenBuffer, template: bool) -> KeyArg {
    let mut cursor = buffer.begin();
    if let Some((punct, next)) = cursor.punct() {
        if punct.as_char() == '&' {
            cursor = next;
        }
    }
    if let Some((literal, _)) = cursor.literal() {
        if let Ok(value) = syn::parse_str::<syn::LitStr>(&literal.to_string()) {
            return if template {
                KeyArg::Template(value.value())
            } else {
                KeyArg::Literal(value.value())
            };
        }
    }
    if let Some((ident, next)) = cursor.ident() {
        if ident.unraw() == "format" && next.punct().is_some_and(|(p, _)| p.as_char() == '!') {
            if let Some((body, _, _, _)) = next.punct().and_then(|(_, after)| after.any_group()) {
                return key_tokens(&syn::buffer::TokenBuffer::new2(body.token_stream()), true);
            }
        }
    }
    KeyArg::Computed
}

impl<'ast> syn::visit::Visit<'ast> for ConfigReads {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !test_only(&module.attrs) {
            syn::visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !test_only(&function.attrs) {
            syn::visit::visit_item_fn(self, function);
        }
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !test_only(&function.attrs) {
            syn::visit::visit_impl_item_fn(self, function);
        }
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let (syn::Pat::Ident(name), Some(init)) = (&local.pat, &local.init) {
            if let KeyArg::Literal(value) = key_arg(&init.expr) {
                self.bindings.insert(name.ident.unraw().to_string(), value);
            }
        }
        syn::visit::visit_local(self, local);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if let KeyArg::Literal(value) = key_arg(&item.expr) {
            self.bindings.insert(item.ident.unraw().to_string(), value);
        }
        syn::visit::visit_item_const(self, item);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.unraw().to_string();
        if method == "clamp" {
            if let syn::Expr::MethodCall(read) = &*call.receiver {
                if ["u64_or", "f64_or"].contains(&read.method.unraw().to_string().as_str()) {
                    self.lenient_clamps
                        .push(read.args.first().map_or(KeyArg::Computed, key_arg));
                }
            }
        }
        if NUMERIC_ACCESSORS.contains(&method.as_str()) {
            if let Some(first) = call.args.first() {
                let key = match first {
                    syn::Expr::Path(path) => path
                        .path
                        .get_ident()
                        .and_then(|ident| self.bindings.get(&ident.unraw().to_string()))
                        .map_or(KeyArg::Computed, |value| KeyArg::Literal(value.clone())),
                    other => key_arg(other),
                };
                self.reads.push((method, key));
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func {
            let names: Vec<String> = path.path.segments.iter().map(|s| s.ident.unraw().to_string()).collect();
            if names.ends_with(&["AppConfig".to_string(), "from_str".to_string()]) {
                if let Some(first) = call.args.first() {
                    self.parses.push(key_arg(first));
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let buffer = syn::buffer::TokenBuffer::new2(node.tokens.clone());
        self.macro_reads(buffer.begin());
        syn::visit::visit_macro(self, node);
    }
}

impl ConfigReads {
    fn macro_reads(&mut self, mut cursor: syn::buffer::Cursor<'_>) {
        while !cursor.eof() {
            if let Some((ident, next)) = cursor.ident() {
                let name = ident.unraw().to_string();
                if NUMERIC_ACCESSORS.contains(&name.as_str()) {
                    if let Some((body, _, _, after)) = next.any_group() {
                        let key = key_tokens(&syn::buffer::TokenBuffer::new2(body.token_stream()), false);
                        let clamped = after.punct().is_some_and(|(dot, rest)| {
                            dot.as_char() == '.' && rest.ident().is_some_and(|(ident, _)| ident.unraw() == "clamp")
                        });
                        if clamped && (name == "u64_or" || name == "f64_or") {
                            self.lenient_clamps
                                .push(key_tokens(&syn::buffer::TokenBuffer::new2(body.token_stream()), false));
                        }
                        self.reads.push((name, key));
                    }
                }
                cursor = next;
            } else if let Some((body, _, _, next)) = cursor.any_group() {
                self.macro_reads(body);
                cursor = next;
            } else {
                cursor = cursor.token_tree().expect("nonempty token cursor").1;
            }
        }
    }
}

/// A present number must be a number (`AppConfig::load`), so every numeric
/// read in production has to name a key the loader checks, with the kind the
/// loader checks for. Operator text reaches `AppConfig` only through `load`:
/// `from_str` parses the embedded default (or an empty literal) in production.
#[test]
fn numeric_config_reads_are_type_checked_at_load() {
    use cgagentharness::common::config::{shipped_numeric_keys, FLOAT_KEYS, FLOAT_LIMITS, NUMERIC_LIMITS};
    let numeric = shipped_numeric_keys();
    let int_row = |key: &str| NUMERIC_LIMITS.iter().any(|limit| limit.key == key);
    let float_row = |key: &str| FLOAT_LIMITS.iter().any(|limit| limit.key == key);
    let mut computed_files = std::collections::BTreeSet::new();
    let is_float = |key: &str| FLOAT_KEYS.contains(&key);
    let leaf_matches = |template: &str, float: bool| {
        let leaf = template.rsplit('.').next().unwrap_or(template);
        numeric
            .iter()
            .any(|key| key.ends_with(&format!(".{leaf}")) && is_float(key) == float)
    };
    let mut float_reads = Vec::new();
    let mut production = String::new();
    for (path, text) in read_tree_raw(&root()) {
        let relative = Path::new(&path)
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let file = syn::parse_file(&text).unwrap_or_else(|error| panic!("{path} does not parse: {error}"));
        let mut visitor = ConfigReads::default();
        syn::visit::visit_file(&mut visitor, &file);
        assert!(
            visitor.lenient_clamps.is_empty(),
            "{relative}: {:?} clamped where read; give each key a row in \
             common::config::NUMERIC_LIMITS (or FLOAT_LIMITS) and read it with cfg.limit",
            visitor.lenient_clamps
        );
        for (method, key) in visitor.reads {
            let float = method.starts_with("f64") || method == "limit_f64";
            if matches!(key, KeyArg::Computed | KeyArg::Template(_)) {
                computed_files.insert(relative.clone());
            }
            match key {
                KeyArg::Literal(key) => {
                    match method.as_str() {
                        "limit" => assert!(int_row(&key), "{relative}: limit(\"{key}\") has no NUMERIC_LIMITS row"),
                        "limit_f64" => assert!(
                            float_row(&key),
                            "{relative}: limit_f64(\"{key}\") has no FLOAT_LIMITS row"
                        ),
                        _ => assert!(
                            !int_row(&key) && !float_row(&key),
                            "{relative}: {method}(\"{key}\") bypasses the key's common::config limit row; \
                             read it with cfg.limit or cfg.limit_f64"
                        ),
                    }
                    assert!(
                        numeric.contains(&key),
                        "{relative}: {method}(\"{key}\") reads a number the shipped default does not declare, \
                         so AppConfig::load cannot type-check it. Ship the key in assets/config.default.yaml."
                    );
                    assert_eq!(
                        is_float(&key),
                        float,
                        "{relative}: {method}(\"{key}\") disagrees with common::config::FLOAT_KEYS"
                    );
                    if float {
                        float_reads.push(key);
                    }
                }
                KeyArg::Template(template) if template.rsplit('.').next().is_some_and(|leaf| leaf.contains('{')) => {
                    assert!(
                        COMPUTED_NUMERIC_KEYS.contains(&relative.as_str()),
                        "{relative}: {method}(format!(\"{template}\")) computes its key; register the file in \
                         COMPUTED_NUMERIC_KEYS with the literal table it reads from"
                    )
                }
                KeyArg::Template(template) => {
                    assert!(
                        leaf_matches(&template, float),
                        "{relative}: {method}(format!(\"{template}\")) matches no shipped {} key",
                        if float { "FLOAT_KEYS" } else { "integer" }
                    );
                    if float {
                        float_reads.push(template);
                    }
                }
                KeyArg::Computed => assert!(
                    COMPUTED_NUMERIC_KEYS.contains(&relative.as_str()),
                    "{relative}: {method} takes a computed key; name the key literally or register the file \
                     in COMPUTED_NUMERIC_KEYS with the literal table it reads from"
                ),
            }
        }
        for parsed in visitor.parses {
            assert!(
                matches!(&parsed, KeyArg::Literal(text) if text == "<embedded default>" || text == "{}"),
                "{relative}: AppConfig::from_str({parsed:?}) in production; operator config must go \
                 through AppConfig::load, which type-checks numbers"
            );
        }
        production.push_str(&strip_comment_lines(&text));
    }
    for file in COMPUTED_NUMERIC_KEYS {
        assert!(
            computed_files.contains(*file),
            "COMPUTED_NUMERIC_KEYS lists {file}, which no longer computes a numeric key"
        );
    }
    for key in FLOAT_KEYS {
        let leaf = key.rsplit('.').next().unwrap();
        assert!(
            float_reads
                .iter()
                .any(|read| read == key || read.ends_with(&format!(".{leaf}")) && read.contains('{'))
                || production.contains(&format!("\"{key}\"")),
            "FLOAT_KEYS lists {key}, but no production code reads it"
        );
    }
}

/// Every `NUMERIC_LIMITS` / `FLOAT_LIMITS` row is documented in the shipped
/// default on its key's line, as "clamped to A–B" or "refused outside A–B"
/// per its policy, and every such comment matches a row, so the YAML cannot
/// drift from the range the code applies. `KiB`/`MiB` scale a bound;
/// "0, or clamped to" is the row that keeps `0`.
#[test]
fn clamped_config_comments_match_their_limit_rows() {
    use cgagentharness::common::config::{AppConfig, OutOfRange, FLOAT_LIMITS, NUMERIC_LIMITS};
    let range =
        regex::Regex::new(r"(0, or )?(clamped to|refused outside) ([0-9.]+) ?(KiB|MiB)?[–-]([0-9.]+) ?(KiB|MiB)?")
            .unwrap();
    let scale = |unit: Option<regex::Match<'_>>| match unit.map(|u| u.as_str()) {
        Some("KiB") => 1024.0,
        Some("MiB") => 1024.0 * 1024.0,
        _ => 1.0,
    };
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut documented = std::collections::BTreeSet::new();
    for line in AppConfig::embedded_default().lines() {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') || body.starts_with('-') {
            continue;
        }
        let indent = line.len() - body.len();
        let Some((key, _)) = body.split_once(':') else { continue };
        path.retain(|(depth, _)| *depth < indent);
        path.push((indent, key.trim().to_string()));
        let Some(caps) = line.split_once('#').and_then(|(_, comment)| range.captures(comment)) else {
            continue;
        };
        let key = path.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>().join(".");
        let min = caps[3].parse::<f64>().unwrap() * scale(caps.get(4));
        let max = caps[5].parse::<f64>().unwrap() * scale(caps.get(6));
        let policy = match (caps.get(1).is_some(), &caps[2]) {
            (true, "clamped to") => OutOfRange::ClampKeepZero,
            (false, "clamped to") => OutOfRange::Clamp,
            (false, _) => OutOfRange::Refuse,
            (true, _) => panic!("{key}: \"0, or refused outside\" is not a policy"),
        };
        assert!(documented.insert(key.clone()), "{key} is documented twice");
        if let Some(row) = NUMERIC_LIMITS.iter().find(|row| row.key == key) {
            assert_eq!(
                (row.min as f64, row.max as f64, row.out_of_range),
                (min, max, policy),
                "config.default.yaml documents {key} as {}; its NUMERIC_LIMITS row says {}–{} ({:?})",
                &caps[0],
                row.min,
                row.max,
                row.out_of_range
            );
        } else if let Some(row) = FLOAT_LIMITS.iter().find(|row| row.key == key) {
            assert_eq!(
                (row.min, row.max, row.out_of_range),
                (min, max, policy),
                "{key}: {}",
                &caps[0]
            );
        } else {
            panic!(
                "config.default.yaml says {key} is {}, but it has no limit row",
                &caps[0]
            );
        }
    }
    for key in NUMERIC_LIMITS
        .iter()
        .map(|row| row.key)
        .chain(FLOAT_LIMITS.iter().map(|row| row.key))
    {
        assert!(
            documented.contains(key),
            "{key} has a limit row, but its config.default.yaml line has no \"clamped to\" or \"refused outside\" comment"
        );
    }
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

/// One function the async-blocking scan read: what it calls directly and by bare name. A closure
/// handed to the blocking pool (`off_worker`, `spawn_blocking` or a [`pool_wrappers`] helper) is
/// left out; any other closure, and each `select!` or `join!` arm, is read as the function's own.
#[derive(Default)]
struct BlockingFacts {
    file: String,
    name: String,
    is_async: bool,
    direct: Vec<String>,
    callees: std::collections::BTreeSet<String>,
}

struct BlockingScan<'p> {
    file: String,
    stack: Vec<BlockingFacts>,
    closure: usize,
    out: Vec<BlockingFacts>,
    /// Calls whose closure argument runs on the blocking pool.
    pool: &'p std::collections::BTreeSet<String>,
    /// Set while the next closure visited is an argument of one of those calls.
    pool_arg: bool,
}

/// Method calls that park the thread whatever their receiver: SQLite, tempfile creation, fsync.
const BLOCKING_METHODS: &[&str] = &[
    "query_row",
    "execute_batch",
    "tempfile",
    "tempfile_in",
    "sync_all",
    "sync_data",
];

fn is_test_item(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let last = a
            .path()
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        match (&a.meta, last.as_str()) {
            (_, "test") => true,
            (syn::Meta::List(l), "cfg") => {
                let t = l.tokens.to_string();
                t.contains("test") && !t.contains("not")
            }
            _ => false,
        }
    })
}

/// The calls that park the thread they run on: file system (std, cap_std, tempfile), SQLite open
/// and the password hash.
fn blocking_call(path: &syn::Path) -> Option<String> {
    const FS: &[&str] = &[
        "read",
        "read_to_string",
        "write",
        "create_dir_all",
        "create_dir",
        "remove_file",
        "remove_dir_all",
        "remove_dir",
        "rename",
        "copy",
        "metadata",
        "read_dir",
        "canonicalize",
        "symlink_metadata",
        "set_permissions",
    ];
    const HASH: &[&str] = &[
        "hash_password",
        "verify_password",
        "hash_bootstrap_password",
        "hash_password_with_salt",
    ];
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    let last = segs.last()?.as_str();
    let prev = if segs.len() >= 2 {
        segs[segs.len() - 2].as_str()
    } else {
        ""
    };
    let named = match (prev, last) {
        ("fs", f) if FS.contains(&f) => true,
        ("File", "open" | "create") | ("OpenOptions", "new") => true,
        ("Connection", "open" | "open_in_memory") => true,
        // The persona read in #1007 opened a cap_std directory; the task file in #1046 was a tempfile.
        ("Dir", "open_ambient_dir") | ("NamedTempFile", "new" | "new_in") | ("tempfile", "tempfile") => true,
        (_, h) if HASH.contains(&h) => true,
        _ => false,
    };
    named.then(|| {
        if prev.is_empty() {
            last.to_string()
        } else {
            format!("{prev}::{last}")
        }
    })
}

impl BlockingScan<'_> {
    fn live(&self) -> bool {
        self.closure == 0 && !self.stack.is_empty()
    }
    /// Visit a pool call's arguments: a closure among them runs on the pool, not here.
    fn visit_pool_args<'ast>(&mut self, args: impl IntoIterator<Item = &'ast syn::Expr>) {
        use syn::visit::Visit;
        for arg in args {
            self.pool_arg = matches!(arg, syn::Expr::Closure(_));
            self.visit_expr(arg);
            self.pool_arg = false;
        }
    }
    fn enter(&mut self, name: String, is_async: bool, walk: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.closure, 0);
        self.stack.push(BlockingFacts {
            file: self.file.clone(),
            name,
            is_async,
            ..Default::default()
        });
        walk(self);
        let done = self.stack.pop().unwrap();
        self.out.push(done);
        self.closure = saved;
    }
}

impl<'ast> syn::visit::Visit<'ast> for BlockingScan<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if !is_test_item(&m.attrs) {
            syn::visit::visit_item_mod(self, m);
        }
    }
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !is_test_item(&f.attrs) {
            self.enter(f.sig.ident.to_string(), f.sig.asyncness.is_some(), |s| {
                syn::visit::visit_item_fn(s, f)
            });
        }
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_test_item(&f.attrs) {
            self.enter(f.sig.ident.to_string(), f.sig.asyncness.is_some(), |s| {
                syn::visit::visit_impl_item_fn(s, f)
            });
        }
    }
    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        // Only a closure handed to the pool leaves this function. `(|| ..)()` and `.then(|| ..)`
        // run right here: #1007's persona read and the carried-reads policy read sat in those.
        let pooled = usize::from(std::mem::take(&mut self.pool_arg));
        self.closure += pooled;
        syn::visit::visit_expr_closure(self, c);
        self.closure -= pooled;
    }
    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        let mut pooled = false;
        if let syn::Expr::Path(p) = &*c.func {
            if let Some(last) = p.path.segments.last() {
                pooled = self.pool.contains(&last.ident.to_string());
                if self.live() {
                    let top = self.stack.last_mut().unwrap();
                    top.direct.extend(blocking_call(&p.path));
                    top.callees.insert(last.ident.to_string());
                }
            }
        }
        if pooled {
            self.visit_expr(&c.func);
            self.visit_pool_args(&c.args);
        } else {
            syn::visit::visit_expr_call(self, c);
        }
    }
    fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
        let method = c.method.to_string();
        if self.live() {
            let top = self.stack.last_mut().unwrap();
            if BLOCKING_METHODS.contains(&method.as_str()) {
                top.direct.push(format!(".{method}"));
            }
            top.callees.insert(method.clone());
        }
        if self.pool.contains(&method) {
            self.visit_expr(&c.receiver);
            self.visit_pool_args(&c.args);
        } else {
            syn::visit::visit_expr_method_call(self, c);
        }
    }
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        // A macro body is a token stream to syn. Long async fns run their main work inside
        // `tokio::select!` (chat_inner's turn does), so its arms and `join!`'s are parsed and read.
        let name = m.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        let arms = match name.as_str() {
            "select" => m.parse_body_with(select_arms).ok(),
            "join" | "try_join" => m
                .parse_body_with(syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated)
                .ok()
                .map(|exprs| exprs.into_iter().collect()),
            _ => None,
        };
        for arm in arms.iter().flatten() {
            self.visit_expr(arm);
        }
    }
}

/// The expressions in a `tokio::select!` body: each branch's future, its `if` precondition and
/// its handler (`biased;` and `else` are skipped). A body this cannot read stays unread.
fn select_arms(input: syn::parse::ParseStream) -> syn::Result<Vec<syn::Expr>> {
    let mut out = Vec::new();
    if input.peek(syn::Ident) && input.fork().parse::<syn::Ident>()? == "biased" {
        input.parse::<syn::Ident>()?;
        input.parse::<syn::Token![;]>()?;
    }
    while !input.is_empty() {
        if input.peek(syn::Token![else]) {
            input.parse::<syn::Token![else]>()?;
        } else {
            syn::Pat::parse_multi_with_leading_vert(input)?;
            input.parse::<syn::Token![=]>()?;
            out.push(input.call(syn::Expr::parse_without_eager_brace)?);
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
                input.parse::<syn::Token![if]>()?;
                out.push(input.call(syn::Expr::parse_without_eager_brace)?);
            }
        }
        input.parse::<syn::Token![=>]>()?;
        out.push(input.parse()?);
        if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
        }
    }
    Ok(out)
}

/// Whether a type or bound names `Fn`, `FnOnce` or `FnMut`.
#[derive(Default)]
struct NamesFn(bool);

impl<'ast> syn::visit::Visit<'ast> for NamesFn {
    fn visit_path_segment(&mut self, s: &'ast syn::PathSegment) {
        self.0 |= s.ident == "Fn" || s.ident == "FnOnce" || s.ident == "FnMut";
        syn::visit::visit_path_segment(self, s);
    }
}

/// The parameters of `sig` that take a closure: `impl Fn..`, `&dyn Fn..` or a generic bounded by one.
fn closure_params(sig: &syn::Signature) -> std::collections::BTreeSet<String> {
    use syn::visit::Visit;
    let bounded = |bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>| {
        let mut v = NamesFn::default();
        bounds.iter().for_each(|b| v.visit_type_param_bound(b));
        v.0
    };
    let mut generic = std::collections::BTreeSet::new();
    for param in &sig.generics.params {
        if let syn::GenericParam::Type(t) = param {
            if bounded(&t.bounds) {
                generic.insert(t.ident.to_string());
            }
        }
    }
    for pred in sig.generics.where_clause.iter().flat_map(|w| &w.predicates) {
        if let syn::WherePredicate::Type(t) = pred {
            if let (true, syn::Type::Path(p)) = (bounded(&t.bounds), &t.bounded_ty) {
                generic.extend(p.path.get_ident().map(|i| i.to_string()));
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(t) = input else { continue };
        let named_generic = matches!(&*t.ty, syn::Type::Path(p)
            if p.path.get_ident().is_some_and(|i| generic.contains(&i.to_string())));
        let mut v = NamesFn::default();
        v.visit_type(&t.ty);
        if let (true, syn::Pat::Ident(id)) = (named_generic || v.0, &*t.pat) {
            out.insert(id.ident.to_string());
        }
    }
    out
}

/// Every identifier an expression mentions.
#[derive(Default)]
struct Idents(std::collections::BTreeSet<String>);

impl<'ast> syn::visit::Visit<'ast> for Idents {
    fn visit_ident(&mut self, i: &'ast syn::Ident) {
        self.0.insert(i.to_string());
    }
}

/// Every call in one function body (nested fns skipped): the callee's last name and the
/// identifiers its arguments mention.
#[derive(Default)]
struct CallArgs(Vec<(String, std::collections::BTreeSet<String>)>);

impl CallArgs {
    fn push<'a>(&mut self, callee: String, args: impl IntoIterator<Item = &'a syn::Expr>) {
        use syn::visit::Visit;
        let mut idents = Idents::default();
        args.into_iter().for_each(|a| idents.visit_expr(a));
        self.0.push((callee, idents.0));
    }
}

impl<'ast> syn::visit::Visit<'ast> for CallArgs {
    fn visit_item_fn(&mut self, _: &'ast syn::ItemFn) {}
    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func {
            if let Some(last) = p.path.segments.last() {
                self.push(last.ident.to_string(), &c.args);
            }
        }
        syn::visit::visit_expr_call(self, c);
    }
    fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
        self.push(c.method.to_string(), &c.args);
        syn::visit::visit_expr_method_call(self, c);
    }
}

/// Each function that takes a closure: its name, those parameters and the calls its body makes.
#[derive(Default)]
struct ClosureTakers(Vec<(String, std::collections::BTreeSet<String>, CallArgs)>);

impl ClosureTakers {
    fn add(&mut self, sig: &syn::Signature, body: &syn::Block) {
        use syn::visit::Visit;
        let params = closure_params(sig);
        if !params.is_empty() {
            let mut calls = CallArgs::default();
            calls.visit_block(body);
            self.0.push((sig.ident.to_string(), params, calls));
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for ClosureTakers {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.add(&f.sig, &f.block);
        syn::visit::visit_item_fn(self, f);
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.add(&f.sig, &f.block);
        syn::visit::visit_impl_item_fn(self, f);
    }
}

/// The calls whose closure argument runs on the blocking pool: the entry points, plus every
/// function that passes one of its own closure parameters into such a call (to a fixed point),
/// as `commit_setting` and `account_change` do. A function that takes a closure and runs it
/// inline, like `web_research::run_with`'s `on_start`, is not one.
fn pool_wrappers(files: &[syn::File]) -> std::collections::BTreeSet<String> {
    use syn::visit::Visit;
    let mut takers = ClosureTakers::default();
    files.iter().for_each(|f| takers.visit_file(f));
    // A thread's closure runs off the async workers too; `tokio::spawn` takes a future, not a closure.
    let mut pool: std::collections::BTreeSet<String> = ["spawn_blocking", "block_in_place", "off_worker", "spawn"]
        .map(String::from)
        .into();
    loop {
        let before = pool.len();
        for (name, params, calls) in &takers.0 {
            let hands_over = calls
                .0
                .iter()
                .any(|(callee, args)| callee != name && pool.contains(callee) && !args.is_disjoint(params));
            if hands_over {
                pool.insert(name.clone());
            }
        }
        if pool.len() == before {
            return pool;
        }
    }
}

/// `file :: fn` for every non-test `async fn` that calls blocking work inline: directly, or through
/// a sync function of a name no other function in `sources` shares and that itself blocks. A name
/// two functions share is skipped, since without types `get` or `new` cannot be told apart.
fn async_blocking_violations(sources: &[(String, String)]) -> Vec<String> {
    use std::collections::BTreeMap;
    use syn::visit::Visit;
    let files: Vec<syn::File> = sources
        .iter()
        .map(|(path, text)| syn::parse_file(text).unwrap_or_else(|e| panic!("{path} does not parse as Rust: {e}")))
        .collect();
    let pool = pool_wrappers(&files);
    let mut facts = Vec::new();
    for ((path, _), file) in sources.iter().zip(&files) {
        let mut scan = BlockingScan {
            file: path.clone(),
            stack: Vec::new(),
            closure: 0,
            out: Vec::new(),
            pool: &pool,
            pool_arg: false,
        };
        scan.visit_file(file);
        facts.extend(scan.out);
    }
    let mut defs: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &facts {
        *defs.entry(f.name.as_str()).or_default() += 1;
    }
    let unique = |f: &BlockingFacts| defs[f.name.as_str()] == 1;
    let mut blocking: std::collections::BTreeSet<String> = facts
        .iter()
        .filter(|f| !f.is_async && unique(f) && !f.direct.is_empty())
        .map(|f| f.name.clone())
        .collect();
    loop {
        let before = blocking.len();
        for f in facts.iter().filter(|f| !f.is_async && unique(f)) {
            if f.callees.iter().any(|c| blocking.contains(c)) {
                blocking.insert(f.name.clone());
            }
        }
        if blocking.len() == before {
            break;
        }
    }
    let mut found: Vec<String> = facts
        .iter()
        .filter(|f| f.is_async && (!f.direct.is_empty() || f.callees.iter().any(|c| blocking.contains(c))))
        .map(|f| format!("{} :: {}", f.file, f.name))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Async functions that still call blocking work inline, as of this guard's introduction. Each was
/// read, not fixed: the list may only shrink. Fix the function (see `routes::structured_memory::off_worker`)
/// and delete its row; the test fails on a row that no longer matches.
const ASYNC_BLOCKING_BASELINE: &[&str] = &[
    "src/common/mcp_worker.rs :: run",
    "src/server/chat_web.rs :: run_inner",
    "src/server/chat_web/evidence.rs :: read",
    "src/server/mod.rs :: build_app_with_sources",
    "src/server/web_google.rs :: google_hop",
    "src/server/web_index.rs :: crawl",
    "src/server/web_index.rs :: fetch_many",
    "src/server/web_index.rs :: search_with_count",
    "src/server/web_research.rs :: run_with",
    "src/server/web_search.rs :: fetch_page",
];

#[test]
fn async_fns_leave_blocking_file_and_database_work_to_the_blocking_pool() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for dir in ["src/server", "src/shim", "src/common", "src/llm", "src/netconnect"] {
        for (path, text) in read_tree_raw(&root.join(dir)) {
            let relative = Path::new(&path).strip_prefix(root).unwrap().display().to_string();
            sources.push((relative.replace('\\', "/"), text));
        }
    }
    let found = async_blocking_violations(&sources);
    let new: Vec<_> = found
        .iter()
        .filter(|f| !ASYNC_BLOCKING_BASELINE.contains(&f.as_str()))
        .collect();
    assert!(
        new.is_empty(),
        "these async fns call file, SQLite or password-hash work inline, which parks an async worker: {new:#?}. \
         Move the work into `routes::structured_memory::off_worker(move || ..)` (or `spawn_blocking`) and await it."
    );
    let stale: Vec<_> = ASYNC_BLOCKING_BASELINE
        .iter()
        .filter(|b| !found.iter().any(|f| f == *b))
        .collect();
    assert!(
        stale.is_empty(),
        "fixed or renamed, so delete them from ASYNC_BLOCKING_BASELINE: {stale:#?}"
    );
}

#[test]
fn the_async_blocking_scan_reads_what_it_claims_to() {
    let src = |text: &str| vec![("x.rs".to_string(), text.to_string())];
    let flagged = |text: &str| async_blocking_violations(&src(text));
    // A direct call in an async fn is flagged; the same call in a closure handed to the pool is not.
    assert_eq!(
        flagged("async fn a() { std::fs::write(p, b).unwrap(); }"),
        ["x.rs :: a"]
    );
    assert!(flagged("async fn a() { off_worker(move || { std::fs::write(p, b).unwrap(); }).await; }").is_empty());
    // A uniquely named sync helper carries the block to its async caller, two hops deep.
    assert_eq!(
        flagged("fn save() { std::fs::write(p, b).unwrap(); } fn persist() { save(); } async fn a() { persist(); }"),
        ["x.rs :: a"]
    );
    // A name two functions share proves nothing, and test code is skipped.
    assert!(flagged("fn get() { std::fs::read(p); } fn get() {} async fn a() { get(); }").is_empty());
    assert!(flagged("#[tokio::test] async fn a() { std::fs::read(p); }").is_empty());
    assert!(flagged("#[cfg(test)] mod t { async fn a() { std::fs::read(p); } }").is_empty());
    // A sync fn that blocks is fine on its own.
    assert!(flagged("fn a() { std::fs::read(p); }").is_empty());
    assert!(flagged("async fn a() { let x = hash_password(p); }") == ["x.rs :: a"]);
    // Shapes the earlier scan passed, from real fixes. A closure not handed to the pool runs
    // here: #1007's persona read sat in `(|| ..)()` through a cap_std directory, and the
    // carried-reads policy read in `.then(|| ..)`.
    let persona = "fn load_text() { let r = (|| cap_std::fs::Dir::open_ambient_dir(p, a))(); }";
    assert_eq!(
        flagged(&format!("{persona} async fn a() {{ on.then(|| load_text()); }}")),
        ["x.rs :: a"]
    );
    // #1046: the /browse task file was a tempfile written from a sync helper.
    let task_file = "fn write_task_file() { tempfile::Builder::new().prefix(\"t\").tempfile(); }";
    assert_eq!(
        flagged(&format!("{task_file} async fn a() {{ write_task_file(); }}")),
        ["x.rs :: a"]
    );
    // A `select!` arm and a `join!` argument are the function's own work.
    assert_eq!(
        flagged("async fn a() { tokio::select! { r = f() => { std::fs::read(p); } _ = c.cancelled() => {} } }"),
        ["x.rs :: a"]
    );
    assert_eq!(
        flagged("async fn a() { tokio::join!(f(), async { std::fs::read(p) }); }"),
        ["x.rs :: a"]
    );
    // A helper that passes its closure parameter to the pool is a pool entry point, as a method
    // too; one that runs its closure inline is not.
    let wrapper = "async fn on_store<T>(work: impl FnOnce() -> T) -> T { off_worker(move || work()).await }";
    assert!(flagged(&format!(
        "{wrapper} async fn a() {{ on_store(|| std::fs::read(p)).await; }}"
    ))
    .is_empty());
    assert!(flagged("async fn a(w: W) { w.off_worker(move |web, _| std::fs::read(p)).await; }").is_empty());
    let inline = "async fn run_with(on_start: impl FnOnce()) { on_start(); spawn_blocking(|| 1).await; }";
    assert_eq!(
        flagged(&format!(
            "{inline} async fn a() {{ run_with(|| {{ std::fs::read(p); }}).await; }}"
        )),
        ["x.rs :: a"]
    );
}

/// What one closure body mentions, for the owner-recheck scan.
#[derive(Default)]
struct ClosureFacts {
    user_id: bool,
    must_change_password: bool,
    grants_stored: bool,
}

impl<'ast> syn::visit::Visit<'ast> for ClosureFacts {
    fn visit_expr_field(&mut self, f: &'ast syn::ExprField) {
        if let syn::Member::Named(name) = &f.member {
            match name.to_string().as_str() {
                "user_id" => self.user_id = true,
                "must_change_password" => self.must_change_password = true,
                _ => {}
            }
        }
        syn::visit::visit_expr_field(self, f);
    }
    fn visit_path(&mut self, p: &'ast syn::Path) {
        self.grants_stored |= p.segments.last().is_some_and(|s| s.ident == "grants_stored");
        syn::visit::visit_path(self, p);
    }
}

struct OwnerCheckScan {
    copies: usize,
}

impl<'ast> syn::visit::Visit<'ast> for OwnerCheckScan {
    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        let mut facts = ClosureFacts::default();
        syn::visit::visit_expr(&mut facts, &c.body);
        if facts.user_id && facts.must_change_password && facts.grants_stored {
            self.copies += 1;
        }
        syn::visit::visit_expr_closure(self, c);
    }
}

/// Files with a closure that rebuilds "this account may still do X" from a user
/// row: it names `user_id`, `must_change_password` and `grants_stored` together,
/// the shape every copy of the owner recheck had (`list_users().iter().any(..)`).
fn copied_owner_checks(sources: &[(String, String)]) -> Vec<String> {
    use syn::visit::Visit;
    let mut found = Vec::new();
    for (path, text) in sources {
        let file = syn::parse_file(text).unwrap_or_else(|e| panic!("{path} does not parse as Rust: {e}"));
        let mut scan = OwnerCheckScan { copies: 0 };
        scan.visit_file(&file);
        if scan.copies > 0 {
            found.push(format!("{path} ({} closure(s))", scan.copies));
        }
    }
    found
}

#[test]
fn the_owner_recheck_is_one_function_not_a_copy_per_caller() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for dir in ["src/server", "src/shim", "src/llm", "src/netconnect"] {
        for (path, text) in read_tree_raw(&root.join(dir)) {
            let relative = Path::new(&path).strip_prefix(root).unwrap().display().to_string();
            sources.push((relative.replace('\\', "/"), text));
        }
    }
    let copies = copied_owner_checks(&sources);
    assert!(
        copies.is_empty(),
        "these files rebuild the owner recheck by hand: {copies:#?}. Call \
         `AuthManager::owner_permits(user_id, Capability)` (or `owner_lapse` for the reason, or \
         `UserSummary::lapse` on a row you already hold) and map the answer to your own error code."
    );
}

#[test]
fn the_owner_recheck_scan_reads_what_it_claims_to() {
    let src = |text: &str| vec![("x.rs".to_string(), text.to_string())];
    let copy = "fn f(a: &A, owner: &str) -> bool { a.list_users().iter().any(|u| u.user_id == owner \
                && !u.disabled && !u.must_change_password && authz::grants_stored(&u.role, Cap)) }";
    assert_eq!(copied_owner_checks(&src(copy)), ["x.rs (1 closure(s))"]);
    // A request-time check on a row already in hand names no `user_id`.
    let in_hand = "fn f(u: Option<U>) -> bool { u.is_some_and(|u| authz::grants_stored(&u.role, Cap) \
                   && !u.must_change_password) }";
    assert!(copied_owner_checks(&src(in_hand)).is_empty());
    // Finding an account by id is not the capability check.
    let lookup = "fn f(v: &[U], id: &str) -> bool { v.iter().any(|u| u.user_id == id) }";
    assert!(copied_owner_checks(&src(lookup)).is_empty());
    // The shared call is clean.
    let shared = "fn f(a: &A, owner: &str) -> bool { a.owner_permits(owner, Cap) }";
    assert!(copied_owner_checks(&src(shared)).is_empty());
}

#[test]
fn console_asset_is_verbatim_with_both_placeholders() {
    let html = cgagentharness::server::console::HARNESS_HTML;
    assert!(html.contains("__CYCLAW_CSRF_TOKEN__"));
    assert!(html.contains("__CYCLAW_CSP_NONCE__"));
    assert!(html.contains("/static/auth_admin.js"));
    // The asset mentions localStorage once, in a comment saying it never uses it; the API call shape is what matters.
    for api in ["localStorage.", "document.cookie"] {
        assert!(
            !html.contains(api),
            "the console keeps the API key out of storage ({api})"
        );
    }
    // why: #1123 (operator decision) keeps one tab-scoped pair, {session_id,
    // job_id}, so a reload finds a waiting /browse task. Every mention of
    // sessionStorage is one of these three calls on that one key.
    let keyed: usize = [
        "sessionStorage.getItem(BROWSE_JOB_KEY)",
        "sessionStorage.setItem(BROWSE_JOB_KEY, JSON.stringify({ session_id: session || null, job_id: id }))",
        "sessionStorage.removeItem(BROWSE_JOB_KEY)",
    ]
    .iter()
    .map(|call| html.matches(call).count())
    .sum();
    assert_eq!(
        html.matches("sessionStorage").count(),
        keyed,
        "sessionStorage holds only the browser-task pair, through BROWSE_JOB_KEY"
    );
    assert_eq!(html.matches("const BROWSE_JOB_KEY = ").count(), 1);
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
    // why: operator-requested (2026-11-26) cg-doc-sync skill: whole-tree docs audit plus a 5-15% trim; 797 words,
    // rounded up to the next 100.
    // why: operator-requested (2026-11-26) merge of doc-sync's diff mode and rules into cg-doc-sync, whose 800-word
    // row is removed with it; 945 words, rounded up to the next 100.
    (".claude/skills/cg-doc-sync/SKILL.md", Agent, 1000),
    (".claude/skills/cgagentharness-config-guard/SKILL.md", Agent, 900),
    (".claude/skills/cgagentharness-gotchas/SKILL.md", Agent, 1700),
    (".claude/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 600),
    // why: operator-approved raise from 1400 to 1776 for the optimize skill rewrite. The file was 1277 words then
    // and 1520 on 2026-11-22; the Agent group cap, not this row, is the binding limit.
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
    // why: operator decision (2026-11-21): +130 words so dep-sync can name every cargo deny
    // command CI runs (all features, the cargo-sandbox fixture lock, the cargo-deny floor).
    (".claude/skills/dep-sync/SKILL.md", Agent, 1100),
    (".claude/skills/fable-protocol/SKILL.md", Agent, 990),
    (".claude/skills/run-cg-agent-harness/SKILL.md", Agent, 500),
    (".claude/skills/verification-specialist/SKILL.md", Agent, 740),
    (".codex/skills/cgagentharness-config-guard/SKILL.md", Agent, 300),
    (".codex/skills/cgagentharness-gotchas/SKILL.md", Agent, 390),
    (".codex/skills/cgagentharness-invariant-guard/SKILL.md", Agent, 370),
    (".codex/skills/cgagentharness-optimize/SKILL.md", Agent, 220),
    (".codex/skills/cgagentharness-project-guidance/SKILL.md", Agent, 190),
    (".codex/skills/cgagentharness-release/SKILL.md", Agent, 290),
    (".codex/skills/cgagentharness-verify/SKILL.md", Agent, 170),
    (".codex/skills/cgagentharness-write-policy-redteam/SKILL.md", Agent, 350),
    (".codex/skills/fable-protocol/SKILL.md", Agent, 270),
    (".codex/skills/verification-specialist/SKILL.md", Agent, 310),
    // why: operator-approved (2026-10-14) raise to 2200 after the change-type, merge-order and ELI5 guidance grew it to 2162 words.
    (".github/PULL_REQUEST_TEMPLATE.md", Agent, 2400),
    (".github/skills/repo-optimize/SKILL.md", Agent, 400),
    // why: operator-requested per-file index of the folder (one line per file); size plus about 10%, rounded up to the next 10.
    // why: operator-approved (2026-11-01) raise from 460 to 500 for the workflow index; the file was at its cap after the desktop-check trigger fix.
    // why: Christopher, 2026-11-18 1:56 PM ET: "increase word cap 100 words". Raise this row from 500 to 600 rather than trim.
    (".github/workflows/README.md", Agent, 600),
    // why: netconnect passive CLI and LAN scope rule; rounded up to the next 100.
    // why: OS credential store sentence for managed provider keys; 1907 words after the main merge, rounded up to the next 100.
    // why: feature contracts folded into an INVARIANTS.md section table; 1491 words, rounded up to the next 100.
    // why: operator-approved raise (1508 words after the project-skills heading edit on main); set to 1510.
    // why: operator-approved headroom for research-job steps 8-12 (#415 review roadmap): ~45 words
    // (gate list, /loop excludes MCP, no job-long gate), plus ~10%, rounded up to the next 50.
    // why: the 2026-10-13 doc-sync cut it to 1441 words; 1490 keeps the ~50-word roadmap allowance above.
    // why: #550/#552 self-manual and red-team guidance grew it to 1459; standing rule is to raise the cap to the reported count, never trim gate text.
    // why: operator-approved (2026-11-02) raise from 1452 to 1776 for the strict-config trap and the
    // strict-by-default rule for unspecified lenient-vs-strict choices (#764).
    // why: refreshed feature pointers and trimmed duplicate guidance to 1456 words; retain the cut.
    // why: operator-approved (2026-11-10) rules from the PR #742-#810 review: stale policy, secret-bearing assert
    // messages, stale console replies, tests for changed defaults. 1527 words, rounded up to the next 10.
    // why: operator-requested (2026-11-10) four facts the #807 audit held back at the cap: hidden
    // subcommand exit codes, the console MAX_GOAL_CHARS duplicate, the cyclaw/ prefix and the
    // Copilot skill scope; raised to the reported count.
    // why: operator decision (2026-11-12): set to 1776, the cap it had on 2026-11-02; the file sat exactly at 1541, so every added word needed a cut.
    // why: operator-requested (2026-11-22) ~30% trim (1775 to 1248 words): the architecture diagram and
    // restated stack details went, every rule stayed; lowered to 1250.
    // why: operator-requested (2026-11-26) 10-20% refresh-and-trim (1241 to 1113 words): restatements the
    // template, CLAUDE.md and INVARIANTS.md already own went, every rule stayed; lowered to 1140.
    ("AGENTS.md", Agent, 1140),
    // why: the per-session summary moved here from .claude/CLAUDE.md so Claude Code loads one file.
    // why: now `@AGENTS.md` plus Claude-only lines; 183 words, rounded up to the next 100.
    // why: operator-requested (2026-11-22) ~10% trim (377 to 334 words); lowered to 340.
    ("CLAUDE.md", Agent, 340),
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
    // why: operator-approved (2026-11-20) +200 for the #965 route capability table contract.
    // why: operator-approved (2026-11-25) +10 so the takeover sentence names the late-submit window exactly (#1138).
    ("INVARIANTS.md", Root, 10_210),
    // why: README refresh folded Origins into the intro and trimmed duplicates; 1443 words, rounded up to the next 100.
    // why: operator-approved (2026-10-14) macOS installer and launcher pointer in Quick start; 1489 words, rounded up to the next 50.
    // why: dependency refresh removed five stale Linux prerequisite words; keep the cut.
    // why: the 2026-11-01 README refresh trimmed it to 1,507 words; lowered so the cut stays cut.
    // why: operator-approved (2026-11-05) raise to the measured count for the opt-in Firecrawl fetch fallback (phase 2 of web search).
    // why: SearXNG removal (phase 3 of web search) trimmed it to 1,521 words; lowered so the cut stays cut.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: operator-set (2026-11-06) allowance of 1,776 for the long-form research reframe, not a measured count;
    // the Root group cap stays the binding limit.
    // why: operator-requested (2026-11-08) 15% trim with the research-loop router section added; measured
    // 1,325 words, rounded up to the next 10 so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 107 words net of the post-#834 additions; measured 1223, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200; the 2026-11-11 trim left 7 words of room.
    // why: the 2026-11-15 sync-and-trim pass cut README to 1138 words; rounded up to the next 10 so the cut stays cut.
    // why: the 2026-11-17 doc sync cut 116 duplicated words (1022); rounded up to the next 10 so the cut stays cut.
    // why: the 2026-11-23 refresh-and-trim pass cut README from 929 to 824 words; rounded up to the next 10 so the cut stays cut.
    // why: the 2026-11-25 refresh-and-trim pass cut README from 824 to 741 words; rounded up to the next 10 so the cut stays cut.
    ("README.md", Root, 750),
    ("SECURITY.md", Root, 690),
    // why: operator-approved +100 (2026-10-13) for the goal web-mode default row; file was at 1728.
    // why: the sandboxed HTML preview route row; measured 1840 words.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 78 words net of the post-#834 additions; measured 1675 after review restored one row per method and path.
    // why: operator decision (2026-11-12): cap set to 1776, about 100 words of room, so a new route row does not need a cap change.
    ("docs/API_ROUTES.md", Guide, 880),
    // why: operator-requested (2026-11-14) browser- and computer-use status and roadmap; measured 869, about 100 words of room.
    // why: operator-approved (2026-11-15) +200 after #895, #896 and #897 left it at 967 of 970.
    // why: operator-approved (2026-11-17) +100 for #945: the auth requirement and the file-name check left it at 1169 of 1170.
    // why: operator-approved (2026-11-22) +300 after the takeover-deadline sentence left it at 1270 of 1270.
    // why: operator-approved (2026-11-25) +50 after #1150 and #1153 together left it at 530 of 530.
    ("docs/Browser-Use.md", Guide, 580),
    // why: #240 folded BOUNDED_EDITS.md, GIT_APPROVAL.md and OFFLINE_CARGO.md in here.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 605 words net of the post-#834 additions; measured 2943, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 2950, within a few words of the file.
    ("docs/CODING_PIPELINE.md", Guide, 1240),
    // why: #246 folded CHAT_WORKFLOWS.md and CHAT_STREAMING.md in here.
    // why: read-only /net panel and slash row; 6605 words, rounded up to the next 100.
    // why: operator-approved +150 (2026-10-18), shared by the same four compaction follow-up PRs.
    // why: operator-approved (2026-10-19) measured count with #546 and #550 together, raised instead of trimming.
    // why: the HTML editor preview sentence; measured 6229 words.
    // why: operator-approved (2026-11-05) raise to the measured count for the opt-in Firecrawl fetch fallback (phase 2 of web search).
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 776 words net of the post-#834 additions; measured 5117, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 5120, within a few words of the file.
    // why: operator-requested (2026-11-14) /browse row in the slash quick reference; +30 over 5320.
    ("docs/CONSOLE.md", Guide, 2190),
    // why: dependency refresh removed 72 words of retired Tantivy and Reqwest constraints; keep the cut with prior headroom.
    // why: the 2026-10-20 dep-sync folded the dated 2026-09 audit notes into Retained constraints; 944 words, keep the cut.
    // why: operator-approved +200 (2026-10-22) so dated advisory notes such as RUSTSEC-2026-0384 fit without trimming history.
    // why: the +50 (2026-10-31) for the block-buffer residual is reverted; the advisory no longer matches. 1139 words.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 96 words net (now 963); the cap drops by the same amount, rounded up to the next 10, keeping its earlier headroom.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 980, within a few words of the file.
    // why: the 2026-11-16 sweep merged the two duplicate glib residual notes and added the npm lockfile note; 1157 words, rounded up to the next 10 so the cut stays cut.
    // why: operator-approved (2026-11-17) raise from 1160 to the measured 1220 for the chrono/chrono-tz waiver watch (RUSTSEC-2026-0543/0555).
    ("docs/DEPENDENCIES.md", Guide, 400),
    // why: OS credential store replaces the dotenv-only startup note; 2839 words, rounded up to the next 100.
    // why: operator-approved (2026-11-05) raise to the measured count for the opt-in Firecrawl fetch fallback (phase 2 of web search).
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 353 words net of the post-#834 additions; measured 2064, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 2070, within a few words of the file.
    // why: the 2026-11-15 sync-and-trim pass moved the browser sections to docs/Browser-Use.md (2167 words); rounded up to the next 10.
    ("docs/DESKTOP.md", Guide, 780),
    // why: operator-approved explainer for the agent-neutral .githooks security gate; 1364 words, rounded up to the next 100.
    // why: hook behavior review trims duplicated reuse instructions; retain the smaller budget.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 154 words net of the post-#834 additions; measured 1128, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 1130, within a few words of the file.
    ("docs/GITHOOKS.md", Guide, 350),
    // why: credentials table now describes the OS store and the plaintext opt-in; 5571 words, rounded up to the next 100.
    // Setup memory guidance replaces repeated setup/verification prose; keep the cut.
    // why: operator-approved (2026-10-14) automated macOS install (section 1) and browser access (section 7); 5188 words plus about 1%, rounded up to the next 50.
    // why: operator-approved (2026-10-14) raise to 7500 for room to grow the install guide after #491 and #496 together hit 5250; the Guide group cap still bounds it.
    // why: operator-approved (2026-11-08) consolidation folded CONFIG_RELOAD.md in as the reloadable-limits
    // section; measured count, rounded up to the next 10.
    // why: the 2026-11-11 trim-and-refresh pass cut 1145 words net (now 5666); the cap drops by the same amount, rounded up to the next 10, keeping its earlier headroom.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 5700, within a few words of the file.
    ("docs/INSTALL.md", Guide, 1800),
    // why: operator-approved (2026-11-08) consolidation of MCP_CLIENT.md (894) and MCP_SERVER.md (951) into one
    // guide with a topology diagram; measured count, rounded up to the next 10.
    // why: the 2026-11-11 trim-and-refresh pass cut 191 words net of the post-#834 additions; measured 1737, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 1740, within a few words of the file.
    ("docs/MCP.md", Guide, 690),
    // why: #244 folded MEMORY_SETUP.md and the user manual's memory sections in here.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 210 words net of the post-#834 additions; measured 1474, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 1480, within a few words of the file.
    ("docs/MEMORY_GUIDE.md", Guide, 580),
    // why: #237 documented refused truncated cloud replies.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 163 words net of the post-#834 additions; measured 820, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 820, within a few words of the file.
    ("docs/MODELS.md", Guide, 470),
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 177 words net (now 973); the cap drops by the same amount, rounded up to the next 10, keeping its earlier headroom.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 990, within a few words of the file.
    ("docs/PROCESS_LIFECYCLE.md", Guide, 420),
    // why: operator-requested per-file index of docs/ (3-5 sentences each) replaces the 150-word folder stub; 1103 words, rounded up to the next 100.
    // why: operator-requested reading paths plus an owning-code and tests line per doc; size plus about 5%, rounded up to the next 50.
    // why: README audit removes 44 words; retain the existing headroom.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 319 words net of the post-#834 additions; measured 1036, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 1040, within a few words of the file.
    ("docs/README.md", Guide, 300),
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 92 words net of the post-#834 additions; measured 592, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 600, within a few words of the file.
    ("docs/RELEASING.md", Guide, 250),
    // why: #245 folded WEB.md and ACCOUNTS.md in here.
    // why: API Keys and SerpAPI now name the OS credential store; 4014 words, rounded up to the next 100.
    // why: operator-approved (2026-10-24) raise to 4000 so #649 and #651 fit together without trimming contract text.
    // why: operator-approved (2026-11-05) raise to the measured count for the opt-in Firecrawl fetch fallback (phase 2 of web search).
    // why: SearXNG removal (phase 3 of web search) trimmed it to 4,102 words; lowered so the cut stays cut.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 525 words net of the post-#834 additions; measured 3371, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 3380, within a few words of the file.
    // why: the 2026-11-22 trim-and-update pass (#772 batch and digest contract) measured 3427; rounded up to the next 10 so the cut stays cut.
    // why: the 2026-11-22 D10 pass (#772 provider-unit ledger) trimmed 167 words and added the ledger contract; measured 3369 with #1072's account-recheck sentence, rounded up to the next 10 so the cut stays cut.
    // why: operator-approved (2026-11-25) +200 after #1133, #1134 and #1135 each added 22 or 23 words to the 1165 on main; any two exceed 1190.
    // why: the 2026-11-26 trim cut 49 words (1306 to 1257) to bring Guide back under its group cap; the row drops by the same amount, rounded to 10, so the cut stays cut and the approved headroom is unchanged.
    ("docs/SECURE_RESEARCH.md", Guide, 1340),
    // why: operator-approved (2026-10-14) fold of ANALYTICS.md in here; 2157 words, cap set to 2750 for room to grow.
    // why: the 2026-11-11 trim-and-refresh pass cut 308 words net (now 1747); the cap drops by the same amount, rounded up to the next 10, keeping its earlier headroom.
    ("docs/SPEND_AND_NOTIFICATIONS.md", Guide, 670),
    // why: #244 made this the one memory contract: a gate table, and the #87
    // closeout facts it lacked.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 562 words net of the post-#834 additions; measured 2880, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 2880, within a few words of the file.
    ("docs/STRUCTURED_MEMORY.md", Guide, 1130),
    // why: operator-approved (2026-10-19) measured count with #546 and #550 together, raised instead of trimming.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 188 words net of the post-#834 additions; measured 1270, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 1270, within a few words of the file.
    // why: operator-approved (2026-11-17) +100: #961 and #966 each added troubleshooting rows, reaching 1495 of 1470.
    ("docs/TROUBLESHOOTING.md", Guide, 620),
    // why: operator-requested conversion of the routing research TXT, with implementation and measured limits.
    // why: moved out of docs/learning (operator reading material agents never open) into docs/ on 2026-11-08.
    // why: the 2026-11-11 trim-and-refresh pass cut 928 words net (now 3646); the cap drops by the same amount, rounded up to the next 10, keeping its earlier headroom.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 3680, within a few words of the file.
    // why: the 2026-11-22 refresh trimmed measurement narrative and restated config numbers (3871 to 3685 words with the week's provider, digest and loop changes added); the cap drops to the next 10 above it so the cut stays cut.
    // why: the 2026-11-28 edit added the operator-named URL rule (#1183) and cut two run anecdotes (1207 to 1200 words); lowered to 1210 so the cut stays cut.
    ("docs/WEB_DECISION_ROUTER.md", Guide, 1210),
    // why: operator netconnect user guide; 3293 words after the doc-sync trim and the diag tier, rounded up to the next 100.
    // why: the 2026-11-06 doc sync trimmed stale and duplicated text; set to the measured count so the cut stays cut.
    // why: the 2026-11-11 trim-and-refresh pass cut 577 words net of the post-#834 additions; measured 2538, rounded up to the next 10 so the cut stays cut.
    // why: operator decision (2026-11-12): +200 after the 2026-11-11 trim left this row at 2540, within a few words of the file.
    ("docs/netconnect.md", Guide, 650),
    ("docs/screenshots/README.md", Evidence, 120),
    // why: the seven folder-README rows below are operator-requested per-file indexes (one line per file);
    // each is its size plus about 10%, rounded up to the next 10.
    // why: operator-approved (2026-10-14) install-macos.sh and start-macos.sh lines, then the operator-requested
    // Terminal-walkthrough, user-guide and init.sh lines; 564 words plus about 2%, rounded up to the next 10.
    ("scripts/README.md", Guide, 780),
    // why: operator-approved (2026-10-30) three-run bakeoff10 matrix beside the script; 1782 words, rounded up to the next 100.
    ("scripts/bakeoff/report.md", Guide, 1800),
    ("src/agentic/README.md", Guide, 550),
    ("src/common/README.md", Guide, 600),
    // why: README audit removes three words; retain the existing headroom.
    ("src/netconnect/README.md", Guide, 387),
    // why: operator-approved (2026-11-05) raise to the measured count for the opt-in Firecrawl fetch fallback (phase 2 of web search).
    ("src/server/README.md", Guide, 748),
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
// why: operator-set (2026-11-06) Root allowance of 12,500 for the long-form research README reframe; an allowance,
// not a measured count, so Root also gets a slack override below.
// why: the 2026-11-11 trim-and-refresh pass cut 7,470 Guide words (now 46,731); the Guide cap drops by the same amount, rounded up
// to the next 100, so the gap to its words is unchanged and stays inside the 19,500 override.
// why: operator-approved (2026-11-15) +200 Guide words with the docs/Browser-Use.md row raise (970 to 1170).
// why: operator-approved (2026-11-17) +100 Guide words with the docs/Browser-Use.md row raise (1170 to 1270, #945).
// why: operator-approved (2026-11-22) +500 Guide words with the docs/Browser-Use.md row raise (1270 to 1570).
// The docs sweep tightened the Guide and Evidence caps to the next 500 words.
// The Codex guidance refresh removed 1,613 Agent words. Preserve the group's headroom.
// why: operator-approved (2026-11-25) +200 Guide words with the docs/SECURE_RESEARCH.md row raise (1190 to 1390); Guide was at 23,459 of 23,500.
const DOCS_GROUP_CAPS: &[(Kind, usize)] = &[(Root, 12_500), (Guide, 23_700), (Evidence, 500), (Agent, 20_297)];

/// A group cap this far above its words fails too, so a deletion locks in
/// instead of leaving room to regrow.
const DOCS_GROUP_SLACK: usize = 1_000;

/// Groups the operator allowed to sit further above their words than
/// `DOCS_GROUP_SLACK`; every other group keeps the default.
// why: operator-approved (2026-10-14) Guide headroom for the 65,000 cap; 17,000 covers 64,500 - 47,827.
// why: operator-approved (2026-10-18) +200-per-row raise: Guide 19,500 covers 68,700 - 49,361 and Agent 4,500
// covers 25,500 - 21,466, each rounded up to the next 500.
// why: operator-set (2026-11-06) Root cap of 12,500 over about 9,900 words; 3,000 covers it, rounded up to the next 500.
const DOCS_GROUP_SLACK_OVERRIDES: &[(Kind, usize)] = &[(Guide, 19_500), (Agent, 4_500), (Root, 3_000)];

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
const DOCS_BUDGET_OWNER_EXEMPT: &[(&str, &str)] = &[];

#[test]
fn documented_http_methods_and_paths_match_route_caps() {
    use cgagentharness::server::routes::ROUTE_CAPS;
    use std::collections::BTreeSet;

    const GUIDE: &str = "docs/API_ROUTES.md";
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(GUIDE)).unwrap();
    let mut documented = BTreeSet::new();
    for row in text.lines().filter_map(|line| line.strip_prefix('|')) {
        let mut cells = row.split('|');
        let (Some(methods), Some(paths)) = (cells.next(), cells.next()) else {
            continue;
        };
        if !paths.trim_start().starts_with("`/") {
            continue;
        }
        for path in paths.split(',').map(str::trim) {
            let path = path
                .strip_prefix('`')
                .and_then(|cell| cell.strip_suffix('`'))
                .unwrap_or_else(|| panic!("{GUIDE}: expected a backticked path in {row}"));
            for method in methods.split(',').map(str::trim) {
                assert!(documented.insert((method, path)), "{GUIDE}: duplicate {method} {path}");
            }
        }
    }

    // The router unit tests independently compare ROUTE_CAPS to build_router.
    let registered: BTreeSet<_> = ROUTE_CAPS
        .iter()
        .map(|(path, method, _)| (method.as_str(), *path))
        .collect();
    let missing: Vec<_> = registered.difference(&documented).collect();
    let stale: Vec<_> = documented.difference(&registered).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "Update {GUIDE} (method, path). Missing: {missing:?}. Stale: {stale:?}."
    );
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
    // Evidence keeps the default slack (Root, Guide and Agent have overrides).
    let evidence_budget: &[(&str, Kind, usize)] = &[("docs/A.md", Evidence, 100), ("docs/B.md", Evidence, 100)];
    expect(
        docs_budget_violations(
            &md(&[("docs/A.md", 10), ("docs/B.md", 10)]),
            evidence_budget,
            &[(Evidence, 1_500)],
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

/// No test server reaches the operator's real OS credential store unless the
/// test opts in and cleans up. `AppOptions::new` leaves the store unset, and
/// an unset store is the OS store (macOS Keychain, Windows Credential
/// Manager, Linux Secret Service), so every `/api/keys` save once left an
/// entry named after a deleted temp home.
#[test]
fn test_servers_use_the_os_credential_store_only_when_they_opt_in() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let compact = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    for (path, text) in read_tree(&tests) {
        let normalized = path.replace('\\', "/");
        if normalized.ends_with("tests/invariant_guard.rs") {
            continue;
        }
        let code = compact(&text);
        if normalized.ends_with("tests/common/mod.rs") {
            assert_eq!(
                code.matches("AppOptions::new(").count(),
                1,
                "{path}: only `app_options` may call AppOptions::new"
            );
            let start = text.find("pub fn app_options(").expect("app_options helper");
            let body = &text[start..];
            let helper = compact(&body[..body.find("\n}").expect("app_options body ends")]);
            assert!(
                helper.contains("AppOptions::new(")
                    && helper.contains(".credential_store=Some(Arc::new(MemoryCredentialStore::default()))"),
                "{path}: app_options must install a MemoryCredentialStore"
            );
            assert!(
                code.contains("letmutapp_opts=app_options(home);"),
                "{path}: prepare_server must start from app_options"
            );
            continue;
        }
        assert!(
            !code.contains("AppOptions::new("),
            "{path}: build test options with common::app_options, which keeps keys in memory"
        );
        assert!(
            !code.contains("credential_store=None") && !code.contains("credential_store:None"),
            "{path}: an unset credential store is the real OS store; set os_credential_store instead"
        );
        assert!(
            !code.contains("OsCredentialStore") || code.contains("OsCredentialCleanup::new("),
            "{path}: a test that touches the OS store removes its entries with OsCredentialCleanup"
        );
        assert!(
            !code.contains("os_credential_store") || code.contains("OsCredentialCleanup::new("),
            "{path}: os_credential_store needs an OsCredentialCleanup for every saved name"
        );
    }
}
