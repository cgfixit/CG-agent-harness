//! Read-only GitHub context bundles with an advisory injection scan over the
//! third-party text. Port of `agentic/context.py`. Findings are metadata,
//! never a gate; the model-feeding paths in `commands.rs` decide to refuse.

use serde_json::{json, Value};

use crate::common::errors::{HarnessError, Result};

use super::ctx::AgenticCtx;
use super::gh_client::{run_read, ReadRequest};

pub const INJECTION_FINDING_CODE: &str = "github_content_injection_pattern";
pub const SCANNER_UNAVAILABLE_CODE: &str = "github_content_scanner_unavailable";
const DEFAULT_LIST_LIMIT: u64 = 10;

fn injection_findings(
    ctx: &AgenticCtx,
    repo: &str,
    number: Option<i64>,
    fields: &[(String, Option<String>)],
) -> Vec<Value> {
    let mut findings = Vec::new();
    if ctx.scanner.is_empty() {
        findings.push(json!({"severity": "warning", "code": SCANNER_UNAVAILABLE_CODE, "field": null, "patterns": []}));
    }
    for (field, value) in fields {
        let Some(text) = value else { continue };
        if text.is_empty() {
            continue;
        }
        let matched = ctx.scanner.scan_normalized(text);
        if !matched.is_empty() {
            findings.push(
                json!({"severity": "warning", "code": INJECTION_FINDING_CODE, "field": field, "patterns": matched}),
            );
        }
    }
    for f in &findings {
        ctx.audit.log(json!({
            "event": "agentic_context_injection_finding", "repo": repo, "number": number,
            "field": f["field"], "code": f["code"], "patterns": f["patterns"],
        }));
    }
    findings
}

fn text_fields(obj: &Value, prefix: &str, keys: &[&str]) -> Vec<(String, Option<String>)> {
    let Some(map) = obj.as_object() else { return Vec::new() };
    keys.iter()
        .map(|k| {
            (
                format!("{prefix}.{k}"),
                map.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()),
            )
        })
        .collect()
}

fn shortlist_title_fields(shortlist: &Value, prefix: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    if let Some(items) = shortlist.get("items").and_then(|i| i.as_array()) {
        for (i, item) in items.iter().enumerate() {
            out.extend(text_fields(item, &format!("{prefix}[{i}]"), &["title"]));
        }
    }
    out
}

fn guard_read_op(ctx: &AgenticCtx, op: &str) -> Result<()> {
    if !ctx.acfg.allowed_read_ops.iter().any(|o| o == op) {
        return Err(
            HarnessError::agentic(format!("read op '{op}' is not in agentic.allowed_read_ops"))
                .detail("op", op)
                .detail("allowed", json!(ctx.acfg.allowed_read_ops)),
        );
    }
    Ok(())
}

fn read(ctx: &AgenticCtx, op: &str, number: Option<i64>, limit: u64) -> Result<Value> {
    run_read(
        &ctx.audit,
        &ReadRequest {
            op,
            repo: &ctx.acfg.repo,
            number,
            limit,
            min_version: ctx.acfg.gh_min_version,
            timeout_sec: ctx.acfg.gh_timeout_sec,
            retries: ctx.acfg.gh_retries,
            dest: None,
        },
    )
}

fn list_with_more(ctx: &AgenticCtx, op: &str, limit: u64) -> Result<Value> {
    let data = read(ctx, op, None, limit + 1)?;
    let items: Vec<Value> = data.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let has_more = items.len() as u64 > limit;
    let trimmed: Vec<Value> = items.into_iter().take(limit as usize).collect();
    Ok(json!({"items": trimmed, "count": trimmed.len(), "has_more": has_more}))
}

