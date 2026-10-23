//! Lock `scripts/ci-docs-only.sh` and the workflow steps that feed it.
//!
//! The backend CI `changes` job pipes `git diff --no-renames --name-only`
//! into the script. Exit 0 is docs-only (`code=false`); exit 1 runs validation.
//! The shell starts at `code=true` and assigns `code=false` only when that
//! pipeline succeeds. Bundle runs on main pushes without this classifier.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_yaml_ng::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script() -> PathBuf {
    repo_root().join("scripts/ci-docs-only.sh")
}

/// Same extraction as the script: `grep -oE 'docs/[A-Za-z0-9_./-]+'`.
fn bundled_docs(packaging: &str) -> Vec<String> {
    let pattern = regex::Regex::new(r"docs/[A-Za-z0-9_./-]+").expect("bundled pattern");
    pattern
        .find_iter(packaging)
        .map(|hit| hit.as_str().to_string())
        .collect()
}

/// Same extraction as the script: the quoted entries of the generator's
/// `INPUTS = [` ... `]` block.
fn manual_inputs(generator: &str) -> Vec<String> {
    let pattern = regex::Regex::new(r#""([^"]+)""#).expect("inputs pattern");
    generator
        .lines()
        .skip_while(|line| !line.starts_with("INPUTS = ["))
        .skip(1)
        .take_while(|line| !line.starts_with(']'))
        .filter_map(|line| pattern.captures(line))
        .map(|hit| hit[1].to_string())
        .collect()
}

fn classify(script_path: &Path, paths: &[&str]) -> Output {
    let mut child = Command::new(std::env::var_os("CGAH_TEST_BASH").unwrap_or_else(|| "bash".into()))
        .arg(script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("spawn {}: {err}", script_path.display()));
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for path in paths {
            writeln!(stdin, "{path}").expect("write path");
        }
    }
    child.wait_with_output().expect("wait for ci-docs-only.sh")
}

