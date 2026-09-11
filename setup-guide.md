# Setup guide (macOS: Apple Silicon and Intel)

CG Agent Harness runs either as a universal macOS app or as a standalone
Rust server with a browser console. This guide covers both paths, optional
credentials, local model selection, persistent work and the governed coding
pipeline. See [README.md](README.md) for the overview and
[desktop details](docs/DESKTOP.md) for the app's architecture and limits.

**Loopback-only** means the server listens on a local address such as
`127.0.0.1`; it does not prove that every subprocess or external model service
has no outbound network access. The **console** is the same interface in the
app's native WKWebView and in a browser. The **coding pipeline** runs in a
separate child process and ships disarmed.

**Version scope:** this guide describes the source branch containing issue #32
phases 0–4. `/prompt`, `/soul edit` and proposals, `/skill use`, and `/goal stage`
require those changes. A release or older installed app may predate them. Check
its `Contents/Resources/COMMIT`, release notes and `/help`; an unknown command is
not fixed by changing your persona or arming a gate. See the
[PR stack and candidate evidence](https://github.com/cgfixit/CG-agent-harness/issues/32#issuecomment-5628682279).

## Quick route through this guide

- **Run a downloaded app:** sections 4, 5.2 and 6; then section 7 for controls.
- **Build from source:** sections 2–5; section 11 for verification.
- **Understand missing soul:** section 7.2; **customize output style / unslop:** section 7.4.
- **Use runtime skills:** section 7.3. Codex development skills are separate.
- **Configure memory, web or connectors:** sections 7.5–7.7; command reference in 7.8.
- **Execute a coding goal:** section 9, after local chat works.
- **Upgrade, preserve data or change release cadence:** section 8.
- **Recover a failed setup:** section 12.

## 1. Choose how to run it

| Path | What you need | Where the console opens |
|---|---|---|
| Existing macOS app bundle | Apple Silicon or Intel Mac; working local model service for chat | Native app; owned loopback port chosen at launch |
| Build the macOS app | Apple Silicon build host, Git, Xcode Command Line Tools, rustup with Rust 1.88 and 1.90 | Native app after packaging in section 5.2 |
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
on every older OS or Intel machine. Consult [desktop acceptance](docs/DESKTOP_ACCEPTANCE.md)
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
See [Offline Cargo verification](docs/OFFLINE_CARGO.md).

### 2.5 Ollama

[Ollama](https://ollama.com) runs the language model locally and is what the console talks to
for chat and planning. Check `command -v ollama` and `ollama --version` first.
If it is already installed, preserve that installation. On a fresh machine you
can install the app with Homebrew:

```bash
brew install --cask ollama
```

Launch Ollama once to complete its own setup, then verify `ollama list` and the
endpoint in section 4. If the CLI is unavailable, follow the official
[Ollama quickstart](https://docs.ollama.com/quickstart). The harness does not
install Ollama or download a model automatically.

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
Neither is assumed to exist on another machine. On a fresh machine with an empty
inventory, deliberately choose and download a model appropriate to your hardware
using Ollama before continuing; that is a separate network download and disk
allocation. Preserve working installed models. An `-mlx` suffix alone proves no
execution backend. Inspect the chosen model, using its actual inventory spelling:

```bash
ollama show qwen3.8:27b
```

Configure both `models.local_llm.model` (chat) and
`agentic.deepagent_github.model` (planner) with your chosen tag in section 6.
`/model use <tag>` changes chat selection only. Existing persisted chat selection
can override the chat config, so inspect `/status` after restart.

The shipped tag is `qwen3.8:27b-mlx`; it is a default string, not an installation
check. A 27B model is not required just to use the app. Do not copy an example tag
unless your inventory contains it. Chat uses `models.local_llm.base_url`; the
planner uses `agentic.deepagent_github.base_url`. Both local paths require a
loopback OpenAI-compatible service. An optional chat fallback is configured under
`models.local_llm.fallback` and ships disabled. It does not configure the planner
or certify the separate shared-readiness work tracked in issue #32.

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

Choose a **published release** for ordinary installation, or an exact **candidate
artifact** when testing an unmerged PR:

| Source | What to download | What it establishes |
|---|---|---|
| [Latest release](https://github.com/cgfixit/CG-agent-harness/releases/latest) | `CG-Agent-Harness-macos-universal.zip` and `SHA256SUMS` | Published release source identified in its notes |
| Successful [Bundle run](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml) | `cg-agent-harness-macos-universal` Actions artifact | The selected run's branch and commit; may be unmerged |
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
pinned toolchains (section 2.4), then both target standard libraries:

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
configuration defaults; review new fields when upgrading. A missing `soul.md` is a valid starting state; section 7.2 explains optional
explicit creation.

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

## 7. Chat, soul, skills and goals

Fresh chat starts without an assigned repository, automatic coding skills or
model tool access. The assistant can explain supplied context but cannot inspect
local files or certify live wiring merely because you ask in chat. Use actual
commands/results for evidence and the separate coding workflow for execution.

Existing homes retain their selected repository and skill files. The new chat
composer stops automatically loading legacy coding skills without modifying them.
To avoid old conversation instructions influencing a test, start `/session new`,
use `/skill clear`, inspect `/memory` and `/soul status`, then `/prompt`. Only
explicitly change notes/persona you want changed; a new session still uses the
home's enabled notes and persona.


Enter slash commands in the chat input, not Terminal. `/help` lists commands
available in the installed version. Begin with `/status`, `/model`, `/skills all`
and `/tools`. Registration is not readiness: an unknown prerequisite or empty
last-result field is not evidence that an operation ran successfully.

### 7.1 Sessions and bounded chat continuation

```text
/session new Setup check
/goal Explain this project's test strategy
/goal
```

Send a short question and confirm a real reply. `/session list` lists saved
sessions; `/session use <id>` reopens one and `/session rename <title>` renames
the current one. `/tokens` shows its token usage; the header's **all sessions**
count is cumulative across saved sessions.

New/session-switch actions replace the visible transcript, stop chat continuation,
discard delayed replies from the previous selection, and clear hidden staged
coding/persona reviews. Saved sessions remain intact; switching restores their
retained messages. A new session has no prior messages, goal, or selected prompt
skills. Persona, enabled notes, and enabled web context remain shared within the
home. `/clear` only clears the display; it does not reset the model's session
history. Use `/session new` for a separate conversation. There is currently no
in-app command to delete saved sessions.

```text
/loop 3
/loop
/loop stop
/goal clear
```

`/loop 3` starts up to three follow-up chat turns toward the current goal. The
default is three, hard maximum five. Manual mode pauses after each turn; another
`/loop` continues. `/loop auto` toggles automatic continuation; enable it before
starting, or follow it with `/loop` to resume a paused sequence. `/loop stop`
requests cancellation during generation or cooldown. Goal clear and session
switching stop continuation. Rate limits, failures, repeated output and token
budgets can stop it earlier; the displayed `GOAL_DONE` marker is only model advice.

This loop does not edit files, run skills as programs, or perform coding checks.
The goal persists, while continuation counters and auto state are page state.
Refresh/restart does not resume an unattended loop. Goal-to-coding execution is
an explicit separate workflow in section 9.4.

### 7.2 Missing soul, effective prompt and persona editing

**Missing soul means no persona file loaded, not a broken installation.** Fresh
homes enable the soul toggle but do not create `soul.md`. The base general-chat prompt still operates. The two seeded coding skills are
optional context and are not injected automatically. A missing persona is not fetched
from CyClaw or Codex automatically.

```text
/soul status
/prompt
/soul edit
```

`/soul status` distinguishes enabled, present, loaded, truncated and a safe failure
reason. `/prompt` previews the next chat system prompt: general-chat header, selected optional prompt skills, enabled persona, session goal, injected
web context and enabled memory notes. Preview is private context; review it before
sharing. It is not the coding planner's prompt.

To create or change persona in the editor:

1. Enter bounded persona text, for example: “Use concise explanations. State
   assumptions and list the evidence needed to verify a proposed fix.”
2. Supply a reason for the edit and select **Preview prompt**.
3. Review the preview, select the confirmation checkbox and **Save persona**.
4. Close the editor, use `/soul on` if needed, then `/soul status` and `/prompt`
   to verify the next chat sees the text.

Edits save to `<harness home>/soul.md`; default maximum is 8,000 characters
(`personality.soul_max_chars`). Empty, oversized, invalid and critical instruction-
override content is refused. A stale revision cannot overwrite newer content:
reload the editor and review again. The fixed contract is not editable through
this dialog, and persona text never authorizes code execution.

`/soul off` disables inclusion without deleting the file. Older versions without
the editor allow deliberate manual editing of the active home's `soul.md`; use
`/soul on` to enable it. New editor/proposal guarantees require the candidate
implementation described at the top of this guide.

**History and proposals:** replacement is atomic and saves the previous content
as a private content-addressed backup. `/soul history` lists backup revisions.
`/soul propose` stores proposed text without applying it and returns an ID. This
accepts text you supply, including model-authored text; it does not automatically
generate a new personality.

```text
/soul review <proposal-id>
/soul apply <proposal-id> <reason>
```

Review first; issuing the apply command with a reason sends explicit confirmation
for that exact reviewed revision. There is no additional proposal-apply dialog. To refuse it, use `/soul reject <proposal-id> <reason>` after review.
Rejection preserves the active persona. Changed base content or already-decided
proposals are refused. Backup/proposal storage is limited to 32 records and does
not automatically delete old history. A storage failure after successful persona
replacement can leave proposal status needing reconciliation; inspect both before
retrying. [Chat workflow details](docs/CHAT_WORKFLOWS.md) explain history retrieval
and restoration through the guarded edit flow.

### 7.3 Runtime skills and Codex development skills

| Kind | Location / selection | What it does |
|---|---|---|
| Seeded coding context | `<home>/skills/ponytail` and `karpathy-guidelines` | Available through explicit `/skill use ponytail karpathy-guidelines`; not automatically loaded |
| Optional runtime prompt skill | `<home>/skills/<id>/SKILL.md`; `/skill use <id>` | Adds bounded context to this session's chat |
| Fixed check | `/skill check:cargo-test` | Selects a known check for an already staged coding request; no immediate execution |
| Governed catalog entry | `/skills all` | Inventory only unless an implemented adapter says otherwise |
| Codex development skill | Repository `.codex/skills` or Codex personal skill directory | Guides Codex maintaining the repository; not automatically an app runtime skill |

To add optional context, create a local file in the **actual active home**, e.g.
`skills/review-notes/SKILL.md`. Use a normal directory and file within the home;
links cannot escape the skills-directory boundary. Example content:

```markdown
---
name: Review notes
---
When explaining a change, identify its observable behavior and a targeted check.
```

Then select its **directory ID**, not its display name:

```text
/skills all
/skill use review-notes
/skill status
/prompt
```

IDs allow lowercase ASCII letters, digits, hyphens and underscores, up to 80
characters. Selection replaces the session's optional list; supply all wanted IDs
in one `/skill use <id...>`. Up to four are retained, with defaults of 6,000
characters per body and 16,000 total. Frontmatter is stripped. These limits live
under `personality.prompt_skill_max_chars` and `personality.prompt_skills_total_chars`.

After a successful chat, `/skill status` reports included IDs, lengths and hashes.
That is the last successful snapshot, which can differ from current files or
selection. It does not prove script execution. Missing/unreadable selected files
refuse subsequent chat; restore them or `/skill clear`. Clearing selection removes all optional skill bodies from subsequent prompts,
including selected seeded skills; it does not delete their files.

### 7.4 Customize response style

Use soul for your usual voice across sessions; use an optional runtime skill for
an explicitly selected task style. Neither changes the app's permissions or gives
chat access to a repository. Start with `/soul edit` and adapt this example:

```text
Lead with the answer. Use plain, direct language.
Default to 1–3 short paragraphs; expand when requested.
Use bullets for steps or comparisons, not every response.
Avoid praise, filler introductions and repeated summaries.
Distinguish verified results from assumptions and proposed actions.
Never sacrifice accuracy or necessary uncertainty for brevity.
```

Save through the review flow in section 7.2, enable `/soul on`, and inspect
`/prompt`. Test with `/session new Style check` so earlier conversation does not
confound the comparison. Try a short factual question, a troubleshooting question
and a request for a detailed explanation. Style instructions guide the model;
they do not guarantee a word count or factual correctness.

For a session-specific alternative, create `<home>/skills/concise/SKILL.md` using
the file format in section 7.3 and put the desired writing rules in its body.
Select `/skill use concise`, then inspect `/prompt`. To combine it with another
skill, supply both IDs in the same command. Avoid contradictory rules in soul,
selected skills and memory: a “give full detail” instruction can conflict with a
“one sentence only” instruction. `/skill clear` removes the session selection;
`/soul off` disables the home-wide persona without deleting it.

The most useful customization input is three actual responses you dislike,
your preferred rewrite of each, and a sentence explaining the difference. Keep
those as manual comparison examples. Describe observable preferences such as
“answer before explanation” or “no repeated closing summary,” rather than only
“sound human.” There is no built-in style-preset selector, `/style` command,
automated style evaluation or rewrite button in this version.

**What `unslop` currently does:** it is an optional local coding-planner prose
probe, not a chat output filter. It scans for 16 fixed, case-insensitive phrases
such as “delve,” “game-changer” and “I hope this helps.” It attempts to exclude
proposed file bodies, records hit counts and a response hash, and supplies a
nudge if the ordinary coding loop needs another iteration. A passing candidate
can finish immediately despite phrase hits. It neither rewrites the current
answer nor forces an extra iteration, and it is not used for cloud-provider
planner overrides. Phrase matching is a heuristic, not a quality score.

If you already use the governed local coding workflow and want that probe,
merge this into the existing active home's `config.yaml`, then restart:

```yaml
unslop:
  enabled: true
  metrics_path: "logs/unslop.jsonl"
```

It ships disabled. Metrics use the bounded JSONL logging policy; there is no
user-configurable phrase list or `/unslop` slash command. Do not enable repository
writes just to customize chat prose. For chat, use the persona/skill method above.
A future chat-specific check or explicit rewrite action would need a separate
implementation and tests; enabling this YAML field does not provide either.
See [the current probe](src/agentic/unslop.rs) and
[its loop integration](src/agentic/real_repo_loop.rs).

### 7.5 Operator memory notes

Memory here means explicit, home-wide operator notes. It is separate from soul,
session history and web extracts. It does not automatically learn from chats,
extract facts, index documents or provide structured RAG retrieval.

```text
/memory
/memory add Prefer metric units in examples.
/memory on
/prompt
```

Chat receives a guide to these controls plus current inclusion settings. It has
no callable tools, including `gh`; it can explain commands and list the notes
actually included in its prompt, but cannot perform saves or deletes itself.
Plain-language requests go to the model; slash commands execute in the console.
`/memory add save all session history` stores those exact words, not a summary
or a reference that loads other sessions. Store concrete facts or a reviewed
summary instead. The app saves bounded conversation histories separately.

Adding a note stores it but does **not** enable inclusion. `/memory` displays the
state and note IDs. `/memory forget <id>` deletes one note; `/memory clear`
deletes all notes. `/memory off` retains the notes but excludes them from future
chat prompts. These changes take effect on subsequent chat requests without a
server restart; they affect other sessions using the same home too.

Notes are stored in `<home>/memory/notes.json`. Current code limits are 20 notes,
500 characters per note and 3,000 characters of assembled prompt context; a full
store is not a promise every note fits in the prompt. Empty, oversized and
blocked instruction-override content is rejected. These note limits are code
constants, not documented YAML settings. If memory is on but absent from
`/prompt`, inspect `/memory` for an unreadable store rather than assuming it
loaded. Back up before manual repair.

The structured-memory capability flags remain disabled. Adding a speculative
`memory.enabled` setting will not install facts, episodes, retrieval fusion or
automatic learning. Use notes for stable preferences and small pieces of supplied
context, and use actual command results to establish runtime facts.

### 7.6 Web fetch, search and injected context

Web ships off and requires an explicit public URL allowlist. It is a bounded
text fetcher, not an autonomous browser or a general search-engine connector.
The following illustrates a single public page; substitute the page you need:

```text
/web
/web allow https://example.com/
/web on
/web fetch https://example.com/
/web inject
/prompt
```

Inspect the fetch result before injection. Fetch stores the last extract;
`/web inject` copies that stored extract into context for subsequent chat. Neither
command asks the model a question by itself. `/web search <query>` searches text
from up to the first eight allowlisted pages, returning bounded snippets. It does
not search the wider internet. A completed search with no hits clears the previous
last extract and injected context, including when every page fetch failed; inspect
its `errors` as well as its `hits`. `/web inject` then refuses until a new fetch or
matching search stores a page. Rejected requests (for example, an empty query or
disabled web) preserve stored state. Cleanup failures report `WEB_CLEAR_FAILED`.

**Allow the exact page you intend to fetch.** The current matcher accepts a path
prefix, but the network target is rebuilt from the matched allowlist entry.
Allowing a site's root and then requesting a deeper path can therefore fetch the
root. An earlier broad entry can also win over a later specific entry. Remove
that broad entry with `/web deny <url>`, allow the exact page, and inspect the
returned source URL. Do not assume a domain entry enables arbitrary page browsing.

Current network restrictions and bounds:

- Only HTTP/HTTPS public destinations; private, loopback, local and metadata
  destinations are refused. `/web` cannot reach the local Ollama endpoint or a
  private LAN service.
- No credentials in URLs, query strings or fragments for fetch requests; no
  redirects or proxy use. There is no authenticated browser session, JavaScript
  execution or cookie-based login.
- Text-like responses only, with HTML text extraction. Maximum response size is
  262,144 bytes and timeout is eight seconds. This is not PDF/document ingestion.
- Up to 32 allowlist entries; search queries up to 200 characters, at most three
  hits per page and 160 characters per snippet. Injected context is capped at
  4,000 characters. These are current code bounds, not YAML knobs.

**`/web off` stops new fetch/search/inject requests and excludes saved web context
from subsequent chat system prompts and `/prompt` previews.** Saved files remain;
`/web on` resumes inclusion of previously injected context. This does not retract
an in-flight request or erase earlier conversation messages. `/web deny <url>`
removes an allowlist entry but does not erase saved context. To delete saved web
state as well as disable web:

```text
/web forget
/web off
/web
/prompt
```

`forget` clears both the last extract and injected context, preserving the
allowlist. `/web` distinguishes `context stored` from `injected`: saved context
is inactive while web is off. The allowlist, last extract and context live at
`<home>/tools/web_allowlist.json`,
`web_last.json` and `web_context.txt`; they are shared across sessions and survive
restart. A new session alone does not clear them. Treat fetched text as untrusted
source material, not permission to execute instructions found on a page.
See [web implementation](src/server/web_search.rs) for exact restrictions.

### 7.7 Tools and connectors: available versus catalog-only

`/tools` reports registered operations and capability information. `/tools all`
includes entries that are not wired; `/tools <name>` filters the inventory.
`/skills` defaults to wired entries, while `/skills all` also shows optional
prompt files and other catalog entries. `/skills <name>` filters the display
name, which can differ from the directory ID required by `/skill use`.
`/connectors` displays connector inventory; `/registry` is another inventory
view. None of these commands installs a connector or proves its prerequisites
are ready. The registry's tools array is not a replacement for `/tools`.

| Capability | Current configuration / action | Actual boundary |
|---|---|---|
| Local model | `models.local_llm` in `config.yaml`; `/model` and `/model use <name>` | Chat uses a configured loopback service; selecting a tag does not download it |
| Public web text | `/web` controls in section 7.6 | Explicit allowlisted fetch/search and manual context injection |
| GitHub coding | Explicit `agentic.repo`, gates and prerequisites in section 9; `/github` reports status | Separate governed child-process workflow; a chat reply does not execute Git commands |
| Local file context for coding | Stage `/agent read <repo-relative-path[#Lx-Ly]>` before confirmation | Bounded reads from the governed repository clone; no general Mac filesystem mount |
| `fsconnect` | Not implemented in this app | No slash command, datasource picker, filesystem indexing or YAML enable switch |
| `netconnect` | Not implemented in this app | No arbitrary network connector; `/web` restrictions still apply |
| `sqlconnect` | Not implemented in this app | No database connection configuration, query tool or ingestion workflow |
| `github-public` catalog entry | Inventory only | Does not provide an independent public-repository connector |
| `openai-compatible` catalog entry | Inventory only as a connector | Separately configured local compatible model/fallback paths exist; the row is not an activation control |
| Runtime prompt skill | Home skill file plus `/skill use` | Prompt context, not an executable plugin or permission grant |

Names from the upstream project do not establish support in this standalone
harness. There are no `/fsconnect`, `/netconnect` or `/sqlconnect` commands to turn
on, and copying upstream connector settings does not implement them. The native
Setup folder chooser prepares offline Cargo inputs; it is not chat filesystem
access. See [port scope](docs/PORT_PARITY.md) and the
[capability ledger](docs/parity/STATUS.md) for remaining work, checked against
[current connector inventory](src/server/views.rs).

### 7.8 Slash-command quick reference

Angle brackets below mean “replace with your value”; do not type the brackets.
These are console commands, not shell commands. Inspect results after each
state-changing command. The detailed sections above and section 9 describe
confirmation and persistence semantics.

| Command family | Supported use |
|---|---|
| `/help`, `/status`, `/tokens` | Command help, runtime status and current session usage |
| `/session new <title>`, `list`, `use <id>`, `rename <title>` | Create, list, reopen or rename saved conversations |
| `/prompt` | Private preview of the next chat system prompt |
| `/soul status`, `on`, `off`, `edit`, `propose`, `history` | Inspect, toggle or open persona editing/proposal flows |
| `/soul review <id>`, `apply <id> <reason>`, `reject <id> <reason>` | Review and explicitly decide a persona proposal |
| `/memory`, `on`, `off`, `add <note>`, `forget <id>`, `clear` | Explicit shared notes; section 7.5 |
| `/model`, `/model use <name>` | Inspect/select an available chat model |
| `/skills [all or name]`, `/tools [all or name]` | Inspect capability inventories |
| `/skill use <id...>`, `clear`, `status` | Replace, clear or inspect session prompt-skill selection |
| `/skill check:<profile>` | Replace the check selection for an already staged coding request |
| `/web`, `on`, `off`, `allow <url>`, `deny <url>` | Inspect/toggle fetching or edit its allowlist |
| `/web fetch <url>`, `search <query>`, `inject`, `forget` | Fetch/search text, include the last extract, or clear stored context |
| `/goal`, `/goal <text>`, `/goal clear` | Inspect, set or clear the saved session goal |
| `/loop [n]`, `/loop auto`, `/loop stop` | Bounded chat continuation; section 7.1 |
| `/goal stage <branch>`, `/goal task` | Explicitly stage coding from a goal or inspect its task linkage |
| `/connectors`, `/registry`, `/github`, `/harness` | Inventory, GitHub status, or retained harness-run listing; not connector activation or optimizer execution |
| `/api`, `/api set <KEY> <value>` | Inspect managed-key presence or save a supported key; section 8 |
| `/users` | Open optional account administration; section 10 |
| `/clear` | Clear visible console output, staged coding request, displayed diff tracking and loop state; does not delete saved chats, notes, persona or web context |

The `/agent` family operates the separate coding workflow:

| Command | Effect |
|---|---|
| `/agent run <branch> <instruction>` | Stage a request for inspection; does not start it |
| `/agent checks [profiles]` | List fixed profiles, or select space/comma-separated profiles |
| `/agent iterations <n>` or `clear` | Set a staged iteration cap from 1–10, or restore the configured default |
| `/agent pr <number>` or `/agent issue <number>`; `clear` | Select one context source; choosing one clears the other |
| `/agent plan` or `/agent plan clear` | Open a file chooser for a supplied plan, or clear it; does not generate a plan |
| `/agent read <repo-relative-path[#Lx-Ly]>` or `clear` | Stage bounded file-context declarations, or clear them |
| `/agent cancel` | Clear the staged request; does not stop an already submitted job |
| `/agent confirm <reason>` | Explicitly submit the staged request through the execution gates |
| `/agent jobs`, `/agent job <id>`, `/agent stop <id>` | List, inspect or request cancellation of retained jobs |
| `/agent runs`, `/agent status <run-id>` | List runs or inspect a run and its diff |
| `/agent approve <run-id> <reason>` | Review then approve the exact candidate; repeat only after reading a newly displayed diff |
| `/agent reject <run-id>` | Reject a candidate |
| `/agent push <run-id> <reason>` | Separately authorize pushing the approved commit |
| `/agent pr-body <run-id>` | Choose a reviewed PR body file |
| `/agent publish <run-id> <reason>` | Separately authorize PR publication with the selected body |
| `/agent discard <run-id>` | Clean up a terminal run's owned clone |

Fixed check profiles map to these commands; selection does not install their
prerequisites or run them immediately. Native Cargo verification additionally
uses the prepared offline sandbox described in section 9.

| Profile | Fixed command |
|---|---|
| `cargo-test` (default) | `cargo test --quiet` |
| `cargo-clippy` | `cargo clippy --all-targets -- -D warnings` |
| `cargo-fmt` | `cargo fmt --check` |
| `pytest` | `python3 -m pytest -q --tb=short` (`python` on Windows) |
| `ruff` | `python3 -m ruff check --select E,F,I,B,C4,UP,S .` (`python` on Windows) |

Arbitrary shell check commands are not accepted. Python profiles need their own
prepared tools/dependencies; the Cargo preparation helper does not install them.
The command implementation is in [the console](assets/static/harness.html), and
fixed profiles are in [agent policy](src/server/agent_policy.rs).

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
| `config.yaml` | Seeded configuration; edit existing mappings and restart |
| `harness.json` | Persisted chat model selection, soul/memory/web toggles and other console settings |
| `soul.md`, `soul-history/` | Optional persona and bounded editor backup/proposal records |
| `skills/<id>/SKILL.md` | Runtime skill bodies; existing files are preserved |
| `sessions/` | Chat history, goals, selected skills and current goal-stage linkage |
| `memory/`, `tools/` | Operator notes and web context/allowlist state |
| `.env`, optional `auth.json` | Managed credentials and configured account records; keep private |
| `data/agentic/` | Registry, retained console jobs, workspaces and run evidence |
| `logs/` | Bounded audit/spend/optional metrics logs |


New homes ship `security.api_key_optional: true`. In an existing home's
`config.yaml`, set that literal boolean and restart for credential-free local
use. Existing settings are never overwritten merely by upgrading the app.
Origin/CSRF checks, rate limits and repository write controls still apply.
Forwarded requests do not qualify for the local key bypass; use key enforcement
behind a proxy, including a proxy that strips forwarding headers.

To opt into API-key enforcement, set `security.api_key_optional: false` and
configure `CGAGENTHARNESS_API_KEY`. Desktop and Unix standalone `serve` read
supported managed keys from the home's private `.env` as data, in addition to the
process environment. Neither sources a shell; explicit inherited values take precedence.
The file must be current-user-owned, regular, not a symlink, mode 0600 and no
larger than 64 KiB. Setup can save a missing key but will not replace an existing
one. Enter the matching value in the console after restart; it stays only in
page memory. Do not put credentials in command output, screenshots or shared logs.
Per-user login is independently optional (section 10).

### Which settings take effect where?

Edit the **active home's** `config.yaml`, not the copy inside the app bundle or
repository. Merge into existing mappings instead of appending duplicate YAML
keys. Use literal `true`/`false` booleans: quoted `"true"` does not enable a gate.
After file-based configuration changes, fully quit/relaunch the app or restart
`serve`; closing the app window only hides it.

| Setting | Scope / persistence | How to verify |
|---|---|---|
| Model endpoint, provider, timeout, temperature and token ceiling | `config.yaml`, home-wide; restart after edits | `/model`, Setup diagnostics and an actual reply |
| `/model use <name>` | Persisted console model selection; subsequent requests | `/model`; it can differ from the seeded YAML tag |
| Persona and soul toggle | `soul.md` plus `harness.json`, shared across sessions; next request | `/soul status`, `/prompt` |
| Runtime skill file | `<home>/skills/<id>/SKILL.md`; selection is per session | `/skill status` and next `/prompt`; last successful snapshot may be older |
| Goal and chat history | Saved session; goal maximum 2,000 characters | `/goal`, `/session list`, `/goal task` for coding linkage |
| Loop counters and automatic continuation | Current page only; not an unattended scheduler | Visible loop state; restart does not resume it |
| Memory notes and inclusion | Home-wide saved notes/toggle; next request | `/memory`, `/prompt` |
| Web allowlist, last extract and injected text | Home-wide saved files; web enablement gates fetch/search/inject and prompt inclusion | `/web`, `/prompt`; use `forget` to remove context |
| Coding repo, gates, budgets and planner | `config.yaml`; separate child execution and explicit approvals | `/github`, staged request and retained job/run results |
| Managed credentials | Home `.env`, loaded at process startup on Unix; inherited values win | `/api` reports presence/masked tail, not proof of provider authentication |
| Optional `unslop` | `config.yaml`; local coding planner only | Coding metrics, not chat phrasing |

For output style, use section 7.4 before tuning generation parameters.
`models.local_llm.max_tokens` is an output ceiling and can truncate a reply;
`temperature` changes sampling, not a deterministic filler filter. Loop requests
also have the separate `api.harness_loop_rate_limit` budget. Keep persona and
selected skills concise enough to leave useful room for the question and context.

### Supported managed keys

`/api` displays configured-key presence. `/api set <KEY> <value>` saves one of
these exact keys; substitute a real value only in your private console. This is
not a general environment-variable editor or connector credential vault.

| Key | Purpose |
|---|---|
| `CGAGENTHARNESS_API_KEY` | Optional authentication for the harness's own guarded routes |
| `GROK_API_KEY` | Optional explicitly governed cloud coding planner |
| `ANTHROPIC_API_KEY` | Optional explicitly governed cloud coding planner |
| `DEEPAGENT_API_KEY` | Bearer credential for a configured non-Ollama compatible local planner |

Saving a key does not enable a provider, select it for chat, open coding gates or
install a connector. Provider and per-run online approvals are separate from key
storage. The slash-command workflow does not expose arbitrary cloud-provider
selection flags; review the separate provider controls in
[the configuration reference](assets/config.default.yaml) before considering
a cloud coding run. Values must be nonempty, no more than 4,096 characters
and contain no newline, carriage return or NUL. There is no generic key-deletion
slash command. Preserve other entries when deliberately maintaining the private
file, and restart after changing credentials.

Some console versions print a stale “your shell sources it” hint after saving.
For this Unix desktop/`serve` implementation, the startup loader reads the managed
file as data; follow the loading and permissions rules above. Do not source it as
a shell script. Never put key values in soul, memory, skill files or shared prompts.

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

Use Setup to confirm this home, then configure its exact model and keep write
gates disarmed unless performing a deliberate disposable coding test. It will
start without your normal sessions, optional skills, credentials or soul. An
ordinary Finder launch later uses its normal environment/home. Do not change
`HOME` to point at a test directory.

### Release cadence and finding updates

The app has no built-in updater. Download and replace it deliberately. The
repository's [release workflow](.github/workflows/release.yml) checks changed main
daily at **08:17 UTC** (04:17 New York during daylight time, 03:17 during standard
time). Scheduled GitHub runs can be delayed. It skips unchanged source, verifies
backend and universal desktop builds, then publishes a regular Latest release.
Opening a PR does not publish a release; after a merge, the next scheduled check
can include that new main source.

Maintainers change the schedule in `.github/workflows/release.yml` through a PR.
The manual workflow's `publish: false` default previews the plan; setting it true
requests publication after verification. A pushed `v*` tag is another release
trigger. See [release planning](scripts/release-plan.py) for source/version checks.
A green Bundle run only creates artifacts; it does not itself publish a release.
Changing cadence is a repository workflow change, not an app preference.

## 9. (Optional, advanced) Arm the coding pipeline

Coding execution is optional and disarmed by default. Complete this section only
when you intend to authorize work in a selected repository.

The coding pipeline can clone a GitHub repository, have the local model (or, optionally, a cloud
model) propose a patch, verify that patch in a locked-down sandbox, and — only after you
personally review and approve it — commit, push, and open a draft pull request.
The combined write policy ships closed: `agentic.enabled`,
`agentic.deepagent_github.enabled` and `allow_git_write_tools` are false. Individual
settings such as `mode: write`, `writes_enabled: true` and cloud-provider flags
are not all false. Inspect the complete policy before enabling its master gates.

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

Merge the following fields into the existing configuration. Fresh homes set `agentic.repo: ""`; select your own `owner/name` before enabling
the coding layer. Repository package metadata is not a runtime target. The repo
field is a selector string, not a boolean gate. Use the installed model chosen in section 4:

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

### 9.4 Stage a session goal as a coding task

After preparing the repository, tools and deliberate write configuration above:

```text
/goal Fix the arithmetic bug and pass the declared Cargo test
/goal stage codex/arithmetic-fix
/agent read src/lib.rs#L40-L80
/skill check:cargo-test
/agent iterations 1
```

Replace the sample file window with real relevant lines in your selected
repository. Staging stores a reviewable goal/branch request; it creates no worker.
Review the request before `/agent confirm <reason>`. One iteration is the default;
retries remain bounded by the coding pipeline's configured/requested limits.

Use `/goal task` to inspect progress or restore an unsubmitted stage after page
refresh. The server binds submission to the current goal and stage and saves the
job association before releasing the worker. A changed goal or duplicate submission
is refused; inspect the existing job before deliberately staging another request.

A checked candidate reports **awaiting_review**. `/agent status <run-id>` presents
the complete diff; `/agent approve <run-id> <reason>` is a separate decision.
**completed_local** requires checked-tree evidence and an approved commit. It does
not mean pushed, published, or that every subjective goal has been independently
proved. Continue with the separate push and PR-body/publication steps in 9.3.

Each session retains one current stage; jobs/runs retain their own evidence within
their documented limits. Reopening never replays execution or approval. Changing
or clearing the chat goal does not cancel an already authorized coding job:
`/agent stop <job-id>` is the explicit cancellation request.

### 9.5 The kill switch

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

Run these after packaging in section 5.2. The shell embeds its signed sidecar's
hash, so do not replace or re-sign just the backend afterward. The app bundle
contains the offline preparation helper and desktop documentation; the separate
binary-only CLI archive does not. Build and HTTP/process evidence are distinct
from actual native window, file chooser, clipboard and quit-choice acceptance.
Use [DESKTOP_ACCEPTANCE.md](docs/DESKTOP_ACCEPTANCE.md) to record those checks.

## 12. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `/prompt`, `/soul edit`, `/skill use` or `/goal stage` is unknown | Installed build predates the phase 0–4 controls | Inspect `/help` and bundle `Resources/COMMIT`; use the matching candidate or a release that includes those changes |
| Soul says missing | No `soul.md` exists in this active home | Valid fresh-home state; use explicit `/soul edit` if you want persona, then `/soul on` and `/prompt` |
| Soul saved but is not in chat | Toggle off, wrong home or rejected/stale save | Confirm Setup home, `/soul status`, editor result and `/prompt`; reload a stale editor before saving again |
| Persona/proposal save conflicts or history is full | Base revision changed or 32-record store reached | Review current content/history; preserve and deliberately archive records if needed; do not retry blindly or delete the active persona |
| Selected skill prevents chat | Missing, empty, unreadable or unsafe file; wrong directory ID | Check `<home>/skills/<id>/SKILL.md`; restore it or `/skill clear`, then inspect `/prompt` |
| `/loop` does not edit the repository | It is chat continuation | Use the explicitly configured goal-to-coding workflow in section 9.4 |
| Goal task is stale/interrupted/unavailable | Goal changed, job already submitted, restart interrupted work or retention removed job evidence | Inspect `/goal task`, `/agent jobs` and `/agent runs`; never infer approval or replay work from `GOAL_DONE` |
| App asks for Rosetta or shows Intel-only kind | Old app copy, forced translation or separate Intel-only component | Verify the installed universal copy and both executable slices; see section 2.1 before changing the OS |
| `401 Unauthorized` on harness requests | Key enforcement is enabled, or forwarding headers prevent the local bypass | For direct local use set `security.api_key_optional: true` and restart; for enforced access configure and enter the matching key |
| App reports a home ownership conflict | Another app/server owns the same home | Quit that owner normally before retrying; do not delete its lock |
| App waits at startup | File-access mediation, unavailable backend or damaged/moved bundle | Check macOS prompts; quit, move the complete app to Applications and retry; inspect Setup without deleting the home |
| Changing config appears to do nothing | The running backend retains startup settings; closing its window only hides it | Quit with Cmd-Q and relaunch, or restart standalone `serve` |
| Offline preparation fails in the app | Missing toolchain, Python/SDK, Cargo.lock, cached sources or an existing snapshot | Inspect Setup and the offline Cargo guide; the app does not download dependencies |
| Chat takes too long or times out | Wrong exact model/endpoint, unavailable service, slow loading or inference timeout | Check section 4 inventory, `/model` selection and Setup diagnostics; use `/loop stop` to cancel. Do not repeatedly submit the same request |
| Chat reports `502 HARNESS_LLM_ERROR` with `model server returned HTTP ...` | The local model server returned a non-success HTTP status | Check that status, the configured endpoint/model, and the model server's own diagnostics. The harness drops upstream error bodies unread and excludes them from debug logs; it releases the chat gate without waiting for those bodies. Error-body details are intentionally unavailable in harness logs |
| `cgagentharness: harness binds loopback only` and the server refuses to start | You passed `--host` with something other than a loopback address (e.g. `0.0.0.0`) | This is intentional — the console will never bind to a non-loopback address. Omit `--host` or use `127.0.0.1` |
| `Address already in use` when starting `serve` | Another process occupies the standalone port | Identify it with `lsof -i :8790`; stop only its known owner or select another `--port`. A second server still needs a different home if the first owns it |
| First `cargo build`/`cargo clippy` is very slow or seems stuck on "downloading components" | `rustup` is fetching the pinned 1.88 toolchain declared in `rust-toolchain.toml` | Expected on first use; let it finish. If it seems to genuinely hang, check your network connection |
| `gh: command not found` when trying `/agent` commands | The GitHub CLI isn't installed (only needed for the optional pipeline in section 9) | `brew install gh && gh auth login` |
| `/agent` commands report "Agentic layer disabled" | `agentic.enabled` is still `false` in `config.yaml` | Follow section 9.2, and make sure you restarted `serve` after editing the file |
| A write action is refused | A current policy gate is closed, reason/confirm is absent, publication has no reviewed body, or the emergency switch is set | Re-read section 9.3/9.5; check `echo $CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` |

## 13. Where to go next

- **`README.md`** — the short project pitch and an overview of what's actually running under the
  hood.
- **`INVARIANTS.md`** — the specific security guarantees this project enforces (process
  isolation, the guard chain, the write gates) and exactly what test locks each one down, if
  you're curious how it holds together.
- **`AGENTS.md`** — the operating rules for anyone (human or AI agent) contributing code to this
  repository, including the quality bar every change is held to.

- [Chat workflow reference](docs/CHAT_WORKFLOWS.md) — persona proposals, skill
  bounds, goal completion evidence and remaining scope.
- [Offline Cargo verification](docs/OFFLINE_CARGO.md) — dependency preparation
  versus sandboxed execution.
- [Canonical parity ledger](docs/parity/STATUS.md) — remaining connectors,
  readiness, integration and governance work; inventory labels do not complete it.
