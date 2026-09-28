//! Fixture checks for `scripts/check-pr-template.sh`.
//!
//! Cursor cloud agents append an HTML footer after the required stamp.
//! On #266, #267, #269, and #271 the order is the stamp, a blank line, then
//! `<!-- CURSOR_AGENT_PR_BODY_END -->`, then the footer. That marker line
//! (surrounding space, tab, vertical tab, or form feed allowed) and
//! everything after it are ignored before the template rules run. A stamp
//! placed below the marker is not that order: the stamp is discarded with
//! the footer and the check fails. No marker keeps the previous rules.
//! `<!-- CURSOR_AGENT_PR_BODY_BEGIN -->` is not a footer.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BEGIN: &str = "<!-- CURSOR_AGENT_PR_BODY_BEGIN -->\n";
const END: &str = "<!-- CURSOR_AGENT_PR_BODY_END -->";
const FOOTER: &str = "<div>open in Cursor</div>";
const STAMP: &str = "Last updated: 2026-09-28 08:48 ET";
const STAMP_MISS: &str = "Last updated timestamp (last line, YYYY-MM-DD HH:MM ET)";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script() -> PathBuf {
    repo_root().join("scripts/check-pr-template.sh")
}

fn valid_prefix() -> String {
    "\
## Proposed changes
The checker ignores a Cursor cloud-agent footer and still requires the stamp.

## Types of changes
- [ ] Bugfix (non-breaking change which fixes an issue)

## Benefits / why
A filled Last updated line stays the last non-blank line operators check.

## Risks to monitor
Text after the stamp, and a stamp placed after the footer marker, still fail.

## Checklist
- [ ] PR body was checked with scripts/check-pr-template.sh before opening

## Suggested merge order of open PRs
This fixture is not a pull request. It only exercises the body gate.

## ELI5

The marker line and the HTML after it are not part of the template body.
"
    .to_string()
}

fn run_body(name: &str, body: &str) -> Output {
    run_body_files(name, body, "scripts/check-pr-template.sh")
}

fn run_body_files(name: &str, body: &str, files: &str) -> Output {
    let dir = std::env::temp_dir().join(format!("cgagentharness-pr-template-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("body.md");
    std::fs::write(&path, body).unwrap();
    let output = run_script_files(&path, files);
    let _ = std::fs::remove_dir_all(&dir);
    output
}

fn run_script(path: &Path) -> Output {
    run_script_files(path, "scripts/check-pr-template.sh")
}

fn run_script_files(path: &Path, files: &str) -> Output {
    run_script_at(path, files, None)
}

fn run_script_at(path: &Path, files: &str, base: Option<&str>) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(script())
        .arg(path)
        .env("CGAGENTHARNESS_PR_FILES", files)
        .env_remove("CGAGENTHARNESS_PR_BODY_FILE")
        .env_remove("CYCLAW_PR_BODY_FILE")
        .current_dir(repo_root());
    match base {
        Some(base) => {
            cmd.env("CGAGENTHARNESS_PR_BASE", base);
        }
        None => {
            cmd.env_remove("CGAGENTHARNESS_PR_BASE");
        }
    }
    cmd.output().expect("run check-pr-template.sh")
}

fn assert_ok(output: &Output) {
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "expected pass, status {:?}\nstdout:\n{out}\nstderr:\n{err}",
        output.status.code()
    );
}

fn assert_stamp_fail(output: &Output) {
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected stamp failure, status {:?}\nstdout:\n{out}\nstderr:\n{err}",
        output.status.code()
    );
    assert!(err.contains(STAMP_MISS), "stderr did not name the stamp rule:\n{err}");
}

/// Stamp, blank line, marker, blank line, footer.
///
/// This is the order on #266, #267, #269, and #271. The stamp stays above
/// the marker, so the footer is ignored and the check passes.
#[test]
fn marker_after_timestamp_ignores_footer() {
    let body = format!("{BEGIN}{}{STAMP}\n\n{END}\n\n{FOOTER}\n", valid_prefix());
    assert_ok(&run_body("marker-after", &body));
}

/// Marker above the stamp. That order was not a body on #266, #267, #269, or #271.
///
/// The first exact marker line is the cut, so the stamp below it is
/// discarded and the last-line rule fails. A stamp inside the ignored
/// footer does not meet that rule.
#[test]
fn marker_before_timestamp_strips_the_stamp_and_fails() {
    let body = format!("{BEGIN}{}\n{END}\n\n{FOOTER}\n\n{STAMP}\n", valid_prefix().trim_end());
    assert_stamp_fail(&run_body("marker-before", &body));
}

/// (c) No end marker. The begin comment stays in the body and is not a heading.
#[test]
fn no_marker_timestamp_last_passes_with_begin_comment() {
    let body = format!("{BEGIN}{}{STAMP}\n", valid_prefix());
    assert_ok(&run_body("no-marker", &body));
}

