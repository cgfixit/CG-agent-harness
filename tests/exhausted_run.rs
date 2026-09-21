//! Bounded exhausted-run diagnostics: closed `reject_code`, capped `reject_detail`.

use cgagentharness::agentic::commands::apply_exhausted_diagnostics;
use cgagentharness::agentic::real_repo_loop::{
    cap_reject_detail, exhausted_reject_code, RealRepoDecision, RealRepoLoopIteration, RealRepoLoopResult,
    REJECT_CODE_MAX_ITERATIONS, REJECT_CODE_PLANNER_REFUSED, REJECT_CODE_VERIFY_FAILED_LOOP, REJECT_DETAIL_MAX_BYTES,
};
use cgagentharness::agentic::run_store::{load_run, new_run_id, save_run, RealRepoRunRecord};
use serde_json::Value;

fn iteration(gates: &[&str]) -> RealRepoLoopIteration {
    let rejected: Vec<String> = gates.iter().map(|s| (*s).to_string()).collect();
    let reason = if rejected.is_empty() {
        "rejected".to_string()
    } else {
        format!("rejected: {}", rejected.join(", "))
    };
    RealRepoLoopIteration {
        retrieval: None,
        step: 1,
        changed_files: Vec::new(),
        decision: RealRepoDecision {
            accepted: false,
            reason,
            rejected_gates: rejected,
        },
        governance_findings: Vec::new(),
    }
}

fn result_with(gates: &[&str], n: usize) -> RealRepoLoopResult {
    RealRepoLoopResult {
        accepted: false,
        branch_name: None,
        commit_message: None,
        iterations: (0..n).map(|_| iteration(gates)).collect(),
    }
}

fn collect_keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.push(k.clone());
                collect_keys(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_keys(v, out);
            }
        }
        _ => {}
    }
}

#[test]
fn max_iterations_sets_exhausted_and_code() {
    let result = result_with(&["write_budget_exceeded"], 3);
    let id = new_run_id();
    let mut record = RealRepoRunRecord::new(&id, "o/r", "/tmp/x", "running");
    apply_exhausted_diagnostics(&mut record, &result, &result.iterations[2].decision.reason);
    assert_eq!(record.status, "exhausted");
    assert_eq!(record.iterations, 3);
    assert_eq!(record.reject_code.as_deref(), Some(REJECT_CODE_MAX_ITERATIONS));
    assert_eq!(record.reject_detail.as_deref(), Some("rejected: write_budget_exceeded"));
}

#[test]
fn planner_refused_and_verify_failed_loop_codes() {
    assert_eq!(
        exhausted_reject_code(&result_with(&["no_files_changed"], 1)),
        REJECT_CODE_PLANNER_REFUSED
    );
    assert_eq!(
        exhausted_reject_code(&result_with(&["file_write_failed"], 1)),
        REJECT_CODE_PLANNER_REFUSED
    );
    assert_eq!(
        exhausted_reject_code(&result_with(&["verification_failed"], 2)),
        REJECT_CODE_VERIFY_FAILED_LOOP
    );
}

#[test]
fn exhausted_json_has_no_prompt_query_completion_keys() {
    let result = result_with(&["verification_failed"], 2);
    let id = new_run_id();
    let mut record = RealRepoRunRecord::new(&id, "o/r", "/tmp/x", "running");
    apply_exhausted_diagnostics(&mut record, &result, &result.iterations[1].decision.reason);
    let json = record.to_json();
    let mut keys = Vec::new();
    collect_keys(&json, &mut keys);
    for banned in ["prompt", "query", "completion", "tokens"] {
        assert!(!keys.iter().any(|k| k == banned), "forbidden key {banned} in {keys:?}");
    }
    assert_eq!(json["reject_code"], REJECT_CODE_VERIFY_FAILED_LOOP);
    assert_eq!(json["status"], "exhausted");
}

#[test]
fn oversize_reject_detail_is_truncated() {
    let oversized = "x".repeat(REJECT_DETAIL_MAX_BYTES + 80);
    let capped = cap_reject_detail(&oversized);
    assert_eq!(capped.len(), REJECT_DETAIL_MAX_BYTES);
    assert!(capped.len() <= 2048);

    let result = result_with(&["no_files_changed"], 1);
    let id = new_run_id();
    let mut record = RealRepoRunRecord::new(&id, "o/r", "d", "running");
    apply_exhausted_diagnostics(&mut record, &result, &oversized);
    let json = record.to_json();
    let detail = json["reject_detail"].as_str().expect("reject_detail");
    assert_eq!(detail.len(), REJECT_DETAIL_MAX_BYTES);
    assert!(!detail.contains("prompt"));
}

#[test]
fn old_run_json_without_reject_fields_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let id = "a".repeat(32);
    std::fs::write(
        runs.join(format!("{id}.json")),
        format!(r#"{{"run_id":"{id}","repo":"o/r","dest":"d","status":"exhausted"}}"#),
    )
    .unwrap();
    let loaded = load_run(&runs, &id).unwrap();
    assert_eq!(loaded.status, "exhausted");
    assert!(loaded.reject_code.is_none());
    assert!(loaded.reject_detail.is_none());
}

#[test]
fn exhausted_record_round_trips_reject_fields() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let result = result_with(&["verification_failed"], 1);
    let id = new_run_id();
    let mut record = RealRepoRunRecord::new(&id, "o/r", "d", "running");
    apply_exhausted_diagnostics(&mut record, &result, &result.iterations[0].decision.reason);
    save_run(&runs, &mut record).unwrap();
    let loaded = load_run(&runs, &id).unwrap();
    assert_eq!(loaded.status, "exhausted");
    assert_eq!(loaded.reject_code.as_deref(), Some(REJECT_CODE_VERIFY_FAILED_LOOP));
    assert_eq!(loaded.reject_detail.as_deref(), Some("rejected: verification_failed"));
}

#[test]
fn cap_reject_detail_does_not_split_utf8() {
    let mut s = "a".repeat(REJECT_DETAIL_MAX_BYTES - 1);
    s.push('é');
    let capped = cap_reject_detail(&s);
    assert!(capped.len() <= REJECT_DETAIL_MAX_BYTES);
    assert!(capped.is_char_boundary(capped.len()));
    assert!(!capped.ends_with('é'));
}
