# Parity status

Generated from [actions.json](actions.json) with `python3 scripts/parity-status.py`. Do not edit this view directly.

Implementation and validation are independent. This is a fixed action inventory, not a percentage of CyClaw ported. Missing actions remain requested work.

Rust baseline: [pinned commit](https://github.com/cgfixit/CG-agent-harness/commit/8cf699a92ecbcce7cbfd8d774eff4aaa5975e3b1). CyClaw reference: [pinned commit](https://github.com/cgfixit/CyClaw/commit/a414ba86ebf5f3c8bb901466c16b4f015bbc79c9).

See [execution state](WORK.md) for baseline evidence and native acceptance blockers, and [non-RAG contracts](CONTRACTS.md) for deliberate adaptations.

| ID | Phase | Implementation | Fixtures | Native | Capability |
|---|---:|---|---|---|---|
| M1 | 2 | missing | pending | unavailable | Structured facts store: stable IDs, content, categories, tags, confidence, provenance, active/inactive state and timestamps. |
| M2 | 2 | missing | pending | unavailable | Governed add/update/deactivate proposals, pending/applied/rejected states, explicit reasons, injection validation and audit. |
| M3 | 2 | missing | pending | unavailable | Episode records and retention: query hash, bounded answer summary, model, time and optional raw-query storage. |
| M4 | 2 | missing | pending | unavailable | Memory status/counts and fact/episode/proposal administration APIs or equivalent console controls. |
| M5 | 2 | missing | pending | unavailable | Bounded, escaped, offline HTML export of facts and recent episodes. |
| C1 | 4 | missing | pending | unavailable | Filesystem read connector: status/list/read/stat/grep/glob/largest and bounded context extraction against operator-approved roots. |
| C2 | 4 | missing | pending | unavailable | Filesystem writes: write/append/mkdir/move/delete with separate writable roots, reason/confirmation, dry run, protected destinations and audit. |
| C3 | 4 | missing | pending | unavailable | Soft-delete trash, restore, explicit purge, retention, quota/rate controls and quota-status. |
| C4 | 4 | missing | pending | unavailable | Filesystem connector self-test, Finder reveal, trash-cleanup scheduling and macOS setup helpers. |
| C5 | 5 | missing | pending | unavailable | PostgreSQL and SQL Server read-only connectors using configured credential sources. |
| C6 | 5 | missing | pending | unavailable | SQL schema enumeration, table preview, bounded SELECT/WITH query, EXPLAIN and row count; JSON/CSV output. |
| C7 | 5 | missing | pending | unavailable | SQL status/self-test, safe context generation and bounded operations API. |
| C8 | 5 | missing | pending | unavailable | Network self-inventory and existing ARP/neighbor-cache inspection. |
| C9 | 5 | missing | pending | unavailable | Network status/self-test, structured scoped results and redacted context output. |
| C10 | 1 | missing | pending | unavailable | Connector configuration, capability reporting and bounded dispatch through the out-of-band shim. |
| I1 | 8 | missing | pending | unavailable | Dropbox/rclone sync transport: pull/copy, opt-in bisync, allowed remote, filters, lock, retries/deadline, transfer/delete fuses and conflict handling. |
| I2 | 8 | missing | pending | unavailable | Sync status/check/setup/self-test, schedule/unschedule and operations API. |
| I3 | 9 | missing | pending | unavailable | Telegram outbound notifications and allowlisted private-chat identity. |
| I4 | 9 | missing | pending | unavailable | Telegram inbound long polling, cursor/dedup state and local chat bridge. |
| I5 | 9 | missing | pending | unavailable | Telegram consent workflow with single-use, expiring approvals; attachment staging and explicitly confirmed save through fsconnect. |
| I6 | 9 | missing | pending | unavailable | Telegram polling and health scheduling helpers. |
| I7 | 10 | missing | pending | unavailable | OpenTweet topic → local generation → validation → remote draft, with separately selected scheduling. |
| I8 | 10 | missing | pending | unavailable | OpenTweet credential/config status, self-test and schedule-plist/task generation. |
| A1 | 1 | needs-extension | pending | unavailable | Extend credential management only for implemented subsystems. |
| A2 | 1 | missing | pending | unavailable | macOS Keychain secret setup and launch-time environment injection. |
| A3 | 1 | implemented | fixture-verified | unavailable | Managed key-loading/startup flow, rather than requiring the user to manually source the console-written dotenv. |
| A4 | 3 | missing | pending | unavailable | Named device bearer tokens: create/list/revoke, hashed storage, role association as implemented upstream, and authenticated identity resolution. |
| A5 | 3 | needs-extension | pending | unavailable | Self-service password change; expose user enable/disable; identity CLI; role-restricted audit summary. |
| A6 | 3 | needs-extension | pending | unavailable | Consistent session/device-token identity on the non-RAG conversation surface when authentication is enabled. |
| A7 | 3 | missing | pending | unavailable | Optional SQLite/PostgreSQL auth and personality persistence and persistent rate-limit storage. |
| A8 | 3 | needs-extension | pending | unavailable | Soul/personality versioning, file/DB shadow consistency and drift/hash checks, history/interaction maintenance, backups, reload and restore. |
| A9 | 3 | missing | pending | unavailable | Soul propose/apply service with reason, scans, atomic update and recovery on partial failure. |
| L1 | 6 | needs-extension | pending | unavailable | Expose standalone `real-repo-run-plan` equivalent: bounded repo/issue/PR context → local or explicitly approved cloud model → reviewable plan text/file. |
| L2 | 6 | needs-extension | pending | unavailable | Non-RAG conversational provider selection and explicitly consented external fallback. |
| L3 | 6 | needs-extension | pending | unavailable | External provider pre-action hook with bounded structured input, allow/deny/error handling, timeout and audit. |
| L4 | 6 | needs-extension | pending | unavailable | Explicitly trusted local-model hosts for an operator-owned container/LAN server. |
| L5 | 1 | needs-extension | pending | unavailable | Broader health/readiness reporting with exact configured-model inventory checks, backend failures and safe optional provider probes. |
| L6 | 6 | needs-extension | pending | unavailable | Full optional UNSLOP advisory scanner behavior: richer phrase/structure findings, surface-aware checks and comparable redacted metrics/feedback. |
| L7 | 6 | needs-extension | pending | unavailable | Audit active non-RAG verification-skill coverage and port relevant checks, without copying Python-specific commands verbatim. |
| O1 | 6 | needs-extension | pending | unavailable | Equivalent optional non-RAG input/output guardrails beyond the existing injection scanner: soul-mutation intent, applicable soul-leak checks and brokered checks around generation. |
| O2 | 6 | missing | pending | unavailable | Guardrail status/self-test, profile reporting, metrics and verified call-site inventory. |
| O3 | 7 | needs-extension | pending | unavailable | Audit summary API/CLI, aggregate operational metrics, provider spend estimation and price-freshness/cost comparison. |
| O4 | 7 | missing | pending | unavailable | Numbat derived event stream and producers: structured redacted/hash-based events, rotation/dedup and fail-soft projection from authoritative audit. |
| O5 | 7 | missing | pending | unavailable | Optional CEL observation and non-RAG-relevant sequence analysis. |
| O6 | 1 | needs-extension | pending | unavailable | Central telemetry/update-check opt-outs and safe child-environment construction for all relevant entrypoints, launchers and new integrations. |
| O7 | 1 | needs-extension | pending | unavailable | macOS installation/launch/service management and supervised launchd plist generation, plus connector schedules and secrets delivery. |
| O8 | 11 | missing | pending | unavailable | Equivalent optional container/service deployment and TLS/certificate tooling for a network-facing deployment. |
| O9 | 11 | needs-extension | pending | unavailable | Cross-platform install/scheduling/acceptance coverage. |