/// (d) Stray text after the stamp, no marker. Fails on the previous checker too.
#[test]
fn stray_text_after_timestamp_without_marker_fails() {
    let body = format!("{BEGIN}{}{STAMP}\nthanks\n", valid_prefix());
    assert_stamp_fail(&run_body("stray-after", &body));
}

/// (e) Stray text between the stamp and the marker stays in the checked body.
#[test]
fn stray_text_between_timestamp_and_marker_fails() {
    let body = format!("{BEGIN}{}{STAMP}\nthanks\n{END}\n\n{FOOTER}\n", valid_prefix());
    assert_stamp_fail(&run_body("stray-between", &body));
}

/// (f) The shipped template's placeholder stamp is still accepted, and only there.
#[test]
fn template_placeholder_still_passes() {
    let output = run_script(&repo_root().join(".github/PULL_REQUEST_TEMPLATE.md"));
    assert_ok(&output);
}

#[test]
fn whitespace_around_the_marker_line_still_strips() {
    let spaced = format!("{BEGIN}{}{STAMP}\n\n  {END}  \n\n{FOOTER}\n", valid_prefix());
    assert_ok(&run_body("marker-spaces", &spaced));
    let tabbed = format!("{BEGIN}{}{STAMP}\n\n\t{END}\t\n{FOOTER}\n", valid_prefix());
    assert_ok(&run_body("marker-tabs", &tabbed));
    let fed = format!("{BEGIN}{}{STAMP}\n\n\u{000b}{END}\u{000c}\n{FOOTER}\n", valid_prefix());
    assert_ok(&run_body("marker-vt-ff", &fed));
}

#[test]
fn marker_with_trailing_text_is_not_a_footer() {
    let body = format!("{BEGIN}{}{STAMP}\n{END} trailing\n", valid_prefix());
    assert_stamp_fail(&run_body("marker-trailing", &body));
}

#[test]
fn crlf_marker_after_timestamp_passes() {
    let body = format!("{BEGIN}{}{STAMP}\r\n\r\n{END}\r\n\r\n{FOOTER}\r\n", valid_prefix());
    assert_ok(&run_body("crlf", &body));
}

/// A heading after `## ELI5` still fails when it is part of the body.
#[test]
fn heading_after_eli5_still_fails_when_no_marker() {
    let body = format!("{BEGIN}{}## Not the footer\n\n{STAMP}\n", valid_prefix());
    let output = run_body("eli5-strict", &body);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(err.contains("ELI5 must be the last section heading"), "{err}");
}

/// The first marker cuts the body. Required sections that sit below it are
/// missing, even when those sections would pass on their own.
#[test]
fn marker_before_required_sections_fails() {
    let intro = "This opening paragraph is above the marker and names no required section. \
                 It is long enough to pass the length floor by itself.\n";
    let body = format!("{BEGIN}{intro}\n{END}\n\n{}{STAMP}\n{FOOTER}\n", valid_prefix());
    let output = run_body("marker-mid-body", &body);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    for label in [
        "Proposed changes (or Why/Benefits/Summary)",
        "Types of changes",
        "Benefits / why",
        "Risks to monitor",
        "Checklist",
        "Suggested merge order of open PRs",
        "ELI5 (## ELI5 must be the last section heading)",
        STAMP_MISS,
    ] {
        assert!(err.contains(label), "missing {label} was not reported:\n{err}");
    }
}

/// An invariant statement that exists only below the marker does not satisfy
/// the core-path rule. The file list names a core path so that rule runs.
#[test]
fn invariant_statement_below_the_marker_does_not_count() {
    let below = "\n\n**Invariant / Governance Impact** (required for any change touching core paths):\n\n\
                 Loopback-only posture is unchanged. I6 process isolation still holds. Write gates stay closed.\n";
    let body = format!("{BEGIN}{}{STAMP}\n\n{END}{below}", valid_prefix());
    let output = run_body_files("invariant-below", &body, "src/server/headers.rs");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains("Invariant / Governance Impact statement"),
        "footer text satisfied the invariant rule:\n{err}"
    );
}

/// The same heading after the marker is footer text and is ignored.
#[test]
fn heading_inside_the_footer_is_ignored() {
    let body = format!(
        "{BEGIN}{}{STAMP}\n\n{END}\n\n## Not a section\n{FOOTER}\n",
        valid_prefix()
    );
    assert_ok(&run_body("footer-heading", &body));
}

