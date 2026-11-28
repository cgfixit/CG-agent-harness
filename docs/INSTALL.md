# Install and first run

## 1. Quick start: automated macOS install

```bash
xcode-select --install
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
scripts/install-macos.sh
scripts/start-macos.sh
```

Run as your user. Finish the Command Line Tools dialog first.
`install-macos.sh` preserves working tools, builds the standalone backend
and verifies a release app with the matching checksums and signature before
copying it to Applications (or `~/Applications`). `--model TAG` pulls a
model deliberately and `--with-gh` adds GitHub CLI.
Ollama installation needs macOS 14+. An Intel Mac without Homebrew needs
operator-provided packages or `--no-ollama`.

### One command: `init.sh`

On Apple Silicon, run `scripts/init.sh` or open `init.command`.
It installs missing prerequisites, sets up profile paths, bootstraps the
admin password into the login Keychain, grants the starter web rules and
offers memory setup and launch choices. An already running same-home
standalone server can be reused for browser setup. Use `--choice 0` through
`3` for none, app, browser or both. `--yes` is unattended.
`--reset-home` deletes the existing home and its selected credentials via
the reset script. See `--help` before using reset or trust options.
The interactive path asks before updating an older working app. Use
`--no-profile`, `--no-allowlist` and `--no-auto-memory` to skip those setup
steps. Its saved admin password is in the Keychain service
`CGagentHarness admin (<home>)`. The private installer log excludes setup
codes and passwords.

### Step by step in Terminal

1. Run `scripts/install-macos.sh --check` and `--dry-run` to inspect dependencies.
2. Install with optional `--no-ollama`, `--no-build` or `--no-app`.
   For a replacement quit the app, then use `--replace-app`. The installer
   can retain the previous working copy in its user cache.
