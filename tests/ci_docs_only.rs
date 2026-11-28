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
        if !needs_changes(job) || matches!(job_name.as_str(), Some(GATE | "test")) {
            continue;
        }
        let gate = job.get("if").and_then(Value::as_str).unwrap_or("");
        assert!(
            gate.contains("needs.changes.outputs.code != 'false'"),
            "{name} job {job_name:?} must run unless classify set code=false, got {gate:?}"
        );
    }
}

const GATE: &str = "required-gate";

fn ci_jobs() -> serde_yaml_ng::Mapping {
    let text = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("ci.yml");
    let doc: Value = serde_yaml_ng::from_str(&text).expect("ci.yml yaml");
    doc.get("jobs")
        .and_then(Value::as_mapping)
        .cloned()
        .expect("ci.yml jobs")
}

/// A job left out of `needs` is a job the ruleset no longer requires.
#[test]
fn required_gate_needs_every_ci_job_and_always_runs() {
    let jobs = ci_jobs();
    let gate = jobs.get(GATE).expect("ci.yml required-gate job");
    assert_eq!(gate.get("if").and_then(Value::as_str), Some("${{ always() }}"));
    let mut needs: Vec<&str> = gate
        .get("needs")
        .and_then(Value::as_sequence)
        .expect("required-gate needs list")
        .iter()
        .map(|need| need.as_str().expect("job id"))
        .collect();
    needs.sort_unstable();
    let mut others: Vec<&str> = jobs.keys().filter_map(Value::as_str).filter(|id| *id != GATE).collect();
    others.sort_unstable();
    assert_eq!(needs, others, "required-gate must need every other ci.yml job");

    let test = jobs.get("test").expect("test job");
    let matrix = test["strategy"]["matrix"]["os"].as_str().expect("test matrix");
    assert!(test.get("if").is_none(), "docs-only PRs still run the Windows suite");
    let windows = jobs.get("windows-mcp-lifecycle").expect("Windows lifecycle job");
    assert!(windows.get("if").is_none() && windows.get("needs").is_none());
    assert_eq!(
        matrix.split_whitespace().collect::<Vec<_>>().join(" "),
        r#"${{ fromJSON(needs.changes.outputs.code == 'false' && '["windows-latest"]' || '["ubuntu-latest","macos-latest","windows-latest"]') }}"#
    );
    for leg in ["ubuntu-latest", "macos-latest", "windows-latest"] {
        let expected = format!("${{{{ steps.leg.outputs.{leg} }}}}");
        assert_eq!(
            test["outputs"][leg].as_str(),
            Some(expected.as_str()),
            "test output {leg}"
        );
    }
    let last = test["steps"]
        .as_sequence()
        .and_then(|steps| steps.last())
        .expect("test steps");
    assert_eq!(
        last["id"].as_str(),
        Some("leg"),
        "the leg record must be the test job's last step"
    );
}

#[cfg(unix)]
fn gate_script() -> String {
    let gate = ci_jobs().get(GATE).cloned().expect("required-gate");
    gate["steps"][0]["run"].as_str().expect("required-gate run").to_string()
}

#[cfg(unix)]
fn ok_need() -> serde_json::Value {
    serde_json::json!({"result": "success", "outputs": {}})
}

/// Every ci.yml job passed, with the given classification and test legs.
#[cfg(unix)]
fn needs_with(code: &str, legs: &[(&str, &str)]) -> serde_json::Value {
    let mut needs = serde_json::Map::new();
    for id in ci_jobs().keys().filter_map(Value::as_str).filter(|id| *id != GATE) {
        needs.insert(id.to_string(), ok_need());
    }
    needs.insert(
        "changes".into(),
        serde_json::json!({"result": "success", "outputs": {"code": code}}),
    );
    let legs: serde_json::Map<String, serde_json::Value> = legs
        .iter()
        .map(|(leg, outcome)| (leg.to_string(), (*outcome).into()))
        .collect();
    needs.insert("test".into(), serde_json::json!({"result": "success", "outputs": legs}));
    serde_json::Value::Object(needs)
}

#[cfg(unix)]
fn gate_exit(event: &str, needs: &serde_json::Value) -> (i32, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("gate.sh");
    std::fs::write(&path, gate_script()).expect("write gate script");
    let out = Command::new("bash")
        .arg(&path)
        .env("EVENT_NAME", event)
        .env("NEEDS", needs.to_string())
        .output()
        .expect("run required-gate script");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), log)
}

