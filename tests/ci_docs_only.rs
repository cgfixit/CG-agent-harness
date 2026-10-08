//! Lock `scripts/ci-docs-only.sh` and the workflow steps that feed it.
//!
//! Both `changes` jobs pipe `git diff --no-renames --name-only` into the
//! script. Exit 0 is docs-only (`code=false`). Exit 1 runs the compile and
//! package jobs. The shell starts at `code=true` and assigns `code=false`
//! only when that pipeline succeeds.

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
fn plain_docs_may_skip() {
    let script = script();
    let packaging =
        std::fs::read_to_string(repo_root().join("scripts/package-desktop.sh")).expect("package-desktop.sh");
    let bundled = bundled_docs(&packaging);
    assert!(
        !bundled.iter().any(|path| path == "docs/INSTALL.md"),
        "docs/INSTALL.md is the unbundled docs fixture, got {bundled:?}"
    );
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
fn workflow_changes_jobs_keep_no_renames_and_fail_closed() {
    for name in ["ci.yml", "bundle.yml"] {
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
