---
name: cgagentharness-optimize
description: Finds evidence-backed Rust, runtime, desktop and CI improvements in CG-agent-harness by running area-sharded read-only scans, verifying every finding against the code, and grouping survivors into focused draft PRs when the operator approves implementation. Use for an optimization or maintainability sweep of this repo, not for one already-known fix.
---

# CG-agent-harness Optimize

Output is **verified findings**, not suspicions; zero is a complete result. The
default is **report-only** (steps 1-3 and 6). Implementation (4-5), and any push,
PR or `@codex review`, need the operator's explicit approval (AGENTS.md Quality
bar). Human merges. `ARGUMENTS` names a shard; a path outside every shard is
scanned as its own ad-hoc shard.

## 1. Scan (read-only)

Run `git fetch origin main && git status -sb` and report the tree state. Launch
one read-only Explore-type subagent per shard, in parallel (with no subagent
tool, scan serially from the same briefs). Shards marked `opt` and server files
in no shard (state, transport, schemas, session_*) are not scanned by default;
the report says so.

| Shard | Scan | Read first (INVARIANTS.md) | Look for |
|---|---|---|---|
| agentic | `src/agentic` | The clone jail; Nothing lands before it is judged; Approval is bound | writer gates, jail, sandbox, digest binding, gh client, dead paths |
| edge | `src/shim`; `src/server/{guards,headers,agent_policy,agent_jobs,agent_schedules}.rs`; `src/server/routes/{mod,agent}.rs`; compare `src/agentic/commands.rs` | I6; Account, transport; The browser never supplies a command | guard order, `ACTIONS` vs dispatch, argv, shim budget vs child bounds, route drift, run/jobs/schedule lockstep |
| io-build | `src/{llm,common,netconnect}`, `desktop/src`, `scripts/` | Secrets never reach; Native Ollama pull; Netconnect | loopback validators, timeouts, secret echo, bounded capture, packaging order and atomicity |
| ci-tests | `.github/workflows`, `tests/` | Signals weaker than their name | coverage gaps (hostile argv, exit 4, jail, shipped gates), assertions that pass for the wrong reason |
| data `opt` | `src/server/{structured_memory*,memory_notes,notes_corpus,retrieval*,passage_index,compaction}.rs` | Account, transport | owner isolation, SQLite off async workers, bounded reads |
| web `opt` | `src/server/{web_*,chat_web,attachment*,mcp*}.rs` | Public web evidence; Private MCP | permission gates, pinned DNS, no proxy/redirect/retry, bounded fetch |
| chat `opt` | `src/server/{sessions,slash,prompts,notification*}.rs`, `routes/{core,panels,auth}.rs` | Session export; Completion notifications | request limits, audit coverage, local-only export and search |

Explore and Plan agents do not load `CLAUDE.md`, so each brief carries everything:

> Read-only: no edits, no cargo or builds, no network; never open
> `docs/learning/`. Scan only `<paths>`. First read `AGENTS.md` Traps and the
> INVARIANTS.md sections `<read first>`. Report at most 5 findings you confirmed
> by reading the whole function or file, not grep hits. Zero is valid; a note or
> question is never a finding. Per finding: title; `path:lines`; trigger (a
> concrete input or state); impact; `verify_with` (a command that runs the
> changed code); `refuted_by` (the check that could have killed it); category;
> effort. Do not report the tells below.

**Tells.** Each is a documented decision. A finding that shows code or another
doc *contradicting* the decision, or an input that defeats its stated test, **is
reportable**: quote both sides.

- Restates a decision in the document's own terms (cite the line): Linux
  `unshare --net` is network-only; the disposable-copy proof on approve;
  ad-hoc signing (`docs/DESKTOP.md`).
- Flags duplication a test pins: `RUN_ID_PATTERN`, planner and check timeouts,
  `MAX_TIMEOUT_SEC` (`duplicated_constants_still_agree`), route-count asserts,
  SHA literals. An unpinned duplicate, or a shim budget shorter than the child's
  own bounds, is reportable.
- Calls a signal a defect only because its name overstates it, where "Signals
  weaker than their name" lists it and the code matches. Windows is weaker than
  Unix only where a doc names the guarantee.
- Style, file size, a newer dependency, a speculative speedup.
- CI hardening already done: SHA-pinned actions, permissions, concurrency,
  `timeout-minutes`, `persist-credentials: false`, zizmor/actionlint/shellcheck,
  `cargo deny` on both crates, `--locked`, Dependabot. A path outside the
  mechanism (another Cargo manifest, scripts outside `scripts/`) is a gap: show
  it with `git ls-files`.

