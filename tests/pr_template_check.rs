//! Fixture checks for `scripts/check-pr-template.sh`.
//!
//! Cursor cloud agents append `<!-- CURSOR_AGENT_PR_BODY_END -->` and an HTML
//! footer. That line (surrounding space, tab, vertical tab, or form feed
//! allowed) and everything after it are ignored before the template rules
//! run. A `Last updated` stamp that sits after the marker is discarded with
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
    let dir = std::env::temp_dir().join(format!("cgagentharness-pr-template-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("body.md");
    std::fs::write(&path, body).unwrap();
    let output = run_script(&path);
    let _ = std::fs::remove_dir_all(&dir);
    output
}

fn run_script(path: &Path) -> Output {
    Command::new("bash")
        .arg(script())
        .arg(path)
        .env("CGAGENTHARNESS_PR_FILES", "scripts/check-pr-template.sh")
        .env_remove("CGAGENTHARNESS_PR_BODY_FILE")
        .env_remove("CYCLAW_PR_BODY_FILE")
        .current_dir(repo_root())
        .output()
        .expect("run check-pr-template.sh")
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

/// (a) Layout 1, as on #266: stamp, blank line, marker, blank line, footer.
#[test]
fn marker_after_timestamp_ignores_footer() {
    let body = format!("{BEGIN}{}{STAMP}\n\n{END}\n\n{FOOTER}\n", valid_prefix());
    assert_ok(&run_body("marker-after", &body));
}

/// (b) Layout 2, marker before the stamp, as one ordering on #269.
///
/// Rule 1 strips the marker line and everything after it, so the stamp is
/// discarded and this fails. The previous checker accepted the same bytes:
/// the stamp was the last non-blank line, and the marker and footer sat
/// above it. That pass is rejected on purpose. A stamp inside the ignored
/// footer does not meet the last-line rule.
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
    assert!(yml.contains("the script and this workflow enforce the same rules"));
    assert!(shell.contains("enforce the same rules, in"));
}
