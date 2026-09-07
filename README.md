# CGagentHarness

A loopback-only agentic coding harness in one Rust binary: the browser console
CyClaw ships as `harness/` plus its real-repo coding pipeline (`agentic/`),
ported from Python with the security posture intact and no RAG, terminal, or
corpus machinery.

[![CI](https://github.com/cgfixit/CGagentHarness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CGagentHarness/actions/workflows/ci.yml)

## What it does

- Serves the CyClaw console (`assets/static/harness.html`, verbatim) on
  `http://127.0.0.1:8790/` with chat against a local OpenAI-compatible model
  (Ollama by default), sessions, `/goal` + `/loop`, `/skills`, `/tools`,
  `/web` (allowlist-only fetch), `/memory` notes, `/api` key panel, and
  optional per-user auth.
- Runs the governed real-repo pipeline: clone -> plan -> patch -> verify in a
  hard sandbox -> human decides -> commit -> push -> draft PR, every step behind
  its own gate, every write judged before it lands.
- Keeps CyClaw's Invariant 6 as a process boundary: the console reaches the
  pipeline only by spawning its own executable as a child (`cgagentharness
  agentic ...`) with a 0/2/3/4 exit-code contract.

## Quick start

```bash
cargo build --release
export CGAGENTHARNESS_API_KEY="$(openssl rand -hex 20)"   # guarded routes refuse without it
./target/release/cgagentharness serve                      # http://127.0.0.1:8790/
```

Ollama must be running on `127.0.0.1:11434`. Paste the key into the console's
key field, send a line, then try `/skills`, `/tools`, `/agent checks`, `/github`.

Everything mutable lives under `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`
overrides): `config.yaml` (every tunable; seeded from `assets/config.default.yaml`),
`harness.json`, `.env` (managed keys, mode 600), `sessions/`, `skills/`,
`tools/`, `memory/`, `data/agentic/` (registry, workspaces, run records),
`logs/audit.jsonl`.

## Arming the coding pipeline

The shipped config keeps every gate closed. To run a real coding loop against
the configured repository edit `~/.CGagentHarness/config.yaml`:

```yaml
agentic:
  enabled: true
  repo: "owner/name"
  deepagent_github:
    enabled: true
    model: "qwen3.8:27b-mlx"          # the local planner
    allow_git_write_tools: true       # gates write_file/add/commit/push in the clone
```

`gh` must be installed and logged in. Then from the console: `/agent plan ...`,
`/agent run ...`, `/agent confirm`, review the diff with `/agent status`,
`/agent approve`, `/agent push`, `/agent publish`. Opening a PR additionally
needs `agentic.mode: write`, `writes_enabled: true` (both ship open), a reason,
and a fresh confirm. `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` is the rollback.

A run can take minutes; the console's `/agent run` blocks on it the way CyClaw
does. To avoid tying up a client, `POST /api/agent/jobs` accepts the identical
body and returns a job id immediately (`202`); poll `GET /api/agent/jobs/{id}`
for its outcome, or `POST /api/agent/jobs/{id}/cancel` to abort it. The run and
chat gates are held by the job itself, not by the request, so this is safe
against a closed tab or a proxy timeout.

Cloud planners (`--provider grok|claude --confirm-online` on the CLI) sit
behind a six-condition chain; keys come from `GROK_API_KEY` /
`ANTHROPIC_API_KEY` only, and every outbound prompt is injection-scanned,
redacted, hashed and audited as egress before it leaves.

## Environment

| Variable | Purpose |
|---|---|
| `CGAGENTHARNESS_API_KEY` | Bearer for every guarded route (fail-closed when unset) |
| `CGAGENTHARNESS_HOME` | Home directory override |
| `CGAGENTHARNESS_HARNESS_HOST` / `_PORT` | Bind (loopback names only; 1024-65535) |
| `CGAGENTHARNESS_AGENT_COMMIT_NAME` / `_EMAIL` / `_BRANCH_PREFIX` | Committer identity and preferred branch namespace |
| `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` | Disable-only kill switch for `gh pr create` |
| `GROK_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPAGENT_API_KEY` | Optional planner credentials |

## Logging

`logging.request_log` (ships `true`) adds one `tracing` line per request under
target `cgagentharness::http`: method, path (query stripped), status, latency,
client. Select it with `RUST_LOG=cgagentharness::http=info` (or plain `info`
for everything). This is separate from the redacted audit file, which records
security-relevant events, not every request.

## Verify locally

```bash
scripts/verify-local.sh          # fmt, clippy -D warnings, deny, tests, release build, live smoke
SKIP_LIVE=1 scripts/verify-local.sh
```

The live smoke (`scripts/smoke-ollama.sh`) binds a throwaway home on port 8791,
checks the bind guard, hardening headers, negative auth controls, the child-process
contract, and one real chat turn.

## Layout

```
src/common/   errors, config, home, atomic writes, audit+redaction, rate limit, identity,
              repo paths, tool broker, injection scanner, API key, scrypt auth, auth store, process
src/llm/      backend resolution (primary/fallback), OpenAI-compatible chat client with abort
src/server/   axum console: guard chain, headers, schemas/422 envelope, sessions, prompts,
              web tool, memory notes, env keys, views, routes
src/shim/     the ONLY server->agentic edge (argv whitelist + child process)
src/agentic/  config, gh client, context, registry, run store, clone jail, executor
              (sandbox/runner/manifest/apply), proposers, governance, loop, writer, CLI
tests/        integration tests against a real ephemeral-port server, a mock model,
              a fake `gh`, and a bare git origin; invariant_guard scans the sources
```

## Not ported (by decision)

The RAG gateway and browser terminal, `fsconnect`/`sqlconnect`/`netconnect`,
the retired DeepAgents graph (`deepagent-plan`), `real-repo-run-plan`,
device-token CLI, Postgres backends, the Numbat event stream, and the vendored
`unslop` scanner (replaced by a small phrase list behind `unslop.enabled`).
Users live in `auth.json` rather than SQLite; scrypt records are byte-compatible
with CyClaw's.

See `INVARIANTS.md` for what is enforced and where, and `CLAUDE.md` for the
operating rules.
