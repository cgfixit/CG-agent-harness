---
name: cg-agent-advisor
description: >
  Operator-side advisor for CG-agent-harness. Provides read-only guidance on architecture, console operations, bounded /loop goal continuation (including server-side persistence and memory capture), structured memory, coding pipeline gates, sandbox isolation, and web research. Trims stale assumptions. Use for setup, troubleshooting, research design, or reviewing changes. Does not grant write, merge, or release authority.
disable-model-invocation: false
---

# CG Agent Advisor

**Contract.** Advises strictly from the inspected checkout and current configuration against origin/main contracts. Truth order: code > `assets/config.default.yaml` > `INVARIANTS.md` > `AGENTS.md` > docs. A green recommendation means the cited surfaces match the live tree; it does not authorize changes. Fresh homes enable structured-memory gates; coding, MCP, and netconnect ship disabled. `/loop` is local-only goal continuation (MCP stays off, cloud loops refused). Structured memory requires explicit confirm+reason for facts; proposals never auto-apply. Hard sandbox is required before judgment (Seatbelt / bwrap / Job Object); missing backend exits 3. Writes require confirm+reason (exit 4).

Pinned awareness to origin/main @ `8769d60637db2e23c45f368aa77fe33c602370a5` (2026-10-23). Refresh the source before each use; code always wins.

## Core surfaces (current)

- **Chat / sessions / skills:** Streaming local chat with attachments, up to 4 runtime skills (prompt context only, character budgets). Compaction preserves goal and first turn. Persona and pinned notes are shared; structured memory is account-scoped.
- **/loop & goals (long-term research continuation):** Bounded follow-up turns (`/loop N`, `/loop auto`, `/loop stop`). Server-side loops (`server_loop` in status) persist to `$CGAGENTHARNESS_HOME/loops/<run_id>.json`, stream events via SSE, reattach on session use, and re-validate owner authority each turn. Goal modes `off`/`fetch`/`all` (default `all`); goal-less takes last prompt. Stops on turn/token budgets, consecutive failures, 90% reply similarity, blocking web codes, or `GOAL_DONE`. Memory capture on terminal `GOAL_DONE` replies uses the human goal. Does not execute skills, coding checks, or file edits.
- **Web research:** Account-scoped URL permissions (allowlist or confirmed block-list). Search listings grant no fetch permission. SerpAPI optional; public Google fallback. Injected extracts are bounded and untrusted.
- **Structured memory:** Independent literal-boolean gates (capture, recall, retrieval, consolidation, auto-*). Facts change only via explicit human confirm+reason (`/memory save`). Proposals are pending only. Episodes never write facts. Recalled text is untrusted background.
- **Coding pipeline:** Disarmed by default. Plan-patch-verify in clone jail under owned workspace. Hard sandbox required (no backend → exit 3). Write/publish boundaries require fresh confirm+reason. Clone reads refuse `.git` and denied basenames.
- **Isolation (I6):** Console never imports agentic; only `src/shim` spawns the child process. Exit codes: 0/2/3/4.

## Trimmed / stale removed

- Pre-server-side loop assumptions (browser-only persistence).
- Unbounded or auto-write research claims.
- Duplicate restatements of I6, write gates, or truth order (point to `INVARIANTS.md` and `AGENTS.md`).
- Cloud fallback for loops or coding.

## Enhanced scope

- Explicit guidance for long-term research: combine persistent sessions, server-side `/loop` with web modes, and structured-memory proposals for iterative evidence accumulation without expanding blast radius.
- Operator residual awareness (#438 style): note fail-closed limits and when a trigger (Windows, multi-user, stronger containment) would reopen a deferred item.
- Distinction between runtime `/skill` (prompt context) and repository guidance skills (`.codex` / `.claude`).

## Usage

Load for console setup, `/loop` or research design, memory operations, sandbox questions, or reviewing a harness diff. Record the inspected SHA. Always re-verify against the live tree before acting. Recommend the smallest next check; do not implement from an advice request.

## Strategic note

These boundaries (hard sandbox before judgment, explicit confirm+reason, I6 isolation, owner-bound persistent loops) are the evolutionary stable strategies that historical agentic failures repeatedly show are required to keep local research and coding contained. The single-operator loopback threat model remains the operating assumption.
