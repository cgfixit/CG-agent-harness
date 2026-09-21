# Coding pipeline

Optional, disarmed-by-default coding loop. Index: [setup-guide.md](../setup-guide.md). Edits: [BOUNDED_EDITS.md](BOUNDED_EDITS.md). Git: [GIT_APPROVAL.md](GIT_APPROVAL.md).

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

If you skipped [GitHub CLI](INSTALL.md#26-optional-for-the-coding-pipeline-only-github-cli), do it now:

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
the first time you ran `serve` in [first run](INSTALL.md#6-first-run) — it's a copy of this repository's
`assets/config.default.yaml`). Open it in your editor of choice:

```bash
open -e ~/.CGagentHarness/config.yaml
```

Merge the following fields into the existing configuration. Fresh homes set `agentic.repo: ""`; select your own `owner/name` before enabling
the coding layer. Repository package metadata is not a runtime target. The repo
field is a selector string, not a boolean gate. Use the installed model chosen in [model setup](MODELS.md):

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
private `desktop-tools.json` described in [desktop setup](DESKTOP.md).

Preparation defaults to offline. If locked dependencies are missing, review the
repository and rerun with `--online` only while explicitly allowing engineering
dependency access. Actual checks remain offline, with fresh bounded writable
locations and read-only prepared sources. See [Offline Cargo](OFFLINE_CARGO.md).

Back in the app or browser console:

1. `/agent run codex/fix-topic Describe the intended change` stages an instruction.
2. `/agent checks` lists supported profiles. The server default is `cargo-test`;
   `/agent checks cargo-fmt` is an example explicit selection.
3. `/agent read src/lib.rs#L40-L80` declares a bounded existing-file window
   (up to 8 staged paths). During a **local** planner run the model may also
   emit `=== READ path ===` or `=== READ path#Lstart-Lend ===`. Those lines
   are stripped before proposal parsing, jailed like operator `--read-file`
   values, capped at 6 accepted model selectors per run, and shown on the
   **next** iteration only. After canonicalization, denied basenames
   (`agentic.deepagent_github.denied_read_basenames`) refuse both model READ
   and operator `--read-file` (`sensitive_basename`). They are data, not
   commands, and do not bypass confirm, reason, or write gates. The deny-list
   is not a secret scanner; the clone jail is not a secrets control.
   Operator-declared paths are never replaced. A **cloud** planner refuses
   every model-requested read so undeclared files are not sent off-machine —
   pre-stage what that run may see here. An unclosed `=== FILE ===` /
   `=== EDITS ===` block in the model reply leaves later READ lines in the
   body instead of extracting them.
4. `/agent confirm <reason>` submits a job and returns its ID immediately.
5. `/agent job <job-id>` resumes monitoring after refresh and account login; the optional harness key may stay empty.
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
to get a proposal accepted. See [Bounded edits](BOUNDED_EDITS.md).

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
proved. Continue with the separate push and PR-body/publication steps in [Run it from the console](#93-run-it-from-the-console).

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

## Cloud planner data egress

With `deepagent_github.allow_cloud_providers`
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
with `cloud_proposer`); a local planner still refuses denied basenames
(`sensitive_basename`) after the jail. Requested reads displace rather than
accumulate under the same char budgets on a local planner.
`CloudProposerClient::invoke`
passes that assembled prompt through `sanitize_handoff` — an injection scan
and secret-pattern redaction pass, `policy.privacy.redact_secrets_like` — and
then sends it to the selected provider's API (`api.x.ai` or
`api.anthropic.com`). Redaction catches known secret shapes; it does not
strip proprietary source code, comments, or issue content, and none of that
data is reviewable before it leaves the machine. Treat enabling a cloud
provider as authorizing repository-content egress for every subsequent
`--confirm-online` run, not as a one-time low-risk toggle.

Cloud proposals require an explicit normal completion: Grok
`choices[0].finish_reason: "stop"` or Claude `stop_reason: "end_turn"`.
Truncated, missing, malformed, or continuation states are refused before any
proposal is applied, even when the returned prefix contains a complete edit
block. These billed 2xx responses are recorded once as `failed_after_billing`
and are not retried automatically. Reduce the task or adjust
`agentic.deepagent_github.planner_max_tokens` before retrying a truncated run.

## Isolation check

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
stay inside. `deepagent_github.denied_read_basenames` is a separate READ
deny-list (not a secret scanner; jail ≠ secrets). `writer.rs` is a different gate: it guards the single executable
GitHub write op (`gh pr create`, always `--draft`). Quoted YAML `"true"` does
not enable a gate.
