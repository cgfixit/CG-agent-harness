# src/agentic

The coding pipeline, run only as a child process; it never references the
server or shim. [`writer.rs`](writer.rs) and [`workspace.rs`](workspace.rs) are
core paths for approved GitHub writes and clone-jailed repository mutations;
[`config.rs`](config.rs) owns their configuration gates. Doc:
[docs/CODING_PIPELINE.md](../../docs/CODING_PIPELINE.md); contracts:
[INVARIANTS.md](../../INVARIANTS.md).

- [`cli.rs`](cli.rs): parses `agentic <action>` argv, catches panics, maps errors to exit codes 0/2/3/4.
- [`cloud_proposer.rs`](cloud_proposer.rs): Grok or Claude cloud proposer behind six gates; handoff sanitized and audited.
- [`commands.rs`](commands.rs): `dispatch` routes the 12 actions to `cmd_*` bodies; owns the planner timeout constant.
- [`config.rs`](config.rs): validated `agentic:` config block: data paths under home, loopback planner, cloud gates.
- [`context.rs`](context.rs): read-only GitHub PR, issue and repo bundles with an advisory injection scan.
- [`ctx.rs`](ctx.rs): `AgenticCtx`: per-invocation config, audit sink, redactors, scanner, run and spend paths.
- [`edits.rs`](edits.rs): bounded file snapshots and exact-edit proposal parsing; model output is never a command.
- [`executor/`](executor/README.md): sandboxed verification, bounded runner, acceptance manifest, disposable-copy proof.
- [`gh_client.rs`](gh_client.rs): read-only `gh` wrapper; allow-listed `READ_OPS`, absolute binary, no token in argv.
- [`git.rs`](git.rs): crate-private git argv runner with an isolated environment for retained clones.
- [`governance.rs`](governance.rs): deterministic injection and code-shape scans (exfiltration, obfuscated exec, reverse shell).
- [`mod.rs`](mod.rs): module list; documents that agentic never references server or shim.
- [`proposer.rs`](proposer.rs): `ProposerClient` trait and the blocking local OpenAI-compatible client.
- [`real_repo_loop.rs`](real_repo_loop.rs): the plan-patch-verify loop and human-driven `finalize_real_repo_change`; never auto-commits.
- [`registry.rs`](registry.rs): governed skills registry: propose never writes; apply needs reason, confirm and a lock.
- [`repo_retrieval.rs`](repo_retrieval.rs): bounded local-only Tantivy retrieval over clone files for planner context.
- [`run_store.rs`](run_store.rs): JSON run records and lifecycle gates; `RUN_ID_PATTERN` duplicated with server `agent_policy`.
- [`unslop.rs`](unslop.rs): optional banned-phrase prose nudge (`unslop.enabled` ships false); feedback, never a gate.
- [`workspace.rs`](workspace.rs): **core path.** Clone jail reads, path-validated writes, fixed git subcommands, approved push.
- [`writer.rs`](writer.rs): **core path.** GitHub write gate; only executes `pr_create --draft`; master, write-mode, writes-enabled, reason and confirmation gates.

Tests: [tests/write_policy.rs](../../tests/write_policy.rs), [tests/git_approval.rs](../../tests/git_approval.rs),
[tests/exact_edits.rs](../../tests/exact_edits.rs), [tests/real_repo_loop.rs](../../tests/real_repo_loop.rs),
[tests/write_kill_switch.rs](../../tests/write_kill_switch.rs).
