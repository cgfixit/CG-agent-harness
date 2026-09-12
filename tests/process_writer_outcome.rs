//! Dedicated test process: PATH/env changes never race with other tests.
#![cfg(unix)]
mod common;
use cgagentharness::agentic::{
    ctx::AgenticCtx,
    writer::{execute_write, plan_write},
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[test]
fn accepted_publication_with_output_overflow_is_indeterminate_and_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = common::config_with(
        tmp.path(),
        &[
            ("agentic.enabled", "true"),
            ("agentic.deepagent_github.enabled", "true"),
            ("agentic.deepagent_github.allow_git_write_tools", "true"),
        ],
    );
    let ctx = AgenticCtx::new(cfg, &tmp.path().join("config.yaml")).unwrap();
    let fake = tmp.path().join("gh");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf 'accepted\\n' >> \"$CGAH_WRITE_MARKER\"\nexec /usr/bin/yes excessive-output\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("PATH", tmp.path());
    std::env::set_var("CGAH_WRITE_MARKER", tmp.path().join("accepted"));
    common::isolate_write_kill_switch();
    let plan = plan_write(
        &ctx.acfg,
        &ctx.audit,
        "pr_create",
        "fixture",
        true,
        json!({"head":"codex/fixture", "title":"fixture", "body":"fixture"}),
    )
    .unwrap();
    let err = execute_write(&ctx, &plan, true, 5).unwrap_err();
    assert_eq!(err.details["indeterminate"], true);
    assert_eq!(err.details["op"], "pr_create");
    assert!(err.message.contains("INDETERMINATE"));
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("accepted")).unwrap(),
        "accepted\n"
    );
    let audit = std::fs::read_to_string(tmp.path().join("logs/audit.jsonl")).unwrap();
    assert!(audit.contains("agentic_write_execute_capture_failed"));
}
