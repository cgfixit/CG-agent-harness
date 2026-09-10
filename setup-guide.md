# Setup guide (macOS, Apple Silicon)

CG Agent Harness runs either as an Apple Silicon macOS app or as a standalone
Rust server with a browser console. This guide covers both paths, optional
credentials, local model selection, persistent work and the governed coding
pipeline. See [README.md](README.md) for the overview and
[desktop details](docs/DESKTOP.md) for the app's architecture and limits.

**Loopback-only** means the server listens on a local address such as
`127.0.0.1`; it does not prove that every subprocess or external model service
has no outbound network access. The **console** is the same interface in the
app's native WKWebView and in a browser. The **coding pipeline** runs in a
separate child process and ships disarmed.

## 1. Choose how to run it

| Path | What you need | Where the console opens |
|---|---|---|
| Existing macOS app bundle | Apple Silicon Mac and a working local model service for chat | Native app; owned loopback port chosen at launch |
| Build the macOS app | Git, Xcode Command Line Tools, rustup with Rust 1.88 and 1.90 | Native app after packaging in section 5.2 |
| Standalone server | Git, Xcode Command Line Tools and Rust 1.88 | Browser at `http://127.0.0.1:8790/` by default |

An already-built app needs no Terminal, external browser, Rust or Python merely
to launch. Coding checks still need their own tools and prepared dependencies.
For that path, use section 4 to check the model and section 5.2 to obtain/install
the app, then follow sections 6–8. Build prerequisites and cloning are only needed
if you build from source or choose the standalone server.

Local harness use requires neither an API key nor account login. No cloud model
or account is required for local chat. Account administration and external services retain their own credentials;
repository writes still require their independent approvals.

## 2. Prerequisites

Install only what your selected path needs. Preserve existing working tools and models.

### 2.1 Confirm you're on Apple Silicon

```bash
uname -m
```

This should print `arm64`. An `x86_64` result may mean an Intel Mac or a shell
running through Rosetta. Confirm the hardware and use a native arm64 shell for
app builds. The current desktop package supports Apple Silicon only; Intel app
packaging is outside this guide. The app targets macOS 12+, with older target
versions still awaiting validation.

### 2.2 Xcode Command Line Tools

Source builds and native Cargo checks need a C compiler and linker, which come from Apple's Command Line Tools,
not the full Xcode app. Install them with:

```bash
xcode-select --install
```

A dialog box will pop up — click "Install" and wait for it to finish (a few minutes on a decent
connection). If you already have them installed, this command will tell you so and do nothing
further, which is fine.

### 2.3 Homebrew

