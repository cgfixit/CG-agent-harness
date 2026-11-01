# CG Agent Harness

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)
[![Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)

**Your own AI workstation, under your rules.** CG Agent Harness is one loopback-only Rust service for a single operator: private chat with a local model, web research fenced by URL rules you write, and an AI coding loop that edits in isolated clones and cannot commit, push, or open a pull request without your approval. Everything ships closed: cloud models, repository writes, external tools, and LAN observation stay off until you turn each one on, and the boundaries are enforced by CI tests, not prose.

Built for developers and security-minded operators who want agentic tooling on their own machine with the audit trail and least privilege they expect of any other process.

[![Console after sign-in: status chips, a local reply with token counts, searchable command guide](docs/screenshots/console-chat-2026-10-30.png)](docs/screenshots/console-chat-2026-10-30.png)

Fresh home, loopback fixture model, 2026-10-30. [Silent 60-second demo](docs/screenshots/demo/CG-Agent-Harness-60-second-demo.mp4) of a synthetic release review ([notes](docs/screenshots/demo/voiceover-cues.txt)).

[Quick start](#quick-start) · [Engineering choices](#engineering-choices) · [Security and defaults](#security-and-defaults) · [Documentation](#documentation)

## What you can do

| Capability | What it provides | Starting state |
|---|---|---|
| **Local chat** | Streaming sessions with search, export, text and DOCX attachments, prompt skills, `/prompt` inspection, and a sandboxed HTML preview. | Ollama on loopback by default; listed, pulled, and tuned from the console. |
| **Goal loops** | `/goal` and `/loop` keep working on a task, in the console or as a server-side run that outlives the browser, under turn, token, and failure limits. | Budgets unlimited; two consecutive failures stop a run; cloud models refuse loops. |
| **Memory** | Shared pinned notes plus account-private structured facts, recall, and proposed summaries. | Capture, recall, and retrieval on; applying a proposal needs review ([setup](docs/INSTALL.md#memory-setup)). |
| **Web research** | SerpAPI, public Google, or loopback SearXNG search; permitted fetches; `/web research` under request, time, byte, and token budgets, quotes checked. | On in allowlist mode with an empty allowlist; listings never authorize page reads; a failed SerpAPI search falls back to public Google, which still needs its URL rule. |
| **Reviewed coding** | Planner/executor loop, isolated clones, sandboxed checks, full diff review, separate Git decisions; detached jobs and schedules reuse the same gates. | Off until configured for a selected repository. |
| **Analytics** | Session, token, and run history with USD where rates exist, plus **Estimate draft** before a cloud call. | On after sign-in; read-only; local inference unpriced. |
| **Cloud chat** | Optional Grok or Claude. Sends only the new message, never history, memory, skills, attachments, or web context. | Needs a provider key; refused for `/loop`. |
| **MCP integrations** | Declared external tools and a separate read-only memory gateway with dedicated machine credentials. | Both off. |
| **LAN observation** | Netconnect reads in-scope interfaces, routes, and neighbors from local tables. | Off with empty scope; only the budgeted `diag` probe sends packets; no scans or device control. |
| **Self-knowledge and diagnostics** | Bundled harness manual and live status as untrusted local-chat context; `/diag` shows bounded, redacted tails of allowlisted home logs. | Manual on, never sent to cloud chat or loops; `/diag` off, admin or `portal_auditor` only. |

Fresh-home states; upgrades keep existing choices ([console guide](docs/CONSOLE.md#what-you-can-do)).

## How reviewed coding works

1. **Scope.** Select a repository, configure the coding gates, prepare its toolchain.
2. **Stage and confirm.** Give the instruction, branch, allowed reads, and check profile.
3. **Generate and verify.** Bounded edits land in the pipeline's own clone; checks run in a sandbox.
4. **Review.** Inspect the full diff and check results; explicit approval commits.
5. **Publish deliberately.** Push and draft PR creation each need their own reason and confirmation.

Passing checks never authorize publication ([Coding pipeline](docs/CODING_PIPELINE.md), [Git approval](docs/CODING_PIPELINE.md#git-approval-and-publication)).

## Engineering choices

| Decision | Purpose | Explore |
|---|---|---|
| Separate console and coding processes | Coding runs behind an allowlisted subprocess interface. | [Shim](src/shim/mod.rs), [invariant tests](tests/invariant_guard.rs) |
| Bind approval to reviewed changes | Content, modes, destination, and commit are rechecked at every Git step. | [Git approval](docs/CODING_PIPELINE.md#git-approval-and-publication), [tests](tests/git_approval.rs) |
| Separate memory proposals from facts | Models suggest; canonical facts change only on approval. | [Memory guide](docs/MEMORY_GUIDE.md), [tests](tests/structured_memory.rs) |
| Contracts as tests | Isolation, shipped gates, routes, and doc size fail `cargo test`, not review. | [Invariants](INVARIANTS.md), [guard](tests/invariant_guard.rs) |

**Stack:** Rust, Tokio, Axum, SQLite, Tantivy, and Tauri for the macOS desktop shell.

## Quick start

A **home** is the application's state directory: `~/.CGagentHarness`, overridden with `CGAGENTHARNESS_HOME`. Run one app or standalone server per home.

**macOS, from scratch:** after `xcode-select --install`, `scripts/install-macos.sh` installs Rust, Ollama, and the release app as needed and builds the backend; `scripts/start-macos.sh` opens the app and console ([automated install](docs/INSTALL.md#1-quick-start-automated-macos-install), [browser access](docs/INSTALL.md#7-open-the-console-in-a-browser)).

### 1. Prepare a local model

Pick an installed model that fits your hardware; any OpenAI-compatible loopback service works, and chat and the coding planner have separate settings. The chat default `gemma4-26b-a4b:ud-q4_k_xl-cg` and planner default `qwen3.8:27b-mlx-cg` are [locally derived tags](docs/MODELS.md#pin-32768-with-a-local-derived-tag), not bundled or required.

The default endpoint is `http://127.0.0.1:11434/v1`. Seeded research budgets assume a **32,768-token context window**; the harness sends no `num_ctx`, so set it when a terminal-managed Ollama starts and restart a running service rather than start a second one ([Models](docs/MODELS.md) covers Ollama.app, smaller windows, and pinned tags):

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
```

### 2. Choose how to run the harness

#### macOS app

Download `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` from [Releases](https://github.com/cgfixit/CG-agent-harness/releases/latest), then run from the download directory:

```bash
shasum -a 256 -c SHA256SUMS
unzip CG-Agent-Harness-macos-universal.zip
open "CG Agent Harness.app"
```

The universal app (Apple Silicon and Intel) runs its backend on an ephemeral loopback port and needs no Rust, Python, or browser; bundles are ad-hoc signed ([Desktop](docs/DESKTOP.md) covers Gatekeeper). Releases also attach Linux x86_64 and macOS arm64 CLI builds with `.sha256` files.

#### Standalone server: macOS or Linux

Source builds need **Rust 1.88** and a C toolchain; the macOS app build also needs 1.90 ([Install](docs/INSTALL.md), [app build](docs/INSTALL.md#52-macos-app)).

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release --locked
./target/release/cgagentharness serve
```

Open `https://127.0.0.1:8790/` and trust the local certificate ([certificate guide](docs/SECURE_RESEARCH.md)). Non-loopback binds are refused.

### 3. Sign in and try a conversation

1. Fresh homes open **Set your password**: enter the setup code (app: **Cmd and +** opens **Setup and recovery**; terminal: `<home>/setup-code`) and a new password.
2. `/model list` shows installed tags; `/model use <tag>` selects one and, with `auto_tune` on, measures it and sets per-model limits (`/model profile`).
3. Run `/status`, send a message, inspect the assembled context with `/prompt`, and browse `/help` (`/help all` is the full catalog).

Start coding only after the separate [coding setup](docs/CODING_PIPELINE.md).

## Security and defaults

- **Local access:** fresh homes require HTTPS and account login; the optional harness API key never replaces them.
- **Web policy mode:** homes ship in allowlist mode; an administrator can confirm a switch to block-list mode, which opens every public URL no block rule covers. Private and local addresses stay refused either way.
- **Network boundaries:** loopback binding limits incoming access only; research, cloud providers, subprocesses, and model runtimes still reach the network.
- **Cloud coding:** six gates end in per-run `--confirm-online`, and requests send repository excerpts before diff review; redaction does not remove proprietary code.
- **Sandbox differences:** macOS uses Seatbelt. Linux prefers bubblewrap; its `unshare --net` fallback isolates networking only.
- **Data scope:** structured memory and sessions are account-owned; other home state is [shared](SECURITY.md#local-authority-and-shared-resources), so this is not tenant isolation.
- **Credentials and records:** provider keys default to the OS credential store, with a startup warning while plaintext storage is on; audit records are redacted and stay in the home.

Target: a local operator workstation; Windows is CI-tested but has no release. Details: [Security](SECURITY.md), [Invariants](INVARIANTS.md), [Secure research](docs/SECURE_RESEARCH.md).

## Command line

Six public commands, each with `--help`: `serve` starts the console; `account` covers login, password, and logout; `web` manages URL rules, search, fetch, and research; `tls` inspects and renews the certificate; `mcp-key` manages memory-gateway keys; `netconnect` offers gate `status`, passive `devices`, and the budgeted `diag` probe.

## Configuration and verification

First launch seeds the home's `config.yaml` from [the defaults](assets/config.default.yaml); edit that copy. Gates require literal YAML `true`; quoted `"true"` or a missing key is off, and startup logs any shipped-`true` setting your file lacks. Most settings need a restart; selected limits [reload live](docs/CONFIG_RELOAD.md).

Before contributing, run `bash scripts/ensure-githooks.sh` ([hook guide](docs/GITHOOKS.md)) and follow [AGENTS.md](AGENTS.md) for focused verification; `SKIP_LIVE=1 scripts/verify-local.sh` runs the comprehensive checks and CI adds invariant guards, browser acceptance, sandbox and MCP lifecycle checks, and security scanning ([Tests and CI/CD](docs/INSTALL.md#tests-and-cicd)).

**Identify builds by commit**, not the Cargo version `0.1.0`: check `Contents/Resources/COMMIT`; `main` can be newer than published binaries.

## Documentation

| Topic | Guide |
|---|---|
| Setup, models, upgrades, recovery | [Install](docs/INSTALL.md), [Models](docs/MODELS.md), [Troubleshooting](docs/TROUBLESHOOTING.md) |
| Chat, attachments, skills, connectors, commands | [Console](docs/CONSOLE.md) |
| Memory behavior and ownership | [Memory](docs/MEMORY_GUIDE.md), [Structured memory](docs/STRUCTURED_MEMORY.md) |
| Coding, jobs, schedules | [Coding pipeline](docs/CODING_PIPELINE.md) |
| External tools and private memory access | [MCP client](docs/MCP_CLIENT.md), [MCP server](docs/MCP_SERVER.md) |
| Passive LAN observation | [Netconnect](docs/netconnect.md) |
| Usage, costs, completion webhooks | [Analytics, spend and notifications](docs/SPEND_AND_NOTIFICATIONS.md) |
| Desktop app, Gatekeeper, reset | [Desktop](docs/DESKTOP.md) |
| Web boundaries and certificates | [Secure research](docs/SECURE_RESEARCH.md), [Security](SECURITY.md) |
| API, dependencies, releases | [Routes](docs/API_ROUTES.md), [Dependencies](docs/DEPENDENCIES.md), [Releasing](docs/RELEASING.md) |

## License

[MIT](LICENSE).
