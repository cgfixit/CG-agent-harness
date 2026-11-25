# Troubleshooting

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `tag_missing` or unexpected fallback | Inventory lacks the exact tag | Compare `/model` and [Models](MODELS.md). Restart to reevaluate fallback. |
| `CHAT_OTHER_ACCOUNT` | Another account holds the shared model | Wait for the owner to finish. |
| `HARNESS_LLM_ERROR` | Local endpoint error, truncation or repeated output | Check `/model profile` and `chat_model_call` audit metadata, including `finish_reason` and `tools_offered`. |
| `/loop` appears stuck | Model busy, request deadline, cooldown or repeated failure | Check `/api/loop/status` and `/loop stop` if needed. See [loop limits](CONSOLE.md#71-sessions-and-bounded-chat-continuation). |
| `/browse` cannot find Playwright | Sidecar dependencies or path missing | Prepare [browser use](Browser-Use.md) and use an absolute desktop sidecar path. |
| `BROWSER_AUTH_REQUIRED` or `BROWSER_ORIGIN_BLOCKED` | Accounts off, or origin blocked/unreadable | Sign in as admin with accounts enabled and check `/web status`. Private origins also refuse. |
| `AUTH_REQUIRED` or login refused | Session expired, revoked or evicted | Sign in again. Check [account caps](SECURE_RESEARCH.md#account-storage-and-recovery). |
| `AUTH_LIMIT` | Account or session capacity refusal | Check the current account/session inventory before retrying. |
| `WEB_SEARCH_COOLDOWN` | Provider is paused after a challenge, block or quota failure | Read the provider code and cooldown. Check [provider selection](WEB_DECISION_ROUTER.md#provider-selection). |
| `WEB_UNITS_UNAVAILABLE` or `WEB_SEARCH_DAILY_CAP` | Paid ledger unavailable or UTC-day allocation exhausted | Inspect current provider and ledger metadata. Failed daily dispatch can use an eligible fallback. |
| Home lock or occupied port | Another server owns it | Identify the owner. Quit it normally or use a separate home and port. |
| App waits at startup | OS file access or incomplete bundle | Check macOS prompts and retry from the complete Applications bundle. |
| Config changes have no effect | Wrong home or restart-only setting | Check Setup and [reloadable limits](INSTALL.md#reload-limits-without-a-restart). Closing the app window keeps the backend running. |
| No memory suggestions | Source/store gate, model or queue/model failure | Read **Suggestion activity** and `/api/structured-memory`. See [memory](MEMORY_GUIDE.md). |
| A persona or skill is missing from chat | Wrong home, selected file invalid, toggle off or edit stale | Check `/soul status`, `/skill status` and `/prompt`. |
| Coding check refuses before inference | Missing preparation or sandbox | Inspect [offline Cargo verification](CODING_PIPELINE.md#offline-cargo-verification) and [execution boundary](CODING_PIPELINE.md#execution-boundary). |
| Coding write refuses | Gate, revoked authority, missing confirmation/reason or missing reviewed PR body | Check [policy](CODING_PIPELINE.md#git-approval-and-publication) and [kill switch](CODING_PIPELINE.md#95-the-kill-switch). |
| Spend is partial | Retained ledger unreadable or invalid | Preserve the ledger and check [coverage](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero). |
| Webhook missing or duplicated | Grant, outbox failure or retry | Check **Job webhooks** and the delivery audit. Deduplicate by `delivery_id`. |
| `stdio MCP child closed` | Child stdout closed | Reproduce under a controlled environment. See [stdio diagnostics](PROCESS_LIFECYCLE.md#mcp-stdio-diagnostics). |

## Read-only host inspection

Use a declared [read-only MCP server](MCP.md#read-only-host-files-on-macos)
and confirm each call. The home and overlaps refuse. Read grants should
exclude secrets. Chat and `/loop` cannot call MCP.

## Diagnostics

`/diag` reads bounded allowlisted `<home>/logs/` files for admin/auditor
with literal `diagnostics.logs_enabled`. Redaction precedes scanning.
`/diag inject` stages one untrusted excerpt for a local turn.
`diagnostics.model_tool: true` enables eligible plain local chat log tools.

| Symptom | Likely cause | Fix |
|---|---|---|
| `DIAG_DISABLED` | Gate off or missing | Set literal true and restart. |
| `DIAG_SCANNER_REFUSED` | Redacted text matched the scanner | Narrow `--event`, `--since` or `--last`. |
| `DIAG_REFUSED` | The signed-in role lacks log investigation permission | Use an administrator or auditor. |
| `DIAG_SURFACE_FORBIDDEN` | Staged logs with cloud or coding | Use local chat or `/diag forget`. |