[Homebrew](https://brew.sh) is the package manager we'll use to install Ollama. If you don't
already have it:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

Follow the prompts at the end of the installer — on Apple Silicon it will ask you to run two
`echo`/`eval` lines to add Homebrew to your shell's `PATH`. Do that, then close and reopen your
terminal (or run `source ~/.zprofile`) so the `brew` command is available. Confirm with:

```bash
brew --version
```

### 2.4 Rust

Inspect the installation already on PATH before changing it:

```bash
command -v rustc cargo rustup
rustc --version
cargo --version
rustc --print sysroot
```

Preserve a working existing installation. This repository pins **Rust 1.88 for
the backend** in `rust-toolchain.toml` and **Rust 1.90 for the desktop shell** in
`desktop/rust-toolchain.toml`. Use rustup's Cargo on PATH so those files take
effect; a Homebrew Cargo executable does not enforce them. An unrelated newer
Clippy version can report different lints.

For backend builds, prepare the pinned components while network access is available:

```bash
rustup toolchain install 1.88 --profile minimal --component clippy --component rustfmt
```

For desktop builds, also prepare:

```bash
rustup toolchain install 1.90 --profile minimal --component clippy --component rustfmt
```

On a fresh machine without Rust, follow the official
[Rust installation instructions](https://www.rust-lang.org/tools/install).
From the repository root, `rustup show active-toolchain` should resolve to 1.88;
inside `desktop/`, it should resolve to 1.90. The packaging script disables
implicit toolchain installation, so install both before running it.

Toolchain/dependency downloads belong to preparation. Generated-code verification
must not attempt a rustup download or use credentials from your real home.
See [Offline Cargo verification](docs/OFFLINE_CARGO.md).

### 2.5 Ollama

[Ollama](https://ollama.com) runs the language model locally and is what the console talks to
for chat and planning. Check `command -v ollama` and `ollama --version` first.
If it is already installed, preserve that installation. On a fresh machine you
can install the app with Homebrew:

```bash
brew install --cask ollama
```

This installs both the Ollama app and its command-line tool (`ollama`). You can launch it once
from Spotlight (search "Ollama") to let it finish its own first-run setup, or just proceed —
section 4 checks its endpoint and starts a daemon only if needed.

### 2.6 (Optional, for the coding pipeline only) GitHub CLI

If you plan to use the optional real-repo coding pipeline in section 9, you'll also need the
GitHub CLI. Skip this for now if you only want the chat console — you can always come back and
install it later.

```bash
brew install gh
```

## 3. Get the code

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
```

For an existing checkout, inspect it before updating:

```bash
git status --short
git fetch origin
git log -1 --oneline origin/main
```

Preserve uncommitted work. A clean checkout already on `main` can use
`git merge --ff-only origin/main`; stop and inspect any divergence rather than
resetting it. Source-build commands below assume your terminal's current directory is this
`CG-agent-harness` folder.

## 4. Select an installed model and check Ollama

Inspect the full local inventory before choosing a model:

```bash
ollama --version
ollama list
curl --fail --silent --show-error http://127.0.0.1:11434/v1/models
```

If the endpoint is unavailable, start the existing Ollama app or `ollama serve`.
Do not start a second daemon on an occupied port. Leave a terminal-started daemon
running in its terminal while you use another terminal for the harness.

Choose the **exact installed identifier**. On the acceptance Mac it was
`qwen3.8:27b`; the separately installed `qwen3.8:27b-mlx` had a different digest.
Neither is assumed to exist on another machine. This guide does not ask you to
pull, replace or rename a model. An `-mlx` suffix alone proves no execution
backend. Inspect the chosen model, using its actual inventory spelling:

```bash
ollama show qwen3.8:27b
```

Configure both `models.local_llm.model` (chat) and
`agentic.deepagent_github.model` (planner) with your chosen tag in section 6.
`/model use <tag>` changes chat selection only. Existing persisted chat selection
can override the chat config, so inspect `/status` after restart.

Keep historical model measurements separate from current settings. The native
CLI and desktop runs used different recorded contexts; neither is a recommended
context value for every machine. Refer to [native CLI acceptance](docs/MAC_ACCEPTANCE.md)
and [desktop acceptance](docs/DESKTOP_ACCEPTANCE.md) for each run's exact evidence.
Do not infer an execution backend from a tag suffix or change the running model
service just to match a historical measurement.

## 5. Build or install

### 5.1 Standalone backend

From inside the `CG-agent-harness` folder:

```bash
cargo build --release --locked
```

The first build compiles dependencies and can take several minutes. Prepare the
pinned toolchain first as described in section 2.4. Subsequent
builds are much faster because Cargo caches the compiled dependencies. When it finishes, the
binary is at:

```
target/release/cgagentharness
```

### 5.2 macOS app

To use an existing CI build, open the repository's
[macOS desktop workflow](https://github.com/cgfixit/CG-agent-harness/actions/workflows/desktop.yml),
select a successful run on `main`, and check its source commit. Download the
`cg-agent-harness-macos-arm64-<commit>` artifact. GitHub's artifact archive contains
the app ZIP and `SHA256SUMS`; extract that outer archive first, then check the
inner ZIP from its extracted directory:

```bash
shasum -a 256 -c SHA256SUMS
```

Artifacts have a 14-day retention period and are not a notarized release. If the
artifact has expired or no successful build exists for the desired commit,
build from a clean checkout instead:

```bash
scripts/package-desktop.sh --dmg
```

This requires both pinned toolchains and builds/signs the backend before the
shell embeds its hash. Outputs in `dist/` are `CG Agent Harness.app`,
`CG-Agent-Harness-macos-arm64.zip`, an optional DMG, and `SHA256SUMS`.
The app's `Contents/Resources/COMMIT` identifies its source revision. A dirty
checkout is refused unless `CGAH_ALLOW_DIRTY=1` explicitly marks a development
build; keep that exception out of ordinary installation instructions.

Quit any existing copy with **Cmd-Q**, then unzip the app and move the complete
bundle to Applications or a directory you own. Open it from Finder or the Dock.
The app owns its backend on a dynamically selected loopback port; it does not
adopt a server already running on port 8790. No login service is installed.

The current app is **ad-hoc signed and not notarized**. Gatekeeper may require
per-app approval under macOS Privacy & Security, or a managed policy may refuse
it. Do not disable global Gatekeeper settings. Signature verification and a
successful build do not prove native interaction acceptance; see
[the acceptance checklist](docs/DESKTOP_ACCEPTANCE.md).

## 6. First run

Local harness use does not require an API key or account login. For the app,
open it from Finder. For the standalone path, start the server:

```bash
./target/release/cgagentharness serve
```

For an existing home, set `security.api_key_optional: true` in its `config.yaml`
and restart. Saved settings are preserved on upgrade. Optional key enforcement
is still available: set that flag false, export a generated
`CGAGENTHARNESS_API_KEY` before `serve`, and enter the matching key in the console.
Desktop Setup can initialize a missing key in the private home `.env`.

The standalone server prints its address, for example:

```
CGagentHarness console on http://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

On first run either path seeds the application home. Before the first chat,
quit the app with Cmd-Q or stop `serve` with Ctrl-C. Open that home's `config.yaml`
and merge your exact installed
model into these existing fields (do not duplicate the YAML mappings):

```yaml
models:
  local_llm:
    model: "qwen3.8:27b"       # replace with your exact installed tag
agentic:
  deepagent_github:
    model: "qwen3.8:27b"       # same selected tag; writes remain disarmed
```

Relaunch the app or restart the same `serve` command. Existing homes are not overwritten with new
configuration defaults; review new fields when upgrading. Do not create a
`soul.md` file just to satisfy a setup check.

In the app, the console opens automatically. **Harness → Setup and recovery**
(Cmd-,) shows its owned endpoint and model/tool diagnostics. Use **Check installed
chat and planner models** to check configured tag availability without downloading.
For the standalone server, leave its terminal open and visit:

```
http://127.0.0.1:8790/
```

You'll land on a dark, terminal-styled page with a text input at the bottom and a small key
field. Leave it empty for default local use. If you explicitly enabled key enforcement, enter the matching `CGAGENTHARNESS_API_KEY` —
you'll need to do this once per browser tab/session, since the console never stores it anywhere
on disk or in cookies; it's held only in that input field. Type a message and
send it, leaving the field empty for normal local use. The current client waits for a complete non-streaming response; latency depends on the model (the first
reply after starting Ollama can be slower, since it has to load the model into memory).

If the browser page hangs and never responds, double-check that Ollama (section 4) is still
available at its configured local endpoint.

## 7. Take the console for a spin

The console understands a set of slash commands typed directly into the same chat input. A few
worth trying right away:

- `/status` — shows the selected model/provider, session settings and token counts.
  Use `/github` for agentic status and inspect the saved config for write gates.
- `/model use <name>` — selects an exact installed chat model; the planner stays separately configured.
- `/skills` — lists the bundled skill files under `assets/skills/` (small prompt snippets the
  console can inject into context).
- `/tools` — lists every API surface the console exposes and whether it's actually wired up; a
  wired entry means a registered surface, not complete behavioral acceptance.
- `/session` — lists your chat sessions (each browser conversation is saved to disk under the
  home directory described in section 8).
- `/soul` — shows whether the "soul" personality file is active.
- `/web` — shows the status of the outbound web-fetch allowlist (off by default; nothing reaches
  the internet on your behalf without you explicitly allowing a domain first).

None of these commands touch a real code repository — that's a separate, optional layer covered
in section 9.

## 8. Persistence, optional keys and recovery

### Home and optional credentials

Both launch paths use `~/.CGagentHarness` by default. An explicit
`CGAGENTHARNESS_HOME` selects another home; set it in the launching process's
environment. The desktop requires an absolute override, and a shell export does
not automatically configure a separately Finder-launched app. Setup shows the
actual home. Config, credentials, sessions, notes, model selection and run evidence
live outside the bundle, so replacing or uninstalling the app preserves them.

New homes ship `security.api_key_optional: true`. In an existing home's
`config.yaml`, set that literal boolean and restart for credential-free local
use. Existing settings are never overwritten merely by upgrading the app.
Origin/CSRF checks, rate limits and repository write controls still apply.
Forwarded requests do not qualify for the local key bypass; use key enforcement
behind a proxy, including a proxy that strips forwarding headers.

To opt into API-key enforcement, set `security.api_key_optional: false` and
configure `CGAGENTHARNESS_API_KEY`. Standalone `serve` reads its process
environment. Desktop startup also reads the home's private `.env` as data;
it never sources a shell, and explicit inherited values take precedence.
The file must be current-user-owned, regular, not a symlink, mode 0600 and no
larger than 64 KiB. Setup can save a missing key but will not replace an existing
one. Enter the matching value in the console after restart; it stays only in
page memory. Do not put credentials in command output, screenshots or shared logs.
Per-user login is independently optional (section 10).

### Close, quit and reopen

Closing the desktop window hides it and keeps its backend running. Dock reopen
or a second launch focuses the same home instance. Cmd-Q offers **Keep running**
or **Cancel work and quit** if work is active. A standalone server stops with
Ctrl-C; the app needs no open Terminal. Neither path installs automatic login
startup, and neither resumes model requests or approvals after a reboot.

One server owns a home at a time. Quit its owner before switching between app
and standalone use; do not delete a live lock file. Different homes can run
independently. This lock does not cover manually invoked agentic CLI writers.

### Retained jobs and runs

Console jobs are saved in `data/agentic/console-jobs.json`, retaining up to 32
terminal jobs plus an active one within 16 MiB. After restart, a formerly running
console job becomes interrupted; it is not reattached or resumed. `/agent jobs`
shows retained handles and `/agent job <id>` inspects one.

`/agent runs` inspects separate durable run records. On Unix, a released worker
lease permits reconciliation to interrupted; a live lease stays running.
Legacy records without ownership evidence remain unknown. Inspect with
`/agent status <id>` before deciding or discarding. No commit, push, publication
or approval is replayed automatically. Cancellation is best-effort; escaped
descendants can survive and need inspection before further writes.

Audit/spend/optional metrics JSONL logs retain a current file and one previous
`.1` generation, default 8 MiB each. Logging remains best-effort. See
[console jobs](docs/CONSOLE_JOBS.md), [desktop recovery](docs/DESKTOP.md) and
[process lifecycle](docs/PROCESS_LIFECYCLE.md) for precise limits.

## 9. (Optional, advanced) Arm the coding pipeline

Everything past this point is off by default and stays off until you deliberately turn it on. If
you only wanted a local chat console, you can stop reading here.

The coding pipeline can clone a GitHub repository, have the local model (or, optionally, a cloud
model) propose a patch, verify that patch in a locked-down sandbox, and — only after you
personally review and approve it — commit, push, and open a draft pull request. Every one of
those steps is behind its own switch, and the shipped defaults keep all of them closed.

### 9.1 Install and authenticate the GitHub CLI

If you skipped section 2.6, do it now:

```bash
brew install gh
gh auth login
```

Follow the interactive prompts (browser-based login is the simplest option). Confirm it worked:

```bash
gh auth status
```

### 9.2 Edit the configuration file

The console's full configuration lives at `~/.CGagentHarness/config.yaml` (created automatically
the first time you ran `serve` in section 6 — it's a copy of this repository's
`assets/config.default.yaml`). Open it in your editor of choice:

```bash
open -e ~/.CGagentHarness/config.yaml
```

Merge the following fields into the existing configuration. The repository is a
selector string, not a boolean gate. Use the installed model chosen in section 4:

```yaml
agentic:
  enabled: true
  repo: "your-github-user/your-repo"
  mode: "write"
  writes_enabled: true
  deepagent_github:
    enabled: true
    provider: "ollama"
    model: "qwen3.8:27b"
    allow_git_write_tools: true
    allow_cloud_providers: false
    providers:
      grok:
        enabled: false
      claude:
        enabled: false
```

Master enablement, write mode, writes-enabled, deepagent enablement and clone-write
capability must all permit the operation. The emergency switch can only disable
writes. Local approval, push and publication each also require their own explicit
reason and confirmation; an earlier approval does not override later policy.
Keep cloud-provider child flags false when their parent allow flag is false.

Quit/relaunch the app, or stop/restart `serve`, to load the changed configuration.

### 9.3 Run it from the console

Select the repository in configuration, then prepare its committed Cargo.lock
into the same application home. From the harness checkout, substitute the actual
repository path and configured home:

```bash
python3 scripts/prepare-cargo.py /path/to/selected/repository "$HOME/.CGagentHarness"
```

In the app, **Harness → Setup and recovery → Choose repository and prepare
offline** uses the bundled helper and a native folder chooser. Python 3, Cargo,
the selected repository's Rust toolchain, SDK, Cargo.lock and cached dependencies
are still prerequisites. No source checkout of the harness is needed for that
bundled action. Setup discovers standard tool locations; custom paths use the
private `desktop-tools.json` described in [desktop setup](docs/DESKTOP.md).

Preparation defaults to offline. If locked dependencies are missing, review the
repository and rerun with `--online` only while explicitly allowing engineering
dependency access. Actual checks remain offline, with fresh bounded writable
locations and read-only prepared sources. See [Offline Cargo](docs/OFFLINE_CARGO.md).

Back in the app or browser console:

1. `/agent run codex/fix-topic Describe the intended change` stages an instruction.
2. `/agent checks` lists supported profiles. The server default is `cargo-test`;
   `/agent checks cargo-fmt` is an example explicit selection.
3. `/agent read src/lib.rs#L40-L80` declares a bounded existing-file window.
4. `/agent confirm <reason>` submits a job and returns its ID immediately.
5. `/agent job <job-id>` resumes monitoring, including after refresh without key entry in default local mode.
6. `/agent status <run-id>` displays the candidate and complete diff. A truncated
   diff cannot satisfy console approval.
7. `/agent approve <run-id> <reason>` commits after explicit diff review.
8. `/agent push <run-id> <reason>` separately pushes the approved branch.
9. Complete the actual repository PR template in a local Markdown file.
   `/agent pr-body <run-id>` selects and previews it; then
   `/agent publish <run-id> <reason>` separately opens the draft PR.

`/agent cancel` discards only a staged request. `/agent stop <job-id>` requests
active cancellation; observed native descendants are cleaned up, but escaped or
interrupted work may survive. `/agent reject <run-id>` rejects a pending candidate;
`/agent discard <run-id>` cleans up a terminal run.

Direct CLI writes require `--reason=<why> --confirm`. Publication additionally
requires a reviewed `--body-file`. API writes require `reason` and `confirm: true`;
publication also requires `body`. Combined approval/push/publication is refused.
Protected tests/build configuration remain protected; do not weaken that policy
to get a proposal accepted. See [Bounded edits](docs/BOUNDED_EDITS.md).

### 9.4 The kill switch

To disable writes for a newly launched backend without editing its config, set
this environment variable before launch (shown here for `serve`):

```bash
export CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1
```

This is a disable-only switch: setting it to any of `1`, `true`, `yes`, or `on` blocks the write
path regardless of what `config.yaml` says. It cannot be used to turn writes *on* — only off.
An export in another shell does not update a running server or child. Restart with the
switch set for new processes, or revoke `agentic.writes_enabled` in YAML to block later
mutation boundaries in an active child. Neither action cancels an already executing
Git command or check. Scope/budget changes also refuse later writes until a fresh invocation.

## 10. (Optional) Turn on per-user login

Direct local harness use does not require account login. To retain named accounts,
sessions and roles for account management, you can enable the optional auth feature:

1. In `~/.CGagentHarness/config.yaml`, set `auth.enabled: true` and restart the app or server.
2. Visit the console; it will detect that no accounts exist yet and walk you through creating a
   bootstrap `admin` account with a password you choose, over your own loopback connection.
3. From then on, `/api/auth/login` (surfaced in the console's login UI, not a slash command) is
   how you and any additional users you create sign in.

Account authentication can stay off for local use. Enabling it protects account
management with sessions and roles; it does not gate chat or the agent pipeline.

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
gh auth status
git remote get-url origin
```

Check both configured model tags, the selected repository and write gates. The
existing hidden `agentic test` command checks some configuration/tools, but is
not a complete doctor: it does not certify model selection, authentication,
prepared dependencies or the end-to-end workflow.

Run the repository gates with application inference omitted:

```bash
test_home="$(mktemp -d)"
CGAGENTHARNESS_HOME="$test_home" CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh
```

The disposable home avoids colliding with a running app's home lock. This runs
formatting, Clippy, tests and release build. It runs cargo-deny only
when installed; record a skipped audit and run the dependency policy separately
when needed. Tests include required native Cargo sandbox and process tests.
An outer tool sandbox can prevent nested Seatbelt; run native acceptance from
the operator's terminal without weakening the application profile.

For the existing live chat smoke, set the exact installed tag explicitly:

```bash
SMOKE_MODEL=qwen3.8:27b scripts/smoke-ollama.sh
```

**Read its output:** the current smoke exits successfully if the model endpoint
is unavailable and the chat turn is skipped. Its success alone is not real-model
acceptance, and it does not exercise the editing pipeline. Full disposable
Chrome/model/edit/verification/approval/publication acceptance is documented in
[Console jobs](docs/CONSOLE_JOBS.md). Measured native results and remaining gaps
are in [Native acceptance](docs/MAC_ACCEPTANCE.md) and [Port parity](docs/PORT_PARITY.md).

For a source-built app, also verify the separate desktop crate and packaged backend:

```bash
(cd desktop && cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
(cd desktop && cargo deny check)
CGAH_TEST_BINARY='dist/CG Agent Harness.app/Contents/MacOS/cgagentharness' python3 scripts/test-desktop-backend.py
scripts/verify-desktop-bundle.sh 'dist/CG Agent Harness.app'
(cd dist && shasum -a 256 -c SHA256SUMS)
```

Run these after packaging in section 5.2. The shell embeds its signed sidecar's
hash, so do not replace or re-sign just the backend afterward. The app bundle
contains the offline preparation helper and desktop documentation; the separate
binary-only CLI archive does not. Build and HTTP/process evidence are distinct
from actual native window, file chooser, clipboard and quit-choice acceptance.
Use [DESKTOP_ACCEPTANCE.md](docs/DESKTOP_ACCEPTANCE.md) to record those checks.

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `401 Unauthorized` on harness requests | Key enforcement is enabled, or forwarding headers prevent the local bypass | For direct local use set `security.api_key_optional: true` and restart; for enforced access configure and enter the matching key |
| App reports a home ownership conflict | Another app/server owns the same home | Quit that owner normally before retrying; do not delete its lock |
| App waits at startup | File-access mediation, unavailable backend or damaged/moved bundle | Check macOS prompts; quit, move the complete app to Applications and retry; inspect Setup without deleting the home |
| Changing config appears to do nothing | The running backend retains startup settings; closing its window only hides it | Quit with Cmd-Q and relaunch, or restart standalone `serve` |
| Offline preparation fails in the app | Missing toolchain, Python/SDK, Cargo.lock, cached sources or an existing snapshot | Inspect Setup and the offline Cargo guide; the app does not download dependencies |
| Chat hangs forever with no reply | Ollama isn't running, or hasn't finished loading the model into memory | Run the `curl` check from section 4; give the first request extra time after a fresh `ollama serve` |
| `cgagentharness: harness binds loopback only` and the server refuses to start | You passed `--host` with something other than a loopback address (e.g. `0.0.0.0`) | This is intentional — the console will never bind to a non-loopback address. Omit `--host` or use `127.0.0.1` |
| `Address already in use` when starting `serve` | Another process occupies the standalone port | Identify it with `lsof -i :8790`; stop only its known owner or select another `--port`. A second server still needs a different home if the first owns it |
| First `cargo build`/`cargo clippy` is very slow or seems stuck on "downloading components" | `rustup` is fetching the pinned 1.88 toolchain declared in `rust-toolchain.toml` | Expected on first use; let it finish. If it seems to genuinely hang, check your network connection |
| `gh: command not found` when trying `/agent` commands | The GitHub CLI isn't installed (only needed for the optional pipeline in section 9) | `brew install gh && gh auth login` |
| `/agent` commands report "Agentic layer disabled" | `agentic.enabled` is still `false` in `config.yaml` | Follow section 9.2, and make sure you restarted `serve` after editing the file |
| A write action is refused | A current policy gate is closed, reason/confirm is absent, publication has no reviewed body, or the emergency switch is set | Re-read section 9.3/9.4; check `echo $CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` |

## 13. Where to go next

- **`README.md`** — the short project pitch and an overview of what's actually running under the
  hood.
- **`INVARIANTS.md`** — the specific security guarantees this project enforces (process
  isolation, the guard chain, the write gates) and exactly what test locks each one down, if
  you're curious how it holds together.
- **`AGENTS.md`** — the operating rules for anyone (human or AI agent) contributing code to this
  repository, including the quality bar every change is held to.

## Cargo verification preparation

Before running the coding pipeline with Cargo checks, follow
[Offline Cargo verification](docs/OFFLINE_CARGO.md). The dependency preparation
step may use authorized engineering network access; actual checks run offline.
Select the repository first and prepare its committed Cargo.lock into the same
application home used by the harness. A fresh home does not inherit the operator's
Cargo cache. Tests must put generated state in their supplied temporary directory.