Behind a gate that ships closed: report it as **latent**, naming the gate key and
the config that opens it. Drop it only if no operator setting or request can.

No time box: subagents have no timeout and prompt text only steers
(`AgentDefinition` has `maxTurns`, depth and spend caps). Stop when the paths are
read or you hold 5 findings; the orchestrator trims to the top 5 by severity,
then by how strong `refuted_by` is.

## 2. Verify blind (separate read-only agents)

Pool findings and merge duplicates (same file, within 30 lines). Give each
verifier about 10 with only title, `path:lines`, trigger and impact: no scanner
identity, no `refuted_by`. Its brief carries the scan brief's rules, the shard's
read-first sections and the tells; it re-opens each line and tries to kill it.
Return: `matches_claim`, `real` (defect, risk, coverage gap or measured cost),
`already_handled` (grep tests and CI), `intentional` (true only if a doc line
states the behavior **and** the code matches it; a mismatch is `real`),
`latent`, `worth_a_pr`, `confidence` (low|medium|high), `evidence` (`path:line`
re-read or a read-only command; quote both sides of a mismatch).

**Keep** matches, real, not handled, not intentional, worth a PR, confidence at
least medium. Latent findings stay, labelled. A model rechecking its own claim
without new evidence is unreliable, hence mandatory evidence. Why this shape:
PR #487 (one 2026-10-14 trial; recall was not measured).

## 3. Dedup and select

`gh pr list --state open --limit 100 --json number,title` (or read-only GitHub
MCP list calls). Drop covered candidates and CyClaw-product ideas (a sibling
project). Rank by severity (correctness and security, then auditability, CI,
performance, maintainability), then confidence, then effort. Select 0-4 chunks,
one concern each; one solid chunk beats four thin ones.

## 4. Implement (only on explicit approval)

Start from a clean tree. Use the branch the session assigns, else
`git switch --no-track -c claude/optimize-<slug> origin/main`. Never
`checkout -B` over commits; never commit to `main`. A shared hotspot file
(`ci.yml`, `config.default.yaml`, `INVARIANTS.md`, `AGENTS.md`, `src/shim`,
`REGISTERED_PATHS`, `deny.toml`): consolidate into one PR, or defer the later
chunk until the first merges. Never stack bases.

Reproduce first: run `verify_with` before editing, then show it passing. Keep
diffs minimal. Core paths (`src/shim`, `guards.rs`, `headers.rs`, `writer.rs`,
`sandbox.rs`, `workspace.rs`, `config.default.yaml`) need an invariant statement
in the PR body, and the operator runs `/cgagentharness-invariant-guard`. Also:
`cgagentharness-config-guard`, `cgagentharness-write-policy-redteam`,
`verification-specialist` (break a chunk), `dep-sync` (lock or toolchain drift).

Trial-merge each branch against every open PR head with
`git merge-tree --write-tree HEAD origin/<branch>` (git 2.38+, no tree change)
and fill `## Suggested merge order of open PRs` from the template.

## 5. Verify each chunk

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Then **run the changed code** against an owned temp `CGAGENTHARNESS_HOME` and a
unique port. `desktop/` is a separate crate on its own toolchain: run the same
from `desktop/`, and `cargo deny` as `desktop.yml` runs it. Workflow edits:
actionlint and zizmor. Script edits: the matching contract test in
`scripts/verify-local.sh`.

If `verify_with` is a test it is the one targeted test: `GROK_API_KEY=
ANTHROPIC_API_KEY= DEEPAGENT_API_KEY= CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=
cargo test --locked --test <target>` (it may prompt; ask). `verify-local.sh`
runs the full suite and is CI's job: leave the PR's quality-bar row unchecked
and name the skip and head SHA under Risks. Run `cargo deny check` if installed;
its absence is not a pass. Performance claims need before/after numbers with
conditions. After a `.md` edit check its `DOCS_BUDGET` cap.

When approved to publish: draft PR from `.github/PULL_REQUEST_TEMPLATE.md`,
`scripts/check-pr-template.sh <body-file>`, then read the advisory review gate.

## 6. Report

Shards scanned, shards **not** scanned; candidates -> killed (with the reason) ->
kept (latent labelled); chunks with merge order; checks run and skipped. Never
weaken a test, gate or invariant for speed or tidiness; I6 and the closed write
gates are not optimization targets. Never transplant CyClaw topology or defaults.
