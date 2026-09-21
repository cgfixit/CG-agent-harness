# Issue 102 selected close-out acceptance

The [review](https://github.com/cgfixit/CG-agent-harness/issues/102#issuecomment-5759850387)
and [operator decisions](https://github.com/cgfixit/CG-agent-harness/issues/102#issuecomment-5760054339)
specify enough to implement the selected remaining work. The ten original roadmap
items already had merged implementations. This close-out preserves those features
and addresses the selected capability, lifecycle, ownership, calendar, delivery and
private gateway work; it does not rebuild spend prediction, repository retrieval
or atomic limit reload.

Status below is pre-merge candidate acceptance on 2026-09-21, based on
`cd55c3f73e7b5877a2ce763f8d5342ca70a6b36b`. Passing local tests does not mean the
new PRs are merged or their hosted checks have passed. Keep #102 open until all
selected PRs merge and the accepted main revision passes the required checks.

| Selected work | Implementation and evidence |
|---|---|
| 1: MCP capability policy | Merged #207. Required filesystem confinement, explicit network policy and actual backend capabilities; missing protection refuses. Existing home overlap, broker and confirmation gates remain. |
| 2: native process lifecycle | Merged #207. Linux service/cgroup supervision and Windows Job Objects have native acceptance; macOS refuses unavailable strict lifecycle modes and documents explicit weaker exceptions. No filesystem/network guarantee follows from process containment alone. |
| 3A: ownership | This candidate makes session storage/routes, search/export, reviewed goals, jobs and schedules owner-scoped. Ownerless sessions remain quarantined until explicit administrator adoption; unknown/corrupt records are preserved. Logout clears account-bound context. |
| 3B: calendar scheduling | This candidate adds versioned intervals/cron, IANA zones, required one-use previews, persistent occurrence identity and current authority checks. DST gaps/missed/overlapping occurrences skip; repeated wall time fires at most once. |
| 3C: durable completion delivery | This candidate adds a bounded private outbox, metadata envelope v2, stable dedup IDs, explicit owner destinations/subscriptions, finite retry/batch/rate/retention, inspection and confirmed replay. Replay cannot restart a job. |
| 3D: safe evolution | Typed bounded settings and versioned records; the exact 22-key reload allowlist is unchanged. Listener, key, destination and sandbox grants remain restart-only; fresh delivery checks may revoke startup authority. |
| 4: private memory gateway | Implemented and locally verified in the separate main-based `codex/private-memory-gateway` candidate under #185. Dedicated hashed machine keys, immutable owner/read scope, three selected read tools, separate loopback listener, default off. Combined integration was verified before drafting either PR. |

## Evidence and limits

Main baseline: 650 tests/47 suites. This automation candidate: 671/48. Gateway:
661/48. Combined: 682/49 plus 11 native desktop sidecar lifecycle/auth/home-lock
tests. Strict Clippy, formatting, cargo-deny, actionlint and affected JavaScript
contracts passed. See [screenshots and artifact provenance](../screenshots/issue-102/FINAL_ACCEPTANCE.md)
for Computer Use of adoption, calendar preview/activation/cancellation, delivery
replay/revocation, account isolation and final logout timing reset.

Merged #207's head `42f681a512f805709506387bb9e782c847559b87` passed native Linux
confinement/service and Windows Job Object acceptance in
[run 35608075769](https://github.com/cgfixit/CG-agent-harness/actions/runs/35608075769).
Those are hosted OS results; local Safari/native sidecar results are macOS.
The new candidates require their own hosted CI before merge.

Scope differences are explicit: pinned notes, persona, selected model, aggregate
spend and underlying agentic run records remain shared portal resources. Sessions
and detached job records are owner-scoped. Cron dispatch is at-most-once and may
miss a crash-window occurrence. Webhooks are at-least-once and require receiver
deduplication; payloads stay metadata-only, and Telegram/rich-content adapters are
not enabled. A single backend owns a home/outbox. Repository retrieval reuses the
Tantivy crate in a separate bounded index. Reload still covers only its documented
22 limits. Gateway remote transport and proposal writes require separate decisions
and acceptance; no public listener or deployment is part of this close-out.

No paid-provider billing, packaged/notarized GUI or remote deployment is proven by
these fixtures. The original checklist is reconciled here without prematurely
closing the issue or claiming an unmerged candidate is released.
