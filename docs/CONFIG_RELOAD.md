# Reload non-secret limits

Edit the active home's `config.yaml`, then choose **Reload limits** as an
administrator who replaced the bootstrap password. The button sends `{}` to
authenticated, CSRF-guarded `POST /api/config/reload`; operators, auditors and
auth-disabled homes cannot call it. Unix SIGHUP to the owned **backend** PID
(the native sidecar, not the desktop parent) runs the same implementation.

Success returns `changed`, `limits.revision`, the applied snapshot and
`reloadable`; a no-op preserves revision. New operations use the new snapshot;
running web fetch/research/chat-tool operations retain theirs. Reload preserves
rate-limit hits, shared fetch capacity, locks and cancellation. Tightening a ceiling can immediately produce HTTP 429;
SIGHUP can recover an HTTP ceiling that currently blocks the reload route.
`Retry-After` waits for enough retained hits to expire under the new ceiling;
increasing a rate window cannot restore hits already expired under its former
one.

## Supported settings

Only these 28 keys reload; values are integers except finite `window_seconds`.
Bounds also apply at startup. Defaults live in `assets/config.default.yaml`;
removing a key selects its code fallback.

| Key | Accepted range |
|---|---|
| `api.rate_limit.max_requests` | 1–100,000 |
| `api.rate_limit.window_seconds` | 0.001–86,400 seconds |
| `api.harness_loop_rate_limit.max_requests` | 1–100,000 |
| `api.harness_loop_rate_limit.window_seconds` | 0.001–86,400 seconds |
| `api.harness_loop_rate_limit.max_tokens` | 1–25,904 for Ollama reasoning `none`; otherwise 1–12,952; legacy 0 selects 2,048; ships 8,192 |
| `loop.max_turns` | 0 (unlimited) or 1–1,000,000 |
| `loop.max_total_completion_tokens` | 0 (unlimited) or 1–100,000,000 |
| `loop.max_failures` | 1–100 |
| `loop.cooldown_ms` | 0–300,000 |
| `web.response_bytes` | 1,024–1,048,576 |
| `web.request_seconds` | 1–30 |
| `web.pages` | 1–40 |
| `web.run_bytes` | 1,024–8,388,608 |
| `web.run_seconds` | 1–180 |
| `web.per_site_pages` | 1–20 |
| `web.pace_ms` | 100–5,000; public Google search keeps a 3,000 floor (code) |
| `web.cache_pages` | 1–256 |
| `web.cache_bytes` | 1,048,576–33,554,432 |
| `web.subqueries` | 1–5 |
| `web.rounds` | 1–2 |
| `web.evidence_tokens` | 256–6,000 |
| `web.model_tokens` | 256–2,048 |
| `web.total_tokens` | 2,048–128,000 (32,000 unless measured larger) |
| `web.research_seconds` | 10–1,800 |
| `web.synthesis_seconds` | 10–1,800 |
| `web.stale_seconds` | 60–31,536,000 |
| `web.chat_tool_calls` | 1–30 |
| `web.carry_forward_sources` | 0–50 |

The `loop.*` keys bound the server-side `/loop` runner (turns and aggregate
tokens per run; consecutive failures before abort; pause between
turns), which the console reads from `/api/status`.

`web.concurrency` is **restart-only** (it sizes shared fetch permits), as are `web.goal_web_default`, `web.search*`.
`web.max_rules` (default 64, 1–256) is restart-only too: a cap below the saved
rule count refuses the policy until it is raised.
TLS, authentication, credentials, `security.allow_plaintext_key_file` (default
false), models (including cloud profiles), sandbox, coding policy, notifications
and unknown keys require restart. Cloud profiles are config-owned:
`context_window` (positive, above output), `max_output_tokens` (1–32768),
`temperature` (optional 0–1), `timeout_sec` (1–720), `count_timeout_sec` (0.1–10),
`tool_support` (informational boolean), and `rpm` (1–100000 chat/prediction calls/minute).
Absent profiles keep shared settings; context/RPM stay unset and temperature is
omitted. Invalid profiles refuse startup without echoing values. [Anthropic](https://platform.claude.com/docs/en/api/messages/create)
deprecates temperature for newer models; [xAI](https://docs.x.ai/developers/model-capabilities/text/best-practices)
supports it. Set it only for a compatible configured model.
This route never writes YAML or changes permissions. Use [normal restart and
settings guidance](INSTALL.md#which-settings-take-effect-where).

## Refusal and recovery

An unreadable or [non-private](INSTALL.md#home-and-optional-credentials) file,
symlink on Unix, non-regular file, invalid UTF-8/YAML, more than 1 MiB of
config, or an invalid limit returns `CONFIG_RELOAD_INVALID`. A mixed
candidate changing anything outside the allowlist returns
`CONFIG_RESTART_REQUIRED`. Both preserve **all** running limits. Restore the edits and retry, or restart
with a valid configuration. Error messages and audit never
include raw YAML or secret values.

Audit `config_reloaded` reports source (`http`/`sighup`), revision and whether
anything changed; `config_reload_refused` reports the source and coarse code.
Refusal leaves the edited file on disk; independent disk readers, including
coding write-policy revalidation, still see it.
Smaller cache bounds govern later operations without promising immediate
eviction.
