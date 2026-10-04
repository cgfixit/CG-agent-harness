# CG-Agent

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)
[![Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)

**A Rust workspace for local AI chat, permissioned research, and reviewed coding.**

CG-Agent brings conversations, files, memory, and selected tools into one local workspace, and turns coding requests into checked, reviewable repository changes with separate approval to commit, push, or open a draft pull request.

Run it as a native macOS app or a local browser console. It is a Rust port of [CyClaw](https://github.com/cgfixit/CyClaw)'s console and coding pipeline, with independent configuration, security contracts, and [parity status](docs/parity/STATUS.md).

![CG Agent Harness](docs/screenshots/image.png)

[Quick start](#quick-start) · [Engineering choices](#engineering-choices) · [Security and defaults](#security-and-defaults) · [Documentation](#documentation)

## What you can do

| Capability | What it provides | Starting state |
|---|---|---|
| **Local chat** | Streaming sessions, attachments, skills, prompt inspection, and bounded, tool-free goal continuation with `/loop`. | Loopback model service; Ollama by default. |
| **Memory** | Shared pinned notes plus account-private structured facts, recall, and proposed summaries or insights. | Capture, retrieval, and suggestions on; applying proposed facts requires review. |
| **Web research** | Google listings, permitted page fetches, and research with request, time, byte, and token budgets. | Web tools on with an empty content allowlist; search results never authorize page reads. |
| **Reviewed coding** | A planner/executor loop, isolated repository clones, checks, complete diff review, and separate Git decisions. | Disabled until configured for a selected repository. |
| **Jobs and schedules** | Detached coding jobs and reviewed interval or cron schedules. | Same coding permissions and owner-bound review. |
| **Cloud chat** | Optional Grok or Claude. Sends only the new message, never local history, memory, skills, attachments, or web context. | Requires provider setup; unavailable for `/loop`. |
| **MCP integrations** | Declared external tools and a separate read-only memory gateway with dedicated machine credentials. | Both disabled. |
| **LAN observation** | Netconnect reads in-scope interfaces, routes, and neighbors from local tables. | Disabled with empty scope; no active scans, probes, or device control. |

Starting states describe fresh homes; upgrades keep existing choices. Only `/web research` produces checked quote references. See the [console guide](docs/CONSOLE.md#what-you-can-do).

## How reviewed coding works

1. **Scope.** Select a repository, configure the coding gates, and prepare its toolchain.
2. **Stage and confirm.** Specify the instruction, branch, allowed reads, and check profile.
3. **Generate and verify.** The pipeline applies bounded edits in its own clone and runs checks in a sandbox.
4. **Review.** Inspect the full diff and check results; explicit approval commits.
5. **Publish deliberately.** Push and draft pull-request creation each require a separate reason and confirmation.

Passing checks never authorize publication; scheduled jobs use the same controls. Start with [Coding pipeline](docs/CODING_PIPELINE.md) and its [Git approval contract](docs/CODING_PIPELINE.md#git-approval-and-publication).

## Engineering choices

| Decision | Purpose | Explore |
|---|---|---|
| Separate console and coding processes | Agentic execution sits behind an allowlisted subprocess interface. | [Shim](src/shim/mod.rs), [invariant tests](tests/invariant_guard.rs) |
| Bind approval to reviewed changes | Accepted content, file modes, destination, and commit are rechecked at every Git step. | [Git approval](docs/CODING_PIPELINE.md#git-approval-and-publication), [tests](tests/git_approval.rs) |
| Separate memory proposals from facts | Models suggest; changing canonical facts requires approval. | [Memory guide](docs/MEMORY_GUIDE.md), [tests](tests/structured_memory.rs) |
| Bound external work | Limit research, model calls, subprocess execution, and tool results. | [Configuration](assets/config.default.yaml), [process lifecycle](docs/PROCESS_LIFECYCLE.md) |
| Treat installation as part of the system | The app owns its backend, keeps state outside the bundle, and is identified by commit. | [Desktop](docs/DESKTOP.md), [releases](docs/RELEASING.md) |

**Stack:** Rust, Tokio, Axum, SQLite, Tantivy, and Tauri for the macOS desktop shell.

## Quick start

A **home** is the application's state directory: `~/.CGagentHarness`, overridden with `CGAGENTHARNESS_HOME`. Run one app or standalone server per home.

### 1. Prepare a local model

Pick an installed model that fits your hardware (`ollama list`); the default `qwen3.8:27b-mlx` is not bundled or required.

The default endpoint is `http://127.0.0.1:11434/v1`. Seeded research budgets assume a **32,768-token context window**; for a terminal-managed Ollama service, set it at startup:

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

Restart an existing service rather than starting a second daemon; the harness sends no `num_ctx` override. See [Models](docs/MODELS.md) for Ollama.app and smaller windows.

Other OpenAI-compatible services work on any loopback address (`127.0.0.1`, `localhost`, or `[::1]`); chat and the coding planner have separate settings.

### 2. Choose how to run the harness

#### macOS app

Download `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` from [Releases](https://github.com/cgfixit/CG-agent-harness/releases/latest), then run from the download directory:

```bash
shasum -a 256 -c SHA256SUMS
unzip CG-Agent-Harness-macos-universal.zip
open "CG Agent Harness.app"
```

The universal app (Apple Silicon and Intel) runs its backend on an ephemeral loopback port and needs no Rust, Python, or browser; coding checks need their own tools. Bundles are ad-hoc signed; see [Desktop](docs/DESKTOP.md) for Gatekeeper and tested platforms.

#### Standalone server: macOS or Linux

Source builds use **Rust 1.88**. macOS requires Xcode Command Line Tools; Linux requires a C toolchain, `pkg-config`, and D-Bus development headers ([Install](docs/INSTALL.md)).

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release --locked
./target/release/cgagentharness serve
```

Open `https://127.0.0.1:8790/` and trust the local certificate ([certificate guide](docs/SECURE_RESEARCH.md)). Non-loopback binds are refused.

<details>
<summary>Build the macOS app from source</summary>

The packaging recipe uses an Apple Silicon build host, Xcode Command Line Tools, and Rust toolchains **1.88 and 1.90**:

```bash
rustup target add --toolchain 1.88 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal
```

Full instructions: [macOS app installation](docs/INSTALL.md#52-macos-app).

</details>

### 3. Sign in and try a conversation

1. Sign in with `admin` / `admin` and **replace the bootstrap password immediately**.
2. Select an exact installed model with `/model use <exact-installed-tag>`.
3. Run `/status`, send a message, and inspect the assembled context with `/prompt`.
4. Run `/help` for topics, `/help web` for research, or `/help all` for the command catalog.

Start coding only after the separate [coding setup](docs/CODING_PIPELINE.md).

## Security and defaults

- **Local access:** fresh homes require HTTPS and account login. The optional harness API key does not replace account authentication.
- **Repository changes:** `agentic.enabled`, `agentic.deepagent_github.enabled`, and `allow_git_write_tools` ship false; some write-mode and provider settings do not, so review the combined policy before enabling execution.
- **Network boundaries:** loopback binding limits incoming access only; research, cloud providers, subprocesses, and external model runtimes still reach the network.
- **Cloud coding:** six gates end in per-run `--confirm-online`. Requests can send repository excerpts and task context before diff review; redaction does not remove proprietary code.
- **Sandbox differences:** macOS uses Seatbelt. Linux prefers bubblewrap; its `unshare --net` fallback isolates networking without equivalent filesystem confinement.
- **Data scope:** structured memory and sessions are account-owned; pinned notes, persona, model selection, spend, run records, and page cache are shared within a home. This is not tenant isolation.
- **Credentials and records:** provider keys default to the OS credential store; audit records use secret-pattern and startup-key redaction and stay under the home.

The deployment model is a local operator workstation; Windows builds and releases remain parked. Read [Security](SECURITY.md), [Invariants](INVARIANTS.md), and [Secure research](docs/SECURE_RESEARCH.md) for boundaries and limitations.

## Command line

Each command supports `--help`.

| Command | Purpose |
|---|---|
| `cgagentharness serve` | Start the console. |
| `cgagentharness account` | Login, password, identity, logout. |
| `cgagentharness web` | URL permissions, search, fetch, research. |
| `cgagentharness tls` | Certificate inspection and renewal. |
| `cgagentharness mcp-key` | Memory-gateway machine keys. |
| `cgagentharness netconnect` | Gate `status` and passive `devices`. |

## Configuration and verification

First launch seeds the home's `config.yaml` from [the defaults](assets/config.default.yaml); edit that copy. Permission gates require literal YAML `true`; quoted `"true"` does not enable them. Most settings require restart; selected web/API limits support [live reload](docs/CONFIG_RELOAD.md).

Run the local quality bar (fmt, clippy, tests, and `cargo deny` when installed; see [AGENTS.md](AGENTS.md)):

```bash
SKIP_LIVE=1 scripts/verify-local.sh
```

CI adds invariant guards, browser acceptance, sandbox and MCP lifecycle, and security scanning; see [Tests and CI/CD](docs/INSTALL.md#tests-and-cicd) and [Actions](https://github.com/cgfixit/CG-agent-harness/actions).

**Identify builds by commit**, not the Cargo version `0.1.0`: check the app's `Contents/Resources/COMMIT`, workflow SHA, and release notes; `main` can be newer than published binaries.

## Documentation

| Topic | Guide |
|---|---|
| Setup, upgrades, backups, recovery | [Install](docs/INSTALL.md), [Troubleshooting](docs/TROUBLESHOOTING.md) |
| Models and optional MLX fine-tuning | [Models](docs/MODELS.md), [Fine-tuning](docs/FINETUNE.md) |
| Chat, attachments, skills, connectors, commands | [Console](docs/CONSOLE.md), [User manual](docs/USER_MANUAL.md) |
| Memory behavior and ownership | [Memory](docs/MEMORY_GUIDE.md), [Structured memory](docs/STRUCTURED_MEMORY.md) |
| Coding, jobs, schedules | [Coding pipeline](docs/CODING_PIPELINE.md), [Console jobs](docs/CONSOLE_JOBS.md) |
| External tools and private memory access | [MCP client](docs/MCP_CLIENT.md), [MCP server](docs/MCP_SERVER.md) |
| Passive LAN observation | [Netconnect](docs/netconnect.md) |
| Usage, costs, completion webhooks | [Analytics](docs/ANALYTICS.md), [Spend and notifications](docs/SPEND_AND_NOTIFICATIONS.md) |
| API, dependencies, releases | [Routes](docs/API_ROUTES.md), [Dependencies](docs/DEPENDENCIES.md), [Releasing](docs/RELEASING.md) |

## Contributing

Start with [AGENTS.md](AGENTS.md) and the project guidance in [`.codex/skills`](.codex/skills) or [`.claude/skills`](.claude/skills). Keep changes focused, preserve the documented invariants, and include verification evidence. PRs use driver-prefixed branches, target `main`, open as drafts, and follow the repository template; validate the body with `scripts/check-pr-template.sh`.

## License

[MIT](LICENSE).
