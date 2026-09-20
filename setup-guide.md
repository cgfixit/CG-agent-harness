# Setup guide (macOS: Apple Silicon and Intel)

CG Agent Harness runs either as a universal macOS app or as a standalone
Rust server with a browser console. Use it to keep local-model conversations
and goals, research permitted public documentation with cited evidence, or
stage a repository change for bounded checks and human review. The app owns
its backend and keeps account and work data outside the bundle, so replacing
the app does not replace that data. See [README.md](README.md) for the
overview and [desktop details](docs/DESKTOP.md) for the app's architecture
and limits.

**Loopback-only** means the server listens on a local address such as
`127.0.0.1`; it does not prove that every subprocess or external model service
has no outbound network access. The **console** is the same interface in the
app's native WKWebView and in a browser. The **coding pipeline** runs in a
separate child process and ships disarmed.

**Version scope:** this guide follows this source tree. Check the
[latest published release](https://github.com/cgfixit/CG-agent-harness/releases/latest)
for its actual source commit; a source checkout can include newer features.
The Cargo package version remains `0.1.0`. Identify an installed build using
`Contents/Resources/COMMIT`, its workflow SHA, and release notes. Dated acceptance
records certify only the source and artifact they name.

## Topic pages

| Topic | Page |
|---|---|
| Install, first run, home/keys, verification | [docs/INSTALL.md](docs/INSTALL.md) |
| Models and `OLLAMA_CONTEXT_LENGTH=32768` | [docs/MODELS.md](docs/MODELS.md) |
| Chat, soul, skills, slash commands | [docs/CONSOLE.md](docs/CONSOLE.md) |
| Operator memory notes | [docs/MEMORY_SETUP.md](docs/MEMORY_SETUP.md) |
| Google search, URL fetch, page research | [docs/WEB.md](docs/WEB.md) |
| Coding pipeline | [docs/CODING_PIPELINE.md](docs/CODING_PIPELINE.md) |
| Accounts and roles | [docs/ACCOUNTS.md](docs/ACCOUNTS.md) |
| Troubleshooting | [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) |

## Quick route

- **Run a downloaded app:** [models](docs/MODELS.md), then [install the app and first run](docs/INSTALL.md#52-macos-app). Console controls: [CONSOLE.md](docs/CONSOLE.md).
- **Build from source:** [prerequisites through first run](docs/INSTALL.md); [verify](docs/INSTALL.md#11-verify-your-setup).
- **Missing soul / output style:** [soul](docs/CONSOLE.md#72-default-soul-effective-prompt-and-persona-editing) and [style](docs/CONSOLE.md#74-customize-response-style).
- **Runtime skills:** [skills](docs/CONSOLE.md#73-runtime-skills-and-codex-development-skills). Codex development skills are separate.
- **Memory, web, connectors:** [MEMORY_SETUP.md](docs/MEMORY_SETUP.md), [WEB.md](docs/WEB.md), [connector catalog](docs/CONSOLE.md#77-tools-and-connectors-available-versus-catalog-only). Slash tables: [CONSOLE.md](docs/CONSOLE.md#78-slash-command-quick-reference).
- **Coding goal:** [CODING_PIPELINE.md](docs/CODING_PIPELINE.md), after local chat works.
- **Start fresh or delete saved chats:** [sessions](docs/CONSOLE.md#71-sessions-and-bounded-chat-continuation). Memory is separate in [MEMORY_SETUP.md](docs/MEMORY_SETUP.md).
- **Upgrade, preserve data, release cadence:** [persistence](docs/INSTALL.md#8-persistence-optional-keys-and-recovery).
- **Sign in, roles, recover auth:** [first run](docs/INSTALL.md#6-first-run) and [ACCOUNTS.md](docs/ACCOUNTS.md).
- **HTTPS from Terminal, renew certificates, migrate URL rules:** [secure operations](docs/SECURE_RESEARCH.md).
- **Lockfiles, toolchains, dependency drift:** [verify](docs/INSTALL.md#11-verify-your-setup) and [dependency policy](docs/DEPENDENCIES.md).
- **Failed setup:** [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## 1. Choose how to run it

| Path | What you need | Where the console opens |
|---|---|---|
| Existing macOS app bundle | Apple Silicon or Intel Mac; working local model service for chat | Native app; owned loopback port chosen at launch |
| Build the macOS app | Apple Silicon build host, Git, Xcode Command Line Tools, rustup with Rust 1.88 and 1.90 | Native app after packaging in [macOS app](docs/INSTALL.md#52-macos-app) |
| Standalone server (macOS) | Git, Xcode Command Line Tools and Rust 1.88 | Browser at `https://127.0.0.1:8790/` by default |
| Standalone server (Linux) | Git, a C toolchain, Rust 1.88, and `bwrap` (preferred) or `unshare` for sandboxed checks | Browser at `https://127.0.0.1:8790/` by default |

This guide is written for macOS. The backend itself is also built and tested on
Linux in CI, and Bundle runs attach a `cgagentharness-linux-x86_64` binary. On
Linux, skip the Xcode and app-bundle steps in [INSTALL.md](docs/INSTALL.md)
and note that the hard sandbox prefers bubblewrap (allowlisted read-only
inputs and writable scratch) and falls back to `unshare --net` when `bwrap`
is missing: that fallback isolates the network but does not give checks the
read-only input confinement that Seatbelt does on macOS. Windows CI and
release legs are parked. `serve` also accepts `--host` and `--port`
(1024-65535); any non-loopback bind host is refused.

An already-built app needs no Terminal, external browser, Rust or Python merely
to launch. Coding checks still need their own tools and prepared dependencies.
For that path, use [MODELS.md](docs/MODELS.md) to check the model and
[INSTALL.md](docs/INSTALL.md#52-macos-app) to obtain or install the app, then
follow first run and persistence in [INSTALL.md](docs/INSTALL.md). Build
prerequisites and cloning are only needed if you build from source or choose
the standalone server.

Fresh homes require a local account login over HTTPS; no harness API key or
cloud account is required for local chat. Provider credentials remain
independent, and repository writes still require their existing approvals.

## Where to go next

- [README.md](README.md) — product one-liner and quickstart.
- [INVARIANTS.md](INVARIANTS.md) — process isolation, guard chain, write gates.
- [AGENTS.md](AGENTS.md) — contributor rules and the quality bar.
- [USER_MANUAL.md](docs/USER_MANUAL.md) — pinned notes vs structured memory.
- [STRUCTURED_MEMORY.md](docs/STRUCTURED_MEMORY.md) — contract and HTTP surfaces.
- [CHAT_WORKFLOWS.md](docs/CHAT_WORKFLOWS.md) — persona proposals, skill bounds, goal evidence.
- [CHAT_STREAMING.md](docs/CHAT_STREAMING.md) — SSE events and `/loop stop`.
- [API_ROUTES.md](docs/API_ROUTES.md) — registered HTTP routes.
- [OFFLINE_CARGO.md](docs/OFFLINE_CARGO.md) — dependency preparation vs sandboxed execution.
- [parity/STATUS.md](docs/parity/STATUS.md) — remaining connector work.
