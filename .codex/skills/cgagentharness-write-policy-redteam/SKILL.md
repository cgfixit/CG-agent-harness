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
| Exact edits, whole-proposal preflight and rollback quarantine | `tests/exact_edits.rs`, `src/agentic/workspace.rs` |
| Jail, symlinks and .git equivalence | `tests/agentic_foundations.rs`, `src/agentic/workspace.rs`. Cover read and landed-path judgment. |
| Local clone retrieval, protected basenames and cloud refusal | `tests/real_repo_loop.rs`, `src/agentic/repo_retrieval.rs` |
| Fixed argv and refused missing confirm | `tests/shim_and_agent_routes.rs`. Check 422 for hostile argv and child exit 4 for missing publish confirm. |
| Shipped YAML and Rust fallback protections | `invariant_guard::shipped_defaults_protect_rust_check_configuration`, `cgagentharness-config-guard` |

Inspect the assertions and run the affected filters with the pinned toolchain:

```sh
GROK_API_KEY='' ANTHROPIC_API_KEY='' DEEPAGENT_API_KEY='' \
  cargo test --locked --test real_repo_loop writer_gates_in_order_and_plan_integrity -- --exact
```

Confirm the output names the intended test. For a full audit, cover all
applicable rows. Use an owned temporary `CGAGENTHARNESS_HOME` and unique
listener for live cases.

- **New bypasses**: a forbidden mutation succeeds or its refusal assertion is
  removed. Treat as a regression and preserve the assertion.
- **Known gaps**: confirm the documented limit against current behavior.
  Follow `AGENTS.md` for unresolved material findings.
- **Fixed findings**: preserve a focused assertion for the observed refusal.
- **False passes**: defaulted confirm, an OR-ed kill switch, skipped confinement
  or another process answering the listener invalidate the result.

For authorized fixes, restore the smallest owning refusal. Keep reason
required and confirm explicit. Re-run affected probes, plus
`invariant_guard`/`auth_guards` checks when shared routing changed. Retain
recovery backups after quarantine. Report refusals, commands, exits, SHA and
verification gaps. Use `cgagentharness-invariant-guard` for wider contract review.
