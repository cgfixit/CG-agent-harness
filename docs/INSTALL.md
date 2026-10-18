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
(only for Ollama or `gh`); rustup with the pinned Rust; Ollama; optional `gh`. It then builds the standalone backend and downloads the latest
release app, checks it against that release's `SHA256SUMS` and `codesign`, and
copies it to `/Applications` (`~/Applications` when that is not writable). It never
touches the harness home (`~/.CGagentHarness`) and pulls a model only with `--model`.

| Option | Effect |
|---|---|
| `--check` | Lists what is present; exits 1 when something required is missing |
| `--dry-run` | Prints every command, changing nothing |
| `--replace-app` | Replaces a working app; keeps the previous one in `~/Library/Caches/CG-agent-harness-installer/previous`. An incomplete or unsigned copy is replaced without it |
| `--no-ollama`, `--no-build`, `--no-app`, `--app-dir DIR` | Skip a step or choose the app location; outside `/Applications` and `~/Applications`, start with `CGAH_APP`; after `--no-app`, with `--standalone` |

The checksum proves integrity, not authorship (both files share one release
page). `curl` downloads skip Gatekeeper's quarantine prompt; compare
`Resources/COMMIT` with the release notes ([5.2](#52-macos-app)). Quit the app
(Cmd-Q) before `--replace-app`.

[`start-macos.sh`](../scripts/start-macos.sh) is the everyday launcher, described
in [section 7](#7-open-the-console-in-a-browser).

### One command: `init.sh`

On an Apple Silicon Mac, [`scripts/init.sh`](../scripts/init.sh) runs the steps
below in order from a fresh clone or download, from zsh or bash:

```bash
cd CG-agent-harness && scripts/init.sh    # or: bash scripts/init.sh
```

Or double-click **init.command** in the clone's top folder in Finder: it runs the
same script in a Terminal window and waits for Return at the end. A git clone opens
directly. A folder unpacked from a downloaded ZIP is quarantined: Control-click
the file and choose **Open** (macOS 15 and later: **Open Anyway** in System Settings
→ Privacy & Security), or run `xattr -dr com.apple.quarantine <folder>` once.

It waits for the Command Line Tools dialog, runs `install-macos.sh` (skipped when
`--check` passes), adds Homebrew and Rust to your login shell's profile
(`~/.zprofile` for zsh, `~/.bash_profile` or `~/.profile` for bash), replaces
admin's one-time setup code (it reads the code itself; in the app, **Cmd and +**
shows it) with a new password and saves that in your login keychain as **CG Agent Harness
admin password**, and grants the web allowlist. It then asks what to open:
**1** the app, **2** the console in your browser (served from that Terminal until
Ctrl-C, or the open app's console), **3** both, **0** nothing. A failed step offers retry,
skip or quit. A rerun skips finished steps and signs in with the saved password.

| Option | Effect |
|---|---|
| `--yes` | Asks nothing; generates the admin password (Keychain only) |
| `--choice 0`–`3` | What opens at the end |
| `--model TAG`, `--with-gh`, `--no-ollama`, `--app-dir DIR` | Passed to `install-macos.sh` |
| `--replace-app` | Passed on too, and runs the installer even when `--check` passes: replaces an installed app with the latest release (quit it first) |
| `--trust-cert`, `--no-profile`, `--no-allowlist` | Trust the console certificate (choices 2 and 3 with the app); skip the profile or allowlist |

Each line is logged with a time to `scripts/logs/install.log` (private, ignored by
git); passwords and setup codes never are. Read the saved password with
`security find-generic-password -a admin -s 'CGagentHarness admin (<home>)' -w`.

### Step by step in Terminal

Open **Terminal** (Applications → Utilities); on Apple Silicon leave its **Open
using Rosetta** unchecked. Run every command from the clone, as your user.

1. **Developer tools:** `xcode-select --install`, then finish the dialog. A rerun
   only reports that they are installed.
2. **Code:** `git clone https://github.com/cgfixit/CG-agent-harness.git && cd CG-agent-harness`.
3. **Inventory:** `scripts/install-macos.sh --check` prints one line per tool and
   exits 1 while something required is missing. It changes nothing.
4. **Preview:** `scripts/install-macos.sh --dry-run --model <tag>` prints every
   command the install would run. Pick the tag in [Models](MODELS.md); omit
   `--model` to pull later (models are multi-GB). Add `--with-gh` for coding.
5. **Install:** the same command without `--dry-run`. The Homebrew and rustup
   installers may ask for your password and print "Next steps"; run those lines,
   then open a new Terminal window so `brew` and `cargo` are on `PATH`. The first
   backend build takes several minutes.
6. **Confirm:** `scripts/install-macos.sh --check` ends with
   `everything required is present`.
7. **Start:** `scripts/start-macos.sh --trust-cert` starts Ollama with a
   32768-token window, opens the app, and opens its console in your browser.
   `--trust-cert` asks for your password once so the browser trusts this home's
   certificate. Note the printed `CGagentHarness console: https://127.0.0.1:<port>/`.
8. **Sign in:** user `admin` with the one-time setup code. To find it, put the
   app window or the browser's console (`harness.html`) in focus and press
   **Cmd and +** (technically Cmd, Shift and =). The app opens **Harness → Setup
   and recovery**, which shows the code; the browser's sign-in card says where
   the code is (the `setup-code` file in the harness home, read with
   `cat ~/.CGagentHarness/setup-code`). Then choose a new password of at least 12
   characters. Until you do, that session can only change its password or log
   out: closing the password dialog leaves a lock card, and the CLI and API
   answer `AUTH_PASSWORD_CHANGE_REQUIRED`. The browser needs its own sign-in.
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

If `start-macos.sh` reports no usable app, rerun step 5. If it reports the home is
already owned, quit that owner or use app mode. More fixes:
[Troubleshooting](TROUBLESHOOTING.md).

## 2. Prerequisites

Install only what your path needs. Preserve working tools and models. These are
the manual steps that [`install-macos.sh`](#1-quick-start-automated-macos-install)
automates.

### 2.1 Hardware and macOS

```bash
sw_vers
uname -m
```

The universal app includes `arm64` and `x86_64`. `uname -m` reports
`arm64` in a native Apple Silicon shell; `x86_64` can mean Intel hardware or a
translated shell. Check **About This Mac** to distinguish them. The supported
packaging recipe below runs on Apple Silicon and cross-builds Intel.

The bundle targets macOS 12+, but that target does not prove acceptance on every
older OS or Intel Mac. See [desktop details](DESKTOP.md) for tested hardware and revisions.

In Finder, press Cmd-I on the app. **Kind** must say Universal. Leave **Open using
Rosetta** unchecked on Apple Silicon; both app and backend have native slices.
If an Intel warning persists, verify Finder or Dock opened the new copy and check
separate Intel-only tools. See Apple's [Rosetta guidance](https://support.apple.com/en-us/102527).

### 2.2 Xcode Command Line Tools

Source builds and native Cargo checks need Apple's Command Line Tools, not the
full Xcode app:

```bash
xcode-select --install
```

Complete the install dialog. If they are present, the command reports that and
changes nothing.

### 2.3 Optional package installation

Homebrew is optional. Preserve a working installation; otherwise follow its
[installation instructions](https://brew.sh) and confirm `brew --version` in a
new terminal. Model runtimes can use vendor installers.

### 2.4 Rust

Inspect the installation on PATH before changing it:

```bash
command -v rustc cargo rustup
rustc --version
cargo --version
rustc --print sysroot
```

Preserve a working installation. The repository pins **Rust 1.88 for the
backend** in `rust-toolchain.toml` and **Rust 1.90 for desktop** in
`desktop/rust-toolchain.toml`. Use rustup's Cargo on PATH so those files take
effect; a Homebrew Cargo executable does not enforce them. An unrelated newer
Clippy version can report different lints.

Prepare backend components with network access:

```bash
rustup toolchain install 1.88 --profile minimal --component clippy --component rustfmt
```

For desktop, prepare:

```bash
rustup toolchain install 1.90 --profile minimal --component clippy --component rustfmt
```

Without Rust, follow the [installation instructions](https://www.rust-lang.org/tools/install).
From the repository root, `rustup show active-toolchain` should resolve to 1.88;
inside `desktop/`, it should resolve to 1.90. The packaging script disables
implicit toolchain installation, so install both before running it.

Download toolchains and dependencies during preparation. Generated-code checks
must not invoke rustup or use credentials from your real home.
See [Offline Cargo verification](CODING_PIPELINE.md#offline-cargo-verification).

### 2.5 Ollama

[Ollama](https://ollama.com) runs the local chat and planning model. First run
`command -v ollama` and `ollama --version`; preserve a working installation. On
a fresh machine, Homebrew can install the app:

```bash
brew install --cask ollama
```

Launch Ollama, then verify `ollama list` and the endpoint in [model setup](MODELS.md).
If the CLI is unavailable, follow the
[Ollama quickstart](https://docs.ollama.com/quickstart). The server never installs
Ollama or downloads a model; `install-macos.sh` installs Ollama unless
`--no-ollama` and pulls a model only with `--model`.

Seeded `web.total_tokens` 28000 and `web.evidence_tokens` 6000 assume
`OLLAMA_CONTEXT_LENGTH=32768` is set **before** the Ollama process starts.
[Model setup](MODELS.md) is the set-and-verify step. The harness sends no `num_ctx`.
A Modelfile `num_ctx` beats that variable; pin 32768 with the
[local derived tag](MODELS.md#pin-32768-with-a-local-derived-tag). The shipped
default stays `qwen3.8:27b-mlx`.

### 2.6 (Optional, for the coding pipeline only) GitHub CLI

The optional [coding pipeline](CODING_PIPELINE.md) requires GitHub CLI. Chat does
not.

```bash
brew install gh
```

## 3. Get the code

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
```

Inspect an existing checkout before updating:

```bash
git status --short
git fetch origin
git log -1 --oneline origin/main
```

Preserve uncommitted work. To update an existing clean local `main`:

```bash
git switch main
git merge --ff-only origin/main
git log -1 --oneline HEAD
```

Inspect divergence instead of resetting it. For contributions, create a
`codex/<topic>` or other driver branch from updated `main`; PRs target `main`.
Source-build commands assume the `CG-agent-harness` directory.

## 5. Build or install

### 5.1 Standalone backend

From inside the `CG-agent-harness` folder:

```bash
cargo build --release --locked
```

The first build compiles dependencies. Prepare the pinned toolchain in
[Rust](#24-rust). Cargo caches dependencies for later builds. The binary is at:

```
target/release/cgagentharness
```

### 5.2 macOS app

Choose a **published release** for its documented source version or a successful
**main artifact** for newer merged features. Bundle runs only after pushes to
`main`; PRs do not produce Bundle artifacts:

| Source | What to download | What it establishes |
|---|---|---|
| [Latest release](https://github.com/cgfixit/CG-agent-harness/releases/latest) | `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` | Published release source identified in its notes |
| Successful [Bundle run](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml) on `main` | `cg-agent-harness-macos-universal` Actions artifact | Merged source at that run's commit; may be newer than the published release |
| Successful [desktop workflow](https://github.com/cgfixit/CG-agent-harness/actions/workflows/desktop.yml) | `cg-agent-harness-macos-universal` for a standalone dispatch | Desktop build checks for its selected source |

The macOS `cgagentharness-macos-arm64` CLI archive is a different deliverable;
it does not contain the native desktop app. Actions downloads may require GitHub
login and expire after 14 days. Release assets are separate from that retention.
Always inspect the source SHA and workflow conclusion, not only the run's date.

For an Actions artifact, extract the outer download first. In the directory
containing the **inner** app ZIP and checksum file, run:

```bash
shasum -a 256 -c SHA256SUMS
ditto -x -k CG-Agent-Harness-macos-universal.zip .
cat 'CG Agent Harness.app/Contents/Resources/COMMIT'
```

If the checksum fails, stop and obtain the matching archive/checksum pair again.
Do not install a modified archive. The embedded commit should match the selected
release or workflow source.

To build your own universal app from a clean committed checkout, install both
pinned toolchains ([Rust](#24-rust)), then both target standard libraries:

```bash
rustup target add --toolchain 1.88 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal --dmg
```

`--dmg` is optional. Outputs in `dist/` are `CG Agent Harness.app`,
`CG-Agent-Harness-macos-universal.zip`, an optional DMG, and `SHA256SUMS`.
Omitting `--universal` creates an arm64 development bundle. Packaging refuses a
dirty checkout by default; commit reviewed source first so `Resources/COMMIT`
identifies it. Do not use a dirty-build override for an ordinary release.
The script signs the combined backend before the shell embeds its hash; replacing
or re-signing only the backend afterward breaks the ownership check.

Quit any existing copy with **Cmd-Q**, then unzip the app and move the complete
bundle to Applications or a directory you own. Open it from Finder or the Dock.
The app owns its backend on a dynamically selected loopback port; it does not
adopt a server already running on port 8790. No login service is installed.

The current app is **ad-hoc signed and not notarized**. Gatekeeper may require
per-app approval under macOS Privacy & Security, or a managed policy may refuse
it. After verifying the source and attempting to open it, follow Apple's
[per-app Open Anyway procedure](https://support.apple.com/en-us/102445) if offered.
Do not disable global Gatekeeper settings or strip quarantine as a blanket fix.
A damaged-bundle warning calls for re-download and integrity checks. Signature verification and a
successful build do not prove native interaction acceptance; see
[desktop details](DESKTOP.md).

## 6. First run

Fresh homes enable login, role permissions, HTTPS and web fetch/search/research
without a harness API key or configuration edit. The content allowlist starts
empty; administrators grant sources through `/web allow`, or grant a starter list
of technical references with
[`web-allowlist-bootstrap.sh`](../scripts/web-allowlist-bootstrap.sh)
([step 9](#step-by-step-in-terminal)). Open the app from
Finder, or start the standalone server:

```bash
./target/release/cgagentharness serve
```

Upgrades keep existing settings; to adopt the boundaries, merge
`auth.enabled: true` and `tls.enabled: true` into the active configuration.
A fresh home's `admin` signs in once with the code in private
`<home>/setup-code` (the app's **Cmd and +** window and `start-web.sh` show it),
then must replace the password. No MCP machine key can be minted for an account
that has not. See
[TLS trust, migration and recovery](SECURE_RESEARCH.md).

The standalone server prints its address, for example:

```
CGagentHarness console on https://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

First run seeds the application home. Chat uses the configured loopback service's
`qwen3.8:27b-mlx` tag by default. Inspect installed tags with `/model list` and
select one with `/model use <exact-tag>`. The harness never downloads missing tags.

To change the endpoint or configure the coding planner, quit with Cmd-Q or stop
`serve` with Ctrl-C. Merge the exact installed tag into the active home's
`config.yaml` without duplicating mappings:

```yaml
models:
  local_llm:
    model: "qwen3.8:27b-mlx"   # chat default; use the exact installed tag
agentic:
  deepagent_github:
    model: "qwen3.8:27b-mlx"   # optional planner; writes remain disarmed
```

Restart only after editing configuration; `/model use` applies to chat without
a restart and does not change the coding planner. Existing homes are not
overwritten with new configuration defaults; review new fields when upgrading.
Fresh homes seed the bundled default `soul.md`; [soul](CONSOLE.md#72-default-soul-effective-prompt-and-persona-editing) explains editing
and existing-home behavior.

In the app, the console opens automatically. **Harness → Setup and recovery**
(**Cmd and +**, that is Cmd, Shift and =) shows the setup code and its owned endpoint and model/tool diagnostics. Use **Check installed
chat and planner models** to check configured tag availability without downloading.
For the standalone server, leave its terminal open and visit:

```
https://127.0.0.1:8790/
```

The app verifies its owned certificate. External browsers require
operator-controlled trust or an operator certificate trusted by the browser;
see [secure setup](SECURE_RESEARCH.md). After login and password replacement,
leave the optional metadata key empty and send a message. Replies
[stream](CONSOLE.md#streaming-chat-and-cancellation); `/loop stop` aborts the turn.
Time to first token depends on the model.

If replies stall, check that [Ollama](MODELS.md) is available at its configured endpoint.

### Memory setup

Run `/memory` to inspect gates. Fresh homes enable capture, suggestions and
retrieval; upgrades preserve settings. To open a disabled store, set `enabled: true`
under `structured_memory` in `config.yaml` and restart. Administrators can then
enable `/memory capture on`, `/memory auto-suggest-chat on` and
`/memory auto-suggest-coding on`.

Select an installed local tag with `/model use <exact-tag>`, including 9B models.
Under `structured_memory`, set `suggestion_mode: summaries` for summaries or `both`
for summaries and useful details; restart after editing. Empty results can be valid.

After completed work, open **Memory** for pending proposals. Review each and enter
a reason to save, update, deactivate or dismiss; generation never changes saved
facts. **Suggestion activity** explains empty, skipped, failed or interrupted work.
Restart preserves proposals, but queued inputs are lost.

`/memory search <query>` only finds facts; `/memory retrieve <query>` includes
matches in that chat. Automatic retrieval separately requires `retrieval` and
`auto_retrieval`; inspect `/prompt`. See [Memory](MEMORY_GUIDE.md) for gates and recovery.

## 7. Open the console in a browser

A browser can use the same server the app owns. This is a supported path, not
a workaround: the app's backend serves the same guarded console on its loopback
port, and a browser gets its own login. Three ways:

| Way | What runs |
|---|---|
| Globe beside **CG Agent / harness** (apps built after `v0.1.48`) | After native confirmation, the bundled `start-web.sh --desktop-url` opens the app's exact origin |
| `scripts/start-macos.sh` | Same `--desktop-url` hand-off from a clone; works with any app version |
| `scripts/start-macos.sh --standalone [port]` | No app: `start-web.sh` builds if needed and runs `serve` on 8790 |

`start-macos.sh` first starts Ollama if it is not answering: the app, or a
CLI-only install's `ollama serve` (log `~/Library/Logs/ollama-serve.log`). It sets
`OLLAMA_CONTEXT_LENGTH=32768` when unset (the app's `launchctl` value is
lost at logout, so rerun the script after login; restart an Ollama started without it). Ollama's first launch asks you to confirm an
app downloaded from the internet; the script waits 30 seconds.
It opens the app if no app backend is running,
reads the loopback port that backend listens on, and passes that origin to
`start-web.sh --desktop-url`. It starts no second server; keep the app running.

Running plain `scripts/start-web.sh` while the app is open fails by design: one
server owns a home, and the second `serve` stops at the home lock.
`--standalone` refuses early when another process holds the target home's lock.

Browsers do not trust the home's generated certificate. `start-macos.sh` reports
whether macOS trusts it; `--trust-cert` exports the public certificate of the
home the app's backend holds and adds it to your login keychain for TLS after a password
prompt. That certificate is a leaf for `localhost`, `127.0.0.1` and `::1`, not
a CA. A renewed certificate needs `--trust-cert` again; remove the old one in
Keychain Access. Firefox keeps its own store. Never click through a certificate warning;
see [HTTPS and native trust](SECURE_RESEARCH.md#https-and-native-trust).

## 8. Persistence, optional keys and recovery

### Home and optional credentials

Both launch paths default to `~/.CGagentHarness`. Set `CGAGENTHARNESS_HOME` in
the launching process to override it; desktop requires an absolute path and does
not inherit shell exports when Finder launches it. Setup shows the active home.
Uninstalling/replacing the bundle preserves this external data.

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


Fresh homes require HTTPS and login; metadata keys grant no account authority.
**API Keys** manages credentials and distinguishes saved/active masks. Startup
loads Keychain, Secret Service or Credential Manager; Linux builds need
`pkg-config` and `libdbus-1-dev`. Inherited values override stored values, including
after clearing a saved key. Legacy `.env` migrates unless
`security.allow_plaintext_key_file` is literal true. Restart to reload.

`config.yaml` must be private at startup, reload and policy rechecks (1 MiB
UTF-8 maximum): Unix owner, one link and no group/other permissions, no extended
ACLs on macOS, and the [credential reader's](../INVARIANTS.md#provider-keys-live-in-the-os-credential-store)
owner/DACL checks on Windows ([contract](../INVARIANTS.md#reload-changes-limits-not-authority)).
Repair Unix permissions with `chmod 600` and remove extended ACLs; refusals never
repair files.

Only local/fallback `api_key`, MCP `env` values and complete SSE `url` accept
whole-value `env:NAME`, e.g. `api_key: env:LOCAL_MODEL_TOKEN`, resolved once at
startup. Arbitrary names must be inherited, not saved through API Keys. MCP
refuses reserved sources; see [declarations](MCP_CLIENT.md).

Forwarded or proxy requests are unsupported. See [secure setup](SECURE_RESEARCH.md).

### Which settings take effect where?

Edit the **active home's** `config.yaml`. Merge existing mappings; duplicate YAML
keys are invalid. Only literal `true` enables gates.
Use **Reload limits** or Unix SIGHUP for [reloadable limits](CONFIG_RELOAD.md).
Other settings require quit/relaunch or `serve` restart; closing the window only
hides it. Coding-policy checks reread disk at mutation boundaries.

| Setting | Scope / persistence | How to verify |
|---|---|---|
| Model endpoint, provider, timeout, temperature and token ceiling | `config.yaml`, home-wide; restart after edits. Local context is the Ollama server window (32768 via `OLLAMA_CONTEXT_LENGTH`, verified in [model setup](MODELS.md)), not a harness `num_ctx`. `max_tokens` is still the output ceiling. | `/model`, Setup diagnostics, `/api/ps` `context_length`, and an actual reply |
| `/model use <name>` | Persisted console model selection; subsequent requests | `/model`; it can differ from the seeded YAML tag |
| Persona and soul toggle | `soul.md` plus `harness.json`, shared across sessions; next request | `/soul status`, `/prompt` |
| Runtime skill file | `<home>/skills/<id>/SKILL.md`; selection is per session | `/skill status` and next `/prompt`; last successful snapshot may be older |
| Goal and chat history | Saved session; goal maximum 2,000 characters | `/goal`, `/session list`, `/goal task` for coding linkage |
| Loop counters and automatic continuation | Current page only; not an unattended scheduler | Visible loop state; restart does not resume it |
| Memory notes and inclusion | Home-wide saved notes/toggle; next request | `/memory`, `/prompt` |
| Structured-memory store and gates | `structured_memory.*` in `config.yaml` (restart); slash overlays in `memory/structured_gates.json` once the store is open | `/memory`, `/memory search`, `/memory consolidate`, `/memory auto-consolidate`, `/prompt`; retrieval-off search is 409 |
| Web allowlist and cache; last extract and injected text | Shared policy/public cache; account-scoped selection with current permission checks | `/web`, `/prompt`; use `forget` to remove context |
| Coding repo, gates, budgets and planner | `config.yaml`; separate child execution and explicit approvals | `/github`, staged request and retained job/run results |
| Managed credentials | Startup rules above | `/api` reports presence/masked tail, not successful authentication |
| Spend ledger and dashboard | Retained `logs/spend.jsonl` and `.1`; dashboard state is page memory only | [Completeness and pricing](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero) |
| Completion webhooks | `notifications.*` plus optional bearer; restart-only; private durable outbox with bounded owner replay | [Setup, destination rules and delivery audit](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook) |
| Optional `unslop` | `config.yaml`; local coding planner only | Coding metrics, not chat phrasing |

Use [response style](CONSOLE.md#74-customize-response-style) before generation
parameters: `max_tokens` limits output, not context (chat zero is invalid), and
`temperature` changes sampling, not filler filtering.

### Tunables you may want to know about

Every tunable lives in `assets/config.default.yaml` and is copied to `config.yaml`
on first run. Common operator settings follow:

| Key | Default | Effect |
|---|---|---|
| `app.agent_job_poll_ms` | 1500 | Browser job polling interval, clamped to 1–30 s |
| `api.rate_limit.max_requests` / `window_seconds` | 60 / 60 | Per-IP sliding-window ceiling on every route |
| `api.harness_loop_rate_limit.*` | 8 / 300 s / 2048 tokens | Separate `/loop` budget; same backend-dependent ceiling as local chat after the legacy `0` → `2048` fallback |
| `models.cloud_chat.enabled` | true | Allows `/model use grok` / `claude`; each provider has its own `enabled` and `model` |
| `models.cloud_chat.max_tokens` / `timeout_sec` | 4096 / 90 | Cloud reply ceiling and timeout; boot refuses 0 or >32768 tokens, or a timeout outside 1–720 s |
| `models.local_llm.max_tokens` | 4096 | Local reply ceiling: 1–25904 with resolved Ollama and explicit reasoning `none`; otherwise 1–12952, with twice the ceiling reserved for completion. Invalid combinations fail startup with `CONFIG_ERROR` |
| `models.local_llm.reasoning_effort` | `none` | Sent only to resolved Ollama: `none`, `low`, `medium`, `high`, `max`. Missing/empty omits the field and uses the doubled reservation |
| `chat.compact_prompt_tokens` / `compact_keep_messages` | 24000 / 8 | Calibrated next-prompt trigger and recent messages retained (2–40); the prompt cap is the hard limit past it. Threshold floor/cap and web interaction: [compaction](CONSOLE.md#local-history-compaction) |
| `chat.compact_tail_percent` | 25 | Verbatim tail budget as a percent of the trigger, 10–50; whole exchanges, at least the last one |
| `compaction.summary_max_tokens` | 768 | Summary output ceiling, clamped 128–2048; missing in an existing home also uses 768; sent doubled where the reply is reserved twice |
| `chat.compact_follow_window` / `compact_max_prefill_sec` | true / 120 | A verified window above 32768 scales the trigger up, within this many seconds of measured prefill (30–1800); literal false keeps it fixed |
| `models.local_llm.inventory.*` | 2.0 s / 262144 bytes | Model-list probe timeout and response cap |
| `auth.max_concurrent_operations` | 2 | Concurrent scrypt derivations (about 128 MiB each); range 1–4 |
| `auth.session.idle_timeout_sec` / `absolute_timeout_sec` | 43200 / 604800 | Session expiry without use (recorded at most once a minute), and regardless of use |
| `auth.session.password_change_idle_timeout_sec` / `password_change_absolute_timeout_sec` | 600 / 1800 | The same limits while the account must still replace its password (setup code); capped at the normal ones |
| `attachments.max_concurrent_uploads` | 2 | Attachment upload bodies (up to ~45 MiB each) buffered at once; range 1–8, excess requests get 503 `ATTACHMENT_BUSY` |
| `attachments.body_timeout_sec` | 120 | Deadline for receiving one upload body; range 5–600, expiry gives 408 `ATTACHMENT_TIMEOUT` and frees the permit |
| `structured_memory.*` | see file | Per-owner caps (facts, proposals, episodes, bytes), search/retrieval limits, suggestion queue and consolidation thresholds |
| `web.evidence_tokens` | 6000 | Evidence budget when no tokenizer is available, clamped 256–6000. Keep 3000 if the Ollama window is smaller than 32768 or unverified |
| `web.model_tokens` | 1024 | Maximum synthesis completion, clamped 256–2048 |
| `web.total_tokens` | 28000 | Planning and synthesis budget, 2048–128000 (32000 without a larger measured window); sized for a **verified** 32768-token Ollama window ([model setup](MODELS.md)). Keep 16000 if `/api/ps` shows a smaller `context_length` |

Upgrades preserve `config.yaml`; missing settings use documented fallbacks.
Explicit settings win. Restart after edits.

Quoted YAML strings such as `"true"` never arm a gate. See [coding pipeline](CODING_PIPELINE.md) for the
coding-pipeline gates and [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md)
for every memory key.

### Supported managed keys

`/api` displays configured-key presence. `/api set <KEY> <value>` saves one of
these exact keys; substitute a real value only in your private console. This is
not a general environment-variable editor or connector credential vault.

| Key | Purpose |
|---|---|
| `CGAGENTHARNESS_API_KEY` | Optional compatibility metadata; never grants account access |
| `CGAGENTHARNESS_WEBHOOK_TOKEN` | Legacy bearer for a single owner's completion webhooks. A second owner needs `data/notifications/bearers.json`. Restart after saving. See [webhook configuration](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook) |
| `GH_TOKEN` | GitHub CLI and its Git credential helper after restart; repository approvals still apply |
| `GROK_API_KEY` | Optional explicit Grok chat and governed cloud coding planner |
| `ANTHROPIC_API_KEY` | Optional explicit Claude chat and governed cloud coding planner |
| `DEEPAGENT_API_KEY` | Bearer credential for a configured non-Ollama compatible local planner |
| `SERPAPI_API_KEY` | Optional fixed search API for chat web search; public Google is the no-key fallback |

Saving a key does not select chat, open coding gates, or install a connector.
After restart, `/model use grok` or `/model use claude` selects cloud chat;
coding still needs provider and per-run online approval. Review provider controls
in [the configuration reference](../assets/config.default.yaml). Values must be
nonempty, at most 4,096 characters, and contain no newline, carriage return, or
NUL. `/api clear <KEY>` removes only that OS-store credential. Migration retains
unknown `.env` lines. Restart after changes except `SERPAPI_API_KEY`.

Prefer the **API Keys** pane for pasting secrets; its password inputs are cleared
after successful save. Use **Clear saved value** to remove a stored credential.
The pane displays masked **Saved** and **Active** values separately, their source
(`startup_store`, `saved_store`, `environment`, or `unset`; `startup_file` or
`saved_file` when the plaintext opt-in is on), and whether a restart is needed.
Clearing a saved value leaves a currently active value running until restart;
an explicit process environment override still wins after restart. If the OS
store is unavailable, save is refused. Set `security.allow_plaintext_key_file`
to literal `true` and restart only to keep the private legacy file. `/registry`
and **View registry** preserve inventory access. Do not source `.env` as a shell
script or put secrets in soul, memory, skill files or shared prompts.

### Close, quit and reopen

Closing the desktop window hides it while the backend runs. Dock reopen or a
second launch focuses the same instance. During work, Cmd-Q offers **Keep
running** or **Cancel work and quit**. Ctrl-C or SIGTERM stops `serve`. Neither
path installs login startup or resumes requests or approvals after reboot.

One server owns a home at a time. Quit its owner before switching between app
and standalone use; do not delete a live lock file. Different homes can run
independently. This lock does not cover manually invoked agentic CLI writers.

### Retained jobs and runs

`data/agentic/console-jobs.json` retains up to 32 terminal jobs plus one active
job within 16 MiB. Restart marks a running job interrupted without resuming it.
Use `/agent jobs` to list handles and `/agent job <id>` to inspect one.

`data/agentic/console-schedules.json` stores schedules. Occurrences are consumed
before launch, and missed windows are skipped. See
[spend and notifications](SPEND_AND_NOTIFICATIONS.md).
Notifications do not replace job records. The private bounded outbox persists
delivery state before networking, resumes eligible attempts after restart, and
supports explicit owner-confirmed replay within current grants and limits.

`/agent runs` shows durable run records. On Unix, a released worker lease allows
reconciliation to interrupted; a live lease remains running. Legacy records
without ownership evidence remain unknown. Inspect `/agent status <id>` before
deciding or discarding. The Harness never replays a commit, push, publication,
or approval. Cancellation is best-effort; inspect escaped descendants before
further writes.

Audit/spend/optional metrics JSONL logs retain a current file and one previous
`.1` generation, default 8 MiB each. Logging remains best-effort; audit lines use
a [bounded background writer](../INVARIANTS.md#secrets-never-reach-a-response-or-a-log-line).
See
[the coding pipeline](CODING_PIPELINE.md), [desktop recovery](DESKTOP.md) and
[process lifecycle](PROCESS_LIFECYCLE.md) for precise limits.

### Update, backup, rollback and uninstall

Before upgrade, quit with Cmd-Q, wait for work to stop, and privately back up the
active home. It contains credentials, conversations, and run data. Retain the
prior app archive and source SHA for comparisons.

Verify and replace the complete app bundle. Upgrade does not rewrite existing
defaults, model selection, persona, or skills. Merge new fields instead of
replacing `config.yaml` with `assets/config.default.yaml`. An older binary may
not understand newer state, so inspect compatibility and restore its matching
backup. Never delete a live owner's lock. To uninstall, quit and remove the app;
home deletion is a separate destructive action ([desktop uninstall](DESKTOP.md#update-recovery-and-uninstall)).
What the scripts leave behind, all optional to remove:

| Left by | Item | Remove with |
|---|---|---|
| `install-macos.sh --replace-app` | Previous app in `~/Library/Caches/CG-agent-harness-installer/previous` | `rm -rf` that directory |
| `start-macos.sh --trust-cert` | "CG Agent Harness local portal" certificate trusted in the login keychain | Keychain Access |
| `start-macos.sh` | `OLLAMA_CONTEXT_LENGTH` for GUI apps until logout; `~/Library/Logs/ollama-serve.log` | `launchctl unsetenv OLLAMA_CONTEXT_LENGTH`; `rm` the log |
| The harness | Home and its Keychain credentials | `scripts/reset-macos.sh` (dry run), then `--yes` after a backup |
| `init.sh` | "CG Agent Harness admin password" in the login keychain; marked lines in your shell profile; `scripts/logs/` | `reset-macos.sh --yes`; delete the lines under `# Added by CG-agent-harness`; `rm -rf scripts/logs` |

Homebrew, rustup, Ollama, models and `gh` are general tools; the installer
never removes them.

To test a candidate without using your normal home, launch it explicitly with a
new absolute temporary home (adjust the app path):

```bash
candidate_home="$(mktemp -d /private/tmp/cgah-candidate.XXXXXX)"
open -n --env "CGAGENTHARNESS_HOME=$candidate_home" '/Applications/CG Agent Harness.app'
```

Confirm the home in Setup or the footer, check the model, and keep write gates
disarmed unless the test needs them. The candidate starts without your normal
sessions, skills, credentials, or soul. A later Finder launch uses the normal
home. A prepared local app copy can keep an absolute home in its launch
environment without changing the published `~/.CGagentHarness` default. Record
both app source and data home when comparing versions. Do not change `HOME`.

### Release cadence and finding updates

The app has no built-in updater. The [release
workflow](../.github/workflows/release.yml) checks changed `main` at **08:17 and
20:17 UTC** (runs may be delayed), skips unchanged source and any SHA without a
successful Bundle run, and verifies backend and universal desktop builds before
publishing Latest. A Bundle run or PR alone does not publish. Manual preview,
`v*` tags and cadence changes are in [releasing](RELEASING.md).

## 11. Verify your setup

For source builds/pipeline use, start with read-only inventory checks. Skip tools
you do not use; `gh auth status` is relevant only to GitHub operations:

```bash
sw_vers
uname -m
command -v rustc cargo git gh ollama
rustc --version
cargo --version
ollama list
# After a one-token generate against the chosen tag (docs/MODELS.md):
curl --fail --silent --show-error http://127.0.0.1:11434/api/ps
gh auth status
git remote get-url origin
```

`/api/ps` must report `context_length` 32768 for the seeded web budgets
([Ollama](#25-ollama)); otherwise keep 16000 and 3000 in the home `config.yaml`.

Check both configured model tags, the selected repository and write gates. The
existing hidden `agentic test` command checks some configuration/tools, but is
not a complete doctor: it does not certify model selection, authentication,
prepared dependencies or the end-to-end workflow.

With the rustup-selected Cargo and Clippy from [Rust](#24-rust), run the
repository gates with application inference omitted:

```bash
test_home="$(mktemp -d)"
CGAGENTHARNESS_HOME="$test_home" SKIP_LIVE=1 scripts/verify-local.sh
```

The disposable home avoids the app's lock. The script runs formatting,
warning-denied Clippy, tests with planner keys blanked, CI contracts and a release
build. Cargo uses `--locked`, not offline mode. Missing cargo-deny is a skip;
run it separately. An outer sandbox can block nested Seatbelt; use the operator
terminal without weakening the profile.

For dependency acceptance, run `cargo build --locked` and `cargo deny check` in
the root and `desktop/` with their pinned toolchains. [Dependency
maintenance](DEPENDENCIES.md) covers updates and MSRV constraints.

For the existing live chat smoke, set the exact installed tag explicitly:

```bash
SMOKE_MODEL=qwen3.8:27b scripts/smoke-ollama.sh
# Local derived tag after ollama create. Not a fingerprint model.
SMOKE_MODEL=qwen3.8:27b-mlx-cg scripts/smoke-ollama.sh
```

Read the output. The smoke succeeds when an unavailable endpoint causes a skip.
It neither proves real-model acceptance nor tests editing. See the [coding
pipeline](CODING_PIPELINE.md) for disposable Chrome, model, edit, verification,
approval, and publication acceptance.

For a source-built app, also verify the separate desktop crate and packaged backend:

```bash
(cd desktop && cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
(cd desktop && cargo deny check)
CGAH_TEST_BINARY='dist/CG Agent Harness.app/Contents/MacOS/cgagentharness' python3 scripts/test-desktop-backend.py
scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app' universal
(cd dist && shasum -a 256 -c SHA256SUMS)
```

For console interaction checks with installed Chrome and a Node runtime providing
WebSocket (the recorded local run used Node 24):

```bash
node scripts/chat-browser-acceptance.mjs
```

The script uses an isolated profile and mock APIs. Set `CHROME_BIN` for another
installation. It checks the console and CSP, not model behavior or WKWebView.
The fixture needs no new app build or model download.

Run desktop checks after [packaging](INSTALL.md#52-macos-app). Build and HTTP
checks do not prove native window, chooser, clipboard or quit behavior; record
those separately in the PR body.

## Tests and CI/CD

Use [focused local checks](#11-verify-your-setup) for changed behavior; CI runs
all targets with provider keys blanked.

Fixtures neutralize `GIT_CONFIG_GLOBAL` and set repository identity. Set
`CGAH_TEST_BINARY` for another backend, including a bundle copy. The standard-library
Python suite uses a loopback model fixture with no downloads or cloud inference.
Its disposable HTTPS account verifies restart, history, goal, notes, selection,
tokens, fresh CSRF, and no replay.

| Gate | Evidence and scope |
|---|---|
| [Backend CI](../.github/workflows/ci.yml) | On PRs, `main`, and reusable release calls: rustfmt, warning-denied Clippy, `cargo deny`, MSRV, I6 scan, Chrome commands, Linux/macOS/Windows tests, and real-repo smoke. Bundle owns release builds and CLI packaging; Release rebuilds before publication. |
| Coding and chat regression tests | Revocation, exact edits, clone jail, reviewed trees, job cancellation, session and goal gates, loop budgets, and failed-chat claim release. See [`tests/`](../tests/). |
| Native Cargo acceptance | macOS tests prepare locked dependencies, then test Seatbelt and fixed Cargo checks. Linux uses bubblewrap when present; `unshare --net` and Windows Job Object do not provide equivalent confinement. |
| [Desktop CI](../.github/workflows/desktop.yml) | Bundle, releases, and manual runs build the universal app on Rust 1.90, test desktop policy and packaged backend, verify signatures, checksums, and extraction, then retain ZIP and checksums. |
| [Bundle](../.github/workflows/bundle.yml) | On pushes to `main` only: packages `linux-x86_64` and `macos-arm64` CLIs beside desktop, checksums and re-verifies each artifact, then tests loopback binding and exit-3 missing configuration. |
| Workflow and source checks | CodeQL, DevSkim, Gitleaks secret scanning, PR-template/base-branch checks, the advisory review gate and a daily RustSec scan (`advisories.yml`) are separate workflows. Workflow changes additionally trigger actionlint/zizmor. The optional `live-ollama.yml` fingerprint runs the release binary against a pinned real Ollama on Linux and macOS. |
| [Netconnect CI](../.github/workflows/netconnect.yml) | Path-filtered flag, scope, collector, and untrusted-string tests on Linux and macOS, plus a Linux `strace` proof that default `netconnect status` opens no packet socket. |
| [Release](../.github/workflows/release.yml) | At 08:17 and 20:17 UTC, publishes the next patch only when `main` differs and its SHA has a successful Bundle. Backend CI, CLI packaging, desktop checks, and downloaded checksums still gate publication. Stable tags and manual preview/publish also work. See [release controls](RELEASING.md). |

CI uses deterministic model fixtures and blanks cloud planner keys. Passing it
does not prove real-model quality, complete native GUI behavior, or notarized
distribution. Live Ollama smoke and native desktop checks recorded in PR bodies
cover different evidence. Windows release legs remain parked.
