# Grok Build ACP Phase 0

Checked 2026-09-17 against `main` at `c1154a8434f8b973dd8511db07d473df73565095`.
This is a disposable feasibility result, not a product connection or live-account
acceptance test.

## Decision

The documented and fixture-tested contract is narrow enough to continue design:
one isolated runtime profile, answer-only ACP, no client filesystem or terminal
capabilities, no MCP servers, no ambient API-key fallback, and owned process-group
cleanup. No production runtime path, login, credential storage, model call, or UI
is added by Phase 0.

The contract is **not** evidence that a subscription is entitled, that included
allowance cannot overrun, or that two real authenticated profiles are isolated by
the vendor on macOS. Those require an explicitly chosen test account, which this
phase did not use.

## Official contract refreshed

- [Settings](https://docs.x.ai/build/settings) and the
  [settings reference](https://docs.x.ai/build/settings/reference) document
  `GROK_HOME` as the home for auth/config/sessions, per-project discovery,
  compatibility scanners, and feature switches.
- [Enterprise deployments](https://docs.x.ai/build/enterprise) documents
  `model.api_key > model.env_key > active session token > XAI_API_KEY` and the
  `disable_api_key_auth` requirement pin. Subscription mode therefore strips
  API-key variables and pins API-key auth off; API-key mode requires an explicit
  key-bearing launch. There is no fallback between them.
- [Headless and scripting](https://docs.x.ai/build/cli/headless-scripting)
  documents newline-delimited ACP over `grok agent stdio`, authentication,
  `session/new` with `mcpServers`, assistant `session/update` chunks, and the
  `session/prompt` completion response.
- The [CLI reference](https://docs.x.ai/build/cli/reference),
  [hooks](https://docs.x.ai/build/features/hooks), and
  [MCP](https://docs.x.ai/build/features/mcp-servers) pages document tool flags
  and every discovery surface the launch contract must exclude.
- ACP v1 defines `session/cancel` as a notification. Runtime termination remains
  the fallback when a child ignores it.

## Measured local evidence

The installed artifact was inspected without its normal profile or account:

| Property | Result |
| --- | --- |
| Executable | `/Users/cg/.grok/downloads/grok-1.0.34-macos-aarch64` |
| Version | `grok 1.0.34 (3736acbc8658)` |
| SHA-256 | `9cd26b579840f0f5c9148a8059ad651904c08b41b7f2ef0b4ec04b9ba898844e` |
| macOS signature | Hardened runtime; Team ID `5Y6N3AJ54S` |
| Isolated discovery | no config layers, hooks, skills, plugins, MCP or LSP servers, or project instructions |
| Safe argv parse | accepted empty tools, deny-all/dontAsk, strict sandbox, no web/subagents/plan/update, and a profile-local leader socket |
| Isolated ACP initialize | protocol v1; auth method `grok.com`; no active MCP servers; no login or prompt |
| Subscription policy inspect | profile-local requirements loaded; `disableApiKeyAuth` and `apiKeyAuthDisabled` both true |

`grok inspect --json` still lists built-in agent definitions; `--no-subagents` and
`GROK_SUBAGENTS=0` are therefore retained at execution rather than treating an
empty discovery list as proof. The strict sandbox does not block in-process model
network and its child-network restriction is a no-op on macOS; no-tool operation
is the primary control here.

The current xAI headless example selects `cached_token` or `xai.api_key`, but
version 1.0.34 advertised only `grok.com` in both the no-key and synthetic-key
initialize handshakes. The fixture follows the measured version: subscription
mode authenticates with `grok.com`; API-key mode is selected solely by its exact
environment and does not invent an unadvertised ACP method. This discrepancy is
fail-closed and must be rechecked on every supported runtime version.

## Fixture contract

Run:

```sh
python3 scripts/grok-acp-probe.py
python3 scripts/test-grok-acp-probe.py
```

The stdlib-only probe creates separate temporary `HOME`, `GROK_HOME`, `TMPDIR`,
working directory, requirements file, and leader socket for each funding mode.
It passes an exact environment, disables Cursor/Claude discovery, uses an empty
ACP client capability set and `mcpServers: []`, refuses every fake filesystem,
terminal, and permission request, and reads assistant text only from
`session/update` before accepting the prompt completion boundary.

The cancellation fixture spawns a descendant, observes `session/cancel`, then
deliberately ignores it. The probe sends `SIGTERM` to the owned process group,
then `SIGKILL` after a short grace period even if the leader has already
exited, and fails if the descendant remains as a live (non-zombie) process.
This proves ordinary same-group cleanup, not containment of a hostile daemon
that reparents or escapes its group.

## Remaining Phase 0 live questions

- No account sign-in, subscription inference, API request, auth expiry, logout,
  entitlement, limit, overage, or model-list test was performed.
- Two fake profiles cannot see one another. Two real authenticated profiles and
  any macOS keychain behavior remain unverified.
- Native hook/MCP/plugin absence was measured for the isolated local profile and
  enforced by the fake launch contract; a future supported-version check must
  rerun `inspect --json` before each live acceptance run.
- Other operating systems and packaged-app behavior are unverified.

Do not expose a connection UI or claim subscription billing until those live
questions are answered with a deliberately selected account.
