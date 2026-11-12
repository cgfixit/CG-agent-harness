# Troubleshooting

See also [models](MODELS.md) and [coding](CODING_PIPELINE.md).

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Spend reports partial, unreadable, too large or invalid UTF-8 | A retained ledger generation could not be summarized | Preserve the ledger; inspect permissions and size. [Missing evidence is not zero spend](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero) |
| A job finished but no webhook arrived | Notifications off, no restart, refused destination, queue overflow or delivery failure | Check **Job webhooks**, `notification_delivery`/`notification_dropped` audit and `/agent job <id>`. Replay the delivery, never rerun the job |
| Webhook receiver reports duplicates | A lost response retried the batch | Deduplicate on stable `delivery_id`; batch membership can change. At most three attempts per delivery/replay |
| MCP reports `stdio MCP child closed` | Child stdout ended; stderr is discarded | Reproduce the child in a controlled environment. [MCP diagnostics](PROCESS_LIFECYCLE.md#mcp-stdio-diagnostics) |
| Command or control missing | Older build | Check `/help`, `Resources/COMMIT`; install [Latest](https://github.com/cgfixit/CG-agent-harness/releases/latest) or a successful main Bundle |
| Coding loop never fetches a file the planner asked for | Cloud planner, clone-jail refusal, `denied_read_basenames` (`.env`, keys, credentials), 6-read cap, or unclosed FILE/EDITS block swallowing `=== READ ===` | Pre-stage ordinary files with `/agent read`. Sensitive basenames refuse operator and model reads (`sensitive_basename`); cloud refuses all model READs; local refusals audit `agentic_real_repo_read_request_refused`. Neither deny-list nor jail detects every secret |
| New sessions share old context | Shared notes/persona and account web context remain by design | Inspect `/prompt`; `/session new` separates history only |
| Chat denies memory exists or claims it can run `gh` | Model output conflicts with the capability contract | Inspect `/memory`, `/tools` and `/prompt`; prose cannot execute commands |
| Automatic suggestions do not appear | Store/capture/source gate off, invalid mode, queue expiry/full, model/store error, or empty valid output | Read **Suggestion activity** and `/api/structured-memory` `automatic_suggestions`; check the `/memory capture` and `auto-suggest-*` gates. Completions do not guarantee suggestions |
| `/memory search`, `save` or `consolidate` reports the store closed or a gate off | `structured_memory.enabled` false, store failed to open, or `retrieval` or `consolidation` (auto: also `auto_consolidation`) off | Enable the store and restart; then `/memory retrieval on` or `/memory consolidation on`, plus `/memory auto-consolidate on` for auto. Explicit off choices survive upgrades. Search never injects, and prose is not a save command |
| Inventory says `tag_missing`, or fallback is selected unexpectedly | Exact configured ID absent from a responding inventory | Compare [inventory/config](MODELS.md) and persisted `/model`; restart to reevaluate fallback |
| Soul says missing, or is saved but not in chat | No `soul.md` in this home (a valid state), toggle off, wrong home or rejected/stale save | `/soul edit` and `/soul on`; confirm Setup home, `/soul status` and `/prompt`; reload a stale editor before saving again |
| Persona/proposal save conflicts or history is full | Changed base revision or 32-record limit | Review content and history; archive records deliberately. Never blindly retry or delete the persona |
| Selected skill prevents chat | Missing, empty, unreadable or unsafe file; wrong directory ID | Restore `<home>/skills/<id>/SKILL.md` or `/skill clear` |
| `/loop` does not edit files, or warns its budgets are unlimited | It is chat continuation; both caps are zero or absent | Use [goal staging](CODING_PIPELINE.md#94-stage-a-session-goal-as-a-coding-task); set a finite cap, then [Reload limits](INSTALL.md#reload-limits-without-a-restart) |
| Unattended `/loop` ends with `loop aborted` | `loop.max_failures` turns failed in a row, or the model stayed busy for two hours | Fix the cause the run states. A busy model is waited for, not counted; `/loop stop` ends the wait |
| Goal task is stale/interrupted/unavailable | Goal changed, job already submitted, restart interrupted work or retention removed evidence | Inspect `/goal task`, `/agent jobs` and `/agent runs`; never infer approval from `GOAL_DONE` |
| App asks for Rosetta or shows Intel-only kind | Old app copy or forced translation | Verify the installed universal copy; see [hardware](INSTALL.md#21-hardware-and-macos) |
| `401 Unauthorized` on harness requests | Missing, expired or revoked account session | Sign in again; a fresh bootstrap account must replace its password. A harness key cannot grant access |
| App reports a home ownership conflict | Another app/server owns the home | Quit that owner normally; do not delete its lock |
| App waits at startup | File-access mediation, unavailable backend or damaged/moved bundle | Check macOS prompts; quit, move the complete app to Applications and retry. Do not delete the home |
| Changing config appears to do nothing | Most settings require restart; supported limits need an explicit reload | [Reload limits](INSTALL.md#reload-limits-without-a-restart), or Cmd-Q/relaunch or restart `serve`. Window close only hides it |
| Offline preparation fails in the app | Missing toolchain, Python/SDK, Cargo.lock, cached sources or an existing snapshot | See Setup and [offline Cargo verification](CODING_PIPELINE.md#offline-cargo-verification); the app downloads nothing |
| Chat takes too long or times out | Wrong model/endpoint, unavailable service, slow loading or inference timeout | Check [inventory](MODELS.md), `/model` and Setup diagnostics; cancel with `/loop stop` rather than resubmitting |
| Chat reports `502 HARNESS_LLM_ERROR` with `model server returned HTTP ...` | The local model server returned a non-success status | Check the endpoint, model and model-server logs; the harness drops the error body unread and never logs it |
| `Address already in use` when starting `serve` | Another process occupies the port | `lsof -i :8790`; stop only a known owner or choose another `--port`. Each server needs its own home |
| `/agent` reports `gh: command not found` or "Agentic layer disabled" | GitHub CLI missing, or `agentic.enabled` still `false` | `brew install gh && gh auth login`; follow [coding config](CODING_PIPELINE.md#92-edit-the-configuration-file), then restart |
| A write action is refused | A policy gate is closed, reason/confirm is absent, publication has no reviewed body, or the emergency switch is set | See [coding console steps](CODING_PIPELINE.md#93-run-it-from-the-console) and [the kill switch](CODING_PIPELINE.md#95-the-kill-switch); check `echo $CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` |
| Managed-key save or legacy migration fails | OS store unavailable, unsafe legacy file, or Windows replacement denied (a sharing lock; the destination stays intact) | Keep plaintext opt-in off unless required and do not broaden file access; see [credential privacy and recovery](INSTALL.md#8-persistence-optional-keys-and-recovery) |

## Read-only host inspection

The harness reads no host file outside its home, and chat (including
`/loop`) cannot call MCP tools. To read host files, declare a read-only stdio
filesystem MCP server ([example](MCP.md#read-only-host-files-on-macos)) and
call its tool yourself; each call needs your `confirm: true`, which model
prose cannot supply. Roots overlapping the harness home are refused
(`MCP_HOME_REFUSED`). Read-only is not secret-safe: never grant `~/`, `~/.ssh`,
`~/Library` or a folder holding `.env` or key material.

## Diagnostics

`/diag` reads allowlisted logs under `<home>/logs/` for an admin or
`portal_auditor` account once `diagnostics.logs_enabled` is literal `true`.
Output is redacted, or refused whole on an injection-scanner hit; `/diag
inject` stages it as untrusted text for one local turn ([contract](../INVARIANTS.md)).

| Symptom | Likely cause | Fix |
|---|---|---|
| `/diag` says `DIAG_DISABLED` | `diagnostics.logs_enabled` is missing, quoted or false (new homes seed `true`) | Set literal `true`, then restart |
| `/diag` says `DIAG_REFUSED` or `AUTH_PERMISSION_DENIED` | The account is `operator` or `audit` | Use an admin or `portal_auditor` account |
| A read or `/diag inject` says `DIAG_SCANNER_REFUSED` | The redacted excerpt matched the injection scanner | Narrow with `--event`, `--since` or `--last`; nothing from it is shown or staged |
| Chat says `DIAG_SURFACE_FORBIDDEN` | An excerpt is staged and the turn is cloud chat or `/agent` | Send a local turn first, or `/diag forget` |
| `/diag unslop` says `DIAG_NOT_FOUND` | Nothing written yet, or `unslop.metrics_path` is outside `logs/` | `/diag` reads only `<home>/logs/unslop.jsonl` |
