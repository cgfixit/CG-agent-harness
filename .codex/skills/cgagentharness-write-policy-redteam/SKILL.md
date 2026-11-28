---
name: cgagentharness-write-policy-redteam
description: Adversarially verify Harness write gates, approval binding, clone jail and argv refusals when those boundaries change or an audit is requested.
---

# Write-policy redteam

Read the write, approval and clone-jail sections of `INVARIANTS.md`. Trace
the affected entrypoint. Preserve fresh disk policy at each mutation,
disable-only kill-switch semantics and separate run, approval, push and
publication intent.

| Surface | Evidence |
|---|---|
| Writer gate order, reason, confirm and plan integrity | `tests/real_repo_loop.rs::writer_gates_in_order_and_plan_integrity` and publish cases, `src/agentic/writer.rs` |
| Fresh policy and kill switch | `tests/write_policy.rs`, `tests/write_kill_switch.rs`, `invariant_guard::writer_kill_switch_is_and_not_or` |
| Approval binds digest, mode, tree, index, origin and commit | `tests/git_approval.rs`. Include index locks and refused Git extensions. |
| Approval recovery | `src/agentic/commands.rs`, `tests/real_repo_loop.rs`. A pre-branch failure leaves the run pending. An indeterminate approval with a durable branch commit marks the run failed. |
| Exact edits, whole-proposal preflight and rollback quarantine | `tests/exact_edits.rs`, `src/agentic/workspace.rs` |
| Jail, symlinks and .git equivalence | `tests/agentic_foundations.rs`, `src/agentic/workspace.rs`. Cover read and landed-path judgment. |
| Clone retrieval, protected basenames and planner locality | `tests/real_repo_loop.rs`, `src/agentic/repo_retrieval.rs`. Check cloud tags and remote aliases before cloning or recording a run. |
| Coding step authority | `src/server/routes/agent.rs`, `tests/secure_portal.rs`. Recheck after the audit flush and after the child exits. |
| Fixed argv and refused missing confirm | `tests/shim_and_agent_routes.rs`. Check 422 for hostile argv and child exit 4 for missing publish confirm. |
| Shipped YAML and Rust fallback protections | `invariant_guard::shipped_defaults_protect_rust_check_configuration`, `cgagentharness-config-guard` |

Inspect the assertions and run the affected filters with the pinned toolchain:

```sh
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test real_repo_loop writer_gates_in_order_and_plan_integrity -- --exact
```

Confirm the named test ran. Use a temporary
`CGAGENTHARNESS_HOME` and unique listener for live cases.

Keep refusal assertions. Check for false passes from defaulted confirm, an
OR-ed kill switch, skipped confinement or an unrelated listener. Follow
`AGENTS.md` for unresolved material findings.

Re-run affected checks after a fix. Use `cgagentharness-invariant-guard`
for shared routing. Retain recovery backups after quarantine. Record
refusals, commands, exits, SHA and gaps.