fn assert_exit(name: &str, script_path: &Path, paths: &[&str], expected: i32) {
    let output = classify(script_path, paths);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{name} paths={paths:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn code_and_uncertain_paths_run_everything() {
    let script = script();
    let packaging =
        std::fs::read_to_string(repo_root().join("scripts/package-desktop.sh")).expect("package-desktop.sh");
    let bundled = bundled_docs(&packaging);
    // Packaging leaves PR CI, so a moved bundled doc would first fail on main.
    for path in &bundled {
        assert!(
            repo_root().join(path).is_file(),
            "package-desktop.sh bundles {path}, which does not exist"
        );
    }
    assert!(
        bundled.iter().any(|path| path == "docs/DESKTOP.md"),
        "package-desktop.sh must still bundle docs/DESKTOP.md, got {bundled:?}"
    );

    let cases: &[(&str, &[&str])] = &[
        ("empty input", &[]),
        ("README.md", &["README.md"]),
        ("AGENTS.md", &["AGENTS.md"]),
        ("CLAUDE.md", &["CLAUDE.md"]),
        ("pull request template", &[".github/PULL_REQUEST_TEMPLATE.md"]),
        ("data/", &["data/agentic/note.md"]),
        ("src/", &["src/main.rs"]),
        // `git diff --name-only` prints the path for a deletion. The script
        // has no status column, so a deleted src file is that path.
        ("deleted src/foo.rs", &["src/foo.rs"]),
        // `git diff --no-renames` prints both sides of a src/ to docs/ move.
        ("src to docs move", &["src/foo.rs", "docs/foo.md"]),
        ("subfolder outside docs/", &["notes/outside.md"]),
        ("assets/", &["assets/config.default.yaml"]),
    ];
    for (name, paths) in cases {
        assert_exit(name, &script, paths, 1);
    }
    for path in &bundled {
        assert_exit("bundled desktop doc", &script, &[path.as_str()], 1);
    }
    // The destination alone is an unbundled doc. The move stays code only
    // because the deleted src path is still in the list.
    assert!(
        !bundled.iter().any(|path| path == "docs/foo.md"),
        "docs/foo.md is the unbundled side of the move fixture"
    );
    assert_exit("unbundled side of a move", &script, &["docs/foo.md"], 0);
}

#[test]
fn self_manual_inputs_run_everything() {
    let script = script();
    let generator =
        std::fs::read_to_string(repo_root().join("scripts/build-self-manual.py")).expect("build-self-manual.py");
    let inputs = manual_inputs(&generator);
    for path in &inputs {
        assert!(
            repo_root().join(path).is_file(),
            "build-self-manual.py reads {path}, which does not exist"
        );
    }
    // The docs-shaped inputs are what the other rules would let through.
    for expected in ["INVARIANTS.md", "docs/CONSOLE.md"] {
        assert!(
            inputs.iter().any(|path| path == expected),
            "build-self-manual.py must still read {expected}, got {inputs:?}"
        );
    }
    for path in &inputs {
        assert_exit("self-manual input", &script, &[path.as_str()], 1);
    }
}

#[test]
fn an_unreadable_self_manual_input_list_runs_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("ci-docs-only.sh");
    std::fs::copy(script(), &script_path).expect("copy classifier");
    std::fs::copy(
        repo_root().join("scripts/package-desktop.sh"),
        dir.path().join("package-desktop.sh"),
    )
    .expect("copy packaging");
    std::fs::write(dir.path().join("build-self-manual.py"), "INPUTS = [\n]\n").expect("write empty generator");
    assert_exit("empty INPUTS list", &script_path, &["docs/INSTALL.md"], 1);
}

#[test]
fn plain_docs_may_skip() {
    let script = script();
    let packaging =
        std::fs::read_to_string(repo_root().join("scripts/package-desktop.sh")).expect("package-desktop.sh");
    let bundled = bundled_docs(&packaging);
    assert!(
        !bundled.iter().any(|path| path == "docs/INSTALL.md"),
        "docs/INSTALL.md is the unbundled docs fixture, got {bundled:?}"
    );
    let generator =
        std::fs::read_to_string(repo_root().join("scripts/build-self-manual.py")).expect("build-self-manual.py");
    let inputs = manual_inputs(&generator);
    for fixture in ["docs/INSTALL.md", "SECURITY.md"] {
        assert!(
            !inputs.iter().any(|path| path == fixture),
            "{fixture} is the docs-only fixture and must stay outside the self-manual inputs, got {inputs:?}"
        );
    }
    assert_exit("unbundled docs/INSTALL.md", &script, &["docs/INSTALL.md"], 0);
    assert_exit("SECURITY.md", &script, &["SECURITY.md"], 0);
}

#[test]
fn a_docs_glob_in_package_desktop_runs_everything() {
    let live = script();
    // The live packaging script lists files. A glob is not one of those
    // paths, so this fixture plants one beside a copy of the classifier.
    // `docs/shipped-glob/note.md` matches that glob and is not in the
    // explicit list, so only the glob check can reject it.
    assert_exit(
        "live script has no matching glob",
        &live,
        &["docs/shipped-glob/note.md"],
        0,
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("ci-docs-only.sh");
    std::fs::copy(&live, &script_path).expect("copy classifier");
    let mut packaging =
        std::fs::read_to_string(repo_root().join("scripts/package-desktop.sh")).expect("package-desktop.sh");
    packaging.push_str("cp docs/shipped-glob/*.md \"$app/Contents/Resources/\"\n");
    std::fs::write(dir.path().join("package-desktop.sh"), packaging).expect("write packaging fixture");

    assert_exit(
        "docs path matched by a package-desktop.sh glob",
        &script_path,
        &["docs/shipped-glob/note.md"],
        1,
    );
}

#[test]
fn ci_changes_job_keeps_no_renames_and_fail_closed() {
    let name = "ci.yml";
    let text = std::fs::read_to_string(repo_root().join(".github/workflows").join(name)).expect(name);
    let doc: Value = serde_yaml_ng::from_str(&text).unwrap_or_else(|err| panic!("{name} yaml: {err}"));
    let jobs = doc.get("jobs").unwrap_or_else(|| panic!("{name} jobs"));
    let changes = jobs.get("changes").unwrap_or_else(|| panic!("{name} changes"));
    let output = changes
        .get("outputs")
        .and_then(|outputs| outputs.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name} changes output"));
    assert_eq!(output, "${{ steps.classify.outputs.code }}", "{name}");

    let steps = changes
        .get("steps")
        .and_then(Value::as_sequence)
        .unwrap_or_else(|| panic!("{name} steps"));
    let run = steps
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some("classify"))
        .and_then(|step| step.get("run"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name} classify step"));
    assert_fail_closed(name, run);

    let jobs_map = jobs.as_mapping().unwrap_or_else(|| panic!("{name} jobs map"));
    for (job_name, job) in jobs_map {
        if !needs_changes(job) {
            continue;
        }
        let gate = job.get("if").and_then(Value::as_str).unwrap_or("");
        assert!(
            gate.contains("needs.changes.outputs.code != 'false'"),
            "{name} job {job_name:?} must run unless classify set code=false, got {gate:?}"
        );
    }
}

fn needs_changes(job: &Value) -> bool {
    match job.get("needs") {
        Some(Value::String(name)) => name == "changes",
        Some(Value::Sequence(names)) => names.iter().any(|name| name.as_str() == Some("changes")),
        _ => false,
    }
}

fn assert_fail_closed(workflow: &str, run: &str) {
    let mut saw_true = false;
    let mut saw_if = false;
    let mut saw_diff = false;
    let mut saw_script = false;
    let mut saw_false = false;
    for line in run.lines() {
        let trimmed = line.trim();
        if trimmed == "code=true" {
            assert!(
                !saw_if && !saw_false,
                "{workflow}: code=true is the default, before the if"
            );
            saw_true = true;
        } else if trimmed.starts_with("if ") {
            assert!(saw_true, "{workflow}: code=true before the if");
            saw_if = true;
        } else if trimmed.contains("git diff") {
            assert!(
                trimmed.contains("git diff --no-renames --name-only"),
                "{workflow}: path list must be `git diff --no-renames --name-only`, got {trimmed}"
            );
            assert!(
                saw_if && !saw_false,
                "{workflow}: diff stays inside the classify condition"
            );
            saw_diff = true;
        } else if trimmed.contains("scripts/ci-docs-only.sh") {
            assert!(
                trimmed.contains("printf '%s\\n' \"$paths\" | scripts/ci-docs-only.sh"),
                "{workflow}: classify must pipe the path list into the script, got {trimmed}"
            );
            assert!(saw_diff && !saw_false, "{workflow}: the script is the last condition");
            saw_script = true;
        } else if trimmed == "code=false" {
            assert!(saw_script, "{workflow}: code=false only after the classifier succeeds");
            let indent = line.len() - line.trim_start().len();
            assert!(indent > 0, "{workflow}: code=false stays inside the then branch");
            saw_false = true;
        }
    }
    assert!(
        saw_true && saw_if && saw_diff && saw_script && saw_false,
        "{workflow}: {run}"
    );
    assert_eq!(run.matches("code=true").count(), 1, "{workflow}");
    assert_eq!(run.matches("code=false").count(), 1, "{workflow}");
}

/// Name filters on `cargo test` lines in every workflow, as (target, filter).
/// A target is `lib` or a `tests/<file>.rs` stem.
fn workflow_test_filters() -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    let dir = repo_root().join(".github/workflows");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let Some(at) = line.find("cargo test") else { continue };
            let mut target: Option<String> = None;
            let mut tokens = line[at + "cargo test".len()..].split_whitespace();
            while let Some(token) = tokens.next() {
                match token {
                    "--" | ">" | "|" | "&&" | ";" => break,
                    "--test" => target = tokens.next().map(str::to_string),
                    "--lib" => target = Some("lib".into()),
                    "--manifest-path" | "--target" | "-p" | "--package" | "--features" => {
                        tokens.next();
                    }
                    flag if flag.starts_with('-') => {}
                    filter => {
                        let filter = filter.trim_matches(|c| c == '\'' || c == '"');
                        // Anything else is shell, not a libtest name filter.
                        if !filter
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
                        {
                            break;
                        }
                        let file = path.file_name().unwrap().to_string_lossy().to_string();
                        found.push((file, target.clone().unwrap_or_default(), filter.to_string()));
                    }
                }
            }
        }
    }
    found
}

