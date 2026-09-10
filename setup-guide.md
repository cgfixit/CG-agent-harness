# Setup guide (macOS, Apple Silicon)

This is a full, step-by-step walkthrough for getting CGagentHarness running on a Mac with an
Apple Silicon chip (including M5), written for someone comfortable in a terminal but new to
this particular project. If you just want the two-minute pitch, read `README.md` first — this
document is the thing you actually follow, top to bottom, on a fresh machine.

A few terms used throughout: **loopback-only** means the program only ever listens on
a loopback address such as `127.0.0.1`. It refuses a non-loopback bind. This does
not by itself prevent outbound traffic or protect a deliberately configured proxy. The **console** is the browser page you'll chat through.
The **coding pipeline** is a separate, optional feature that can clone a real GitHub repository
and propose patches to it; it ships completely switched off, and arming it is its own section
near the end.

## 1. What you'll end up with

By the end of section 6 you'll have a single Rust program running on your Mac, serving a chat
console at `http://127.0.0.1:8790/` that talks to a language model also running locally on your
Mac (via Ollama). No account, no cloud service, and no data leaving your machine is required for
this part. Everything past section 6 is optional: turning on the real-repo coding pipeline, and
turning on per-user login.

## 2. Prerequisites

Work through this checklist in order — each step depends on the one before it.

### 2.1 Confirm you're on Apple Silicon

```bash
uname -m
```

This should print `arm64`. If it prints `x86_64`, you're on an Intel Mac; the rest of this guide
still mostly applies, but skip anything that specifically says "Apple Silicon."

### 2.2 Xcode Command Line Tools

Rust's build tooling needs a C compiler and linker, which come from Apple's Command Line Tools,
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

Preserve a working existing toolchain. The acceptance machine used Homebrew
Rust/Cargo 1.98.0 without rustup on PATH and passed the native gates. Homebrew
executables do not enforce `rust-toolchain.toml`; Rust 1.88 compatibility is
checked separately in CI.

If you already use rustup, explicitly prepare the repository's pinned toolchain
and components while engineering network access is available:

```bash
rustup toolchain install 1.88 --component clippy --component rustfmt
```

