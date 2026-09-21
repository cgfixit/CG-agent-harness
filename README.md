# CG-Agent

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)
[![Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)

A local harness for **chat, permitted web research, and reviewed coding**,
written in Rust. Runs as a universal macOS app or as a standalone server you
open in a browser. Local chat stays on loopback. Cloud chat requires provider
setup and selection; web reads require the account and content permissions
described in [WEB.md](docs/WEB.md).

![CG Agent Harness running on macOS](docs/screenshots/CG-Agent-1.png)

## What it does

| Surface | What it is | Default state |
|---|---|---|
| **Local chat** | Chat against an OpenAI-compatible **loopback** model server (Ollama by default), with local history, memory, skills, attachments and web context. `/loop` continues chat toward a session goal. | On |
| **Cloud chat** | Explicitly selecting `grok` or `claude` routes through a separate cloud path after provider setup. Sends **only the new user message** — no local history, memory, skills, attachments or web context. Not available for `/loop`. | Off until a provider is configured |
| **Web research** | Google listings, URL fetch and page research under configurable budgets and URL rules. | Governed by [WEB.md](docs/WEB.md) |
| **Coding pipeline** | `/agent` drives a separate planner/executor loop. Ships closed behind **six gates**, including a per-run `--confirm-online`. Enabling the planner permits **repository-content egress**. | Off — read [CODING_PIPELINE.md](docs/CODING_PIPELINE.md) first |

Capability table: [CONSOLE.md](docs/CONSOLE.md#what-you-can-do).

> **Security posture, in one paragraph.** Fresh homes require HTTPS and
> account login; the harness API key is optional. First login is
> `admin` / `admin` — replace the password immediately. Repository mutations
> stay disabled until explicitly configured, and commit, push and draft-PR
> publication each require a **separate** operator decision. Details:
> [SECURE_RESEARCH.md](docs/SECURE_RESEARCH.md), [GIT_APPROVAL.md](docs/GIT_APPROVAL.md),
> [INVARIANTS.md](INVARIANTS.md).

## Quickstart

Pick one run path. A **home** is the harness state directory,
`~/.CGagentHarness` (override with `CGAGENTHARNESS_HOME`). Run one server
*or* one app per home, never both.

### Option A — macOS app (recommended)

1. Download `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` from
   [Releases](https://github.com/cgfixit/CG-agent-harness/releases/latest).
2. Verify, extract, open:

   ```bash
   shasum -a 256 -c SHA256SUMS
   unzip CG-Agent-Harness-macos-universal.zip
   open "CG Agent Harness.app"
   ```

The app owns a bundled backend on an ephemeral loopback port. Packaging,
ad-hoc signing and desktop limits: [DESKTOP.md](docs/DESKTOP.md).

**Build the app from source** (macOS, both Rust toolchains are required):

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
rustup target add --toolchain 1.88 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal
```

Full prerequisites and first-run login: [INSTALL.md](docs/INSTALL.md).

### Option B — standalone server

Requires Rust 1.88. On Linux, also Git and `bwrap` or `unshare` (in place of
Xcode). Windows CI and release legs are currently parked.

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release --locked
./target/release/cgagentharness serve
```

Open `https://127.0.0.1:8790` and trust the certificate explicitly in your
browser — see [SECURE_RESEARCH.md](docs/SECURE_RESEARCH.md).

`serve` accepts `--host` and `--port` (1024–65535). **Any non-loopback bind
host is refused.**

### Local model

The harness talks to `http://127.0.0.1:11434/v1` by default and sends no
`num_ctx`, so context length must be set on the Ollama side **before** the
Ollama process starts:

```bash
export OLLAMA_CONTEXT_LENGTH=32768   # seeded web budgets assume this value
ollama list                          # select an EXACT installed tag in the harness
```

Set-and-verify procedure: [MODELS.md](docs/MODELS.md). To fine-tune
`qwen3.8:27b-mlx` on Apple Silicon and serve the fused model to chat **and**
the coding planner, see [FINETUNE.md](docs/FINETUNE.md) and the
[`finetune/`](finetune) toolchain.

Long local conversations compact automatically while retaining the first user
turn, session goal and recent messages. Prompt estimates learn from reported
local usage, and prior summaries survive repeated compaction without being
clipped. [Compaction settings and limits](docs/CONSOLE.md#local-history-compaction)
include the summary budget and the extra reservation for reasoning backends.

### First run

1. Sign in with `admin` / `admin` and replace the bootstrap password.
2. Run `/help`.

The Commands pane and `/help` list every supported command alphabetically
with required arguments and modifiers. Clicking a menu entry fills the input
without executing it.

## Using the console

**Commands.** Use the listed slash syntax, not shell-style flags (`--help`,
`--dry-run` are not commands). Typos such as `/memroy` produce suggestions;
suggestions are never executed. `/memory` requires an exact, single-line
form — aliases and conversational phrasing only suggest, including retrieval
that would otherwise start a chat. Slash-command tables:
[CONSOLE.md](docs/CONSOLE.md#78-slash-command-quick-reference).

**Soul and styles.** Fresh homes load a default soul that asks for
human-readable plain text, with Markdown only on request. Existing saved
personas are preserved. For local chat, `/style concise`, `beginner`,
`technical-deep` or `unslop` layer a style on top; styles default to off and
never edit the soul. Inspect with `/soul status` and `/prompt`; compare styles
in fresh sessions. These are model instructions, not guaranteed formatting.
Details: [Chat workflows](docs/CHAT_WORKFLOWS.md#inspect-and-edit-the-chat-prompt).

**Memory, web, spend.** Memory enable steps: [MEMORY_SETUP.md](docs/MEMORY_SETUP.md).
Web research rules: [WEB.md](docs/WEB.md). The Spend view reports retained
usage and available costs; optional completion webhooks send only job
metadata and stay disabled until configured — including which feature PRs the
installed build must contain:
[SPEND_AND_NOTIFICATIONS.md](docs/SPEND_AND_NOTIFICATIONS.md). MCP stderr
capture bounds: [PROCESS_LIFECYCLE.md](docs/PROCESS_LIFECYCLE.md#mcp-stdio-diagnostics).

**Tune running limits.** Administrators can edit supported web/API limits and
choose **Reload limits**; Unix backends also accept SIGHUP. Invalid or mixed
restart-only changes preserve the running snapshot. See [configuration reload](docs/CONFIG_RELOAD.md).

**Something broken?** Symptom table: [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## Which version am I running?

The Cargo package version (`0.1.0`) does **not** establish feature
availability. Use the source commit of the
[latest release](https://github.com/cgfixit/CG-agent-harness/releases/latest)
instead. Identify an installed build by:

- `Contents/Resources/COMMIT` inside the app bundle,
- the workflow SHA of the build,
- `/help` output.

How tip `main` relates to releases, and how to upgrade:
[setup-guide.md](setup-guide.md).

## Documentation

Start with [setup-guide.md](setup-guide.md) — the index and run-path chooser.

### Install and run

| Document | Purpose |
|---|---|
| [docs/INSTALL.md](docs/INSTALL.md) | Install, first run, persistence, tests/CI |
| [docs/MODELS.md](docs/MODELS.md) | Ollama inventory and `OLLAMA_CONTEXT_LENGTH=32768` |
| [docs/DESKTOP.md](docs/DESKTOP.md) | App ownership, setup/recovery, packaging, distribution limits |
| [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) | Toolchains, lockfiles, retained pins |
| [docs/OFFLINE_CARGO.md](docs/OFFLINE_CARGO.md) | Locked dependency set for sandboxed Cargo checks |
| [docs/RELEASING.md](docs/RELEASING.md) | Release cadence, tagging, manual preview/publish |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Symptom table |
| [assets/config.default.yaml](assets/config.default.yaml) | Shipped settings and configurable budgets |

### Chat, memory and web

| Document | Purpose |
|---|---|
| [docs/CONSOLE.md](docs/CONSOLE.md) | Chat, soul, skills, slash-command tables |
| [docs/USER_MANUAL.md](docs/USER_MANUAL.md) | Operator navigation, spend/notifications and memory how-to |
| [docs/CHAT_WORKFLOWS.md](docs/CHAT_WORKFLOWS.md) | Chat-first defaults, persona/skill commands, goal staging |
| [docs/CHAT_STREAMING.md](docs/CHAT_STREAMING.md) | SSE chat streaming and `/loop stop` |
| [docs/MEMORY_SETUP.md](docs/MEMORY_SETUP.md) | Enable steps for pinned notes and structured memory |
| [docs/STRUCTURED_MEMORY.md](docs/STRUCTURED_MEMORY.md) | Facts, proposals, episodes, explicit recall, facts-only FTS |
| [docs/WEB.md](docs/WEB.md) | Google listings, URL fetch, page research |
| [docs/SPEND_AND_NOTIFICATIONS.md](docs/SPEND_AND_NOTIFICATIONS.md) | Spend completeness, webhook setup, retries and privacy |

### Coding pipeline and process control

| Document | Purpose |
|---|---|
| [docs/CODING_PIPELINE.md](docs/CODING_PIPELINE.md) | Optional disarmed coding loop and cloud-planner egress |
| [docs/BOUNDED_EDITS.md](docs/BOUNDED_EDITS.md) | Exact-content edit format, scope and budget |
| [docs/GIT_APPROVAL.md](docs/GIT_APPROVAL.md) | Approval binding, commit/push/publish separation |
| [docs/CONSOLE_JOBS.md](docs/CONSOLE_JOBS.md) | Asynchronous console runs and browser acceptance |
| [docs/PROCESS_LIFECYCLE.md](docs/PROCESS_LIFECYCLE.md) | Child-process timeouts, cancellation, descendant cleanup |
| [docs/API_ROUTES.md](docs/API_ROUTES.md) | Registered HTTP route inventory |

### Security and accounts

| Document | Purpose |
|---|---|
| [docs/SECURE_RESEARCH.md](docs/SECURE_RESEARCH.md) | HTTPS, SQLite migration, roles, URL rules, API keys |
| [docs/ACCOUNTS.md](docs/ACCOUNTS.md) | Roles, login, Terminal account commands |
| [INVARIANTS.md](INVARIANTS.md) | Process isolation, guard chain, write policy, clone jail, sandbox |
| [SECURITY.md](SECURITY.md) | Supported surface and how to report a vulnerability |

### Project history and parity

| Document | Purpose |
|---|---|
| [docs/PORT_PARITY.md](docs/PORT_PARITY.md) and [docs/parity/](docs/parity) | CyClaw ↔ harness port ledger |
| [docs/DESKTOP_ACCEPTANCE.md](docs/DESKTOP_ACCEPTANCE.md) / [docs/MAC_ACCEPTANCE.md](docs/MAC_ACCEPTANCE.md) | Historical native acceptance records — **not** current operator procedure |

## Contributing

Repository guidance for coding agents lives in [`.codex/skills`](.codex/skills)
and [`.claude/skills`](.claude/skills); start with
`cgagentharness-project-guidance` and `fable-protocol`. Contributor rules and
required verification: [AGENTS.md](AGENTS.md).

PRs use a driver-prefixed branch, target `main`, open as drafts, and are
checked with `scripts/check-pr-template.sh`.

## Origins

The harness began as a Rust port of the console and agentic pipeline from
[CyClaw](https://github.com/cgfixit/CyClaw). Its focus here is local coding,
chat context, controlled tools and explicit operator review.