/// A filtered `cargo test` exits 0 when its filter matches no test, so a renamed
/// test silently leaves a required CI job proving nothing. Every filter must name
/// an existing test function or a library module with tests.
#[test]
fn workflow_test_filters_name_existing_tests() {
    let filters = workflow_test_filters();
    assert!(
        filters.iter().any(|(_, target, _)| target == "real_repo_loop"),
        "parser found no filtered run: {filters:?}"
    );
    for (workflow, target, filter) in &filters {
        assert!(
            !target.is_empty(),
            "{workflow}: filter {filter} has no --test/--lib target"
        );
        if target == "lib" {
            let module = filter.trim_end_matches("::");
            let rel = module.replace("::", "/");
            let file = ["src/{}.rs", "src/{}/mod.rs"]
                .iter()
                .map(|f| repo_root().join(f.replace("{}", &rel)))
                .find(|p| p.is_file())
                .unwrap_or_else(|| panic!("{workflow}: --lib filter {filter} names no module"));
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(
                text.contains("#[test]") || text.contains("#[tokio::test]"),
                "{workflow}: {filter} has no tests"
            );
        } else {
            let file = repo_root().join(format!("tests/{target}.rs"));
            let text = std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{workflow}: no tests/{target}.rs"));
            assert!(
                text.contains(&format!("fn {filter}(")) || text.contains(&format!("fn {filter}<")),
                "{workflow}: `cargo test --test {target} {filter}` matches no test function"
            );
        }
    }
}
