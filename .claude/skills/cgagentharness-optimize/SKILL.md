---
name: cgagentharness-optimize
description: Finds evidence-backed Rust, runtime, desktop and CI improvements in CG-agent-harness by running area-sharded read-only scans, verifying every finding against the code, and grouping survivors into focused draft PRs when publication is authorized. Use for an optimization or maintainability sweep of this repo, not for one already-known fix.
---

# CG-agent-harness Optimize

Output is **verified findings**, not a list of suspicions. Zero verified findings
is a complete result; never manufacture work. Human merges. Optional `ARGUMENTS`
(an area, path or topic) restricts the sweep to that one shard.

## 0. Preflight

```bash
git fetch origin main && git status -sb && git log --oneline origin/main -8
git switch -c claude/optimize-<topic> origin/main   # skip for a read-only sweep
```

Use the branch the session assigns if it names one. Never `checkout -B` over a
branch that has commits; never commit to `main`.

## 1. Shard and scan (read-only)

One scan per shard, run in parallel. The `server-*` shards (about 25k lines) are
**opt-in**: name one in `ARGUMENTS`. A full sweep reports them as not scanned.

| Shard | Paths | Look for |
|---|---|---|
| agentic | `src/agentic` | writer gates, clone jail, sandbox, digest binding, gh client, process lifecycle, dead paths (`writer.rs`, `workspace.rs`, `executor/sandbox.rs` are core) |
| edge | `src/shim`, `src/server/{guards,headers}.rs`, `routes/mod.rs`, `agent_policy`, the `prepare_run` path | guard order, `ACTIONS` vs CLI dispatch, argv encoding, shim timeouts vs child bounds, route drift, run/jobs/schedule lockstep |
| io-build | `src/llm`, `src/common`, `src/netconnect`, `desktop/src`, `scripts/` | loopback validators, timeout and retry bounds, secret echo, bounded capture, packaging order and atomicity (re-check live docs before asserting) |
| ci-tests | `.github/workflows`, `tests/` | coverage gaps (hostile argv, exit-4 refusal, jail, shipped gates), assertions that pass for the wrong reason, CI gaps not in the done-list below |
| server-data | `src/server/{structured_memory*,memory_notes,notes_corpus,retrieval*,passage_index,compaction}.rs` | owner isolation, SQLite kept off async workers, bounded reads |
| server-web | `src/server/{web_*,chat_web,attachment*,mcp*}.rs` | permission gates, pinned DNS, no proxy/redirect/retry, bounded fetch, secret echo |
| server-chat | `src/server/{sessions,slash,prompts,notification*}.rs`, `routes/{core,panels,auth}.rs` | request limits, audit coverage, session export and search staying local |

Explore and Plan subagents do **not** inherit `CLAUDE.md`, so the brief must
carry the repo's decisions. Each brief says:

> Scan ONLY `<paths>`. First read `AGENTS.md` Traps and the `INVARIANTS.md`
> section for this area (AGENTS.md area table). Report at most 5 findings you
> confirmed by reading the whole function or file, not grep hits. Zero is valid;
> do not pad; a process note or question is never a finding. Per finding: title;
> `path:lines`; trigger (concrete input or state); impact; `verify_with` (exact
> command or test); `refuted_by` (the check you ran that could have killed it);
> category (perf/security/auditability/maintainability/CI); effort.

and appends these **tells of a false finding** (each was a real false positive):

- Restates a documented decision. Only a *new* bypass of the decision's test
  counts. Examples: Linux `unshare --net` fallback, ad-hoc signing,
  disposable-copy re-verify on approve, token passthrough to the child, Windows
  guarantees weaker than Unix.
- Flags deliberate duplication or lock-step pins: `RUN_ID_PATTERN`, timeout
  pairs, route-count asserts, SHA literals in tests.
- Treats a signal documented as weaker than its name as a defect
  (`unslop.enabled`).
- Sits behind an earlier gate and is unreachable today. Report as latent, naming
  the gate.
- Is style, file size, a newer dependency or a speculative speedup.
- Is CI hardening already done: SHA-pinned actions, permissions, concurrency,
  `timeout-minutes`, `persist-credentials: false`, zizmor/actionlint/shellcheck,
  `cargo deny` on both crates, `--locked`, Dependabot. Re-report one only with a
  command showing a current gap.

**Stopping rule, not a time box.** Subagents expose no timeout (Anthropic
documents `maxTurns`, depth and spend caps; prompt text only steers), so "~N
minutes" cannot be enforced or measured. Stop a scan when its paths are read or
it holds 5 confirmed findings. Trim to the top 5 yourself: in the trial below 3
of 4 scanners exceeded a prompt-level "at most 5".