pub fn fetch_pr_context(ctx: &AgenticCtx, number: i64, include_diff: bool) -> Result<Value> {
    guard_read_op(ctx, "pr_view")?;
    let pr = read(ctx, "pr_view", Some(number), 30)?["data"].clone();
    let mut fields = text_fields(&pr, "pr", &["title", "body"]);
    let mut bundle = json!({"repo": ctx.acfg.repo, "number": number, "pr": pr});
    if include_diff {
        guard_read_op(ctx, "pr_diff")?;
        let diff = read(ctx, "pr_diff", Some(number), 30)?["diff"]
            .as_str()
            .unwrap_or("")
            .to_string();
        fields.push(("diff".into(), Some(diff.clone())));
        bundle["diff"] = json!(diff);
    }
    bundle["governance_findings"] = json!(injection_findings(ctx, &ctx.acfg.repo, Some(number), &fields));
    Ok(bundle)
}

pub fn fetch_issue_context(ctx: &AgenticCtx, number: i64) -> Result<Value> {
    guard_read_op(ctx, "issue_view")?;
    let issue = read(ctx, "issue_view", Some(number), 30)?["data"].clone();
    let mut fields = text_fields(&issue, "issue", &["title", "body"]);
    if let Some(comments) = issue.get("comments").and_then(|c| c.as_array()) {
        for (i, c) in comments.iter().enumerate() {
            fields.extend(text_fields(c, &format!("issue.comments[{i}]"), &["body"]));
        }
    }
    let findings = injection_findings(ctx, &ctx.acfg.repo, Some(number), &fields);
    Ok(json!({"repo": ctx.acfg.repo, "number": number, "issue": issue, "governance_findings": findings}))
}

pub fn fetch_pr_list(ctx: &AgenticCtx, max_items: u64) -> Result<Value> {
    guard_read_op(ctx, "pr_list")?;
    let mut shortlist = list_with_more(ctx, "pr_list", max_items)?;
    let fields = shortlist_title_fields(&shortlist, "items");
    shortlist["governance_findings"] = json!(injection_findings(ctx, &ctx.acfg.repo, None, &fields));
    Ok(shortlist)
}

pub fn fetch_issue_list(ctx: &AgenticCtx, max_items: u64) -> Result<Value> {
    guard_read_op(ctx, "issue_list")?;
    let mut shortlist = list_with_more(ctx, "issue_list", max_items)?;
    let fields = shortlist_title_fields(&shortlist, "items");
    shortlist["governance_findings"] = json!(injection_findings(ctx, &ctx.acfg.repo, None, &fields));
    Ok(shortlist)
}

pub fn fetch_repo_context(ctx: &AgenticCtx) -> Result<Value> {
    guard_read_op(ctx, "repo_view")?;
    let overview = read(ctx, "repo_view", None, 30)?["data"].clone();
    let mut fields = text_fields(&overview, "overview", &["description"]);
    let mut bundle = json!({"repo": ctx.acfg.repo, "overview": overview});
    if ctx.acfg.allowed_read_ops.iter().any(|o| o == "pr_list") {
        let prs = list_with_more(ctx, "pr_list", DEFAULT_LIST_LIMIT)?;
        fields.extend(shortlist_title_fields(&prs, "open_prs"));
        bundle["open_prs"] = prs;
    }
    if ctx.acfg.allowed_read_ops.iter().any(|o| o == "issue_list") {
        let issues = list_with_more(ctx, "issue_list", DEFAULT_LIST_LIMIT)?;
        fields.extend(shortlist_title_fields(&issues, "open_issues"));
        bundle["open_issues"] = issues;
    }
    bundle["governance_findings"] = json!(injection_findings(ctx, &ctx.acfg.repo, None, &fields));
    Ok(bundle)
}

/// Findings that must stop a run before a model sees the text (selected by CODE).
pub fn blocking_context_findings(bundle: &Value) -> Vec<Value> {
    bundle
        .get("governance_findings")
        .and_then(|f| f.as_array())
        .map(|list| {
            list.iter()
                .filter(|f| {
                    matches!(
                        f.get("code").and_then(|c| c.as_str()),
                        Some(INJECTION_FINDING_CODE) | Some(SCANNER_UNAVAILABLE_CODE)
                    )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

pub fn describe_findings(findings: &[Value]) -> String {
    findings
        .iter()
        .map(|f| {
            format!(
                "{}:{}",
                f.get("code").and_then(|c| c.as_str()).unwrap_or(""),
                f.get("field").and_then(|c| c.as_str()).unwrap_or("bundle")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
