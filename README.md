# CGagentHarness

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)

![CG Agent Harness running on macOS](assets/app-ss.png)

Loopback-only agentic coding harness. One Rust binary: crate and CLI `cgagentharness`.

This is **not** CyClaw. CyClaw is the offline-first RAG soul agent
([cgfixit/CyClaw](https://github.com/cgfixit/CyClaw)). This repo is the
`harness/` console plus the `agentic/` real-repo pipeline, ported from that
Python stack: same security posture, no RAG, no corpus, no terminal, no
fsconnect / sqlconnect / netconnect.

Status: `0.1.0`. MSRV Rust 1.88. [MIT](LICENSE). Bind is loopback-only. Every
write gate ships **closed**.

- macOS app: [desktop setup, packaging and limitations](docs/DESKTOP.md) (Apple Silicon; native interaction acceptance pending)
- Console: `http://127.0.0.1:8790/` (`assets/static/harness.html`, served verbatim)
- Chat: local OpenAI-compatible model (Ollama on `127.0.0.1:11434` by default)
- Pipeline: clone → plan → patch → hard-sandbox verify → human decide → commit → push → draft PR
- Isolation: the HTTP process never calls the pipeline in-process. It only
  spawns `cgagentharness agentic …` as a child (`src/shim`). Exit codes
  `0 / 2 / 3 / 4` are the whole interface.

## Why it exists

CyClaw's Python `harness/` + `agentic/` layer is the part worth extracting:
the console, the child-process I6 boundary, the clone jail, and the write
gates. Everything else (RAG, soul, Telegram, fsconnect) stays in CyClaw.

If you want an offline knowledge agent, use CyClaw. If you want a local
coding harness that cannot reach the pipeline except through `src/shim`,
use this.

Shipped defaults do nothing to a repository. You chat with a local model
in the browser; only after you arm the gates can the same binary clone a
repo, propose a patch, verify it in a hard sandbox, and open a draft PR —
and only after a human reviews a digest-bound diff.

## Prerequisites

| Need | Why |
|---|---|
| Rust 1.88+ (`rust-toolchain.toml`) | Build |
| Python 3 (standard library only) | Explicit Cargo dependency preparation |
| Ollama on `127.0.0.1:11434` with a chat model | Console chat and local planner |
| `gh` ≥ 2.40.0, logged in | Real-repo pipeline only |

The shipped config references `qwen3.8:27b-mlx`, but do not assume that tag is installed. Run `ollama list` and select the exact installed tag you intend to use, then set both `models.local_llm.model` and `agentic.deepagent_github.model` to that tag. An `-mlx` suffix alone does not prove the execution backend.

Mutable state lives under `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`
overrides), seeded from `assets/config.default.yaml`.

## Quick start (chat only)

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release

./target/release/cgagentharness serve
# http://127.0.0.1:8790/
```

Ollama must be running. No API key or account login is needed for local use.
Send a line, then try `/status`, `/skills`,
`/tools`. None of those touch a GitHub repository.

Step-by-step macOS walkthrough: [setup-guide.md](setup-guide.md).

## Optional credentials

For an existing home, set `security.api_key_optional: true` in its `config.yaml`
and restart the server/app. Saved homes are not overwritten on upgrade.
This permits all harness operations without a key or account login on a direct
loopback connection. Repository write gates, reason/confirmation, diff review,
and separate approval/push/publication actions still apply.

To enforce a key, set `security.api_key_optional: false`, configure
`CGAGENTHARNESS_API_KEY` in the server environment (or the desktop home's private
`.env`), restart, and enter the matching key in the console. The key field remains
available and keeps its value only in page memory. Forwarded requests never use
the local bypass. Set the flag false behind any proxy, including one stripping
forwarding headers.

`auth.enabled` retains optional account login, sessions and role-based account
management. It does not require login for chat or the agent pipeline; managing
accounts still requires the appropriate logged-in role. Credentials required by
external services, such as GitHub publication, remain those services' requirements.

## Optional: arm the pipeline

The shipped config keeps every write gate closed. After `gh auth login`,
edit `~/.CGagentHarness/config.yaml` and set all four:

```yaml
agentic:
  enabled: true
  repo: "owner/name"
  deepagent_github:
    enabled: true
    allow_git_write_tools: true
```

Restart `serve`. In the console a typical loop is `/agent run …` (stage),
`/agent confirm <why>` (clone → plan → sandbox verify; no commit yet),
`/agent status <run-id>`, then `/agent approve <run-id> <why>` →
`/agent push <run-id> <why>` → `/agent publish <run-id> <why>`. Each mutation
requires fresh explicit intent. CLI approval, push and publication each require
`--reason=<why> --confirm`; API calls require `reason` and `confirm: true`.
Approval commits locally only; combined `decide --push/--publish` is refused.

Before a Cargo check, prepare the selected repository's unchanged `Cargo.lock`
outside verification using [the offline Cargo preparation flow](docs/OFFLINE_CARGO.md).
Checks get read-only sources/toolchain and a fresh writable build directory;
missing preparation refuses before asking the planner. [Bounded exact edits](docs/BOUNDED_EDITS.md)
allow small changes in larger files using declared line windows and original hashes.
Tests must write temporary
state under their supplied temporary directory, not into the candidate repository.

`CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` disables repository and PR mutations
when inherited by the process. Exporting it in another shell does not stop an
existing process. Revoking YAML write policy blocks later mutation boundaries;
it does not cancel work already executing.
Depth of the gates, clone jail, and digest-bound approval:
[INVARIANTS.md](INVARIANTS.md). Operator rules: [AGENTS.md](AGENTS.md).
Full arming walkthrough: [setup-guide.md](setup-guide.md) §9.

## Security defaults

| Default | Behavior |
|---|---|
| Unset `CGAGENTHARNESS_API_KEY` | Direct local use works; origin and CSRF checks still apply |
| Non-loopback bind (`--host 0.0.0.0`, …) | Refused at startup |
| Host header not a loopback name | Refused (DNS-rebinding defense) |
| `agentic.enabled`, `deepagent_github.enabled`, `allow_git_write_tools` | All ship **false** |
| `security.api_key_optional` | Ships **true**; set false to require a configured Bearer key |
| `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` (`1` / `true` / `yes` / `on`) | Disable-only write kill switch (cannot arm writes) |
| `GROK_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPAGENT_API_KEY` in CI | Blanked; tests must not assert a developer key is present |

Quoted YAML `"true"` is **off** for every gate (`flag_is_true`). A reason
is never optional on a write.

## Verify

```bash
scripts/verify-local.sh              # fmt, clippy -D warnings, deny, tests, release build, live smoke
SKIP_LIVE=1 scripts/verify-local.sh  # static + tests + build only
```

CI blanks planner keys the same way. The live smoke
(`scripts/smoke-ollama.sh`) needs Ollama; skip it with `SKIP_LIVE=1`.
Quality bar and traps: [AGENTS.md](AGENTS.md).

## Docs

| Doc | What it is |
|---|---|
| [INVARIANTS.md](INVARIANTS.md) | What the code enforces and where (I6, gates, clone jail) |
| [AGENTS.md](AGENTS.md) | Operator / agent rules; do not "deduplicate" across the shim |
| [setup-guide.md](setup-guide.md) | Fresh-machine walkthrough (macOS) |
| [assets/config.default.yaml](assets/config.default.yaml) | Every tunable; no hardcoded tunables elsewhere |
| [LICENSE](LICENSE) | MIT |

For asynchronous console runs and native browser acceptance, see [Console jobs](docs/CONSOLE_JOBS.md).
