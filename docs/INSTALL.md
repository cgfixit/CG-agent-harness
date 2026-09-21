# Install and first run

Prerequisites, clone, build, first run, home/keys, and verification. Index: [setup-guide.md](../setup-guide.md). Model window: [MODELS.md](MODELS.md). Accounts: [ACCOUNTS.md](ACCOUNTS.md).

## 2. Prerequisites

Install only what your selected path needs. Preserve existing working tools and models.

### 2.1 Hardware and macOS

```bash
sw_vers
uname -m
```

The universal app includes `arm64` and `x86_64` executables. `uname -m` reports
`arm64` in a native Apple Silicon shell; `x86_64` can mean Intel hardware or a
translated shell. Check **About This Mac** to distinguish them. The supported
packaging recipe below runs on Apple Silicon and cross-builds Intel.

The bundle targets macOS 12+, but a deployment target is not proof of acceptance
on every older OS or Intel machine. Consult [desktop details](DESKTOP.md). Historical native matrix: [DESKTOP_ACCEPTANCE.md](DESKTOP_ACCEPTANCE.md).
for the exact tested hardware and source revision.

In Finder, select the installed app and press Cmd-I: **Kind** should identify a
Universal application. If **Open using Rosetta** is offered, leave it unchecked
for normal Apple Silicon use. This bundle's app and backend have native slices;
installing Rosetta is not a prerequisite. If an Intel-support warning persists,
verify that Finder/Dock is opening the new copy and identify any separately
installed Intel-only tools. Apple's [Rosetta guidance](https://support.apple.com/en-us/102527)
explains how to identify the application type.

### 2.2 Xcode Command Line Tools

Source builds and native Cargo checks need a C compiler and linker, which come from Apple's Command Line Tools,
not the full Xcode app. Install them with:

```bash
xcode-select --install
```

A dialog box will pop up — click "Install" and wait for it to finish (a few minutes on a decent
connection). If you already have them installed, this command will tell you so and do nothing
further, which is fine.

### 2.3 Optional package installation

