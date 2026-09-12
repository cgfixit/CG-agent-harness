# CG-Agent

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)
[![Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)

![CG Agent Harness running on macOS](assets/app-ss.png)

A local agentic harness for **coding, chat, and controlled tool use**. Work with a
local model, keep session goals and context, and take repository changes through a
bounded plan → edit → check → feedback loop before reviewing and publishing them.

Use the universal macOS app or run the Rust backend in a browser. Chat is served
only by an OpenAI-compatible **loopback** model server; a non-loopback chat
endpoint is refused. The coding pipeline can optionally route its planner to a
cloud provider (xAI Grok or Anthropic Claude) instead of the local model,
behind six gates including a per-run `--confirm-online`; that path ships
closed and is never used for chat. Enabling it is a real repository-content
egress decision, not just a plan handoff: each loop iteration sends the
instruction, the approved plan, prior check feedback, the full contents of
bounded excerpts of read/declared source files, and any fetched GitHub PR/issue context to
the provider's API (`real_repo_loop.rs` builds this payload;
`CloudProposerClient::invoke` sends it). It passes through an injection scan
and redaction pass first (`sanitize_handoff`), but "sanitized" describes that
scan, not a reduction to a small prompt — read the full data-egress note in
[Run a coding task](#run-a-coding-task) before turning it on.

Chat starts without an assigned repository or automatically injected coding
skills. It can discuss supplied context; it does not inspect files or run tools
from a model reply. Coding execution is separately staged and confirmed.
> **Fresh homes require HTTPS and account login; the harness API key is optional.** Start with `admin` / `admin`, then replace the password immediately. See [secure setup](docs/SECURE_RESEARCH.md). Repository mutations
remain disabled until explicitly configured, and commit, push, and draft PR
publication each require a separate operator decision.

**Version scope:** this document describes this source tree, including secure
web research, SQLite accounts and native HTTPS. These changes are absent from
the September 12 `v0.1.7` release. Until merged and released, build this branch or
use its verified PR artifact. Check the selected workflow SHA and the app's
`Contents/Resources/COMMIT`; a release tag or the Cargo package version `0.1.0`
alone does not establish feature availability. See [setup](setup-guide.md) for
upgrade and recovery, and [verification](docs/SECURE_RESEARCH.md#reproducible-evidence)
for the acceptance boundaries.

## What you can do

| Capability | How it works |
|---|---|
| Chat and sessions | Create, rename, and revisit separate conversations with saved messages and token counts. New Session replaces the transcript; the confirmed Clear all session history control deletes saved conversations. |
| Persona, memory and prompt context | Inspect `/prompt`, edit or review proposals for shared `soul.md`, select per-session prompt skills, and save literal operator notes with `/memory`. Chat explains these controls; the operator executes them. |
| Local model readiness | Select an exact installed model tag. Desktop Setup checks chat and planner inventories; optional chat fallback requires the configured model to be listed, not just a reachable endpoint. |
| Chat continuation | `/goal` and `/loop` provide bounded follow-up turns with request limits, completion-token budgets, cancellation, and optional auto-continue. |
| Tool visibility and use | `/skills` and `/tools` distinguish registered adapters from readiness and execution evidence. Console commands invoke backend operations through fixed, validated interfaces; model prose does not become an arbitrary shell command. |
| Web research | Enable exact/wildcard URL permission, bounded discovery and BM25 passage search; run `/web research` for local-model answers with verified quote references, usage and partial coverage. Web access ships off; selection/injection is account scoped. |
| Accounts and API Keys | Fresh `admin` / `admin` requires password replacement. Administrator, Portal operator and Auditor permissions are enforced on API reads and writes. Administrators manage masked saved/active credentials in API Keys. |
| Coding loop | Stage a repository task and inspect files or a plan; confirm an isolated run that proposes bounded edits, runs fixed check profiles in a hard sandbox, and feeds check results back into later attempts. |
| Review and publication | Inspect retained run status and diffs, approve the reviewed tree for a local commit, then separately push and publish a draft PR with a reviewed repository template. |
| Recovery | Rediscover retained jobs and runs after reopening. Worker leases distinguish active work from interrupted runs; reopening does not automatically resume work or replay a publication. |

**There are two different loops:** `/loop` continues chat toward a session goal;
it does not execute repository edits or checks. `/agent` drives the coding
pipeline with its own iteration budget, write policy, and review steps.

## Start the app or server

### macOS desktop (Apple Silicon and Intel)

For a published build, download `CG-Agent-Harness-macos-universal.zip` and
`SHA256SUMS` from [Releases](https://github.com/cgfixit/CG-agent-harness/releases/latest).
For newer main features, download `cg-agent-harness-macos-universal` from a
successful [Bundle](https://github.com/cgfixit/CG-agent-harness/actions/workflows/bundle.yml)
run on `main`; extract the outer Actions archive first. Verify the inner ZIP with
`shasum -a 256 -c SHA256SUMS`, extract it, then open **CG Agent Harness.app** from
Finder, Applications, or the Dock. The app owns a bundled backend on an ephemeral
loopback port; ordinary launch needs no Terminal, external browser, Rust, or Python.

To build the app from source on macOS:

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
rustup target add --toolchain 1.88 aarch64-apple-darwin x86_64-apple-darwin
rustup target add --toolchain 1.90 aarch64-apple-darwin x86_64-apple-darwin
scripts/package-desktop.sh --universal
# dist/CG Agent Harness.app
# dist/CG-Agent-Harness-macos-universal.zip and dist/SHA256SUMS
```

The backend crate pins Rust 1.88 (`rust-toolchain.toml`); the separate `desktop/`
Tauri crate pins 1.90 and is not a workspace member. The build also requires the
macOS developer tools described in [desktop setup](docs/DESKTOP.md). The app is
**ad-hoc signed, universal (Apple Silicon + Intel), and not notarized**. Automated
bundle checks do not establish complete native interaction acceptance; see
[the acceptance record](docs/DESKTOP_ACCEPTANCE.md).

### Standalone server

With Rust 1.88 installed:

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release --locked
./target/release/cgagentharness serve
# Open https://127.0.0.1:8790 with explicit browser trust; see docs/SECURE_RESEARCH.md
```

`serve` accepts `--host` and `--port` (1024–65535). Any bind host that is not a
loopback address is refused, and a request whose `Host` header is not a loopback
name is rejected before routing. The standalone CLI/server is one
self-reexecuting Rust binary, `cgagentharness`. Public commands are `serve`,
`account`, `web` and `tls`; `account`/`web` use the authenticated running service,
while certificate export/renewal is local filesystem-owner administration.
`agentic` is hidden and spawned only by `src/shim` (never
by anything else in the server). `desktop` is a separate hidden subcommand of
the same binary, but the server does not spawn it — the desktop shell
(`desktop/src/backend.rs::Backend::start`, a separate crate) launches it as its
bundled sidecar. Prebuilt CLI binaries for `linux-x86_64` and `macos-arm64` are attached
to Bundle runs; Windows CI and release legs are parked. The desktop shell is a
separate package that bundles this same backend.

On macOS the verification sandbox is Seatbelt; on Linux it is
`unshare --net` (network isolation only, no read-only input confinement); on
Windows it is a Job Object process-tree boundary. A missing or unprobeable backend
fails closed with exit 3 rather than running checks unconfined. The
[setup guide](setup-guide.md) is written for macOS; a Linux standalone server
needs Git, Rust 1.88, and `unshare` instead of the Xcode and app-bundle steps.

### Connect a local model

Chat and the local planner require a running OpenAI-compatible local inference
server. The default is Ollama at `http://127.0.0.1:11434/v1`.

```bash
ollama list
```

Select the **exact installed model tag** you intend to use. The shipped config
references `qwen3.8:27b-mlx`; that is not proof it is installed, and the suffix
does not establish the execution backend. Set `models.local_llm.model` and
`agentic.deepagent_github.model` in your home's `config.yaml` for chat and planner
respectively. `/model use <name>` changes the console's selected chat model and
leaves the planner model alone. A configurable loopback fallback supports another
OpenAI-compatible server (`models.local_llm.fallback`, ships disabled).
Fallback is selected at backend startup, not retried after a failed chat turn.
Desktop **Harness → Setup and recovery → Check installed chat and planner models**
distinguishes an installed tag, missing tag, unavailable inventory, and an endpoint
that was not probed. Inventory is not an inference test; send a short chat after
selection. See [model setup and fallback](setup-guide.md#4-select-an-installed-model-and-check-ollama).

Existing homes keep their settings on upgrade. Mutable config, sessions, notes,
persona, and run evidence live under `~/.CGagentHarness`; an absolute
`CGAGENTHARNESS_HOME` overrides that directory. Replacing the app preserves this
data. Use one server/app per home.

Complete install, configuration, and troubleshooting:
[macOS setup guide](setup-guide.md).

## Work in the console

Start with `/help`, `/status`, `/skills all`, and `/tools`. `/help` prints the
authoritative command list for the build you are running; the full annotated
reference is
[section 7.8 of the setup guide](setup-guide.md#78-slash-command-quick-reference).
For a chat session:

```text
/session new
/goal Explain this project's test strategy
/model use <installed-model-tag>
```

Send your question, then use `/loop 3` for a bounded sequence of continuation
turns (default 3, ceiling 5). The console normally pauses for the operator between
turns; `/loop auto` toggles auto-continue and `/loop stop` cancels it. Server-side
budgets still apply. `/memory` manages optional notes and `/soul` controls persona
context; neither gives the model permission to mutate a repository. A missing
`soul.md` is reported as missing rather than loaded. `GOAL_DONE` is an unverified
model report, not evidence that coding work is complete.

Use `/prompt` to inspect the next chat's system prompt, `/soul edit` for the
confirmed persona editor, and `/skill use <directory-id>` to select optional
bounded prompt context, including the optional seeded `ponytail` and
`karpathy-guidelines` coding skills. `/skill status` shows selection and the last successful
inclusion hashes; `/skill clear` clears it. Repository `.codex/skills` and
`.claude/skills` guide agent development and are separate from these runtime
skills.

`/session new` clears the visible conversation and starts separate message/goal/skill
context. Switching sessions restores only that session’s saved messages. Shared
persona and enabled memory remain shared; your account's permitted web selection
survives session changes. `/prompt` shows the resulting context. Chat knows
the operator commands, but cannot execute them. `/memory` lists real saved notes;
`/memory add <note>` saves literal text, not an instruction to archive every session.

`/tokens` reports the current session tally, `/api` inspects managed-key presence
(`CGAGENTHARNESS_API_KEY`, `GROK_API_KEY`, `ANTHROPIC_API_KEY`,
`DEEPAGENT_API_KEY` — presence and a masked tail only, never a stored secret),
`/users` opens administrator-only account management, and `/harness` lists
retained harness-optimizer runs.

Choose the reset that matches your intent:

| Action | Effect |
|---|---|
| `/clear` | Clear visible output, the staged coding request, displayed-diff tracking and loop state; saved messages remain in this session's model context. |
| **+ new session** or `/session new` | Start a separate conversation; retain old sessions and shared context. |
| **Clear all session history** below **+ new session** | Open a confirmation dialog; confirming deletes saved chats, goals, skill selections and token totals. Shared memory/persona/web context and coding runs remain. |

Session directories, named `.CGagentHarness` homes and dotenv files are Git-ignored
in this repository. Other files in an arbitrarily named custom home are not
automatically protected; keep runtime homes outside source checkouts. Do not
force-add private files: ignore rules do not untrack existing commits, and clearing
history does not erase backups or other applications' retained copies.

To deliberately turn the current goal into coding work, use
`/goal stage <prefix>/<topic>`, inspect the staged request and checks, then
`/agent confirm <reason>`. `/goal task` restores a staged request or shows its
retained job evidence. No coding work starts from ordinary `/loop`.
See [chat, persona, skills and goal controls](docs/CHAT_WORKFLOWS.md) for bounds,
proposal review, recovery and completion semantics. Interrupted persona applies
reconcile proposal status on startup or the next persona operation; recovery
never reapplies text or overwrites a changed document.

`/web` exposes enable/allow/fetch/search/research/cancel/inject controls. See
[secure research](docs/SECURE_RESEARCH.md#url-permission-and-migration) for URL
rules, separately permitted robots traversal, source groups and terminal examples.
`/connectors` is a catalog, not a claim that every listed connector is executable;
use `/tools <name>` to inspect registration and known prerequisites. This harness does not include CyClaw's RAG corpus,
terminal, or fsconnect/sqlconnect/netconnect services.

## Run a coding task

Coding runs require Git on `PATH`, a `gh` at or above `agentic.gh_min_version`
(2.40.0 as shipped) and authenticated for the target repository, the chosen
planner, and the tools for the selected check profiles. `gh` is needed from the
very start, not only to publish: the workspace clone itself goes through
`gh repo clone` via `gh_client::run_read`, which re-checks the minimum version,
so a missing or too-old `gh` fails the run before planning. Cargo dependency
preparation additionally requires Python 3's standard library. A packaged app does not supply repository build dependencies or replace
the execution sandbox. `agentic test` covers only part of that list — config
parsing, workspace placement, the `gh` version, Git on `PATH`, the hard sandbox
and the injection scanner. It does not probe the planner, Python, check-profile
executables or prepared dependencies, so a passing self-test is not a complete
readiness report.

The shipped master, deepagent, and Git-write gates are false. Adjacent fields
such as `mode: write` and `writes_enabled: true` already ship permissive, so read
the whole policy rather than assuming everything is off. To intentionally arm the
pipeline, merge these fields into your existing `config.yaml` and restart:

```yaml
agentic:
  enabled: true            # ships false — master switch
  repo: "owner/name"       # ships empty; explicitly choose a target
  mode: write              # already the shipped value
  writes_enabled: true     # already the shipped value
  deepagent_github:
    enabled: true          # ships false
    allow_git_write_tools: true  # ships false — gates clone writes and push
```

Merge those fields into the existing configuration rather than replacing the
whole file. A typical console sequence is:

1. `/agent run <prefix>/<topic> <instruction>` stages a task and sends nothing.
   Allowed branch prefixes are `claude`, `codex`, `grok`, `kimi`, `CyClaw`,
   `cyclaw`, `agent`, plus any prefix you configure. `/agent read <path>` (up to
   8 paths, optional `#L10-L40` window), `/agent plan` and `/agent checks` select
   context, a reviewed local plan and fixed check profiles; `/agent iterations
   <1-10>`, `/agent pr <n>` and `/agent issue <n>` set the remaining staged
   options. `/agent cancel` drops the staged request only.
2. `/agent confirm <why>` starts clone → plan → bounded edit → sandboxed check →
   feedback. This authorizes candidate work in a harness-owned clone, with no
   commit yet. The console polls a detached job; `/agent jobs`, `/agent job <id>`
   and `/agent runs` rediscover retained work, and `/agent stop <id>` requests
   cancellation. A **local** planner may also emit `=== READ path ===` or
   `=== READ path#Lstart-Lend ===` (same selector grammar as `/agent read`).
   Those lines are stripped before proposal parsing, jailed like operator
   `--read-file` values, capped at 6 accepted model selectors per run
   (`MAX_MODEL_READ_REQUESTS`), and shown on the **next** iteration only. They
   are data, not commands: they do not bypass confirm, reason, or write gates.
   Operator-declared paths are never replaced. A **cloud** planner refuses every
   model-requested read so undeclared files are not sent off-machine — pre-stage
   what that run may see with `/agent read`. Accepted and refused selectors are
   audit-logged (`agentic_real_repo_read_request` /
   `agentic_real_repo_read_request_refused`).
3. `/agent status <run-id>` prints the run record and its diff. Approval is bound
   to the reviewed files, modes, and base commit.
4. `/agent approve <run-id> <why>` commits locally. It re-fetches the run and
   commits only against a diff this console has already displayed and that has
   not changed since — so after step 3 it commits on the first call. When the
   diff has not been shown, or the re-fetch returns a different one, it displays
   the diff and withholds approval until you repeat the command. Treat the
   approve call as the commit, not as a preview.
   `/agent reject <run-id>` discards the candidate instead.
5. `/agent push <run-id> <why>` separately pushes the approved commit.
6. `/agent pr-body <run-id>` loads and previews a completed repository PR template;
   `/agent publish <run-id> <why>` separately creates the draft PR from the
   reviewed body. `/agent discard <run-id>` reclaims a terminal run's clone.

There is no `/agent diff`; the diff is part of `/agent status` and of the
approval display.

CLI mutations require `--reason=<why> --confirm`; API mutations require a reason
and `confirm: true`. Combined approval/push/publication is refused.

The console never sends a command. It sends a check-profile **name**, and the
server maps that name to a fixed argv:

| Profile | Fixed command |
|---|---|
| `cargo-test` (default) | `cargo test --quiet` |
| `cargo-clippy` | `cargo clippy --all-targets -- -D warnings` |
| `cargo-fmt` | `cargo fmt --check` |
| `pytest` | `python3 -m pytest -q --tb=short` (`python` on Windows) |
| `ruff` | `python3 -m ruff check --select E,F,I,B,C4,UP,S .` (`python` on Windows) |

An unknown profile name is an error, not a silent skip, and selecting a profile
does not install its prerequisites.

Prepare the selected unchanged `Cargo.lock` before Cargo verification using
[offline Cargo preparation](docs/OFFLINE_CARGO.md), available through desktop
Setup or the preparation helper. Missing preparation refuses before invoking
the planner. On macOS, checks receive read-only candidate/source/toolchain inputs
and fresh writable scratch, with network access denied. Tests must write temporary
state under the supplied temporary directory. See [bounded edits](docs/BOUNDED_EDITS.md)
and [Git approval](docs/GIT_APPROVAL.md) for scope, reviewed-tree checks, and limits.

**Cloud planner data egress.** With `deepagent_github.allow_cloud_providers`
and a provider enabled, `--confirm-online` does not send a short plan prompt —
it sends the real working context for that iteration. `real_repo_loop.rs`
assembles, per attempt: the operator's instruction, the approved plan (if
staged), the prior attempt's check failure feedback, and a bounded excerpt of
every source file the run declared or read via `/agent read` — `edits::collect`
caps each file at `MAX_READ_FILE_CHARS` (4,000 chars), caps the aggregate at
`MAX_TOTAL_READ_CHARS` (12,000 chars), can omit unreadable or oversized files,
and an explicit `#L10-L40` selector sends only that window — plus any GitHub
PR/issue text pulled into session context. A cloud planner never expands that
read set from model `=== READ ===` output (`apply_model_read_request` refuses
with `cloud_proposer`); requested reads displace rather than accumulate under
the same char budgets on a local planner. `CloudProposerClient::invoke`
passes that assembled prompt through `sanitize_handoff` — an injection scan
and secret-pattern redaction pass, `policy.privacy.redact_secrets_like` — and
then sends it to the selected provider's API (`api.x.ai` or
`api.anthropic.com`). Redaction catches known secret shapes; it does not
strip proprietary source code, comments, or issue content, and none of that
data is reviewable before it leaves the machine. Treat enabling a cloud
provider as authorizing repository-content egress for every subsequent
`--confirm-online` run, not as a one-time low-risk toggle.

## Optional credentials and enforced boundaries

Fresh homes set `auth.enabled: true` and `tls.enabled: true`. Sign in with
`admin` / `admin`, then replace the password before using the portal. Account
roles protect all operational routes, including reads. Secure cookies, same-origin,
CSRF, rate limits and every existing coding/write/publication gate remain enforced.
The harness API key is optional metadata and never grants account access. The old
key-required setting is deprecated. Forwarded/proxy connections are refused.

Existing homes keep their configuration. Valid legacy accounts migrate to private
transactional SQLite, retaining hashes and permissions; initialized missing or
corrupt stores fail closed. Research is account scoped; chat sessions, jobs, notes
and persona remain shared portal resources. See [secure research and migration](docs/SECURE_RESEARCH.md)
for the role matrix, TLS trust/renewal, terminal login, exact/wildcard URL rules,
research budgets and API Keys saved-versus-active status.

The HTTP server reaches agentic execution only by spawning one of twelve
whitelisted actions through `src/shim`. The bar is wider than the server module:
`tests/invariant_guard.rs` scans all four console-side trees — `src/server`,
`src/shim`, `src/llm` and `src/common` — for four literal substrings
(`crate::agentic`, `agentic::`, `super::agentic`, `use crate::agentic`), and
scans the pipeline for four matching substrings back. It is a literal-text
scan, not a full import-graph analysis: a grouped or aliased form such as
`use crate::{agentic as pipeline}` would not contain any of those substrings
and would not be caught. Child exit codes are the entire
interface: `0` ok, `2` failed, `3` env/config, `4` write refused. A non-zero
child exit is HTTP 200 with `ok=false`; only shim failures map to 400/502/504
and a disabled layer to 409.

Model output is judged before it lands: `real_repo_loop.rs` applies the
injection, edit-budget and protected-path decisions, and
`workspace.rs::apply_proposal` rechecks protected destinations and aggregate
size before staging, with exact-content binding on each edit.
`deepagent_github.protected_write_paths` is a refusal list — a candidate
touching one of those destinations is rejected — not a scope that edits must
stay inside. `writer.rs` is a different gate: it guards the single executable
GitHub write op (`gh pr create`, always `--draft`). Quoted YAML `"true"` does
not enable a gate.

Chat reports upstream HTTP failures as `502 HARNESS_LLM_ERROR` with the model
server's status code. It drops error bodies without reading or logging them,
including at debug level, so a stalled error body does not hold the chat gate.
See [troubleshooting](setup-guide.md#12-troubleshooting) for diagnosis.

`CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` disables repository and PR mutations when
inherited by the process; it is AND-ed with policy and can only disable, never
enable. Setting it in another shell does not affect an existing process. Revoking
YAML policy blocks later mutation boundaries; it does not cancel a command already
executing. Native sandbox and descendant-cleanup limits are recorded in
[INVARIANTS.md](INVARIANTS.md) and
[process lifecycle](docs/PROCESS_LIFECYCLE.md).

`/web off` excludes saved web context from subsequent chat and prompt previews;
`/web on` can resume it. `/web forget` deletes the saved extract and context.
A completed search with no hits clears your account's last/injected selection to
prevent stale reuse; it does not erase the shared public-document cache.
See [web setup and controls](setup-guide.md#76-web-fetch-search-and-injected-context) for bounds.

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
| [Backend CI](.github/workflows/ci.yml) | PRs, `main`, and reusable release verification: rustfmt, Clippy with warnings denied, `cargo deny`, an MSRV check against `rust-version`, the I6 invariant source scan, a Chrome slash-command browser acceptance job, Rust/public-backend tests on Linux and macOS, a real-repo-run smoke, and release builds with verified CLI packaging. |
| Coding and chat regression tests | Write-policy revocation, exact edits, clone jail, reviewed Git trees, detached-job cancellation, session/goal gates, loop budgets, and release of failed/cancelled chat claims. See [`tests/`](tests/). |
| Native Cargo acceptance | Required macOS tests prepare locked dependencies, then exercise real Seatbelt restrictions and fixed Cargo checks. Linux (`unshare --net`) and Windows (Job Object) backends do not establish equivalent confinement. |
| [Desktop CI](.github/workflows/desktop.yml) | Bundle PRs/`main`, tag releases, and manual runs reuse this job to build the universal app on Rust 1.90, run desktop policy and packaged-backend tests, verify signatures/checksums and the extracted bundle, then retain the universal ZIP and checksums as artifacts. |
| [Bundle](.github/workflows/bundle.yml) | Packages the loopback CLI for `linux-x86_64` and `macos-arm64` alongside the universal desktop job, checksums each artifact, and re-verifies it after extraction. |
| Workflow and source checks | CodeQL, DevSkim, Gitleaks secret scanning, and PR-template/base-branch checks are separate workflows. Workflow changes additionally trigger actionlint/zizmor. |
| [Release](.github/workflows/release.yml) | Every 12 hours at 08:17 and 20:17 UTC, publish the next patch as Latest only if `main` differs from the latest release **and** that SHA already has a successful Bundle run. Backend CI, CLI packaging, universal desktop checks, and downloaded checksums still gate publication. Stable tags and manual preview/publish are also supported. See [release controls](docs/RELEASING.md). |

CI uses deterministic model fixtures and blanks cloud planner keys. Passing it
does not prove real-model quality, complete native GUI behavior, or notarized
distribution. Live Ollama smoke and [native acceptance](docs/DESKTOP_ACCEPTANCE.md)
cover different evidence. Windows CI/release legs remain parked.

## Development and reference

Repository guidance lives in [`.codex/skills`](.codex/skills) for Codex and
[`.claude/skills`](.claude/skills) / [`.claude/commands`](.claude/commands) for
Claude Code. Start with `cgagentharness-project-guidance` and `fable-protocol`;
use the task-specific verify, optimize, release or guard skill listed in
[AGENTS.md](AGENTS.md). These are development instructions, separate from app
runtime `/skill` context, and loading one authorizes nothing. Contributor PRs use
a driver-prefixed branch, target `main`, open as drafts, and are validated with
`scripts/check-pr-template.sh` against
[the actual PR template](.github/PULL_REQUEST_TEMPLATE.md). A change to
`src/shim`, `src/server/guards.rs`, `src/server/headers.rs`,
`src/agentic/writer.rs`, `src/agentic/executor/sandbox.rs`,
`src/agentic/workspace.rs`, or `assets/config.default.yaml` additionally needs an
explicit invariant statement in the PR body.

| Document | Purpose |
|---|---|
| [setup-guide.md](setup-guide.md) | macOS setup, output style, slash commands, skills, memory/web configuration, connector limits and troubleshooting |
| [AGENTS.md](AGENTS.md) | Contributor rules and required verification |
| [INVARIANTS.md](INVARIANTS.md) | Process isolation, guard chain, write policy, clone jail, and sandbox guarantees |
| [SECURITY.md](SECURITY.md) | Supported surface and how to report a vulnerability |
| [assets/config.default.yaml](assets/config.default.yaml) | Shipped settings and configurable budgets |
| [docs/SECURE_RESEARCH.md](docs/SECURE_RESEARCH.md) | HTTPS trust/renewal, SQLite migration, roles, terminal commands, URL rules, research budgets and API Keys |
| [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) | Toolchains, lockfiles, feature choices, retained pins and dependency drift checks |
| [docs/CHAT_WORKFLOWS.md](docs/CHAT_WORKFLOWS.md) | Chat-first defaults, persona/skill commands, goal staging, and recovery |
| [docs/BOUNDED_EDITS.md](docs/BOUNDED_EDITS.md) | Exact-content edit format, scope and budget limits |
| [docs/GIT_APPROVAL.md](docs/GIT_APPROVAL.md) | Approval binding, commit/push/publish separation |
| [docs/CONSOLE_JOBS.md](docs/CONSOLE_JOBS.md) | Asynchronous console runs and browser acceptance |
| [docs/PROCESS_LIFECYCLE.md](docs/PROCESS_LIFECYCLE.md) | Child-process timeouts, cancellation and descendant-cleanup limits |
| [docs/OFFLINE_CARGO.md](docs/OFFLINE_CARGO.md) | Preparing a locked dependency set for sandboxed Cargo checks |
| [docs/DESKTOP.md](docs/DESKTOP.md) | App ownership, setup/recovery, packaging, and distribution limits |
| [docs/DESKTOP_ACCEPTANCE.md](docs/DESKTOP_ACCEPTANCE.md) / [docs/MAC_ACCEPTANCE.md](docs/MAC_ACCEPTANCE.md) | What native acceptance has and has not established |
| [docs/RELEASING.md](docs/RELEASING.md) | Release cadence, tagging, and manual preview/publish |
| [docs/PORT_PARITY.md](docs/PORT_PARITY.md) and [docs/parity/](docs/parity) | CyClaw↔harness port ledger and `scripts/parity-status.py` |

The harness originated as a Rust port of the console and agentic pipeline from
[CyClaw](https://github.com/cgfixit/CyClaw). Its focus here is local coding,
chat context, controlled tools, and explicit operator review.
