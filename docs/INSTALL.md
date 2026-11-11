# Install and first run

For model and account details, see [Models](MODELS.md) and
[Accounts](SECURE_RESEARCH.md#accounts-and-roles).

## 1. Quick start: automated macOS install

On macOS 12+, two scripts do sections 2–7 on a fresh Mac. Installing Ollama needs
macOS 14+, and Homebrew installs only on Apple Silicon: on an Intel Mac
without it, install Ollama (and `gh`) yourself or pass `--no-ollama`. Run them as
your user (no `sudo`):

```bash
xcode-select --install     # once; finish the dialog (git comes with it)
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
scripts/install-macos.sh   # add --model <tag> to pull a model, --with-gh for coding
scripts/start-macos.sh     # opens the app, then its console in your browser
```

[`install-macos.sh`](../scripts/install-macos.sh) keeps every tool that already
works and installs only what is missing, in order: Command Line Tools; Homebrew
(only for Ollama or `gh`); rustup with the pinned Rust; Ollama; optional `gh`.
It then builds the standalone backend, downloads the latest release app, checks
it against that release's `SHA256SUMS` and `codesign`, and copies it to
`/Applications` (`~/Applications` when that is not writable). It never touches
the harness home (`~/.CGagentHarness`) and pulls a model only with `--model`.

| Option | Effect |
|---|---|
| `--check` | Lists what is present; exits 1 when something required is missing. A working app older than the latest release gets a note, not a failure |
| `--dry-run` | Prints every command, changing nothing |
| `--replace-app` | Replaces a working app (quit it first), keeping the previous one in `~/Library/Caches/CG-agent-harness-installer/previous`. An incomplete or unsigned copy is replaced without it |
| `--no-ollama`, `--no-build`, `--no-app`, `--app-dir DIR` | Skip a step or choose the app location; outside `/Applications` and `~/Applications`, start with `CGAH_APP`; after `--no-app`, with `--standalone` |

The checksum proves integrity, not authorship (both files share one release
page). `curl` downloads skip Gatekeeper's quarantine prompt; compare
`Resources/COMMIT` with the release notes ([5.2](#52-macos-app)).
[`start-macos.sh`](../scripts/start-macos.sh) is the everyday launcher
([section 7](#7-open-the-console-in-a-browser)).

### One command: `init.sh`

On an Apple Silicon Mac, [`scripts/init.sh`](../scripts/init.sh) runs the whole
setup from a fresh clone or download, from zsh or bash:

```bash
cd CG-agent-harness && scripts/init.sh    # or: bash scripts/init.sh
```

Or double-click **init.command** in the clone's top folder: it runs the same
script in a Terminal window and waits for Return at the end. A folder unpacked
from a downloaded ZIP is quarantined: Control-click the file and choose **Open**
(macOS 15 and later: **Open Anyway** in System Settings → Privacy & Security),
or run `xattr -dr com.apple.quarantine <folder>` once.

In order, the script:

1. Waits for the Command Line Tools dialog and runs `install-macos.sh` (skipped
   when `--check` passes).
2. Adds Homebrew and Rust to your login shell's profile (`~/.zprofile` for zsh,
   `~/.bash_profile` or `~/.profile` for bash).
3. Replaces admin's one-time setup code with a new password, saved in your
   login keychain as **CG Agent Harness admin password**.
4. Grants the web allowlist.
5. Offers to list the local model for automatic memory (default yes): after a
   backup it adds `structured_memory.suggestion_supported` to the home
   `config.yaml` (missing means no model may draft memory proposals); restart
   the app afterwards.
6. Asks what to open: **1** the app, **2** the console in your browser (served
   from that Terminal until Ctrl-C, or the open app's console), **3** both,
   **0** nothing.

A failed step offers retry, skip or quit. A rerun skips finished steps and signs
in with the saved password; when the installed app is older than the latest
release it asks before updating (never with `--yes`). A `serve` of yours that
already owns the home is used for setup and left running: choice 2 opens it,
while choices 1 and 3 stop until you quit it. With `--reset-home` and an
existing home, the script first deletes the home and its Keychain entries
through `reset-macos.sh` (interactively: default no, and you type `DELETE`
after its dry run).

| Option | Effect |
|---|---|
| `--yes` | Asks nothing; generates the admin password (Keychain only) |
| `--choice 0`–`3` | What opens at the end |
| `--model TAG`, `--with-gh`, `--no-ollama`, `--app-dir DIR` | Passed to `install-macos.sh` |
| `--replace-app` | Passed on too, and runs the installer even when `--check` passes (quit the app first) |
| `--trust-cert`, `--no-profile`, `--no-allowlist` | Trust the console certificate (choices 2 and 3 with the app); skip the profile or allowlist |
| `--no-auto-memory` | Leave `suggestion_supported` unlisted |
| `--reset-home` | Delete the previous home and its Keychain entries first (confirmed unless `--yes`) |

Each line is logged with a time to `scripts/logs/install.log` (private, ignored by
git); passwords and setup codes never are. Read the saved password with
`security find-generic-password -a admin -s 'CGagentHarness admin (<home>)' -w`.

### Step by step in Terminal

Open **Terminal** (Applications → Utilities); on Apple Silicon leave its **Open
using Rosetta** unchecked. Run every command from the clone, as your user.

1. **Developer tools and code:** `xcode-select --install` (finish the dialog),
   then `git clone https://github.com/cgfixit/CG-agent-harness.git && cd CG-agent-harness`.
2. **Inventory:** `scripts/install-macos.sh --check` prints one line per tool and
   exits 1 while something required is missing. It changes nothing.
3. **Preview:** `scripts/install-macos.sh --dry-run --model <tag>` prints every
   command the install would run. Pick the tag in [Models](MODELS.md); omit
   `--model` to pull later (models are multi-GB). Add `--with-gh` for coding.
4. **Install:** the same command without `--dry-run`. The Homebrew and rustup
   installers may ask for your password and print "Next steps"; run those lines,
   then open a new Terminal window so `brew` and `cargo` are on `PATH`. The first
   backend build takes several minutes.
5. **Confirm:** `scripts/install-macos.sh --check` ends with
   `everything required is present`.
6. **Model tag:** build the derived default tag
   ([Models](MODELS.md#pin-32768-with-a-local-derived-tag)), or `/model use` an
   installed one later.
7. **Start:** `scripts/start-macos.sh --trust-cert` starts Ollama, opens the app
   and its console in your browser, and trusts this home's certificate after one
   password prompt ([section 7](#7-open-the-console-in-a-browser)). Note the
   printed `CGagentHarness console: https://127.0.0.1:<port>/`.
8. **Set your password:** fresh homes open a form for the private setup code
   and a new password of at least 12 characters. The app keeps **Harness → Setup
   and recovery** open with the code and a **Copy setup code** button until you
   do (**Cmd and +**, that is Cmd, Shift and =, reopens it); for a terminal
   server, `cat ~/.CGagentHarness/setup-code`. Until then that session can only
   change its password or log out, and the CLI and API answer
   `AUTH_PASSWORD_CHANGE_REQUIRED`. The browser needs its own sign-in.
9. **Optional web allowlist:** with that console origin, sign the CLI in and grant
   the technical-docs starter list:

   ```bash
   url=https://127.0.0.1:<port>          # the origin printed in step 7
   target/release/cgagentharness account --url "$url" login admin
   CGAH_BIN=target/release/cgagentharness CGAH_URL="$url" scripts/web-allowlist-bootstrap.sh
   ```

10. **Every day:** `scripts/start-macos.sh` (without the app:
    `scripts/start-macos.sh --standalone`, port 8790, where the CLI needs no `--url`).
11. **Update:** quit the app (Cmd-Q), `git pull --ff-only`, then
    `scripts/install-macos.sh --replace-app`.
12. **Uninstall:** see [update, backup, rollback and uninstall](#update-backup-rollback-and-uninstall).

If `start-macos.sh` reports no usable app, rerun step 4. If it reports the home is
already owned, quit that owner (app mode refuses one that is not the app's
backend). More fixes: [Troubleshooting](TROUBLESHOOTING.md).

## 2. Prerequisites

These are the manual steps [`install-macos.sh`](#1-quick-start-automated-macos-install)
automates. Install only what you need and preserve working tools.

### 2.1 Hardware and macOS

```bash
sw_vers
uname -m
```

The universal app includes `arm64` and `x86_64`. `uname -m` reports `arm64` in a
native Apple Silicon shell; `x86_64` can mean Intel hardware or a translated
shell, so check **About This Mac**. The bundle targets macOS 12+, which does not
prove acceptance on every older OS or Intel Mac ([desktop details](DESKTOP.md)).
In Finder, Cmd-I on the app must say **Kind: Universal**; leave **Open using
Rosetta** unchecked on Apple Silicon
([Rosetta guidance](https://support.apple.com/en-us/102527)).

### 2.2 Xcode Command Line Tools

Source builds and native Cargo checks need Apple's Command Line Tools, not the
full Xcode app:

```bash
xcode-select --install
```

### 2.3 Optional package installation

Homebrew is optional ([installation instructions](https://brew.sh)).

### 2.4 Rust

Inspect the installation on PATH before changing it:

```bash
command -v rustc cargo rustup
rustc --version
rustc --print sysroot
```

The repository pins **Rust 1.89 for the backend** in `rust-toolchain.toml` and
**Rust 1.90 for desktop** in `desktop/rust-toolchain.toml`. Use rustup's Cargo on
PATH so those files take effect; a Homebrew Cargo does not enforce them.
Prepare both (the second only for desktop work):

```bash
rustup toolchain install 1.89 --profile minimal --component clippy --component rustfmt
rustup toolchain install 1.90 --profile minimal --component clippy --component rustfmt
```

Without Rust, follow the [installation instructions](https://www.rust-lang.org/tools/install).
`rustup show active-toolchain` should resolve to 1.89 in the repository root and
1.90 inside `desktop/`. The packaging script disables implicit toolchain
installation, so install both before running it.

### 2.5 Ollama

[Ollama](https://ollama.com) runs the local chat and planning model. Check
`command -v ollama` first; on a fresh machine, Homebrew can install the app
(or follow the [Ollama quickstart](https://docs.ollama.com/quickstart)):

```bash
brew install --cask ollama
```

Launch Ollama, then verify `ollama list` and the endpoint in [model setup](MODELS.md).
The server never installs Ollama or downloads models on its own; deliberate
[API pulls](API_ROUTES.md#chat-model-and-chat-sessions) are supported.

Seeded web budgets assume `OLLAMA_CONTEXT_LENGTH=32768` is set **before** the
Ollama process starts; the harness sends no `num_ctx`. [Model setup](MODELS.md)
covers that window and the locally derived `-cg` tag the shipped defaults name.

### 2.6 (Optional, for the coding pipeline only) GitHub CLI

Only the optional [coding pipeline](CODING_PIPELINE.md) requires GitHub CLI:

```bash
brew install gh
```

## 3. Get the code

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
```

To update an existing clean local `main`, preserving uncommitted work:

```bash
git status --short
git fetch origin
git switch main
git merge --ff-only origin/main
```

Inspect divergence instead of resetting it. Contributors: see
[AGENTS.md](../AGENTS.md#quality-bar) for branches and PRs.

## 5. Build or install

### 5.1 Standalone backend

Source builds need the pinned Rust toolchain ([Rust](#24-rust)) and a C compiler
(Xcode Command Line Tools on macOS; for example, `build-essential` on
Debian/Ubuntu). Reqwest's locked AWS-LC dependency normally uses that compiler
directly, without CMake. If an alternate AWS-LC build selects CMake (for
example, `AWS_LC_SYS_CMAKE_BUILDER=1`), install it first (`brew install cmake`,
or `sudo apt-get install cmake`).

```bash
cargo build --release --locked
```

The binary is `target/release/cgagentharness`.

### 5.2 macOS app

Choose a **published release** for its documented source version or a successful
**main artifact** for newer merged features. Bundle runs only after pushes to
`main`; PRs produce no Bundle artifacts:

| Source | What to download | What it establishes |
|---|---|---|
| [Latest release](https://github.com/cgfixit/CG-agent-harness/releases/latest) | `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` | Published release source identified in its notes |
| Successful [Bundle run](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml) on `main` | `cg-agent-harness-macos-universal` Actions artifact | Merged source at that run's commit; may be newer than the published release |
| Successful [desktop workflow](https://github.com/cgfixit/CG-agent-harness/actions/workflows/desktop.yml) | `cg-agent-harness-macos-universal` for a standalone dispatch | Desktop build checks for its selected source |

The `cgagentharness-macos-arm64` CLI archive is a different deliverable without
the desktop app. Actions downloads may require GitHub login and expire after 14
days; release assets do not. For an Actions artifact, extract the outer
download first. In the directory containing the **inner** app ZIP and checksum
file, run:

```bash
shasum -a 256 -c SHA256SUMS
ditto -x -k CG-Agent-Harness-macos-universal.zip .
cat 'CG Agent Harness.app/Contents/Resources/COMMIT'
```

If the checksum fails, stop and obtain the matching pair again; never install a
modified archive. The embedded commit should match the selected release or
workflow source.

To build your own universal app from a clean committed checkout, install both
pinned toolchains ([Rust](#24-rust)), then both target standard libraries:

```bash
rustup target add --toolchain 1.89 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal --dmg
```

`--dmg` is optional. Outputs land in `dist/`; omitting `--universal` creates an
arm64 development bundle. Packaging refuses a dirty checkout by default, so
`Resources/COMMIT` identifies reviewed source
([rebuild and verify](DESKTOP.md#rebuild-and-verify)).

Quit any existing copy with **Cmd-Q**, then unzip the app and move the complete
bundle to Applications or a directory you own. The app owns its backend on a
dynamically selected loopback port; it does not adopt a server already running
on port 8790, and it installs no login service.

The app is **ad-hoc signed and not notarized**: after verifying the source,
follow Apple's [per-app Open Anyway procedure](https://support.apple.com/en-us/102445)
if Gatekeeper asks, and never disable Gatekeeper globally or strip quarantine as
a blanket fix. A damaged-bundle warning calls for re-download and integrity
checks ([desktop details](DESKTOP.md)).

## 6. First run

Fresh homes enable login, role permissions, HTTPS and web fetch/search/research
without a harness API key or configuration edit. The allowlist starts with
one Google search rule; administrators grant sources through `/web allow`, or a
starter list of technical references with
[`web-allowlist-bootstrap.sh`](../scripts/web-allowlist-bootstrap.sh)
([step 9](#step-by-step-in-terminal)). Upgrades keep existing settings
([adopting the defaults](SECURE_RESEARCH.md)). Open the app from Finder, or
start the standalone server:

```bash
./target/release/cgagentharness serve
```

It prints its address, for example:

```
CGagentHarness console on https://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

A fresh home's `admin` signs in once with the code in private
`<home>/setup-code` (the app's **Cmd and +** window and `start-web.sh` show it),
then must replace the password; no MCP machine key can be minted before that.
In the app the console opens automatically, and **Harness → Setup and
recovery** also shows the owned endpoint and diagnostics. For the standalone
server, leave its terminal open and visit the printed address. The app
verifies its owned certificate; a browser needs it trusted first
([browser access](#7-open-the-console-in-a-browser)).

Chat uses the configured loopback service's `qwen3.8:27b-mlx-cg` tag by
default, a [locally derived tag](MODELS.md#pin-32768-with-a-local-derived-tag)
reported `tag_missing` until `ollama create` builds it. `/model list` shows
installed tags, or an example `ollama pull` command when none is installed;
`/model use <exact-tag>` selects one for chat without a restart. Selection
never downloads a tag and never changes the planner.

To change the endpoint or configure the coding planner, quit with Cmd-Q or stop
`serve` with Ctrl-C, then merge the exact installed tag into the active home's
`config.yaml` without duplicating mappings:

```yaml
models:
  local_llm:
    model: "qwen3.8:27b-mlx-cg"   # chat; use the exact installed tag
agentic:
  deepagent_github:
    model: "qwen3.8:27b-mlx-cg"   # optional planner; writes remain disarmed
```

Upgrades do not rewrite existing configuration; review new fields. Fresh homes
leave the persona unset
([soul](CONSOLE.md#72-default-soul-effective-prompt-and-persona-editing)).
After login, leave the optional metadata key empty and send a message. Replies
[stream](CONSOLE.md#streaming-chat-and-cancellation); `/loop stop` aborts the
turn. If replies stall, check that [Ollama](MODELS.md) answers at its
configured endpoint.

### Memory setup

Run `/memory` to inspect gates. Fresh homes enable capture, suggestions and
retrieval; upgrades preserve settings ([enable an existing home](MEMORY_GUIDE.md#enable-in-an-existing-home)).
Suggestions need the selected local tag listed in
`structured_memory.suggestion_supported` and a `suggestion_mode` (`summaries`,
`insights` or `both`); restart after editing. Review each proposal in
**Memory** and enter a reason to save or dismiss; generation never changes
saved facts ([Memory](MEMORY_GUIDE.md)).

## 7. Open the console in a browser

A browser can use the same server the app owns: the backend serves the same
guarded console on its loopback port, and a browser gets its own login. Three
ways:

| Way | What runs |
|---|---|
| Globe beside **CG Agent / harness** (apps built after `v0.1.48`) | After native confirmation, the bundled `start-web.sh --desktop-url` opens the app's exact origin |
| `scripts/start-macos.sh` | Same `--desktop-url` hand-off from a clone; works with any app version |
| `scripts/start-macos.sh --standalone [port]` | No app: `start-web.sh` builds if needed and runs `serve` on 8790 |

`start-macos.sh` first starts Ollama if it is not answering: the app, or a
CLI-only install's `ollama serve` (log `~/Library/Logs/ollama-serve.log`). It sets
`OLLAMA_CONTEXT_LENGTH=32768` when unset (the app's `launchctl` value is lost at
logout, so rerun the script after login; restart an Ollama started without it).
Ollama's first launch asks you to confirm an app downloaded from the internet;
the script waits 30 seconds. It opens the app if no app backend is running,
reads that backend's loopback port, and passes the origin to
`start-web.sh --desktop-url`. It starts no second server; keep the app running.

Plain `scripts/start-web.sh` while the app is open fails by design: one server
owns a home, and a second `serve` stops at the home lock. `--standalone`
refuses early when another process holds that lock.

Browsers do not trust the home's generated certificate. `start-macos.sh` reports
whether macOS trusts it; `--trust-cert` exports the public certificate of the
home the app's backend holds and adds it to your login keychain for TLS after a
password prompt. That certificate is a leaf for `localhost`, `127.0.0.1` and
`::1`, not a CA. A renewed certificate needs `--trust-cert` again; remove the old
one in Keychain Access. Firefox keeps its own store. Never click through a
certificate warning ([HTTPS and native trust](SECURE_RESEARCH.md#https-and-native-trust)).

## 8. Persistence, optional keys and recovery

### Home and optional credentials

Both launch paths default to `~/.CGagentHarness`. Set `CGAGENTHARNESS_HOME` in
the launching process to override it; desktop requires an absolute path and does
not inherit shell exports when Finder launches it. Setup shows the active home,
which survives replacing or uninstalling the bundle.

| Home content | Purpose |
|---|---|
| `config.yaml` | Private configuration (1 MiB maximum); reload supported limits or restart |
| `harness.json` | Persisted model selection, toggles and console settings |
| `soul.md`, `soul-history/`, `soul-pending-apply.json` | Optional persona, bounded history/proposals, apply-recovery marker |
| `skills/<id>/SKILL.md` | Runtime skill bodies; existing files are preserved |
| `sessions/` | Chat history, goals, selected skills, goal-stage linkage |
| `memory/`, `tools/` | Pinned notes (`memory/notes.json`); optional structured store (`memory/structured.sqlite3`) and gate overlay (`memory/structured_gates.json`); web context/allowlist under `tools/` |
| `.env`, `auth.sqlite3`, `auth.initialized` | Managed-key migration input/opt-in store; transactional accounts and initialization marker; legacy JSON retained for recovery |
| `tls/server.pem`, `cli-session.json` | Private persisted TLS material and optional CLI login bound to its origin/certificate |
| `data/agentic/` | Registry, retained console jobs, workspaces and run evidence |
| `logs/` | Bounded audit/spend/optional metrics logs |

**API Keys** manages credentials and distinguishes saved from active masks.
Startup loads Keychain, Secret Service or Credential Manager; Linux builds
vendor the D-Bus client and need no D-Bus development headers. Inherited values
override stored values, including after clearing a saved key. Legacy `.env`
migrates unless `security.allow_plaintext_key_file` is literal true. Restart to
reload.

`config.yaml` must be private at startup, reload and policy rechecks (1 MiB
UTF-8 maximum): Unix owner, one link and no group/other permissions, no extended
ACLs on macOS, and the [credential reader's](../INVARIANTS.md#provider-keys-live-in-the-os-credential-store)
owner/DACL checks on Windows ([contract](../INVARIANTS.md#reload-changes-limits-not-authority)).
Repair Unix permissions with `chmod 600` and remove extended ACLs; refusals never
repair files.

Only local/fallback `api_key`, MCP `env` values and complete SSE `url` accept
whole-value `env:NAME`, e.g. `api_key: env:LOCAL_MODEL_TOKEN`, resolved once at
startup. Arbitrary names must be inherited, not saved through API Keys. MCP
refuses reserved sources; see [declarations](MCP.md#migrate-a-stdio-declaration).
Forwarded or proxy requests are unsupported ([secure setup](SECURE_RESEARCH.md)).

### Which settings take effect where?

Edit the **active home's** `config.yaml`. Merge existing mappings; duplicate YAML
keys are invalid, and only literal `true` enables a gate. Nest `local_llm` under
`models`; a root-level block is rejected, not ignored. `workspace_root` must stay
beneath the active home's `data/`.
Use **Reload limits** or Unix SIGHUP for the [reloadable limits](#reload-limits-without-a-restart).
Other settings require quit/relaunch or a `serve` restart; closing the window
only hides it. Coding-policy checks reread disk at mutation boundaries.

| Setting | Scope / persistence | How to verify |
|---|---|---|
| Model endpoint, provider, timeout, temperature and token ceiling | `config.yaml`, home-wide; restart after edits. Local context is the Ollama server window ([model setup](MODELS.md)), not a harness `num_ctx`; `max_tokens` is the output ceiling | `/model`, Setup diagnostics, `/api/ps` `context_length`, and an actual reply |
| `/model use <name>` | Persisted console model selection; subsequent requests | `/model`; it can differ from the seeded YAML tag |
| Persona, notes and their toggles | `soul.md`, `memory/notes.json` and `harness.json`, shared across sessions; next request | `/soul status`, `/memory`, `/prompt` |
| Runtime skills, goal and chat history | Skill files are home-wide, selection is per session; goals (2,000 characters) and history live in the saved session | `/skill status`, `/goal`, `/session list` |
| Loop counters and automatic continuation | Manual loops: current page. Auto loops run server-side when `/api/status` reports `server_loop`, outliving the tab | `/api/loop/status`; restart does not resume a run |
| Structured-memory store and gates | `structured_memory.*` in `config.yaml` (restart); slash overlays in `memory/structured_gates.json` once the store is open | `/memory`, `/prompt`; retrieval-off search is 409 |
| Web allowlist and cache; last extract and injected text | Shared policy/public cache; account-scoped selection with current permission checks | `/web`, `/prompt`; `/web forget` removes context |
| Coding repo, gates, budgets and planner | `config.yaml`; separate child execution and explicit approvals | `/github`, staged request and retained job/run results |
| Managed credentials | Startup rules above | `/api` reports presence/masked tail, not successful authentication |
| Spend ledger, completion webhooks | `logs/spend.jsonl` and `.1`; `notifications.*` plus optional bearer, restart-only, with a private durable outbox | [Spend and notifications](SPEND_AND_NOTIFICATIONS.md) |

Use [response style](CONSOLE.md#74-customize-response-style) before generation
parameters: `max_tokens` limits output, not context (chat zero is invalid), and
`temperature` changes sampling, not filler.

### Reload limits without a restart

Edit `config.yaml`, then choose **Reload limits** as a non-bootstrap administrator.
Authenticated, CSRF-guarded `POST /api/config/reload` accepts `{}`; operators,
auditors and auth-disabled homes cannot call it. Unix SIGHUP to the owned
**backend** PID (native sidecar, not the desktop parent) runs the same code.
Success returns `changed`, `limits.revision`, the applied snapshot and
`reloadable`; a no-op keeps the revision. New operations use the new snapshot;
running web fetch, research and chat-tool operations keep theirs. Rate-limit
hits, shared fetch capacity, locks and cancellation survive, so tightening a
ceiling can answer HTTP 429 at once; SIGHUP recovers a ceiling that blocks the
route itself. The route never writes YAML or changes permissions.

`src/server/config_reload.rs::RELOADABLE` lists the 28 keys; the shipped YAML
comments state each range, and startup shares the bounds:

| Group | Reloadable keys |
|---|---|
| API | `api.rate_limit.max_requests`, `.window_seconds`; `api.harness_loop_rate_limit.max_requests`, `.window_seconds`, `.max_tokens` |
| Loop | `loop.max_turns`, `max_total_completion_tokens`, `max_failures`, `cooldown_ms` |
| Web limits | `web.response_bytes`, `request_seconds`, `pages`, `run_bytes`, `run_seconds`, `per_site_pages`, `pace_ms`, `cache_pages`, `cache_bytes` |
| Web research | `web.subqueries`, `rounds`, `evidence_tokens`, `model_tokens`, `total_tokens`, `research_seconds`, `synthesis_seconds`, `stale_seconds`, `chat_tool_calls`, `carry_forward_sources` |

Both loop caps zero or absent sets `/api/status.loop.unbounded`; a transition
into unbounded warns and audits `loop_budgets_unlimited`
([loop guidance](CONSOLE.md#71-sessions-and-bounded-chat-continuation)).
`web.concurrency`, `web.goal_web_default`, `web.search*`, `web.firecrawl*` and
`web.max_rules` (a cap below the saved rule count refuses the policy) are
restart-only, as are TLS, authentication, credentials,
`security.allow_plaintext_key_file`, models, sandbox, coding policy, MCP
declarations, notifications and unknown keys.

An unreadable or [non-private](#home-and-optional-credentials) file, a symlink,
invalid UTF-8/YAML, more than 1 MiB, or a malformed number or limit returns
`CONFIG_RELOAD_INVALID`; a candidate that also changes anything outside the
allowlist returns `CONFIG_RESTART_REQUIRED`. Both keep every running limit.
Audit `config_reloaded` records source (`http`/`sighup`), revision and whether
anything changed; `config_reload_refused` records source and a coarse code,
never raw YAML or secrets. Refusal leaves the edited file on disk, where
coding write-policy revalidation still reads it. Smaller cache bounds govern
later operations without promising immediate eviction.

### Tunables you may want to know about

Every tunable lives in `assets/config.default.yaml` and is copied to `config.yaml`
on first run. Common operator settings:

| Key | Default | Effect |
|---|---|---|
| `api.rate_limit.max_requests` / `window_seconds` | 300 / 240 | Per-IP sliding-window ceiling on every route: 75 a minute sustained, above the 44 a console polls during an `/agent` job |
| `api.harness_loop_rate_limit.*` | 34 / 500 s / 4096 tokens | Separate `/loop` budget; same backend-dependent ceiling as local chat after the legacy `0` → `2048` fallback. A tuned model's loop reply never exceeds `models.local_llm.max_tokens`, so raise both together |
| `loop.max_turns` / `loop.max_total_completion_tokens` | 50 / 200000 | Per-run caps; zero or absent keys stay unlimited, with a startup and console warning when both are. [Reloadable](#reload-limits-without-a-restart) |
| `models.cloud_chat.enabled`, `max_tokens` / `timeout_sec` | true, 10000 / 360 | Allows `/model use grok` / `claude` (each provider has its own `enabled` and `model`); reply ceiling and timeout, where boot refuses 0 or >32768 tokens or a timeout outside 1–720 s |
| `models.local_llm.max_tokens` | 4096 | Local reply ceiling: 1–25904 with resolved Ollama and explicit reasoning `none`; otherwise 1–12952, reserved twice. Invalid combinations fail startup with `CONFIG_ERROR` |
| `models.local_llm.reasoning_effort` | `none` | Sent only to resolved Ollama: `none`, `low`, `medium`, `high`, `max`. Missing/empty omits the field and uses the doubled reservation |
| `models.local_llm.timeout_sec` | 1800 | Local fallback deadline, refused outside 1–3600; missing uses 720 s; auto-tune replaces it when a speed sample is available |
| `models.local_llm.auto_tune` | true | Times one generate per model: budgets drop only for a window below 32768, timeouts follow measured speed ([Models](MODELS.md#exact-model-diagnostics-and-optional-fallback)) |
| `chat.compact_prompt_tokens`, `compact_keep_messages`, `compact_tail_percent`; `compaction.summary_max_tokens` | 24000 / 10 / 25 / 2048 | Compaction trigger, recent messages kept (2–40), verbatim tail percent (10–50) and summary ceiling (128–2048; a home missing the key uses 768). `chat.compact_follow_window` (false) and `compact_max_prefill_sec` (1500) scale the trigger for a verified larger window ([compaction](CONSOLE.md#local-history-compaction)) |
| `auth.max_concurrent_operations` | 1 | Concurrent scrypt derivations (about 128 MiB each); range 1–4 |
| `auth.session.idle_timeout_sec` / `absolute_timeout_sec` | 43200 / 604800 | Session expiry without use (recorded at most once a minute), and regardless of use; each 60–2592000 (30 days), else startup stops. `password_change_*` twins (600 / 1800) apply while the setup code must still be replaced |
| `attachments.max_concurrent_uploads` / `body_timeout_sec` | 2 / 360 | Upload bodies (up to ~45 MiB each) buffered at once, 1–8, excess gets 503 `ATTACHMENT_BUSY`; deadline for one body, 5–600 s, expiry gives 408 `ATTACHMENT_TIMEOUT` |
| `web.evidence_tokens` | 6000 | Evidence budget when no tokenizer is available, refused outside 256–6000. Keep 3000 if the Ollama window is smaller than 32768 or unverified |
| `web.model_tokens` | 2048 | Maximum synthesis completion, refused outside 256–2048 |
| `web.total_tokens` | 28000 | Planning and synthesis budget, 2048–128000 (32000 without a larger measured window); sized for a **verified** 32768-token Ollama window. Keep 16000 if `/api/ps` shows a smaller `context_length`. A web turn just under the compaction trigger has `total_tokens` minus `chat.compact_prompt_tokens` (4000 as shipped) for its web results |

Quoted YAML strings such as `"true"` never arm a gate, and a quoted, empty or
non-finite number stops startup. Coding gates are in
[coding pipeline](CODING_PIPELINE.md), memory keys in
[STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md).

### Supported managed keys

`/api` displays configured-key presence. `/api set <KEY> <value>` saves one of
these exact keys; type a real value only in your private console. This is
not a general environment-variable editor or connector credential vault.

| Key | Purpose |
|---|---|
| `CGAGENTHARNESS_API_KEY` | Optional compatibility metadata; never grants account access |
| `CGAGENTHARNESS_WEBHOOK_TOKEN` | Legacy bearer for a single owner's completion webhooks. A second owner needs `data/notifications/bearers.json`. Restart after saving ([webhook configuration](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook)) |
| `GH_TOKEN` | GitHub CLI and its Git credential helper after restart; repository approvals still apply |
| `GROK_API_KEY` | Optional explicit Grok chat and governed cloud coding planner |
| `ANTHROPIC_API_KEY` | Optional explicit Claude chat and governed cloud coding planner |
| `DEEPAGENT_API_KEY` | Bearer credential for a configured non-Ollama compatible local planner |
| `SERPAPI_API_KEY` | Optional search API for chat web search; with `web.search.provider: auto` it is used when no Tavily key is saved, and public Google is the no-key fallback |
| `TAVILY_API_KEY` | Optional Tavily search, used first when `web.search.provider` is `auto` or `tavily` (the setting is restart-only; the key applies to the next search); snippets only, never page bodies |
| `FIRECRAWL_API_KEY` | Optional Firecrawl page reader, used only when `web.firecrawl.enabled` is literal `true` (the setting is restart-only; the key applies to the next read); re-reads a permitted exact URL the built-in fetch could not, so that URL leaves the machine |

Saving a key does not select chat, open coding gates, or install a connector.
After restart, `/model use grok` or `/model use claude` requests cloud consent;
coding still needs provider and per-run online approval. Values must be
nonempty, at most 4,096 characters, and contain no newline, carriage return, or
NUL. `/api clear <KEY>` removes only that OS-store credential. Migration retains
unknown `.env` lines. Restart after changes except `SERPAPI_API_KEY`,
`TAVILY_API_KEY` and `FIRECRAWL_API_KEY`.

Use `/api` for `CGAGENTHARNESS_API_KEY` and `DEEPAGENT_API_KEY`, and **API
Keys** for the others; its password fields clear on submit. The pane shows
masked **Saved** and **Active** values, their source (`startup_store`,
`saved_store`, `environment`, or `unset`; `startup_file` or `saved_file` under
the plaintext opt-in), and whether a restart is needed. Clearing a saved value
leaves an active value running until restart, and a process environment
override still wins afterwards. If the OS store is unavailable, save is
refused. Set `security.allow_plaintext_key_file` to literal `true` and restart
only to keep the private legacy file: every startup then logs a warning, shows
it in **API Keys** and the desktop handshake, and audits
`credential_plaintext_key_file_enabled`. Never source `.env` as a shell script
or put secrets in soul, memory, skill files or shared prompts.

### Close, quit and reopen

Closing the desktop window hides it while the backend runs; Cmd-Q offers
**Keep running** or **Cancel work and quit** during work ([desktop
recovery](DESKTOP.md#closing-quitting-and-recovery)). Ctrl-C or SIGTERM stops
`serve`. Nothing starts at login or resumes requests or approvals after reboot.
One server owns a home at a time; quit its owner before switching between app
and standalone use, and never delete a live lock file.

### Retained jobs and runs

Restart marks a running job interrupted without resuming it. Use `/agent jobs`
to list handles and `/agent job <id>` to inspect one; inspect `/agent status <id>`
before deciding or discarding a run. The harness never replays a commit, push,
publication or approval.

`data/agentic/console-schedules.json` stores schedules. Occurrences are consumed
before launch, and missed windows are skipped. Notifications do not replace job
records: the private bounded outbox persists delivery state before networking
and resumes eligible attempts after restart ([spend and notifications](SPEND_AND_NOTIFICATIONS.md)).

Retention limits, worker-lease reconciliation and cancellation limits:
[desktop recovery](DESKTOP.md#closing-quitting-and-recovery),
[process lifecycle](PROCESS_LIFECYCLE.md) and [the coding pipeline](CODING_PIPELINE.md).

### Update, backup, rollback and uninstall

Before upgrade, quit with Cmd-Q, wait for work to stop, and privately back up the
active home: it contains credentials, conversations, and run data. Keep the
prior app archive and source SHA.

Verify and replace the complete app bundle. Upgrade does not rewrite existing
defaults, model selection, persona, or skills; merge new fields instead of
replacing `config.yaml` with `assets/config.default.yaml`. An older binary may
not understand newer state, so restore its matching backup when rolling back.
To uninstall, quit and remove the app; home deletion is a separate destructive
action ([desktop uninstall](DESKTOP.md#update-recovery-and-uninstall)).
The scripts leave these behind, all optional to remove:

| Left by | Item | Remove with |
|---|---|---|
| `install-macos.sh --replace-app` | Previous app in `~/Library/Caches/CG-agent-harness-installer/previous` | `rm -rf` that directory |
| `start-macos.sh --trust-cert` | "CG Agent Harness local portal" certificate trusted in the login keychain | Keychain Access |
| `start-macos.sh`, `install-macos.sh --model` | `OLLAMA_CONTEXT_LENGTH` for GUI apps until logout; `~/Library/Logs/ollama-serve.log` | `launchctl unsetenv OLLAMA_CONTEXT_LENGTH`; `rm` the log |
| The harness | Home and its Keychain credentials | `scripts/reset-macos.sh` (dry run), then `--yes` after a backup |
| `init.sh` | "CG Agent Harness admin password" in the login keychain; marked lines in your shell profile; `scripts/logs/` | `reset-macos.sh --yes`; delete the lines under `# Added by CG-agent-harness`; `rm -rf scripts/logs` |

Homebrew, rustup, Ollama, models and `gh` are general tools; the installer
never removes them. A failed reset can leave the home with credentials already
removed: follow [reset failure and recovery](DESKTOP.md#reset-failure-and-recovery)
before retrying or reusing it.

To test a candidate without using your normal home, launch it with a new
absolute temporary home (adjust the app path):

```bash
candidate_home="$(mktemp -d /private/tmp/cgah-candidate.XXXXXX)"
open -n --env "CGAGENTHARNESS_HOME=$candidate_home" '/Applications/CG Agent Harness.app'
```

Confirm the home in Setup or the footer, and keep write gates disarmed unless
the test needs them. The candidate starts without your normal sessions, skills,
credentials, or soul; a later Finder launch uses the normal home. Do not change
`HOME`.

### Release cadence and finding updates

The app has no built-in updater. The [release
workflow](../.github/workflows/release.yml) checks changed `main` at **08:17 and
20:17 UTC** (runs may be delayed) and publishes Latest only for a SHA with a
successful Bundle run. Manual preview, `v*` tags and cadence changes are in
[releasing](RELEASING.md).

## 11. Verify your setup

For source builds and pipeline use, start with read-only inventory checks,
skipping tools you do not use:

```bash
command -v rustc cargo git gh ollama
rustc --version
ollama list
# After a one-token generate against the chosen tag (docs/MODELS.md):
curl --fail --silent --show-error http://127.0.0.1:11434/api/ps
gh auth status
git remote get-url origin
```

Check both configured model tags, the selected repository and write gates. The
hidden `agentic test` command checks some configuration and tools but is not a
complete doctor.

With the rustup-selected Cargo and Clippy from [Rust](#24-rust), run the
repository gates with application inference omitted:

```bash
test_home="$(mktemp -d)"
CGAGENTHARNESS_HOME="$test_home" SKIP_LIVE=1 scripts/verify-local.sh
```

The disposable home avoids the app's lock. The script runs formatting,
warning-denied Clippy, tests with planner keys blanked, CI contracts and a
`--locked` release build. Missing cargo-deny is a skip, not
a pass: run `cargo build --locked` and `cargo deny check` in the root and
`desktop/` with their pinned toolchains ([dependency maintenance](DEPENDENCIES.md)).
An outer sandbox can block nested Seatbelt; use the operator terminal without
weakening the profile.

For the live chat smoke, set the exact installed tag explicitly:

```bash
SMOKE_MODEL=qwen3.8:27b scripts/smoke-ollama.sh
# Local derived tag after ollama create. Not a fingerprint model.
SMOKE_MODEL=qwen3.8:27b-mlx-cg scripts/smoke-ollama.sh
```

Read the output: the smoke also succeeds when an unavailable endpoint causes a
skip, and it neither proves real-model acceptance nor tests editing. The
[coding pipeline](CODING_PIPELINE.md) covers edit, approval and publication
acceptance; for a source-built app, see
[rebuild and verify](DESKTOP.md#rebuild-and-verify).

For console interaction checks with installed Chrome and a Node runtime providing
WebSocket (the recorded local run used Node 24):

```bash
node scripts/chat-browser-acceptance.mjs
```

It uses an isolated profile and mock APIs (`CHROME_BIN` selects another Chrome)
and checks the console and CSP, not model behavior or WKWebView. Native window,
chooser, clipboard and quit behavior need manual checks recorded in the PR body.

## Tests and CI/CD

Use [focused local checks](#11-verify-your-setup) for changed behavior; CI runs
all targets with provider keys blanked and deterministic model fixtures. Set
`CGAH_TEST_BINARY` to test another backend, including a bundle copy.

| Gate | Evidence and scope |
|---|---|
| [Backend CI](../.github/workflows/ci.yml) | Rust 1.89 (`rust-version`), rustfmt, warning-denied Clippy, `cargo deny`, I6, Chrome, Linux/macOS tests and real-repo smoke. Code PRs run focused Windows lifecycle and credential tests; `main`, merge queue and release calls also run the full Windows suite. Linux uses Ubuntu 26.04 (bubblewrap and MCP lifecycle: 24.04). |
| Native Cargo acceptance | macOS tests prepare locked dependencies, then test Seatbelt and fixed Cargo checks. Linux uses bubblewrap when present; `unshare --net` and Windows Job Object do not provide equivalent confinement. |
| [Desktop CI](../.github/workflows/desktop.yml) | Bundle, releases, and manual runs build the universal app on Rust 1.90, test desktop policy and the packaged backend, and verify signatures, checksums and extraction. |
| [Desktop check](../.github/workflows/desktop-check.yml) | On PRs and the merge queue, only when `desktop/`, `src/common/local_tls.rs` or `scripts/start-web.sh` changed: desktop fmt, Clippy, tests and `cargo deny`, no packaging. Not required. |
| [Bundle](../.github/workflows/bundle.yml) | On pushes to `main` only: packages `linux-x86_64` and `macos-arm64` CLIs beside desktop, re-verifies each checksum, then tests loopback binding and exit-3 missing configuration. The Linux CLI needs glibc 2.39+ (Ubuntu 24.04+). |
| Workflow and source checks | CodeQL, DevSkim, Gitleaks, PR-template/base-branch checks, the advisory review gate and a daily RustSec scan run separately; workflow changes trigger actionlint/zizmor. Optional `live-ollama.yml` fingerprints a pinned real Ollama runtime ([workflow index](../.github/workflows/README.md)). |
| [Netconnect CI](../.github/workflows/netconnect.yml) | Path-filtered flag, scope, collector, and untrusted-string tests on Linux and macOS, plus a Linux `strace` proof that default `netconnect status` opens no packet socket. |
| [Release](../.github/workflows/release.yml) | Twice daily, publishes the next patch only when `main` differs and its SHA has a successful Bundle; backend CI, packaging and checksums still gate it ([release controls](RELEASING.md)). |

Passing CI does not prove real-model quality, native GUI behavior, or
notarized distribution. Windows release legs remain parked.