Preserve an existing working installation. Homebrew is optional for running the
app; it is convenient for installing development tools such as `gh`. If needed,
follow the [official Homebrew installation instructions](https://brew.sh), then
confirm `brew --version` in a new terminal. The model runtime can also be
installed directly from its vendor.

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
See [Offline Cargo verification](OFFLINE_CARGO.md).

### 2.5 Ollama

[Ollama](https://ollama.com) runs the language model locally and is what the console talks to
for chat and planning. Check `command -v ollama` and `ollama --version` first.
If it is already installed, preserve that installation. On a fresh machine you
can install the app with Homebrew:

```bash
brew install --cask ollama
```

Launch Ollama once to complete its own setup, then verify `ollama list` and the
endpoint in [model setup](MODELS.md). If the CLI is unavailable, follow the official
[Ollama quickstart](https://docs.ollama.com/quickstart). The harness does not
install Ollama or download a model automatically.

Seeded `web.total_tokens` 28000 and `web.evidence_tokens` 6000 assume
`OLLAMA_CONTEXT_LENGTH=32768` is set **before** the Ollama process starts.
[Model setup](MODELS.md) is the set-and-verify step. The harness sends no `num_ctx`.

### 2.6 (Optional, for the coding pipeline only) GitHub CLI

If you plan to use the optional real-repo coding pipeline in [coding pipeline](CODING_PIPELINE.md), you'll also need the
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

Preserve uncommitted work. To update an existing clean local `main`:

```bash
git switch main
git merge --ff-only origin/main
git log -1 --oneline HEAD
```

Stop and inspect any divergence rather than resetting it. For contributions,
create a separate `codex/<topic>` or other driver-prefixed branch from the updated
main; PRs must target `main`, not another feature branch. Source-build commands
below assume your terminal's current directory is this `CG-agent-harness` folder.

## 5. Build or install

### 5.1 Standalone backend

From inside the `CG-agent-harness` folder:

```bash
cargo build --release --locked
```

The first build compiles dependencies and can take several minutes. Prepare the
pinned toolchain first as described in [Rust](#24-rust). Subsequent
builds are much faster because Cargo caches the compiled dependencies. When it finishes, the
binary is at:

```
target/release/cgagentharness
```

### 5.2 macOS app

Choose a **published release** for its documented source version, a successful
**main artifact** for newer merged features, or a **PR artifact** to test an
unmerged change:

| Source | What to download | What it establishes |
|---|---|---|
| [Latest release](https://github.com/cgfixit/CG-agent-harness/releases/latest) | `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` | Published release source identified in its notes |
| Successful [Bundle run](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml) on `main` | `cg-agent-harness-macos-universal` Actions artifact | Merged source at that run's commit; may be newer than the published release |
| Successful Bundle run for a PR | Same artifact name | Candidate source only; not evidence it is on main or released |
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
[desktop details](DESKTOP.md). Historical checklist: [DESKTOP_ACCEPTANCE.md](DESKTOP_ACCEPTANCE.md).

## 6. First run

Fresh homes enable account login, role permissions, HTTPS and web
fetch/search/research automatically; no harness API key or configuration edit
is required for those features. Web starts with an empty URL allowlist, so an
administrator must grant sources through `/web allow` before content reads. For
the app, open it from Finder. For the standalone path, start the server:

```bash
./target/release/cgagentharness serve
```

Existing settings are preserved on upgrade. Merge `auth.enabled: true` and
`tls.enabled: true` into the active configuration to adopt the new boundaries.
The fresh-install login hint appears between HARNESS and the authentication
controls. Sign in as `admin` / `admin` on a fresh account store; the password
replacement dialog opens automatically and requires a new password before
portal use. See
[TLS trust, migration and recovery](SECURE_RESEARCH.md).

The standalone server prints its address, for example:

```
CGagentHarness console on https://127.0.0.1:8790/ (home /Users/you/.CGagentHarness)
```

On first run either path seeds the application home. If the configured loopback
service already has the shipped `qwen3.8:27b-mlx` tag, chat needs no model-config
edit. Use `/model list` to inspect available tags and `/model use <exact-tag>` to
select another installed chat model. The harness never downloads a missing tag.

Only if the model endpoint needs changing, or you intend to configure the optional
coding planner, quit with Cmd-Q or stop `serve` with Ctrl-C and update the active
home's `config.yaml`. Merge the exact installed tag into the applicable existing
fields (do not duplicate YAML mappings):

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
(Cmd-,) shows its owned endpoint and model/tool diagnostics. Use **Check installed
chat and planner models** to check configured tag availability without downloading.
For the standalone server, leave its terminal open and visit:

```
https://127.0.0.1:8790/
```

The native app verifies its owned local certificate. External browsers need
explicit operator-controlled trust or an operator certificate already trusted by
the browser; see the secure setup guide. After login and password replacement,
leave the optional metadata key field empty and send a message. The console
requests `POST /api/chat` with `Accept: text/event-stream`, so text arrives as
`delta` events until a final `done`; `/loop stop` (`POST /api/chat/cancel`)
aborts the turn. Time to first token still depends on the selected local model.
See [CHAT_STREAMING.md](CHAT_STREAMING.md).

If the browser page hangs and never responds, double-check that Ollama ([model setup](MODELS.md)) is still
available at its configured local endpoint.

## 8. Persistence, optional keys and recovery

### Home and optional credentials

Both launch paths use `~/.CGagentHarness` by default. An explicit
`CGAGENTHARNESS_HOME` selects another home; set it in the launching process's
environment. The desktop requires an absolute override, and a shell export does
not automatically configure a separately Finder-launched app. Setup shows the
actual home. Config, credentials, sessions, notes, model selection and run evidence
live outside the bundle, so replacing or uninstalling the app preserves them.

| Home content | Purpose |
|---|---|
| `config.yaml` | Seeded configuration; edit existing mappings, reload supported limits or restart |
| `harness.json` | Persisted chat model selection, soul/memory/web toggles and other console settings |
| `soul.md`, `soul-history/`, `soul-pending-apply.json` | Optional persona, bounded backups/proposals, and temporary apply-recovery marker |
| `skills/<id>/SKILL.md` | Runtime skill bodies; existing files are preserved |
| `sessions/` | Chat history, goals, selected skills and current goal-stage linkage |
| `memory/`, `tools/` | Pinned notes (`memory/notes.json`); optional structured store (`memory/structured.sqlite3`) and gate overlay (`memory/structured_gates.json`); web context/allowlist under `tools/` |
| `.env`, `auth.sqlite3`, `auth.initialized` | Private provider credentials, transactional accounts and initialization marker; legacy JSON retained for recovery |
| `tls/server.pem`, `cli-session.json` | Private persisted TLS material and optional CLI login bound to its origin/certificate |
| `data/agentic/` | Registry, retained console jobs, workspaces and run evidence |
| `logs/` | Bounded audit/spend/optional metrics logs |


New homes require HTTPS and account login. The old key-required setting is
deprecated; a harness metadata key is optional and grants no account authority.
Administrators save/replace/clear credentials in **API Keys**; saved and active
masks are separate. Unix startup reads private bounded `.env` as data; explicit
inherited environment values override stored values. Restart to reload, and remove
an inherited value separately when clearing a saved key is insufficient.
Forwarded/proxy requests are unsupported. See [secure setup](SECURE_RESEARCH.md).

### Which settings take effect where?

Edit the **active home's** `config.yaml`, not the copy inside the app bundle or
repository. Merge into existing mappings instead of appending duplicate YAML
keys. Use literal `true`/`false` booleans: quoted `"true"` does not enable a gate.
For the [supported web/API limits](CONFIG_RELOAD.md), use **Reload limits** or
Unix SIGHUP after editing. Other startup settings require a full quit/relaunch
or `serve` restart; closing the app window only hides it. Existing coding-policy
checks still reread disk at mutation boundaries.

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
| Managed credentials | Home `.env`, loaded at process startup on Unix; inherited values win | `/api` reports presence/masked tail, not proof of provider authentication |
| Spend ledger and dashboard | Retained `logs/spend.jsonl` and `.1`; dashboard state is page memory only | [Completeness and pricing](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero) |
| Completion webhooks | `notifications.*` plus optional managed bearer; restart-only; queue is not durable | [Setup, destination rules and delivery audit](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook) |
| Optional `unslop` | `config.yaml`; local coding planner only | Coding metrics, not chat phrasing |

For output style, use [response style](CONSOLE.md#74-customize-response-style) before tuning generation parameters.
`models.local_llm.max_tokens` is an output ceiling and can truncate a reply.
To keep at least 4096 estimated prompt tokens inside the 30000-token safety
limit, startup rejects unsigned numeric local-chat values of `0`, and caps
both local chat and `/loop` at 25904 with resolved Ollama and explicit reasoning
`none`, or 12952 otherwise (a doubled reservation). The legacy `/loop` value
`0` keeps its existing meaning of the 2048-token default. Web-enabled
chat remains subject to the separate `web.total_tokens` per-turn budget.
Local context is the Ollama server window (32768 via `OLLAMA_CONTEXT_LENGTH`,
verified in [model setup](MODELS.md)); the harness sends no `num_ctx`. `temperature` changes
sampling, not a deterministic filler filter. Loop requests also have the
separate `api.harness_loop_rate_limit` budget. Keep persona and selected skills
concise enough to leave useful room for the question and context.

### Tunables you may want to know about

Every tunable lives in `assets/config.default.yaml` and is copied into your
home `config.yaml` on first run; there are no hidden hardcoded budgets. The
ones operators most often ask about:

| Key | Default | Effect |
|---|---|---|
| `app.agent_job_poll_ms` | 1500 | Browser job polling interval, clamped to 1–30 s |
| `api.rate_limit.max_requests` / `window_seconds` | 60 / 60 | Per-IP sliding-window ceiling on every route |
| `api.harness_loop_rate_limit.*` | 8 / 300 s / 2048 tokens | Separate `/loop` budget; same backend-dependent ceiling as local chat after the legacy `0` → `2048` fallback |
| `models.cloud_chat.enabled` | true | Allows `/model use grok` / `claude`; each provider has its own `enabled` and `model` |
| `models.cloud_chat.max_tokens` / `timeout_sec` | 4096 / 90 | Cloud reply ceiling and timeout; boot refuses 0 or >32768 tokens, or a timeout outside 1–720 s |
| `models.local_llm.max_tokens` | 4096 | Local reply ceiling: 1–25904 with resolved Ollama and explicit reasoning `none`; otherwise 1–12952, with twice the ceiling reserved for completion. Invalid combinations fail startup with `CONFIG_ERROR` |
| `models.local_llm.reasoning_effort` | `none` | Sent only to resolved Ollama: `none`, `low`, `medium`, `high`, `max`. Missing/empty omits the field and uses the doubled reservation |
| `chat.compact_prompt_tokens` / `compact_keep_messages` | 24000 / 8 | Calibrated next-prompt trigger and recent messages retained (2–40). Threshold floor/cap and web interaction: [compaction](CONSOLE.md#local-history-compaction) |
| `compaction.summary_max_tokens` | 768 | Summary output ceiling, clamped 128–2048; missing in an existing home also uses 768 |
| `models.local_llm.inventory.*` | 2.0 s / 262144 bytes | Model-list probe timeout and response cap |
| `auth.max_concurrent_operations` | 2 | Concurrent scrypt derivations (about 128 MiB each); range 1–4 |
| `auth.session.idle_timeout_sec` / `absolute_timeout_sec` | 43200 / 604800 | Session expiry without use, and regardless of use |
| `attachments.max_concurrent_uploads` | 2 | Attachment upload bodies (up to ~45 MiB each) buffered at once; range 1–8, excess requests get 503 `ATTACHMENT_BUSY` |
| `attachments.body_timeout_sec` | 120 | Deadline for receiving one upload body; range 5–600, expiry gives 408 `ATTACHMENT_TIMEOUT` and frees the permit |
| `structured_memory.*` | see file | Per-owner caps (facts, proposals, episodes, bytes), search/retrieval limits, suggestion queue and consolidation thresholds |
| `web.evidence_tokens` | 6000 | Evidence budget when no tokenizer is available, clamped 256–6000. Keep 3000 if the Ollama window is smaller than 32768 or unverified |
| `web.model_tokens` | 1024 | Maximum synthesis completion, clamped 256–2048 |
| `web.total_tokens` | 28000 | Planning and synthesis budget, clamped 2048–32000, sized for a **verified** 32768-token Ollama window ([model setup](MODELS.md)). Stay under 32000 so an over-budget estimate does not hard-fail. Keep 16000 if `/api/ps` shows a smaller `context_length` |

`assets/config.default.yaml` is seed-only. Existing homes keep their seeded
`config.yaml` until you edit it. Missing settings use their documented runtime
fallbacks; existing explicit settings remain authoritative. Restart after edits.

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
| `CGAGENTHARNESS_WEBHOOK_TOKEN` | Optional completion-webhook bearer; restart after saving. Requires [webhook-capable build and configuration](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook) |
| `GH_TOKEN` | GitHub CLI and its Git credential helper after restart; repository approvals still apply |
| `GROK_API_KEY` | Optional explicit Grok chat and governed cloud coding planner |
| `ANTHROPIC_API_KEY` | Optional explicit Claude chat and governed cloud coding planner |
| `DEEPAGENT_API_KEY` | Bearer credential for a configured non-Ollama compatible local planner |
| `SERPAPI_API_KEY` | Optional fixed search API for chat web search; public Google is the no-key fallback |

Saving a key does not select it for chat, open coding gates or install a
connector. After restart, `/model use grok` or `/model use claude` explicitly
selects cloud chat; provider and per-run online approvals for coding remain
separate from key storage. The slash-command workflow does not expose arbitrary
cloud-provider configuration flags; review the separate provider controls in
[the configuration reference](../assets/config.default.yaml) before considering
a cloud coding run. Values must be nonempty, no more than 4,096 characters
and contain no newline, carriage return or NUL. `/api clear <KEY>` removes only
the named managed credential. Preserve other entries when deliberately maintaining the private
file, and restart after changing credentials.

Prefer the **API Keys** pane for pasting secrets; its password inputs are cleared
after successful save. Use **Clear saved value** to remove a stored credential.
The pane displays masked **Saved** and **Active** values separately, their source
(`startup_file`, `environment` or `unset`), and whether a restart is needed.
Clearing a saved value leaves a currently active value running until restart;
an explicit process environment override still wins after restart. `/registry`
and **View registry** preserve inventory access. The Unix startup loader reads
the managed file as data. Do not source it as a shell script or put secrets in
soul, memory, skill files or shared prompts.

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

Schedules are separately persisted in `data/agentic/console-schedules.json`;
occurrences are consumed before launch and missed windows are skipped. See
[schedules and completion notifications](CONSOLE_JOBS.md#schedules-and-completion-notifications).
Notifications do not replace job records, and queued deliveries are lost on exit.

`/agent runs` inspects separate durable run records. On Unix, a released worker
lease permits reconciliation to interrupted; a live lease stays running.
Legacy records without ownership evidence remain unknown. Inspect with
`/agent status <id>` before deciding or discarding. No commit, push, publication
or approval is replayed automatically. Cancellation is best-effort; escaped
descendants can survive and need inspection before further writes.

Audit/spend/optional metrics JSONL logs retain a current file and one previous
`.1` generation, default 8 MiB each. Logging remains best-effort. See
[console jobs](CONSOLE_JOBS.md), [desktop recovery](DESKTOP.md) and
[process lifecycle](PROCESS_LIFECYCLE.md) for precise limits.

### Update, backup, rollback and uninstall

Quit with Cmd-Q (closing the window only hides it), wait for work to stop, and
back up the active home to a private location before upgrading a home you care
about. It contains credentials and private conversation/run data. Keep the prior
app archive and its source SHA if you need to compare versions.

Verify and replace the complete app bundle. Configuration and existing skills
are seeded only when absent; upgrading does not rewrite your existing defaults,
model selection or persona. Merge new config fields deliberately instead of
replacing your file with `assets/config.default.yaml`. An older binary may not
understand newer state; inspect compatibility and use a preserved matching backup
rather than blindly rolling back a live home. Never delete a lock to defeat a
running owner. Uninstalling means quitting and removing only the app; deleting
the home is a separate destructive choice.

To test a candidate without using your normal home, launch it explicitly with a
new absolute temporary home (adjust the app path):

```bash
candidate_home="$(mktemp -d /private/tmp/cgah-candidate.XXXXXX)"
open -n --env "CGAGENTHARNESS_HOME=$candidate_home" '/Applications/CG Agent Harness.app'
```

Use the console footer or Setup to confirm this home, check the selected model,
and keep write gates disarmed unless performing a deliberate disposable coding test. It will
start without your normal sessions, optional skills, credentials or soul. An
ordinary Finder launch later uses its normal environment/home. A deliberately
prepared local app copy can retain a separate absolute home through its per-copy
launch environment; that local customization does not change the published app's
`~/.CGagentHarness` default. A different app source and a different data home are
separate choices: record both when comparing versions. Do not change `HOME` to
point at a test directory.

### Release cadence and finding updates

The app has no built-in updater. Download and replace it deliberately. The
repository's [release workflow](../.github/workflows/release.yml) checks changed main
every 12 hours at **08:17 and 20:17 UTC** (04:17/16:17 New York during daylight
time, 03:17/15:17 during standard time). Scheduled GitHub runs can be delayed.
It skips unchanged source, and also skips until that exact SHA has a successful
Bundle run. A green Bundle run does not itself publish. When both conditions
hold, it verifies backend and universal desktop builds, then publishes a regular
Latest release. Opening a PR does not publish a release; after a merge, the next
scheduled check can include that new main source.

Maintainers change the schedule in `.github/workflows/release.yml` through a PR.
The manual workflow's `publish: false` default previews the plan; setting it true
requests publication after verification. A pushed `v*` tag is another release
trigger. See [release planning](../scripts/release-plan.py) for source/version checks.
A green Bundle run only creates artifacts; it does not itself publish a release.
Changing cadence is a repository workflow change, not an app preference.

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

`/api/ps` must report `context_length` 32768 before using the seeded
`web.total_tokens` 28000 and `web.evidence_tokens` 6000. If it does not, keep
16000 and 3000 in the home `config.yaml`.

Check both configured model tags, the selected repository and write gates. The
existing hidden `agentic test` command checks some configuration/tools, but is
not a complete doctor: it does not certify model selection, authentication,
prepared dependencies or the end-to-end workflow.

Run the repository gates with application inference omitted:

```bash
test_home="$(mktemp -d)"
CGAGENTHARNESS_HOME="$test_home" SKIP_LIVE=1 scripts/verify-local.sh
# If rustup's cargo-clippy is older than Homebrew's:
# CLIPPY=/opt/homebrew/bin/cargo-clippy CGAGENTHARNESS_HOME="$test_home" SKIP_LIVE=1 scripts/verify-local.sh
```

The disposable home avoids colliding with a running app's home lock. Use a unique
`--port` if a leftover `serve` already owns `:8790`. This is the
same invocation as [verify](INSTALL.md#11-verify-your-setup): formatting, Clippy with
warnings denied, tests (planner keys blanked), and a release build.
`scripts/verify-local.sh` uses `--locked` for its release build; its Clippy and
test invocations are not locked, and it does not set `CARGO_NET_OFFLINE`.
It runs cargo-deny only when installed; record a skipped audit and run the
dependency policy separately when needed. Tests include required native Cargo sandbox and process tests.
An outer tool sandbox can prevent nested Seatbelt; run native acceptance from
the operator's terminal without weakening the application profile.

For reproducible dependency acceptance, also run `cargo build --locked` and
`cargo deny check` in both the repository root and `desktop/`, using each pinned
toolchain. [Dependency maintenance](DEPENDENCIES.md) documents compatible
updates, retained pins, feature review and the exact drift commands. A lockfile
can be valid even when newer releases exist; a blind update can exceed the MSRV.

For the existing live chat smoke, set the exact installed tag explicitly:

```bash
SMOKE_MODEL=qwen3.8:27b scripts/smoke-ollama.sh
```

**Read its output:** the current smoke exits successfully if the model endpoint
is unavailable and the chat turn is skipped. Its success alone is not real-model
acceptance, and it does not exercise the editing pipeline. Full disposable
Chrome/model/edit/verification/approval/publication acceptance is documented in
[Console jobs](CONSOLE_JOBS.md). Measured native results and remaining gaps
are in [historical native CLI record](MAC_ACCEPTANCE.md) and [Port parity](PORT_PARITY.md).

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

The script uses an isolated browser profile and local mock APIs. On a different
Chrome installation, set `CHROME_BIN` to its executable. It checks the real console
and CSP but does not establish actual-model behavior or native WKWebView interaction.
No new app build or model download is needed for this browser fixture.

Run these after packaging in [macOS app packaging](INSTALL.md#52-macos-app). The shell embeds its signed sidecar's
hash, so do not replace or re-sign just the backend afterward. The app bundle
contains the offline preparation helper and desktop documentation; the separate
binary-only CLI archive does not. Build and HTTP/process evidence are distinct
from actual native window, file chooser, clipboard and quit-choice acceptance.
Use [historical desktop acceptance](DESKTOP_ACCEPTANCE.md) to record those checks.

## Tests and CI/CD

```bash
test_home="$(mktemp -d)"
CGAGENTHARNESS_HOME="$test_home" SKIP_LIVE=1 scripts/verify-local.sh
# fmt, clippy -D warnings, optional installed cargo-deny, tests, release build
# If rustup's cargo-clippy is older than Homebrew's: CLIPPY=/opt/homebrew/bin/cargo-clippy
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets
cargo test --test invariant_guard          # fast I6 source scan
python3 scripts/test-desktop-backend.py    # built release backend; disposable homes
node scripts/chat-browser-acceptance.mjs   # installed Chrome + Node with WebSocket; mock APIs
# Optional, with your configured local inference service running:
scripts/smoke-ollama.sh
```

Always blank `GROK_API_KEY`, `ANTHROPIC_API_KEY` and `DEEPAGENT_API_KEY` when
running tests: a real key on a developer machine must never become something the
suite asserts on. Tests drive a real `git` but do not need your global Git
identity: the fixtures neutralize `GIT_CONFIG_GLOBAL` and set `user.name` /
`user.email` per seed repository. Set `CGAH_TEST_BINARY` to test another built backend,
including the copy inside a macOS bundle. The Python suite uses only the standard
library and a temporary loopback HTTP model fixture; it downloads no models and
makes no cloud inference requests. Its chat/restart test verifies history, goal,
notes, model selection, token counts, fresh CSRF and no replay using a disposable
account, HTTPS and an optional empty harness key.

| Gate | Evidence and scope |
|---|---|
| [Backend CI](../.github/workflows/ci.yml) | PRs, `main`, and reusable release verification: rustfmt, Clippy with warnings denied, `cargo deny`, an MSRV check against `rust-version`, the I6 invariant source scan, a Chrome slash-command browser acceptance job, Rust/public-backend tests on Linux and macOS, and a real-repo-run smoke. Release builds and CLI packaging are not part of this gate: Bundle covers them on PRs and `main`, and Release builds its own before publishing. |
| Coding and chat regression tests | Write-policy revocation, exact edits, clone jail, reviewed Git trees, detached-job cancellation, session/goal gates, loop budgets, and release of failed/cancelled chat claims. See [`tests/`](../tests/). |
| Native Cargo acceptance | Required macOS tests prepare locked dependencies, then exercise real Seatbelt restrictions and fixed Cargo checks. Linux bubblewrap is the filesystem-confined backend when present; `unshare --net` fallback and Windows Job Object do not establish equivalent confinement. |
| [Desktop CI](../.github/workflows/desktop.yml) | Bundle PRs/`main`, tag releases, and manual runs reuse this job to build the universal app on Rust 1.90, run desktop policy and packaged-backend tests, verify signatures/checksums and the extracted bundle, then retain the universal ZIP and checksums as artifacts. |
| [Bundle](../.github/workflows/bundle.yml) | Packages the loopback CLI for `linux-x86_64` and `macos-arm64` alongside the universal desktop job, checksums each artifact, re-verifies it after extraction, and smokes the staged binary for the loopback-only bind guard and the exit-3 missing-config contract. |
| Workflow and source checks | CodeQL, DevSkim, Gitleaks secret scanning, and PR-template/base-branch checks are separate workflows. Workflow changes additionally trigger actionlint/zizmor. |
| [Release](../.github/workflows/release.yml) | Every 12 hours at 08:17 and 20:17 UTC, publish the next patch as Latest only if `main` differs from the latest release **and** that SHA already has a successful Bundle run. Backend CI, CLI packaging, universal desktop checks, and downloaded checksums still gate publication. Stable tags and manual preview/publish are also supported. See [release controls](RELEASING.md). |

CI uses deterministic model fixtures and blanks cloud planner keys. Passing it
does not prove real-model quality, complete native GUI behavior, or notarized
distribution. Live Ollama smoke and [historical desktop acceptance](DESKTOP_ACCEPTANCE.md)
cover different evidence. Windows CI/release legs remain parked.
