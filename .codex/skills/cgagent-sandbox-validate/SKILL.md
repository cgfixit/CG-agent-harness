---
name: cgagent-sandbox-validate
description: >
  Grok-adapted CG-agent-harness sandbox validator. Evidence-backed static verification of origin/main against hard-sandbox, I6 isolation, clone-jail, and coding-pipeline gates. Limited to what pure code inspection and mock logic in the code_execution REPL or GitHub fetches can test (no live OS confinement, no real Cargo builds, no external installs, no real models). Verifies production_sandbox selection, fail-closed exit 3, Seatbelt/bwrap/netns/JobObject backends, no ArgvList in production, write-before-judge, confirm+reason, I6 process isolation. Explicitly SKIPs live Seatbelt/bwrap runs, full offline Cargo, real-repo clones, and platform-specific process cleanup.
disable-model-invocation: true
---

# CG-agent Sandbox Validate (Grok)

**The contract, stated precisely.** A green run means the static invariants hold for the pinned SHA: production_sandbox never selects ArgvListSandbox; missing backend yields HARD_SANDBOX_UNAVAILABLE (exit 3); Linux prefers bwrap (fs+net) with explicit netns fallback; macOS uses deny-default Seatbelt; clone jail and write gates remain closed by default; I6 isolation is intact. It does **not** mean live confinement was exercised, real Cargo verified, or residual process-cleanup limits were eliminated. Code, INVARIANTS.md, and config.default.yaml always win over this document.

Pinned to origin/main @ `905138160e91984083558f7332058491603f8ad0` (2026-10-23, [security] Tokio interval race fix). Refresh the source before each run; this is evidence, not a permanent pin. Baseline adapted from CyClaw cyclaw-sandbox-validator and harness verification-specialist / invariant-guard skills, with untestable live OS surfaces removed.

## Operator map — Grok-executable ladders only

| Ladder | What it proves | How (code_execution / GitHub fetch) | Does **not** prove |
|--------|----------------|-------------------------------------|--------------------|
| Static sandbox selection | production_sandbox() prefers DarwinSeatbelt / LinuxBubblewrap / WindowsJobObject; ArgvList excluded; both Linux backends missing => exit 3 | Fetch sandbox.rs; AST or string-scan for production_sandbox, prefer_linux_sandbox, HARD_SANDBOX_UNAVAILABLE | Runtime probe success, actual Seatbelt/bwrap binary presence |
| Fail-closed & exit codes | No software fallback; missing sandbox-exec/unshare/bwrap => exit 3; write refused => 4; I6 preserved | Scan error paths, tests/agentic_foundations.rs sandbox_* comments, INVARIANTS.md | Live exit codes on host |
| Profile construction | seatbelt_profile deny-default, no net grant by default; bwrap_argv never binds host root, includes --unshare-net, --tmpfs, exclusive scratch | Fetch sandbox_wrap.rs; assert refuse conditions (host root, home overlap) | Actual confinement strength, residual Unix sockets |
| Clone jail & write gates | .git name-equivalence refusal, capability-relative opens, confirm+reason required, judged-before-land | Fetch workspace.rs, writer.rs, INVARIANTS.md clone-jail / nothing-lands sections | Real clone escape attempts |
| I6 isolation | src/server never imports agentic; only shim crosses | String/AST scan on module boundaries | Runtime linkage |

## Explicit SKIP list (Grok cannot test)

* Live sandbox-exec / bwrap / unshare execution, process-group kill, descendant cleanup
* Full offline Cargo --frozen in owned scratch
* Real-repo clone jail probes or exact-edit landings
* MCP stdio capability enforcement at spawn time
* Windows Job Object kill-on-close or cgroup v2 limits
* Platform smokes, scripts/smoke-ollama.sh, native desktop acceptance
* Any network call or real model inference

## Steps (for the code_execution tool)

1. Record the target SHA (9051381... or fresher HEAD via GitHub tools).
2. Fetch raw sources: src/agentic/executor/sandbox.rs, src/common/sandbox_wrap.rs, INVARIANTS.md, assets/config.default.yaml, relevant tests.
3. Assert production_sandbox selection logic and fail-closed paths.
4. Scan for host-root bind refusal, home-overlap refusal, ArgvList exclusion.
5. Confirm I6 source isolation and write-gate defaults.
6. Emit a report: SHA, PASS/FAIL/SKIP per ladder, evidence snippets (no secrets). A SKIP is not a pass.

## Gotchas

* The REPL has no internet or OS sandbox binaries; do not attempt cargo build or live probes.
* Numbers and exact profile strings are derived from the live tree; if they disagree with this document, trust the tree.
* Residual risks (best-effort ancestry cleanup, macOS detached descendants, Windows no fs/net secrecy) are documented in PROCESS_LIFECYCLE.md and are not claimed closed by this skill.

## Strategic note

These checks pin the evolutionary stable strategies (hard sandbox before judgment, fail-closed on missing backend, I6 isolation, explicit confirm+reason) that historical agentic tool failures (injection, exfil, unbounded side-effects) repeatedly demonstrate are required to separate contained local coding from repeated containment breaks. The trusted-operator loopback threat model remains the operating assumption; this skill does not expand the blast radius.