## 2. Verify blind (separate agents)

Pool findings, merge duplicates (same file, nearby lines), and give each
verifier about 10 candidates with no scanner identity. It re-opens each cited
line and tries to kill it. Fields: `matches_claim`, `real` (defect, risk,
coverage gap or measured cost), `already_handled` (grep tests and CI),
`intentional` (documented), `worth_a_pr`, `confidence`, `evidence` (`path:line`
re-read or command run). **Keep** only matches, real, not handled, not
intentional, worth a PR, confidence at least medium. A model rechecking its own
claim without new evidence is unreliable, so evidence is mandatory.

*Why (one trial, 2026-10-14, two runs per control arm; a prior, not proof):*
raising "~4 minutes" to "~5" left verified yield flat while files read rose
about 30%. Unguided single-pass scans kept 69% of findings and swung 4/9 to 9/9
between identical runs; sharded scans with tells and this gate kept 91%. Only 10
of 48 findings came from more than one scan, so repeating a shard adds
coverage, not confirmation.

## 3. Dedup, select, topology

List open PRs (`gh pr list` or the GitHub MCP tools; keep `number` and `title`
only). Drop covered candidates and CyClaw-product ideas. Select 0-4 chunks that
earn a PR; one solid chunk beats four thin ones. Rank by severity (correctness
and security, then auditability, CI, performance, maintainability), then
confidence, then effort. A chunk is 1-2 major concepts or 3-5 minor tasks.
Announce each in one line: section touched, leverage.

Map **file -> chunks**. Hotspots: `.github/workflows/ci.yml`,
`assets/config.default.yaml`, `INVARIANTS.md`, `AGENTS.md`, `src/shim`,
`REGISTERED_PATHS`, `deny.toml`. For a shared file either consolidate into one
PR, or sequence both on `main`: land the first, then
`git rebase --onto origin/main <old-parent-tip>`. Never stack bases. Trial-merge
each branch on fresh `origin/main` (`git merge --no-commit --no-ff`, then
`--abort`) and fill `## Suggested merge order of open PRs` from the PR template.

## 4. Implement authorized scope only

Keep diffs minimal and one concern per PR. Reproduce first: run the finding's
`verify_with` before editing, then show the same check passing. Core paths (`src/shim`, `guards.rs`,
`headers.rs`, `writer.rs`, `sandbox.rs`, `workspace.rs`,
`assets/config.default.yaml`) need an explicit invariant statement in the PR
body, and the operator runs `/cgagentharness-invariant-guard` (operator-only).
Config edits: `cgagentharness-config-guard`. Write-surface changes:
`cgagentharness-write-policy-redteam`. Dependency or lock drift:
`dep-sync`, not this skill.

Publish only when authorized: draft PR, body from
`.github/PULL_REQUEST_TEMPLATE.md`, then
`scripts/check-pr-template.sh <body-file>`. Skill selection never authorizes
push, merge or release, and never comment `@codex review` unasked.

## 5. Verify each chunk

```bash
cargo fmt --all -- --check
${CLIPPY:-cargo clippy} --all-targets --all-features --locked -- -D warnings
```

Then **run the changed code** (route, binary or function) against an owned temp
`CGAGENTHARNESS_HOME` and a unique port. Only if nothing can run, one targeted
test with keys blanked: `GROK_API_KEY= ANTHROPIC_API_KEY= DEEPAGENT_API_KEY=
CGAGENTHARNESS_AGENTIC_WRITE_DISABLE= cargo test --locked --test <target>`
(`invariant_guard` after structural or doc edits). Never the full suite locally;
CI runs it. Run `cargo deny check` if installed: `verify-local.sh` skips it when
missing, so its green run is not deny evidence. Performance claims need
before/after numbers with conditions. After a `.md` edit run `wc -w` against its
`DOCS_BUDGET` cap and lower the cap of any file that shrank.

## 6. Report

Shards scanned **and not scanned**; candidates -> killed (with the reason) ->
kept; chunks with merge order; checks run and skipped. Do not restate
`AGENTS.md` or `INVARIANTS.md`: truth order and traps live in `AGENTS.md`,
contracts in `INVARIANTS.md`, session traps in `cgagentharness-gotchas`. Never
weaken a test, gate or invariant for speed or tidiness; I6 and the closed write
gates are not optimization targets. Never transplant CyClaw topology or defaults.