3. [Select an installed model](MODELS.md), then start via `scripts/start-macos.sh`.
4. Complete [first login](#6-first-run). Optionally give the signed-in
   admin CLI the printed console origin and run
   [web-allowlist-bootstrap.sh](../scripts/web-allowlist-bootstrap.sh)
   with `CGAH_BIN` and `CGAH_URL`:

   ```bash
   console_origin='https://127.0.0.1:8790'
   target/release/cgagentharness account --url "$console_origin" login admin
   CGAH_BIN=target/release/cgagentharness CGAH_URL="$console_origin" scripts/web-allowlist-bootstrap.sh
   ```

   Use the printed port, which is dynamic in app mode.

## 2. Prerequisites

### 2.1 Hardware and macOS

The universal app targets macOS 12+ with Apple Silicon and Intel slices.
Check Finder **Kind: Universal**. On Apple Silicon use native tools and leave
**Open using Rosetta** unchecked.

### 2.2 Xcode Command Line Tools

Install with `xcode-select --install` for source builds and native checks.

### 2.3 Optional package installation

Homebrew can supply Ollama and GitHub CLI.

### 2.4 Rust

Use rustup's Cargo. Install pinned backend 1.89 and, for desktop builds,
1.90 with `rustfmt` and `clippy`. Verify `rustup show active-toolchain`
in root and `desktop/`. Packaging disables implicit toolchain installation.

### 2.5 Ollama

Install Ollama, start its app or `ollama serve` and check the exact local tag.
Use [Models](MODELS.md) for the loaded context check, derived default tag
and intentional package pulls.

### 2.6 (Optional, for the coding pipeline only) GitHub CLI

Install `gh` and run `gh auth login`, then `gh auth status`.
See [coding setup](CODING_PIPELINE.md).

## 3. Get the code

Clone the repository as above. To refresh a clean local `main` use
`git fetch origin` and `git merge --ff-only origin/main` while on `main`.
Inspect divergence and preserve local edits.

## 5. Build or install

### 5.1 Standalone backend

With pinned Rust and a C compiler, use `cargo build --release --locked`.
Run `target/release/cgagentharness serve`. The normal locked AWS-LC build
uses the C compiler directly. A build explicitly selecting its CMake
backend also needs CMake.

### 5.2 macOS app

Use the universal ZIP and checksums from [Latest](https://github.com/cgfixit/CG-agent-harness/releases/latest)
or a successful [main Bundle](../.github/workflows/bundle.yml) artifact.
Extract the outer Actions artifact first. In the inner archive directory:

```bash
shasum -a 256 -c SHA256SUMS
ditto -x -k CG-Agent-Harness-macos-universal.zip .
cat 'CG Agent Harness.app/Contents/Resources/COMMIT'
```

Match the commit to the selected release or run. Quit an existing copy
before moving the complete bundle. The app is ad-hoc signed and
unnotarized. See [desktop build and launch](DESKTOP.md) for local
packaging and per-app Gatekeeper handling.

## 6. First run

Fresh homes seed accounts, HTTPS and web controls, with a Google search URL
rule. Existing configs are preserved. Launch the app or visit the address
printed by `serve`. Fresh `admin` must replace the setup code with a password
of at least 12 characters. **Harness → Setup and recovery** (Cmd and +)
shows the code. Standalone users read private `<home>/setup-code`. The
browser's **Where's my setup code?** explains both and can read a file
you select locally. The server never sends the code. Until password change,
only password replacement and logout are permitted.

Use `/model list` and `/model use <exact-tag>`, then send a short chat.
Choose the coding planner separately in `agentic.deepagent_github` if needed.
The local model endpoint and configuration defaults are in [Models](MODELS.md).
The compatibility harness API key is optional.

### Memory setup

Use `/memory` and the [Memory guide](MEMORY_GUIDE.md#enable-in-an-existing-home)
to inspect and configure the store, capture, model support and review gates.

## 7. Open the console in a browser

The app's globe opens its exact owned loopback origin after native
confirmation. `scripts/start-macos.sh` can open the app and its browser
console, starting Ollama if necessary. `--standalone [port]` instead runs
the standalone server (default port 8790). Keep the owner running and sign
in separately in the browser. `start-macos.sh` sets
`OLLAMA_CONTEXT_LENGTH=32768` when unset on its startup path. It cannot
change an already running Ollama instance's window. One server owns a home.

The app verifies its generated certificate for the owned origin. A browser
needs trust configured for that public leaf. `--trust-cert` uses the login
Keychain. Firefox may use its own store. Recheck the fingerprint and renew
trust on replacement. See [HTTPS](SECURE_RESEARCH.md#https-and-native-trust).

## 8. Persistence, optional keys and recovery

### Home and optional credentials

Both launch paths default to `~/.CGagentHarness`. Set absolute
`CGAGENTHARNESS_HOME` in the launching process to override it. Finder does
not source shell profiles. Confirm the actual home in Setup or the footer.

| Location | Contents |
|---|---|
| `config.yaml`, `harness.json` | Private config and persisted console choices |
| `sessions/`, `soul.md`, `soul-history/`, `skills/` | Conversations, persona and runtime skill files |
| `memory/`, `tools/` | Notes, structured store/overrides, web policy/cache and paid-unit ledger |
| `auth.sqlite3`, `auth.initialized`, `tls/server.pem`, `cli-session.json` | Accounts, initialization, TLS and optional origin-bound CLI session |
| `data/agentic/`, `logs/` | Workspaces, jobs, run evidence and bounded logs |

Startup, reload and policy checks require private `config.yaml` (1 MiB
UTF-8 limit) with owner/link/mode checks, macOS ACL checks and Windows
owner/DACL checks. See [config invariant](../INVARIANTS.md#reload-changes-limits-not-authority).
Only supported whole-value `env:NAME` fields resolve at startup (local model
keys and specified MCP fields). Inherit arbitrary names from the launcher.

### Which settings take effect where?

Edit the active config and merge mappings. Only literal YAML true enables
boolean gates. See [shipped settings](../assets/config.default.yaml) and
[missing-key exceptions](../AGENTS.md#traps). Model selection, persona,
memory overrides and web-policy UI changes have their own persistence.
Most file edits require a server restart. Coding writes recheck relevant
policy at mutation boundaries.

### Reload limits without a restart

A non-bootstrap administrator may use **Reload limits** or CSRF-guarded
`POST /api/config/reload` with `{}`. Unix SIGHUP targets the owned backend
PID. [RELOADABLE](../src/server/config_reload.rs) owns the limit list
(API rate, loop and selected web budgets). Success reports `changed` and
`limits.revision`. New work takes the new snapshot. In-flight web work,
rate-limit hits, claims and shared capacity persist.
A tighter ceiling can immediately return 429. SIGHUP remains available
if the route is rate-limited. Audit records `config_reloaded` or
`config_reload_refused` without raw YAML. The route never saves edits.
Consult `/api/status` for the active limits after reload.

Invalid input returns `CONFIG_RELOAD_INVALID`. A restart-only change in the
same edit returns `CONFIG_RESTART_REQUIRED`. Both retain the running
snapshot and the edited disk file. Authority, credentials, provider selection,
`web.concurrency` and notifications need a restart. An unbounded loop
warns when both caps are zero or absent.

### Supported managed keys

| Key | Consumer |
|---|---|
| `CGAGENTHARNESS_API_KEY` | Optional compatibility metadata |
| `CGAGENTHARNESS_WEBHOOK_TOKEN` | Single-owner webhook bearer |
| `GH_TOKEN` | GitHub CLI and Git credential helper |
| `GROK_API_KEY`, `ANTHROPIC_API_KEY` | Explicit cloud chat or governed planner |
| `DEEPAGENT_API_KEY` | Compatible local planner bearer |
| `SERPAPI_API_KEY`, `TAVILY_API_KEY` | Search |
| `FIRECRAWL_API_KEY` | Opt-in exact-URL rereads |

Save via **API Keys** or `/api set <KEY> <value>` in a private console.
`/api clear <KEY>` removes the saved value. Startup loads the OS credential
store, and inherited environment wins even when empty. SerpAPI, Tavily and
Firecrawl changes apply at the next request. Other keys require restart.
Legacy `.env` migrates by default. Literal `security.allow_plaintext_key_file`
opts into the private legacy store at restart. If the OS store cannot
be accessed, saving fails closed. **Saved** and **Active** masks can differ
until restart. Clearing a saved value leaves an inherited override intact.
Use a destination-specific file for multiple webhook owners
([credentials](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook)). A key grants no account
session, provider consent or repository write.

### Close, quit and reopen

Close hides the app. Cmd-Q can cancel active work and quit. Ctrl-C or
SIGTERM stops `serve`. Quit the current owner before switching launch
modes. See [desktop recovery](DESKTOP.md#closing-quitting-and-recovery).

### Retained jobs and runs

Restart marks running jobs interrupted. `/agent jobs`, `/agent job <id>`
and `/agent status <id>` inspect retained state. Nothing replays an approval,
commit, push or publication. Schedules consume occurrences before launch
and skip expired windows. A separate durable notification outbox can retry
under current grants. See [coding](CODING_PIPELINE.md) and
[notifications](SPEND_AND_NOTIFICATIONS.md).

### Update, backup, rollback and uninstall

Stop the owner and back up the active home privately. Keep the previous
bundle/source and matching home backup for rollback. Verify replacement
checksums and keep existing YAML. The app has no updater.
`uninstall.command` or `scripts/uninstall-macos.sh --dry-run` previews
uninstall scope. Removing the app and resetting its home are separate
operations. Backing up the home alone does not copy OS-store credentials
or browser certificate trust. Use [reset recovery](DESKTOP.md#reset-failure-and-recovery)
if a credential deletion failed. Shared tools need separate opt-in removal. The uninstaller preserves a
clone with uncommitted or unpushed work. For an isolated candidate, launch
with a new absolute home and confirm it in Setup:

```bash
candidate_home="$(mktemp -d)"
open -n --env "CGAGENTHARNESS_HOME=$candidate_home" '/Applications/CG Agent Harness.app'
```

## 11. Verify your setup

Check pinned tools, active home, exact installed tag and loaded context.
Use an isolated home for smoke tests. `SKIP_LIVE=1 scripts/verify-local.sh`
runs the repository's local quality checks without live inference and
reports missing cargo-deny as a skip. The local inference
`scripts/smoke-ollama.sh` can skip unavailable endpoints. Check for an actual
reply. `node scripts/chat-browser-acceptance.mjs` uses a fixture, Chrome
and a WebSocket-capable Node. Exercise the packaged WKWebView separately
when desktop behavior changes.

## Tests and CI/CD

[Workflow index](../.github/workflows/README.md) owns the matrix.
Backend CI exercises process, policy, fixture and platform checks.
Bundle packages Linux and macOS CLI targets and the universal app on main.
Every PR requires the full Windows suite and the Windows MCP lifecycle check,
including docs-only changes. See [release controls](RELEASING.md). Release packaging,
browser fixtures, native GUI interaction and live providers have separate
evidence.
