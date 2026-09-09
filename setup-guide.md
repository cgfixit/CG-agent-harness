# Setup guide (macOS, Apple Silicon)

This is a full, step-by-step walkthrough for getting CGagentHarness running on a Mac with an
Apple Silicon chip (M1, M2, M3, or M4), written for someone comfortable in a terminal but new to
this particular project. If you just want the two-minute pitch, read `README.md` first — this
document is the thing you actually follow, top to bottom, on a fresh machine.

A few terms used throughout: **loopback-only** means the program only ever listens on
`127.0.0.1`, your own machine's internal address — nothing outside your Mac can reach it, and it
will refuse to bind to anything else. The **console** is the browser page you'll chat through.
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

Install Rust through `rustup`, the official installer, rather than through Homebrew — this
project pins an exact toolchain version (more on that in a moment) and `rustup` is what makes
that pin work automatically.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

When it asks, choose option 1 ("Proceed with installation (default)"). Once it finishes, load
Rust into your current shell:

```bash
source "$HOME/.cargo/env"
```

(New terminal windows will pick this up automatically from now on; you only need to `source` it
manually in the terminal you already have open.) Confirm with:

```bash
rustc --version
cargo --version
```

You don't need to install a specific version yourself — this repository ships a
`rust-toolchain.toml` file that pins Rust 1.88 with the `clippy` and `rustfmt` components. The
first time you run any `cargo` command inside the project folder, `rustup` will notice that file
and silently download exactly that toolchain if you don't already have it. That first command
will pause for a little while ("info: syncing channel updates...", "info: downloading N
components") — that's expected, not a hang.

### 2.5 Ollama

[Ollama](https://ollama.com) runs the language model locally and is what the console talks to
for chat. Install it with Homebrew:

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

## 4. Pull a model and start Ollama

The console needs a chat model available through Ollama before it can hold a conversation. The
project's default configuration expects a model tagged `qwen3.8:27b-mlx`, which is a good match
for Apple Silicon (it uses Apple's MLX format). Pull it now — this downloads several gigabytes,
so it may take a while depending on your connection:

```bash
ollama pull qwen3.8:27b-mlx
```

If you'd rather start with something smaller and faster while you get everything else working,
substitute a lighter model here (for example `ollama pull qwen2.5:7b`) — you'll just need to tell
the console to use that model instead once it's running (see the `/model` command in section 7),
or edit `models.local_llm.model` in the config file described in section 9.

Ollama needs to be running as a background server for the console to reach it. If you already
opened the Ollama app once, it's likely already running. To start it explicitly from the
terminal instead:

```bash
ollama serve
```

This will occupy that terminal window (it keeps running in the foreground) — open a **new**
terminal tab or window for the rest of this guide, or run `ollama serve &` to background it in
the same window. Either way, confirm it's actually listening before moving on:

```bash
curl -s http://127.0.0.1:11434/v1/models | head -c 200
```

You should see a chunk of JSON starting with `{"object":"list","data":[...`. If instead you get
`curl: (7) Failed to connect`, Ollama isn't running yet — go back and start it.

## 5. Build the binary

From inside the `CG-agent-harness` folder:

```bash
cargo build --release
```

The first build compiles every dependency from scratch and will take a few minutes (longer if
`rustup` is also downloading the pinned toolchain from section 2.4 at the same time). Subsequent
builds are much faster because Cargo caches the compiled dependencies. When it finishes, the
binary is at:

```
target/release/cgagentharness
```

## 6. First run

The console's write-guarded routes (chat, sessions, and everything else that isn't purely
read-only) require an API key — a secret you generate yourself and hand to the console through a
header on every request. Nothing in this project ever picks a default key for you; if you don't
set one, those routes simply refuse every request with `401 Unauthorized`, which is the intended
fail-closed behavior, not a bug.

Generate a random key and start the server:

```bash
export CGAGENTHARNESS_API_KEY="$(openssl rand -hex 20)"
./target/release/cgagentharness serve
```

You should see one log line confirming the console is up, something like:

```
CGagentHarness console on http://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

This terminal is now occupied by the running server (leave it be — `Ctrl+C` stops it). Open a
web browser and go to:

```
http://127.0.0.1:8790/
```

You'll land on a dark, terminal-styled page with a text input at the bottom and a small key
field. Paste the same value you exported into `CGAGENTHARNESS_API_KEY` into that key field —
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
- `/model <name>` — switches the active chat model, if you pulled more than one.
- `/skills` — lists the bundled skill files under `assets/skills/` (small prompt snippets the
  console can inject into context).
- `/tools` — lists every API surface the console exposes and whether it's actually wired up; a
  healthy install shows every entry marked wired.
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

**Persist the API key.** Instead of re-running `export CGAGENTHARNESS_API_KEY=...` in every new
terminal, add it to your shell's startup file once:

```bash
echo 'export CGAGENTHARNESS_API_KEY="'"$CGAGENTHARNESS_API_KEY"'"' >> ~/.zshrc
```

(Use `~/.zprofile` instead if you'd rather it apply to login shells only. If you're on `bash`
instead of the default `zsh`, use `~/.bash_profile`.) Open a new terminal to confirm it's picked
up: `echo $CGAGENTHARNESS_API_KEY` should print your key.

Everything the console persists to disk — sessions, the config file, logs — lives under a home
directory of its own, `~/.CGagentHarness` by default (override it by setting
`CGAGENTHARNESS_HOME` before you first run `serve`). You never need to create this folder
yourself; the binary creates and populates it on first run.

**Auto-start on login (optional, more advanced).** If you want the console running in the
background permanently without a terminal window open, the standard macOS mechanism is a
LaunchAgent — a small XML file under `~/Library/LaunchAgents/` that tells `launchd` to run the
binary for you at login. Setting one up is outside the scope of this guide; search for "macOS
LaunchAgent plist" if you want to go this route, and point its `ProgramArguments` at the full
path to `target/release/cgagentharness serve` with the `CGAGENTHARNESS_API_KEY` environment
variable set in the plist's `EnvironmentVariables` dictionary.

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

Find the `agentic:` block near the bottom and change these four values:

```yaml
agentic:
  enabled: true                # the master switch for this whole layer
  repo: "your-github-user/your-repo"   # the repository the pipeline will operate on
  deepagent_github:
    enabled: true               # lets the pipeline actually use a planner model
    allow_git_write_tools: true # lets it run git add/commit/push inside its own clone
```

Each of those four settings guards something different, and all of them have to be `true` at once
before anything can write to a repository:

| Setting | What it unlocks |
|---|---|
| `agentic.enabled` | The pipeline responds at all instead of printing "Agentic layer disabled" |
| `agentic.repo` | Which repository it's allowed to touch |
| `agentic.deepagent_github.enabled` | It's allowed to ask a model to propose a patch |
| `agentic.deepagent_github.allow_git_write_tools` | It's allowed to run git write commands inside its own isolated clone |

There's a fifth gate you don't set in the config file: even with all of the above on, opening an
actual pull request additionally requires you to type a `reason` and pass `confirm: true` on that
specific request — a fresh, explicit confirmation every single time, which the console's
`/agent publish` command prompts you for.

Restart the server (`Ctrl+C` in its terminal, then re-run the `serve` command from section 6) so
it picks up the config change.

### 9.3 Run it from the console

Back in the browser, a typical session looks like:

1. `/agent checks` — lists the named verification profiles you can run (for example, running the
   target repo's own test suite) without needing to know the underlying command.
2. `/agent confirm` — starts one coding run: give it an instruction describing what to change, a
   branch name, a commit message, and a reason. This step clones the target repository, has the
   model propose a patch, and verifies it inside a sandbox — it does **not** commit anything yet.
3. `/agent status` — check on a run's progress or see its final proposed diff once it's done.
4. `/agent approve <run-id> <reason>` — after you've reviewed the diff, this is the one command that actually
   commits inside the local clone.
5. `/agent push <run-id> <reason>` — separately authorizes and pushes the approved branch to GitHub.
6. `/agent publish <run-id> <reason>` — separately authorizes a draft pull request.
   Approval, push, and publication each need fresh `reason` and explicit confirmation.
   Direct CLI calls require `--reason=<why> --confirm`; API calls require `reason` and
   `confirm: true`. Combined CLI approval/push/publication is refused.
7. `/agent reject <run-id>` rejects a pending candidate; `/agent discard <run-id>`
   cleans up a run that has already reached a terminal state.

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

By default, anyone who can reach `http://127.0.0.1:8790/` on your Mac and knows (or reads out of
your shell history) the `CGAGENTHARNESS_API_KEY` can use the console — there's no concept of
separate user accounts. That's normally fine for a single-operator machine that only you use. If
this Mac is shared, or you want named accounts and roles instead of one shared key, turn on
per-user auth:

1. In `~/.CGagentHarness/config.yaml`, set `auth.enabled: true` and restart `serve`.
2. Visit the console; it will detect that no accounts exist yet and walk you through creating a
   bootstrap `admin` account with a password you choose, over your own loopback connection.
3. From then on, `/api/auth/login` (surfaced in the console's login UI, not a slash command) is
   how you and any additional users you create sign in.

If you're the only person who will ever touch this installation, the API key from section 6 is
simpler and this section can stay off.

## 11. Verify your setup

Two scripts in the repository double-check that everything above actually works, and are useful
any time you pull new changes or aren't sure something's configured correctly.

```bash
scripts/verify-local.sh
```

This runs formatting and lint checks, the full automated test suite, a release build, and then a
live smoke test against your running Ollama instance — a real chat turn included. It takes a few
minutes. If you'd rather skip the part that needs Ollama running (for example, to just confirm
the code itself is sound):

```bash
SKIP_LIVE=1 scripts/verify-local.sh
```

If you only want the live smoke test on its own (after you've already built once):

```bash
scripts/smoke-ollama.sh
```

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `401 Unauthorized` on every request | `CGAGENTHARNESS_API_KEY` isn't set in the terminal that ran `serve`, or you haven't pasted it into the console's key field | Re-check section 6; confirm `echo $CGAGENTHARNESS_API_KEY` prints something in that same terminal |
| Chat hangs forever with no reply | Ollama isn't running, or hasn't finished loading the model into memory | Run the `curl` check from section 4; give the first request extra time after a fresh `ollama serve` |
| `cgagentharness: harness binds loopback only` and the server refuses to start | You passed `--host` with something other than a loopback address (e.g. `0.0.0.0`) | This is intentional — the console will never bind to a non-loopback address. Omit `--host` or use `127.0.0.1` |
| `Address already in use` when starting `serve` | Another process (maybe a previous `serve` you forgot about) is already on port 8790 | Find and stop it (`lsof -i :8790`), or start this one on a different port with `--port` |
| First `cargo build`/`cargo clippy` is very slow or seems stuck on "downloading components" | `rustup` is fetching the pinned 1.88 toolchain declared in `rust-toolchain.toml` | Expected on first use; let it finish. If it seems to genuinely hang, check your network connection |
| `gh: command not found` when trying `/agent` commands | The GitHub CLI isn't installed (only needed for the optional pipeline in section 9) | `brew install gh && gh auth login` |
| `/agent` commands report "Agentic layer disabled" | `agentic.enabled` is still `false` in `config.yaml` | Follow section 9.2, and make sure you restarted `serve` after editing the file |
| A write action (`/agent publish`) is refused even with everything configured | You forgot the fresh `reason`/`confirm` that command needs on top of the config gates, or `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` is set | Re-read section 9.3/9.4; check `echo $CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` |

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
