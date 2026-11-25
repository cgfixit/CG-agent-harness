# Documentation index

Read the owning guide, then the implementing code and
[shipped config](../assets/config.default.yaml).
[AGENTS.md](../AGENTS.md) owns contribution rules.
[INVARIANTS.md](../INVARIANTS.md) owns security and lifecycle contracts.
Use the [source index](../src/README.md) and [tests index](../tests/README.md)
for execution paths and verification.

## Setup, models and recovery

- [Install](INSTALL.md) covers first run, managed keys, reload and recovery.
- [Models](MODELS.md) covers exact tags, allocated context, tuning and cloud consent.
- [Desktop](DESKTOP.md) covers owned WKWebView, build, packaging and quit.
- [Troubleshooting](TROUBLESHOOTING.md) maps errors to commands and logs.
- [Releasing](RELEASING.md) covers Bundle, publication delay and manual dispatch.

## Console and daily use

- [Console](CONSOLE.md) covers chat, goals, loops, attachments and commands.
- [HTTP routes](API_ROUTES.md) lists method, path and purpose.
- [Spend and notifications](SPEND_AND_NOTIFICATIONS.md) covers billing evidence,
  estimates, destination grants and replay.

## Memory

- [Memory guide](MEMORY_GUIDE.md) covers pinned notes, uploaded notes, fact
  review and retrieval.
- [Structured memory](STRUCTURED_MEMORY.md) covers owner binding, gates,
  local suggestions, proposal application and FTS. Start in
  `src/server/structured_memory.rs` and `src/server/routes/structured_memory.rs`.

## Web, browser and external tools

- [Secure research](SECURE_RESEARCH.md) covers roles, HTTPS, URL authority
  and standalone research.
- [Web decision router](WEB_DECISION_ROUTER.md) covers provider fallback,
  passage receipts and goal evidence reuse. Start in
  `src/server/chat_web`, `web_provider.rs` and `web_index.rs`.
- [Browser use](Browser-Use.md) covers Chrome tasks, takeover and egress.
  Start in `browser/sidecar.mjs` and `src/common/browser_host.rs`.
- [MCP](MCP.md) covers declared clients and the separate private gateway.
- [Netconnect](netconnect.md) covers scoped passive data and diagnostics.

## Coding and maintenance

- [Coding pipeline](CODING_PIPELINE.md) covers gating, clone edits, offline
  verification, approval and draft publication.
- [Process lifecycle](PROCESS_LIFECYCLE.md) covers capture and OS-specific
  subprocess ownership.
- [Git hooks](GITHOOKS.md) covers staged and pushed changes and operator overrides.
- [Dependencies](DEPENDENCIES.md) covers toolchain, lock and advisory policy.

## Folders

[Learning](learning/README.md) is operator reading material.
[Screenshots](screenshots/README.md) indexes recorded evidence.
Follow the [docs policy](../AGENTS.md#docs-policy) for edits and budgets.
