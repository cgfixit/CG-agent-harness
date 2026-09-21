# Reload non-secret limits

Edit the active home's `config.yaml`, then use **Reload limits** in the console
header as an administrator. The button appears after replacing the bootstrap
password. It calls authenticated, CSRF-guarded `POST /api/config/reload` with
an empty JSON object (`{}`). Operators, audit accounts and auth-disabled legacy
homes cannot call this route. On Unix, send SIGHUP to the owned **backend** PID
(the native sidecar, not its desktop parent) to invoke the same implementation.

A successful response includes `changed`, `limits.revision`, the applied
snapshot and `reloadable`. A no-op keeps the revision. New operations use the
new snapshot; a web fetch/research/chat-tool operation already running keeps
its original limits. Reload retains rate-limit hits and shared fetch capacity,
locks and cancellation. Tightening a ceiling can immediately produce HTTP 429;
SIGHUP can recover an HTTP ceiling that currently blocks the reload route.
`Retry-After` waits for enough retained hits to expire under the new ceiling.
Increasing a rate window cannot restore hits already expired under its former
window. This is tuning, not a fresh quota allocation.

## Supported settings

Only these 22 leaf keys can change. All values are integers except the two
`window_seconds` values, which accept finite numbers. Bounds also apply at
startup. Seeded defaults are in `assets/config.default.yaml`; removing a key
selects its code fallback, which can differ from a fresh-home default.

| Key | Accepted range |
|---|---|
| `api.rate_limit.max_requests` | 1–100,000 |
| `api.rate_limit.window_seconds` | 0.001–86,400 seconds |
| `api.harness_loop_rate_limit.max_requests` | 1–100,000 |
| `api.harness_loop_rate_limit.window_seconds` | 0.001–86,400 seconds |
| `api.harness_loop_rate_limit.max_tokens` | 1–25,904 for Ollama reasoning `none`; otherwise 1–12,952; legacy 0 selects 2,048 |
| `web.response_bytes` | 1,024–1,048,576 |
| `web.request_seconds` | 1–30 |
| `web.pages` | 1–40 |
| `web.run_bytes` | 1,024–8,388,608 |
| `web.run_seconds` | 1–180 |
| `web.per_site_pages` | 1–20 |
| `web.pace_ms` | 100–5,000 |
| `web.cache_pages` | 1–256 |
| `web.cache_bytes` | 1,048,576–33,554,432 |
| `web.subqueries` | 1–5 |
| `web.rounds` | 1–2 |
| `web.evidence_tokens` | 256–6,000 |
| `web.model_tokens` | 256–2,048 |
| `web.total_tokens` | 2,048–32,000 |
| `web.research_seconds` | 10–1,800 |
| `web.stale_seconds` | 60–31,536,000 |
| `web.chat_tool_calls` | 1–5 |

`web.concurrency` is **restart-only** because it sizes shared fetch permits.
All other fields, including TLS, authentication, credentials, models, sandbox,
coding policy, notifications and unknown keys, are outside this reload contract.
This route never writes YAML or changes permissions. Use [normal restart and
settings guidance](INSTALL.md#which-settings-take-effect-where) for those fields.

## Refusal and recovery

An unreadable file, symlink on Unix, non-regular file, invalid UTF-8/YAML, more than
1 MiB of config, or an invalid limit returns `CONFIG_RELOAD_INVALID`. A mixed
candidate changing anything outside the allowlist returns
`CONFIG_RESTART_REQUIRED`. Either preserves **all** running limits; no partial
update occurs. Restore unsupported or invalid edits and retry, or deliberately
restart with a valid complete configuration. Error messages and audit never
include raw YAML or secret values.

Audit `config_reloaded` reports source (`http`/`sighup`), revision and whether
anything changed; `config_reload_refused` reports the source and coarse code.
A refusal leaves the on-disk file as edited. Existing components that independently
read disk, including coding write-policy revalidation, still see that file and
can refuse their operations. Reload does not freeze or roll back those checks.
A smaller web cache bound governs subsequent cache operations; it does not
promise immediate background eviction of already stored pages.