/// The gate's real script, against each result GitHub can hand it.
#[cfg(unix)]
#[test]
fn required_gate_fails_every_result_its_event_does_not_allow() {
    const U: (&str, &str) = ("ubuntu-latest", "success");
    const M: (&str, &str) = ("macos-latest", "success");
    const W: (&str, &str) = ("windows-latest", "success");
    let set = |mut needs: serde_json::Value, job: &str, result: &str| {
        needs[job]["result"] = result.into();
        needs
    };
    let docs_only_pr = || {
        let mut needs = needs_with("false", &[W]);
        for job in [
            "browser-guards",
            "clippy",
            "linux-bwrap",
            "linux-mcp-lifecycle",
            "bakeoff-self-test",
        ] {
            needs[job]["result"] = "skipped".into();
        }
        needs
    };
    let cases: Vec<(&str, &str, serde_json::Value, i32)> = vec![
        ("PR missing Windows", "pull_request", needs_with("true", &[U, M]), 1),
        ("PR with every leg", "pull_request", needs_with("true", &[U, M, W]), 0),
        (
            "PR with failed Windows tests",
            "pull_request",
            needs_with("true", &[U, M, ("windows-latest", "failure")]),
            1,
        ),
        (
            "docs-only PR missing Windows",
            "pull_request",
            needs_with("false", &[]),
            1,
        ),
        (
            "docs-only PR skipped matrix",
            "pull_request",
            set(docs_only_pr(), "test", "skipped"),
            1,
        ),
        (
            "docs-only PR skipped Windows lifecycle",
            "pull_request",
            set(docs_only_pr(), "windows-mcp-lifecycle", "skipped"),
            1,
        ),
        (
            "queue without the Windows leg",
            "merge_group",
            needs_with("true", &[U, M]),
            1,
        ),
        ("queue with every leg", "merge_group", needs_with("true", &[U, M, W]), 0),
        (
            "queue, Windows advisory outcome",
            "merge_group",
            needs_with("true", &[U, M, ("windows-latest", "failure")]),
            1,
        ),
        (
            "queue, macOS leg never finished",
            "merge_group",
            needs_with("true", &[U, W]),
            1,
        ),
        (
            "main push, Windows advisory outcome",
            "push",
            needs_with("true", &[U, M, ("windows-latest", "failure")]),
            0,
        ),
        (
            "main push without the Windows leg",
            "push",
            needs_with("true", &[U, M]),
            1,
        ),
        (
            "docs-only PR passes with Windows and skips eligible jobs",
            "pull_request",
            docs_only_pr(),
            0,
        ),
        (
            "docs-only PR, changes skipped",
            "pull_request",
            set(docs_only_pr(), "changes", "skipped"),
            1,
        ),
        (
            "docs-only PR, fmt failed",
            "pull_request",
            set(docs_only_pr(), "fmt", "failure"),
            1,
        ),
        (
            "code PR, skipped job",
            "pull_request",
            set(needs_with("true", &[U, M, W]), "linux-bwrap", "skipped"),
            1,
        ),
        (
            "queue, code=false skip",
            "merge_group",
            set(needs_with("false", &[U, M, W]), "clippy", "skipped"),
            1,
        ),
        (
            "queue, cancelled job",
            "merge_group",
            set(needs_with("true", &[U, M, W]), "windows-mcp-lifecycle", "cancelled"),
            1,
        ),
        (
            "PR, failed test job",
            "pull_request",
            set(needs_with("true", &[U]), "test", "failure"),
            1,
        ),
    ];
    for (name, event, needs, expected) in cases {
        let (code, log) = gate_exit(event, &needs);
        assert_eq!(code, expected, "{name}\n{log}");
    }
    let mut missing = needs_with("true", &[U, M, W]);
    missing.as_object_mut().unwrap().remove("test");
    assert_eq!(gate_exit("merge_group", &missing).0, 1, "a gate without test in needs");
}

/// Each required context has one producer that runs, unskippable, on pull
/// requests and merge-queue candidates: a skipped required check passes.
#[test]
fn ruleset_contexts_have_one_unskippable_producer_on_prs_and_the_queue() {
    let ruleset: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join(".github/rulesets/main-protection.json")).expect("ruleset"),
    )
    .expect("ruleset json");
    let rules = ruleset["rules"].as_array().expect("rules");
    assert!(rules.iter().all(|rule| rule["type"] != "merge_queue"));
    let checks = rules
        .iter()
        .find(|rule| rule["type"] == "required_status_checks")
        .expect("required status checks");
    assert_eq!(checks["parameters"]["strict_required_status_checks_policy"], true);
    let contexts: Vec<String> = checks["parameters"]["required_status_checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|check| check["context"].as_str().expect("context").to_string())
        .collect();
    assert!(!contexts.is_empty());

    let mut producers: Vec<(String, String, Value, Value)> = Vec::new();
    for entry in std::fs::read_dir(repo_root().join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yml") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let doc: Value = serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|err| panic!("{file}: {err}"));
        let on = doc
            .get("on")
            .or_else(|| doc.get(Value::Bool(true)))
            .cloned()
            .unwrap_or(Value::Null);
        for (id, job) in doc.get("jobs").and_then(Value::as_mapping).into_iter().flatten() {
            let name = job.get("name").or(Some(id)).and_then(Value::as_str).unwrap_or_default();
            producers.push((file.clone(), name.to_string(), on.clone(), job.clone()));
        }
    }
    for context in &contexts {
        let found: Vec<_> = producers.iter().filter(|(_, name, _, _)| name == context).collect();
        assert_eq!(
            found.len(),
            1,
            "{context:?} needs exactly one producing job, found {}",
            found.len()
        );
        let (file, _, on, job) = found[0];
        for event in ["pull_request", "merge_group"] {
            let trigger = on
                .get(event)
                .unwrap_or_else(|| panic!("{file} ({context:?}) must run on {event}"));
            for filter in ["paths", "paths-ignore", "branches", "branches-ignore"] {
                assert!(
                    trigger.get(filter).is_none(),
                    "{file}: {event} {filter} can leave {context:?} pending"
                );
            }
        }
        let gate = job.get("if").and_then(Value::as_str);
        assert!(
            gate.is_none() || gate == Some("${{ always() }}"),
            "{file} ({context:?}) carries a skip condition: {gate:?}"
        );
        assert!(
            job.get("strategy").is_none(),
            "{file} ({context:?}): a matrix name is not a fixed context"
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