/// The workflow reads the pull-request body, then strips the same marker,
/// then applies the stamp check. Triggers and permissions stay put.
#[test]
fn workflow_strips_the_footer_before_the_stamp_check() {
    let yml = std::fs::read_to_string(repo_root().join(".github/workflows/pr-template-check.yml")).unwrap();
    let shell = std::fs::read_to_string(script()).unwrap();
    let read_body = yml.find("context.payload.pull_request.body").expect("body read");
    let strip = yml
        .find("/^[ \\t\\v\\f]*<!-- CURSOR_AGENT_PR_BODY_END -->[ \\t\\v\\f]*$/")
        .expect("marker regex");
    let stamp = yml.find("Last updated: [0-9]").expect("stamp regex");
    assert!(
        read_body < strip && strip < stamp,
        "strip must sit between the body read and the stamp check"
    );
    assert!(
        yml.contains("const body = cursorFooterAt === -1 ? rawBody : rawLines.slice(0, cursorFooterAt).join(\"\\n\");")
    );
    assert!(yml.contains("types: [opened, edited, synchronize, reopened]"));
    assert!(yml.contains("permissions: {}"));
    assert!(yml.contains("contents: read"));
    assert!(yml.contains("pull-requests: write"));
    assert!(yml.contains("actions/github-script@3a2844b7e9c422d3c10d287c895573f7108da1b3 # v9.0.0"));
    assert!(shell.contains("gsub(/^[ \\t\\v\\f]+|[ \\t\\v\\f]+$/, \"\", t)"));
    assert!(shell.contains("<!-- CURSOR_AGENT_PR_BODY_END -->"));
    assert!(yml.contains("stay in lock-step with scripts/check-pr-template.sh"));
    assert!(yml.contains("script and this workflow enforce the same rules"));
    assert!(shell.contains("enforce the same rules, in"));
}

/// `CGAGENTHARNESS_PR_FILES` selects the changed-file list. It does not skip
/// the template comparison. A core path in that list plus a body whose
/// contributed text fails the invariant comparison exits non-zero.
#[test]
fn pr_files_set_with_failing_template_comparison_exits_nonzero() {
    let body = format!("{}{STAMP}\n", valid_prefix());
    let dir = std::env::temp_dir().join(format!("cgagentharness-pr-template-{}-files-core", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("body.md");
    std::fs::write(&path, body).unwrap();
    let output = run_script_at(&path, "src/server/headers.rs", None);
    let _ = std::fs::remove_dir_all(&dir);
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a failing template comparison must not pass when CGAGENTHARNESS_PR_FILES is set\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        err.contains("Invariant / Governance Impact statement"),
        "stderr did not report the template comparison failure:\n{err}"
    );
    assert!(
        !out.contains("OK —"),
        "a failing comparison must not report success:\n{out}"
    );
}

/// A non-core file list still loads the base template. Success names that
/// template instead of reporting that it was not compared.
#[test]
fn pr_files_set_still_compares_the_base_template() {
    let output = run_body("files-noncore", &format!("{}{STAMP}\n", valid_prefix()));
    assert_ok(&output);
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("template at origin/main"),
        "expected the base template to be compared:\n{out}"
    );
    assert!(
        !out.contains("template not compared"),
        "a pass must not report that the template was skipped:\n{out}"
    );
}

/// A base ref that resolves without the template file is a visible warning
/// and a non-zero exit, including when `CGAGENTHARNESS_PR_FILES` is set.
#[test]
fn missing_template_with_pr_files_set_is_not_a_pass() {
    let tree = Command::new("git")
        .args(["mktree"])
        .current_dir(repo_root())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("git mktree");
    assert!(
        tree.status.success(),
        "git mktree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree_sha = String::from_utf8(tree.stdout).unwrap();
    let commit = Command::new("git")
        .args(["commit-tree", tree_sha.trim(), "-m", "pr-template-check empty tree"])
        .env("GIT_AUTHOR_NAME", "pr-template-check")
        .env("GIT_AUTHOR_EMAIL", "pr-template-check@example.com")
        .env("GIT_COMMITTER_NAME", "pr-template-check")
        .env("GIT_COMMITTER_EMAIL", "pr-template-check@example.com")
        .current_dir(repo_root())
        .output()
        .expect("git commit-tree");
    assert!(
        commit.status.success(),
        "git commit-tree failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let sha = String::from_utf8(commit.stdout).unwrap();
    let body = format!("{}{STAMP}\n", valid_prefix());
    let dir = std::env::temp_dir().join(format!(
        "cgagentharness-pr-template-{}-missing-template",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("body.md");
    std::fs::write(&path, body).unwrap();
    let output = run_script_at(&path, "scripts/check-pr-template.sh", Some(sha.trim()));
    let _ = std::fs::remove_dir_all(&dir);
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a missing template must not pass\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        err.contains("warning:") && err.contains("absent"),
        "stderr did not warn that the template is absent:\n{err}"
    );
    assert!(
        !out.contains("OK —"),
        "a missing template must not report success:\n{out}"
    );
}