On a fresh machine without Rust, install a toolchain using the official
[Rust installation instructions](https://www.rust-lang.org/tools/install).
Toolchain installation and dependency downloads belong to preparation. Offline
verification must not attempt a rustup download or depend on credentials from
the operator's real home. See [Offline Cargo verification](docs/OFFLINE_CARGO.md).

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
section 4 below starts it from the terminal either way.

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

Every command in the rest of this guide assumes your terminal's current directory is this
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

The measured M5 Pro/48 GiB run used GGUF Q4_K_M through Metal, context 8192 and
one request at a time. It did not maximize context or modify the operator's
normal daemon. See [Native acceptance](docs/MAC_ACCEPTANCE.md) for measurements,
network isolation and limitations. A loopback URL alone does not establish
Ollama's outbound-network policy.

## 5. Build the binary

From inside the `CG-agent-harness` folder:

```bash
cargo build --release --locked
```

The first build compiles every dependency from scratch and will take a few minutes (longer if
`rustup` is also downloading the pinned toolchain from section 2.4 at the same time). Subsequent
builds are much faster because Cargo caches the compiled dependencies. When it finishes, the
binary is at:

```
target/release/cgagentharness
```

## 6. First run

Local harness use does not require an API key or account login. Start the server:

```bash
./target/release/cgagentharness serve
```

For an existing home, set `security.api_key_optional: true` in its `config.yaml`
and restart. Saved settings are preserved on upgrade. Optional key enforcement
is still available: set that flag false, export a generated
`CGAGENTHARNESS_API_KEY` before `serve`, and enter the matching key in the console.
Desktop Setup can initialize a missing key in the private home `.env`.

You should see one log line confirming the console is up, something like:

```
CGagentHarness console on http://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

On first run the binary seeds its application home. Before the first chat, stop
it with Ctrl-C, open that home's `config.yaml`, and merge your exact installed
model into these existing fields (do not duplicate the YAML mappings):

```yaml
models:
  local_llm:
    model: "qwen3.8:27b"       # replace with your exact installed tag
agentic:
  deepagent_github:
    model: "qwen3.8:27b"       # same selected tag; writes remain disarmed
```

Restart the same `serve` command. Existing homes are not overwritten with new
configuration defaults; review new fields when upgrading. Do not create a
`soul.md` file just to satisfy a setup check.

This terminal is now occupied by the running server (leave it be — `Ctrl+C` stops it). Open a
web browser and go to:

```
http://127.0.0.1:8790/
```

You'll land on a dark, terminal-styled page with a text input at the bottom and a small key
field. Leave it empty for default local use. If you explicitly enabled key enforcement, enter the matching `CGAGENTHARNESS_API_KEY` —
you'll need to do this once per browser tab/session, since the console never stores it anywhere
on disk or in cookies; it's held only in that input field. Once the key is in, type a message and
send it. The current client waits for a complete non-streaming response; latency depends on the model (the first
reply after starting Ollama can be slower, since it has to load the model into memory).

If the browser page hangs and never responds, double-check that Ollama (section 4) is still
running in its own terminal window.

## 7. Take the console for a spin

The console understands a set of slash commands typed directly into the same chat input. A few
worth trying right away:

- `/status` — shows the running configuration: which model is selected, the backend URL, and
  whether the coding pipeline is armed (it won't be yet).
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

## 8. Make it survive a terminal close or a reboot

As set up so far, closing the terminal window running `serve` stops the console, and the API key
only exists in that one shell's environment. Two things to fix, in order of how much you probably
want them:

For manual starts, generate a fresh API key in the launching shell as shown in
section 6 and paste it into the browser. To check presence without printing it:

```bash
# No harness API key is required with security.api_key_optional: true.
```

The application does not install a login service or configure secure credential
storage for you. Keep deployment-specific credential handling separate from
this manual setup; do not put a key in command output or shared logs.

Everything the console persists to disk — sessions, the config file, logs — lives under a home
directory of its own, `~/.CGagentHarness` by default (override it by setting
`CGAGENTHARNESS_HOME` before you first run `serve`). You never need to create this folder
yourself; the binary creates and populates it on first run.

Automatic login startup is not configured or verified by this guide. Job handles
currently live in the server process: restarting loses them, and interrupted
work may survive. Session files and run records persist, but they are not a
complete restart-recovery mechanism. See [Console jobs](docs/CONSOLE_JOBS.md).

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

Restart the server (`Ctrl+C` in its terminal, then re-run the `serve` command from section 6) so
it picks up the config change.

### 9.3 Run it from the console

Select the repository in configuration, then prepare its committed Cargo.lock
into the same application home. From the harness checkout, substitute the actual
repository path and configured home:

```bash
python3 scripts/prepare-cargo.py /path/to/selected/repository "$HOME/.CGagentHarness"
```

Preparation defaults to offline. If locked dependencies are missing, review the
repository and rerun with `--online` only while explicitly allowing engineering
dependency access. Actual checks remain offline, with fresh bounded writable
locations and read-only prepared sources. See [Offline Cargo](docs/OFFLINE_CARGO.md).

Back in the browser:

1. `/agent run codex/fix-topic Describe the intended change` stages an instruction.
2. `/agent checks` lists supported profiles. The server default is `cargo-test`;
   `/agent checks cargo-fmt` is an example explicit selection.
3. `/agent read src/lib.rs#L40-L80` declares a bounded existing-file window.
4. `/agent confirm <reason>` submits a job and returns its ID immediately.
5. `/agent job <job-id>` resumes monitoring, including after refresh/key re-entry.
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

If you ever want to shut the write path off immediately without editing the config file — for
example while debugging, or if you're not sure what state it's in — set this environment
variable before starting `serve`:

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

1. In `~/.CGagentHarness/config.yaml`, set `auth.enabled: true` and restart `serve`.
2. Visit the console; it will detect that no accounts exist yet and walk you through creating a
   bootstrap `admin` account with a password you choose, over your own loopback connection.
3. From then on, `/api/auth/login` (surfaced in the console's login UI, not a slash command) is
   how you and any additional users you create sign in.

Account authentication can stay off for local use. Enabling it protects account
management with sessions and roles; it does not gate chat or the agent pipeline.

## 11. Verify your setup

Start with read-only inventory and configuration checks:

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
CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh
```

This runs formatting, clippy, tests and release build. It runs cargo-deny only
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

The native package was verified as arm64 with checksums, embedded assets and
child self-location. It is ad-hoc signed, with no Developer ID or notarization.
The current binary-only archive does not include `scripts/prepare-cargo.py`;
retain the matching checkout for Cargo preparation. No release was published.

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `401 Unauthorized` on harness requests | Key enforcement is enabled, or forwarding headers prevent the local bypass | For direct local use set `security.api_key_optional: true` and restart; for enforced access configure and enter the matching key |
| Chat hangs forever with no reply | Ollama isn't running, or hasn't finished loading the model into memory | Run the `curl` check from section 4; give the first request extra time after a fresh `ollama serve` |
| `cgagentharness: harness binds loopback only` and the server refuses to start | You passed `--host` with something other than a loopback address (e.g. `0.0.0.0`) | This is intentional — the console will never bind to a non-loopback address. Omit `--host` or use `127.0.0.1` |
| `Address already in use` when starting `serve` | Another process (maybe a previous `serve` you forgot about) is already on port 8790 | Find and stop it (`lsof -i :8790`), or start this one on a different port with `--port` |
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
