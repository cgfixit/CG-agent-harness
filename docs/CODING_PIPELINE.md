# Coding pipeline

## 9. (Optional, advanced) Arm the coding pipeline

The optional pipeline owns a disposable clone, plans bounded edits, runs a
fixed check and retains the candidate for human review. The five write
gates include `agentic.enabled`, `mode: write`, `writes_enabled`,
`deepagent_github.enabled` and `allow_git_write_tools`. The master, child
and Git write switches ship false. Each mutation also needs reason and
confirmation. See [write invariants](../INVARIANTS.md#every-write-to-a-repository-is-gated).

### 9.1 Install and authenticate the GitHub CLI

Install `gh` for remote work, then run `gh auth login` and `gh auth status`.
An authenticated CLI grants no harness write permission.

### 9.2 Edit the configuration file

Merge the approved gates and explicit `agentic.repo` (`owner/name`) into
the active `config.yaml`. Choose the exact installed local tag, provider
and endpoint under `agentic.deepagent_github`. Restart after changing planner
or retrieval settings. Console, jobs and schedules bind to startup planner
settings. The child refuses drift before repository work, and an Ollama
cloud model (a `:cloud`/`-cloud` tag or `/api/show` `remote_host`) as the local
planner model.
Leave the cloud-provider parent and child flags closed unless the
explicit cloud path is needed. Local and cloud planners have independent
provider settings and endpoints. `gh_timeout_sec` and `gh_retries`
must fit within the run setup deadline. See
[shipped defaults](../assets/config.default.yaml).

### 9.3 Run it from the console

1. Prepare the repository's lock with
   `python3 scripts/prepare-cargo.py /path/to/repo /path/to/home`
   or the desktop [offline chooser](DESKTOP.md#finder-setup-and-local-inference).
2. Stage with `/agent run <branch> <instruction>`. Select a fixed
   `/agent checks` profile and bounded `/agent read <path>[#Lx-Ly]` if needed.
3. Submit with `/agent confirm <reason>`. Monitor `/agent job <job-id>`.
4. Inspect `/agent status <run-id>` and the complete diff.
   `/agent approve <run-id> <reason>` commits the approved candidate.
5. `/agent push <run-id> <reason>` authorizes push. Select a reviewed
   Markdown body with `/agent pr-body <run-id>`. `/agent publish <run-id>
   <reason>` opens a draft.

`/agent cancel` clears staging. `/agent stop` cancels submitted work; a stopped scheduled run is audited
`agent_schedule_failed`, status `cancelled`.
`/agent reject` rejects a candidate. `/agent discard` removes terminal work
from its owned workspace. CLI and API writes require explicit confirm and
reason. Publication also requires a reviewed body. See
[console commands](CONSOLE.md#78-slash-command-quick-reference).

### 9.4 Stage a session goal as a coding task

`/goal stage <branch>` stages the current saved goal for review.
`/goal task` shows linkage and recovers unsubmitted staging.
Submission binds the current goal and stage and refuses duplicates,
changed goals and sessions with attachment pins. Only `/agent confirm`
starts execution. Changing the goal does not cancel authorized work.
`awaiting_review` requires a decision. `completed_local` records local
verification and approval. Push and draft publication are separate.

### 9.5 The kill switch

Set `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` before launch. `true`, `yes`
and `on` also refuse writes. Disk policy revocation is rechecked at later
mutation and planner boundaries. Neither interrupts a Git command or check
already running.

## Bounded edits

Operator reads bind repository-relative paths or inclusive line windows
(up to eight staged selectors). Local planners may ask for up to six
`=== READ path ===` selectors per run, available on the next iteration.
Canonical clone jail, Git-metadata exclusions and denied basenames constrain
both. The read limit is 4,000 characters per file and 12,000 overall.
Full-file SHA-256 and excerpt completeness accompany content.
The worker reports absent, unsafe or oversized selectors. A later
iteration re-reads its source. An unclosed edit/file block keeps nested
READ markers within that malformed block. Denied basenames protect known
sensitive paths. They cannot classify secrets in arbitrary source files.

`FILE` blocks can write new or fully displayed files. `EDITS` binds an
existing file hash and one nonempty unique old span within the shown
excerpt to a replacement. Malformed, ambiguous, mixed or incomplete
proposals refuse as a batch. Injection, size and protected-path checks
apply to final content and destinations. Symlinks refuse. The worker
stages replacements, rechecks originals and rolls back earlier writes
on failure. Failed rollback quarantines the run.
Protected controls apply to raw, canonical and landed destinations,
including equivalent filesystem names. Checked replacements and checks
are separate phases. A failed check feeds bounded output to the next
eligible iteration. Never edit a live
workspace concurrently.

## Optional repository retrieval

Literal `agentic.deepagent_github.retrieval.enabled` ships true.
The local planner gets bounded Tantivy matches from tracked and
nonignored untracked files after explicit/model-requested reads. Each
selection is re-read and hash-checked. Clone jail, denied basenames and
content checks apply before indexing. Cloud receives no retrieval.
The index examines candidates in lexical order and skips unsafe paths,
non-UTF-8 and scanner refusals. An oversized path listing disables retrieval
for that step. Shipped bounds come from `retrieval` in the YAML.
Run status records selected path, span, hash and limits. Use explicit
windows for missing lexical matches.

## Cloud planner data egress

Enable the parent and selected provider under `deepagent_github`, restart,
then stage `/agent planner grok confirm-online` or `claude confirm-online`.
The child rechecks provider authority before sending. Schedules refuse
cloud planners. Each attempt can send the instruction, operator-staged
plan and source windows, selected PR/issue text, and check feedback.
The local handoff scans injection and redacts known secret shapes, while
ordinary repository content can leave. Cloud cannot request additional
reads. Grok requires `finish_reason: "stop"` and Claude `end_turn`.
Abnormal billed completions fail without applying the proposal or retrying.

## Offline Cargo verification

`prepare-cargo.py` inventories the operator-selected toolchain and vendors
the committed lock into `data/agentic/cargo-prepared/<lock-sha256>`.
Preparation defaults offline. An explicit `--online` is operator-only.
Existing snapshots are never overwritten. Keep the home outside the
candidate. A changed lock, moved toolchain or SDK requires fresh preparation.
Pin the operator's prepared toolchain via `PATH` and, for rustup,
`RUSTUP_TOOLCHAIN`. Preparation runs outside candidate workspace
configuration and does not run build scripts or tests.
Missing offline inputs are setup errors. Compile/test failures can guide a
new proposal.

### Execution boundary

Cargo uses owned scratch, isolated HOME/CARGO_HOME, pinned compiler inputs,
`CARGO_NET_OFFLINE=true` and `--frozen` except fmt. macOS Seatbelt and Linux
bubblewrap enforce the configured filesystem and network boundary.
Candidate, Git metadata and vendors are read-only. Tests write to scratch.
A Linux network-namespace fallback requires explicit opt-in and cannot
confine the filesystem. Windows Job Objects own lifetime, with no
filesystem confinement. See [process limits](PROCESS_LIFECYCLE.md).

### Required native gate

Prepare cached dependencies outside the check. Run
`cargo test --test macos_cargo --locked --offline` with cloud keys empty
and `CARGO_NET_OFFLINE=true` on macOS with usable Seatbelt.
Linux CI's separate required bubblewrap job exercises real confinement.
A matrix skip is not confinement evidence.

## Git approval and publication

Approval verifies the exact accepted bytes, modes and base, rejects foreign
staged changes, and builds a private index under the normal index lock.
Git helpers isolate environment, config, hooks, filters and replacement
objects. A candidate's local Git config is treated as data, with unsupported
includes, helpers, filters and transport rewrites refused. Unrelated staged
changes are rejected before constructing the accepted commit. Literal
pathspecs and raw blob insertion bind paths, content and executable mode. Content-transforming attributes refuse selected edits. A failure
after a durable commit may be indeterminate. Inspect the recorded commit
and clone before retrying or discarding.

The run records its origin, base branch and approved commit. Push pins the
destination and object ID. Publish reads back the remote branch, then opens
the draft against the recorded base only if it still matches the approved
commit. Old unpinned runs cannot push. Each operation rechecks gates and
requires its own reason and confirmation. The HTTPS path uses the approved
GitHub CLI credential helper. GitHub.com SSH can use the operator's SSH
configuration. Each remote readback describes that moment in time.

## Isolation check

Server calls cross the fixed shim CLI into a child.
[I6](../INVARIANTS.md#i6--process-isolation-between-the-console-and-the-agentic-pipeline)
and `tests/invariant_guard.rs` define and enforce that boundary.
