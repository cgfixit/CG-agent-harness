# CG-Agent

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)
[![Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)

**A Rust workspace for local AI chat, permissioned research, and reviewed coding.**

CG-Agent brings conversations, files, memory, and selected tools into one local workspace. It can turn a coding request into a checked, reviewable repository change, with separate approval before committing, pushing, or opening a draft pull request.

Run it as a native macOS app or local browser console. Coding uses a separate child process, bounded edits, sandboxed checks, and explicit review.

A Rust port of [CyClaw](https://github.com/cgfixit/CyClaw)'s console and coding pipeline, with independent configuration, security contracts, and [parity status](docs/parity/STATUS.md).

![CG Agent Harness](docs/screenshots/image.png)

[Quick start](#quick-start) · [Engineering choices](#engineering-choices) · [Security and defaults](#security-and-defaults) · [Documentation](#documentation)

## What you can do

| Capability | What it provides | Starting state |
|---|---|---|
| **Local chat** | Streaming conversations, sessions, attachments, skills, prompt inspection, and bounded, tool-free continuation toward a goal with `/loop`. | Uses a loopback model service; Ollama is the default. |
| **Memory** | Shared pinned notes plus account-private structured facts, recall, and proposed summaries or insights. | Fresh homes enable capture, retrieval, and suggestions. Applying proposed facts requires review. |
| **Web research** | Google listings, permitted page fetches, and research with request, time, byte, and token budgets. | Fresh homes enable web tools with an empty content allowlist. Search results do not authorize page reads. |
| **Reviewed coding** | A planner/executor loop, isolated repository clones, checks, complete diff review, and separate Git decisions. | Disabled until configured for a selected repository. |
| **Jobs and schedules** | Detached coding jobs and reviewed interval or cron schedules. | Execution still requires the coding permissions and owner-bound review. |
| **Cloud chat** | Optional Grok or Claude. Sends only the new message; excludes local history, memory, skills, attachments, and web context. | Requires provider setup and selection; unavailable for `/loop`. |
| **MCP integrations** | Declared external tools and a separate read-only memory gateway using dedicated machine credentials. | Both are disabled by default. |
| **LAN observation** | Netconnect reads in-scope interfaces, routes, and neighbors from local tables. | Disabled with empty scope. No active scans, probes, or device control in this build. |

These defaults describe fresh homes; upgrades preserve existing choices. See the [console guide](docs/CONSOLE.md#what-you-can-do) for commands and capabilities.

Use `/web research` for checked quote references; ordinary chat answers are not validated research citations.

## How reviewed coding works

1. **Define the scope.** Select a repository, configure the coding gates, and prepare its tools and dependencies.
2. **Stage and confirm a task.** Specify the instruction, branch, allowed reads, and check profile.
3. **Generate and verify.** The pipeline works in its own clone, evaluates bounded edits, and runs checks in a sandbox.
4. **Review the result.** Inspect the complete diff and check results. Explicit approval commits the accepted changes.
5. **Publish deliberately.** Push and draft pull-request creation each require a separate reason and confirmation.

A successful check does not authorize publication. Scheduled jobs use the same execution controls.

Start with [Coding pipeline](docs/CODING_PIPELINE.md), including its [Git approval contract](docs/CODING_PIPELINE.md#git-approval-and-publication).

## Engineering choices

| Decision | Purpose | Explore |
|---|---|---|
| Separate console and coding processes | Keep agentic execution behind an allowlisted subprocess interface. | [Shim](src/shim/mod.rs), [invariant tests](tests/invariant_guard.rs) |
| Bind approval to reviewed changes | Check the accepted content, file modes, destination, and approved commit across Git operations. | [Git approval](docs/CODING_PIPELINE.md#git-approval-and-publication), [tests](tests/git_approval.rs) |
| Separate memory proposals from facts | Let models suggest context while requiring approval to change canonical facts. | [Memory guide](docs/MEMORY_GUIDE.md), [tests](tests/structured_memory.rs) |
| Bound external work | Limit research, model calls, subprocess execution, and tool results. | [Configuration](assets/config.default.yaml), [process lifecycle](docs/PROCESS_LIFECYCLE.md) |
| Treat installation as part of the system | Own the desktop backend lifecycle, retain state outside the app, and identify builds by commit. | [Desktop](docs/DESKTOP.md), [releases](docs/RELEASING.md) |

**Stack:** Rust, Tokio, Axum, SQLite, Tantivy, and Tauri for the macOS desktop shell.

## Quick start

A **home** is the application's state directory: `~/.CGagentHarness`, overridden with `CGAGENTHARNESS_HOME`. Run one app or standalone server per home.

### 1. Prepare a local model

Choose an installed model suited to your hardware. The default `qwen3.8:27b-mlx` tag is not bundled; a 27B model is not required.

For Ollama, inspect your inventory:

```bash
ollama list
```

The default endpoint is `http://127.0.0.1:11434/v1`. Seeded research budgets assume a **32,768-token context window**. For a terminal-managed Ollama service, set it at startup:

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

Restart an existing service rather than launching a second daemon. The harness sends no `num_ctx` override. Follow [Models](docs/MODELS.md) to verify the window, configure Ollama.app, or reduce budgets for a smaller window.

Other loopback OpenAI-compatible services can be configured; chat and the coding planner have separate settings.

### 2. Choose how to run the harness

#### macOS app

Download `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` from [Releases](https://github.com/cgfixit/CG-agent-harness/releases/latest), then run from the download directory:

```bash
shasum -a 256 -c SHA256SUMS
unzip CG-Agent-Harness-macos-universal.zip
open "CG Agent Harness.app"
```

The app includes Apple Silicon and Intel executables and owns its backend on an ephemeral loopback port. A built app needs no Rust, Python, Terminal, or external browser to launch; coding checks require their own tools.

Published bundles are ad-hoc signed. See [Desktop](docs/DESKTOP.md) for Gatekeeper, signing, setup, and tested-platform limits.

#### Standalone server: macOS or Linux

Source builds use **Rust 1.88**. macOS requires Xcode Command Line Tools; Linux requires a C toolchain, `pkg-config`, and D-Bus development headers. See [Install](docs/INSTALL.md) for prerequisites.

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release --locked
./target/release/cgagentharness serve
```

Open `https://127.0.0.1:8790/` and explicitly trust the local certificate using the [certificate guide](docs/SECURE_RESEARCH.md). Non-loopback bind addresses are refused.

<details>
<summary>Build the macOS app from source</summary>

The packaging recipe uses an Apple Silicon build host, Xcode Command Line Tools, and Rust toolchains **1.88 and 1.90**. From the checkout:

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
3. Run `/status`, send a short message, and use `/prompt` to inspect assembled context.
4. Run `/help` for topics, `/help web` for research, or `/help all` for the command catalog.

Start coding only after completing the separate [coding setup](docs/CODING_PIPELINE.md).

## Security and defaults

- **Local access:** fresh homes require HTTPS and account login. The optional harness API key does not replace account authentication.
- **Repository changes:** `agentic.enabled`, `agentic.deepagent_github.enabled`, and `allow_git_write_tools` ship false. Individual write-mode and provider settings are not all false; review the combined policy before enabling execution.
- **Network boundaries:** loopback binding limits incoming access. It does not prevent outbound traffic from web research, cloud providers, subprocesses, or an external model runtime.
- **Cloud coding:** a six-gate chain ends in per-run `--confirm-online`. Requests can send repository excerpts and task context before diff review. Redaction does not remove proprietary code; cloud chat's new-message-only restriction does not apply.
- **Sandbox differences:** macOS uses Seatbelt. Linux prefers bubblewrap; its `unshare --net` fallback isolates networking without equivalent filesystem confinement.
- **Data scope:** structured memory and sessions are account-owned; pinned notes, persona, model selection, spend, run records, and public-page cache are shared within a home. This is not universal tenant isolation.
- **Credentials and records:** managed provider keys default to the OS credential store. Audit records use secret-pattern and startup-key redaction and remain under the application home.

The deployment model is a local operator workstation. General Windows builds and releases remain parked; focused Windows security tests do not establish full platform support.

Read [Security](SECURITY.md), [Invariants](INVARIANTS.md), and [Secure research](docs/SECURE_RESEARCH.md) for boundaries and limitations.

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

First launch seeds the home's `config.yaml` from [the default configuration](assets/config.default.yaml). Edit that home copy. Permission gates require literal YAML `true`; quoted `"true"` does not enable them.

Most settings require restart. Selected web/API limits support [live reload](docs/CONFIG_RELOAD.md).

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets --locked
cargo deny check
SKIP_LIVE=1 scripts/verify-local.sh
```

CI includes Linux/macOS tests, invariant guards, browser acceptance, sandbox and MCP lifecycle checks, dependency checks, and security scanning. Coverage varies by workflow and changed paths. See [Tests and CI/CD](docs/INSTALL.md#tests-and-cicd) and [Actions](https://github.com/cgfixit/CG-agent-harness/actions) for exact runs.

**Identify builds by commit.** The Cargo version `0.1.0` does not identify features. Check the app's `Contents/Resources/COMMIT`, workflow SHA, and release notes; `main` can be newer than published binaries.

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

Start with [AGENTS.md](AGENTS.md) and the project guidance in [`.codex/skills`](.codex/skills) or [`.claude/skills`](.claude/skills).

Keep changes focused, preserve the documented invariants, and include verification evidence. PRs use driver-prefixed branches, target `main`, open as drafts, and follow the repository template. Validate the body with `scripts/check-pr-template.sh`.

## Origins

A Rust port of [CyClaw](https://github.com/cgfixit/CyClaw)'s console and coding
pipeline, focused on local coding, chat context, controlled tools and operator review.

## License

[MIT](LICENSE).