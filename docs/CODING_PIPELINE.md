# Coding pipeline

Optional, disarmed-by-default coding loop. Index: [README.md](../README.md).

## 9. (Optional, advanced) Arm the coding pipeline

Coding execution is optional and disarmed by default. Complete this section only
when you intend to authorize work in a selected repository.

The pipeline clones a GitHub repository, asks a local or optional cloud model to
propose a patch, verifies it in a sandbox, and waits for personal review before
commit, push, and draft pull request creation.
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

The first `serve` creates `~/.CGagentHarness/config.yaml` from
`assets/config.default.yaml`. Open it:

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

Master enablement, write mode, writes-enabled, deepagent enablement, and clone
write capability must all permit an operation. The emergency switch only disables
writes. Approval, push, and publication each require a reason and confirmation;
an earlier approval does not authorize a later boundary.
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
repository and rerun with `--online` while allowing dependency access. Checks
remain offline, with fresh bounded writable locations and read-only prepared
sources. See [offline Cargo verification](#offline-cargo-verification).

Back in the app or browser console:

1. `/agent run codex/fix-topic Describe the intended change` stages an instruction.
2. `/agent checks` lists supported profiles. The server default is `cargo-test`;
   `/agent checks cargo-fmt` is an example explicit selection.
3. `/agent read src/lib.rs#L40-L80` declares a bounded existing-file window
   (up to 8 staged paths). A **local** planner may also request reads (see
   [Bounded edits](#bounded-edits)); a **cloud** planner refuses every
   model-requested read so undeclared files are not sent off-machine —
   pre-stage what that run may see here.
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
See [Git approval and publication](#git-approval-and-publication).

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

Values `1`, `true`, `yes`, and `on` block writes regardless of YAML. The switch
cannot enable writes. An export in another shell does not update a running process.
Restart with the switch set, or revoke `agentic.writes_enabled` to block later
mutation boundaries in an active child. Neither action cancels a running Git
command or check. Scope and budget changes also refuse later writes.

## Bounded edits

Declare each file needed by the task with `--read-file src/lib.rs`, or select
an inclusive line window with `--read-file 'src/lib.rs#L590-L630'`. The same
selector can be staged with `/agent read src/lib.rs#L590-L630`. A `#Lstart-Lend`
suffix is reserved for this interface. Select one window per file. Missing,
unsafe, oversized and omitted selections are reported explicitly.

During a **local** planner run the model may also emit `=== READ path ===` or
`=== READ path#Lstart-Lend ===`. Those lines are stripped before proposal
parsing, jailed like operator `--read-file` values, capped at 6 accepted model
selectors per run, and shown on the **next** iteration only. They are data, not
commands, and do not bypass confirm, reason, or write gates. Operator-declared
paths are never replaced. An unclosed `=== FILE ===` / `=== EDITS ===` block in
the model reply leaves later READ lines in the body instead of extracting them.

After clone-jail canonicalization, both operator `--read-file` and local
planner `=== READ ===` selectors are refused when any path segment is `.git`
name-equivalent (`is_dotgit_name`). They are also refused (`sensitive_basename`)
when the final path segment matches `agentic.deepagent_github.denied_read_basenames`
(default `.env` / `.env.*`, `*.pem`, `id_rsa` / `id_rsa*` / `id_ed25519*` /
`id_ecdsa*`, `credentials*` / `credentials.json`, `.npmrc`, `.netrc`, `*.p12`;
name-equivalence folded). The deny-list is not a secret scanner: secrets can use
arbitrary names. The clone jail stops path escape and clone Git metadata reads.
It is not a secrets control.

Read ceilings are 4,000 characters per file and 12,000 total, with a
256,000-byte internal file-read ceiling. The exact displayed span is retained
separately from headings. These are character budgets, not a claim of exact
Qwen token counts. Context includes the full-file SHA-256 and a completeness
flag. A later attempt takes a fresh snapshot; having written a file earlier
never authorizes blind replacement. Predeclare new paths when corrections may
need to read them in later attempts.

For existing large files the planner can emit:

```text
=== EDITS ===
{"edits":[{"path":"src/lib.rs","sha256":"<provided hash>","old":"<unique displayed text>","new":"<replacement>"}]}
=== END EDITS ===
```

Use JSON escapes and one edit per file. `old` must be nonempty, unique in the
whole original (including overlapping matches), and contained in the actual
displayed excerpt. Hash mismatch, hidden/ambiguous text, duplicate destinations,
invalid JSON, mixed block formats and incomplete trailing markers reject the
whole proposal. `FILE` blocks are for new files or fully displayed current
originals; they cannot overwrite a partially viewed file.

Model output is judged before it lands: `real_repo_loop.rs` applies the
injection, edit-budget and protected-path decisions to all final replacement
content, and `workspace.rs::apply_proposal` rechecks protected destinations and
aggregate size before staging, with exact-content binding on each edit.
Response bytes are bounded by the handoff ceiling (`max_handoff_chars`); large
original files still count toward the final write budget. Raw, canonical and
landed destinations pass protected-path policy, including Unicode and case
equivalents. `deepagent_github.protected_write_paths` is a refusal list — a
candidate touching one of those destinations is rejected — not a scope that
edits must stay inside. Proposal writes refuse symlink ancestors/leaves.
Protected tests and build configuration stay protected, with no tests-directory
exemption: inline existing tests may be read, and operator-authored regression
PRs are the reviewed mechanism for new protected tests. Do not weaken that
policy to get a proposal accepted.

Every file's existence/content precondition is checked before staging. Retained
parent directory capabilities avoid following a newly substituted symlink while
installing a leaf. Replacements are staged, current originals rechecked, and
renamed; ordinary later application errors roll back earlier replacements. A
failed rollback is fatal, quarantines the run and preserves its recovery backup.
Checks see the resulting batch only after successful application. Failed Cargo
checks include bounded stdout and stderr in the next attempt's feedback.

This is not crash-atomic multi-file commit or atomic compare-and-swap against an
adversarial concurrent writer: comparison and rename are separate syscalls, and
an externally relocated directory remains reachable through its open handle.
Do not concurrently edit an active disposable clone. Empty newly-created parent
directories may remain after staging failure.

`tests/exact_edits.rs` drives a greater-than-12-KB Rust edit through a real
offline Seatbelt Cargo failure and exact correction while preserving unrelated
bytes. Other tests cover multi-file edits, stale state, protected aliases,
budgets, parser refusal, and rollback after the first rename. Scripted planners
test execution semantics; real-Qwen acceptance remains separate.

## Optional repository retrieval

To supply relevant files without hand-selecting every path, merge this into
the active home's existing `agentic.deepagent_github` mapping and restart:

```yaml
agentic:
  deepagent_github:
    retrieval:
      enabled: true
      max_files: 256
      max_index_bytes: 2000000
      top_k: 3
      excerpt_lines: 40
      token_budget: 2048
```

It ships **off**; absent, quoted or non-boolean `enabled` stays off. This is
local-planner-only and grants no execution authority. The same staged request,
`/agent confirm <reason>`, sandboxed checks, diff review and separate approval
steps remain necessary. Cloud planners never receive automatically retrieved
files. `/agent read` and accepted local model READ requests take priority.

The child lists tracked and non-ignored untracked paths, examines at most `max_files`
paths in lexical order, and builds a separate Tantivy RAM index over bounded
UTF-8 source. It retrieves at most `top_k` files using up to 32 path-boosted
terms. Excerpts center on matches; lexical search does not guarantee repository
coverage. Use explicit windows for large repositories, synonyms, or long lines.
Each step rebuilds the index and re-reads each selected file; its full SHA-256
must still match before inclusion.

| Setting | Accepted range | Bound |
|---|---:|---|
| `max_files` | 1–1024 | Candidate paths examined per step, including paths later excluded |
| `max_index_bytes` | 1–16,000,000 | Total indexed UTF-8 source bytes per step |
| `top_k` | 1–8 | Additional candidate files |
| `excerpt_lines` | 1–200 | Maximum line window, also clipped by character budgets |
| `token_budget` | 256–8192 | Conservative UTF-8-byte reservation including rendered headers |

Reservations are not vendor token usage. Existing 4,000-character per-file and
12,000-character aggregate read limits still apply. The full-file read ceiling,
clone capability and denied-basename rules apply before indexing. Leaf symlinks,
paths escaping the clone, `.git` name-equivalent segments, binary/invalid UTF-8, oversized files, unsafe path text and
injection-scanner hits are excluded. Ignored untracked files are not enumerated;
tracked files remain candidates even if a later ignore rule names them. This
is not a general secret scanner: secrets can appear in arbitrary source files.

`/agent status <run-id>` displays each step's indexed counts, whether a bound
limited selection, and a table of selected lines, full-file SHA-256 and reserved
bytes. The run record and audit retain only this metadata. Source is not copied
to the public web cache or indexed with sessions/structured memory. Review the
actual diff; retrieval does not approve it or enable whole-file replacement
when only an excerpt was shown.

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

## Offline Cargo verification

The operator prepares dependencies; untrusted checks never fetch them. Rustup
toolchains use their resolved sysroot; the preparation command runs from the
selected repository, so its toolchain selection applies. Homebrew does not
automatically honor rust-toolchain.toml. Provision required tools/components
explicitly before preparing. No toolchain or model is installed by the harness.

Prepare once per lockfile and application home, as in
[Run it from the console](#93-run-it-from-the-console). The offline default and
`--online` both require an existing Cargo.lock and installed cargo/rustc/rustdoc.
Neither runs build scripts or tests. Cargo vendor uses locked resolution and
produces a separate source snapshot under
`data/agentic/cargo-prepared/<lock-sha256>`. Existing snapshots are refused,
never overwritten; use a separate disposable home to reprepare. Do not place the
home or snapshot inside the candidate. The snapshot records the concrete
toolchain and macOS SDK/linker/runtime paths. Moving/removing those
installations requires preparation again. Preparation adds no Rust
dependencies. Python 3 standard-library availability is an explicit setup
requirement; Cargo remains responsible for lock/checksum validation. See
upstream [Cargo vendor](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html)
and [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html).

Run the harness with `CGAGENTHARNESS_HOME` set to that application home. Keep
Cargo.lock unchanged in proposals; new dependencies require separately reviewed
preparation. No model retry can make uncached dependencies available offline.
Missing components, sources, permission failures and Cargo timeouts return setup
errors; compile/test failures remain feedback for a correction.

### Execution boundary

Every verification creates and removes owned scratch containing HOME, CARGO_HOME,
build outputs and temporary files. The real user home and credentials are not
passed through. Cargo receives the prepared source configuration, explicit
compiler/rustdoc, `--frozen` (except `cargo fmt`), and `CARGO_NET_OFFLINE=true`. SDKROOT and the direct
Clang linker avoid xcrun attempting to write into the operator's cache.

On macOS, Seatbelt denies network operations including loopback. Candidate files,
`.git`, vendor sources, toolchain and runtime inputs are read-only. File data reads
are restricted to those inputs, scratch, named OS/SDK roots, the root directory
itself and the system OpenSSL configuration file needed by Homebrew Cargo. The
whole home, `/usr/local`, `/opt/homebrew`, `/private` and `/Library` are not granted.
Only scratch is writable. Tests that generate source-tree artifacts must instead
use the supplied temporary directory; granting candidate writes would also expose
Git metadata and hooks to build scripts.

File metadata discovery remains allowed. OS libraries and SDKs are readable.
Seatbelt is not a memory/disk quota; pipe capture and escaped child process
groups remain separate work. Linux bubblewrap applies its own allowlisted
read-only filesystem with writable scratch; the `linux-netns` fallback and
Windows Job Objects provide no filesystem policy. Windows preparation uses
platform executable suffixes/path separators but native Windows acceptance is
unverified.

### Required native gate

```sh
# Authorized preparation, outside verification:
cargo fetch --locked
cargo fetch --locked --manifest-path tests/fixtures/cargo-sandbox/Cargo.toml
# Actual native offline verification, with cloud test credentials empty:
GROK_API_KEY= ANTHROPIC_API_KEY= DEEPAGENT_API_KEY= CARGO_NET_OFFLINE=true \
  cargo test --test macos_cargo --locked --offline -- --nocapture
```

Run on macOS outside an outer sandbox that prohibits nested Seatbelt. Missing
sandbox capability fails this gate. The tests compile minimal and serde-bearing
fixtures, execute build scripts/unit tests/doctests, deny synthetic outside and
symlink reads, deny candidate/Git/cache/outside writes, deny access to an active
loopback listener, repeat execution, check scratch cleanup and fail on stale or
missing prepared inputs. Synthetic markers contain no real secrets. This gate
proves Cargo execution and tested boundaries, not complete harness or real-model
acceptance.

## Git approval and publication

The retained clone is data, including its local Git configuration and index.
Approval must commit the bytes and modes that were reviewed. Separate push and
publication must continue to refer to that approved commit.

Ordinary Git commands do not guarantee that binding. Pre-staged files can enter
a commit; a literal `*` filename can match neighbors; hooks can run despite
`--no-verify`; clean filters can replace index bytes; and replacement refs can
reinterpret the base tree. Remote trees do not normally install `.git/config`
or hooks, but contaminated retained metadata can. Index and pathspec failures
need no hook.

Workspace operations, manifest reads, and disposable verification use one Git
helper. It clears ambient variables, global and system config, and system
attributes; disables hooks, fsmonitor, signing, maintenance, and replacement
objects; and uses literal pathspecs. Local config is limited to ordinary clone
metadata. Includes, custom programs, filters, transport rewrites, and unsupported
metadata refuse without echoing values. Read-only diff inspection accepts inert
local external-diff and fsmonitor settings.

Authenticated `gh repo clone` also receives isolated Git settings and an empty
Git template before initial checkout. Publishing Git operations use the installed
`gh auth git-credential` helper for HTTPS rather than ambient Git credential
commands. The selected GitHub.com repository also accepts an SSH origin, which
can use the operator's SSH configuration and keys. Custom Git credential
helpers and enterprise hosts are not accepted; absolute local remotes remain
available for offline runs.

Approval refuses staged changes and index locks. Under the normal index lock, it
builds a private index from the accepted base, inserts only accepted raw blobs
and modes, and writes that tree. Replacement objects and grafts cannot reinterpret
the base. Acceptance digests include mode; older records need a new run. Failure
after a durable branch commit is indeterminate and requires inspection.

Content-transforming attributes (`filter`, `text`, `eol`, `working-tree-encoding`,
`ident`) are refused for selected changes. This deliberately includes LFS and
newline-normalizing workflows: silently bypassing their transformations could
commit incorrect representations. Supporting them requires a separately reviewed
acceptance contract for both worktree and committed representations.

Run records pin the origin and approved commit. Push refuses changed branches or
destinations and sends an object-ID refspec to the pinned URL. Publication reads
the remote branch and refuses mismatches. Every operation rechecks write policy,
reason, and confirmation. Older unpinned records cannot be pushed; inspection and cleanup remain available.

`tests/git_approval.rs` uses disposable Git repositories to cover unrelated staged
changes, glob filenames, hooks, filters, index locks, file modes, branch and
destination drift, and replacement objects. Other tests cover policy revocation,
inspection, CLI and API paths, and local-bare operation. This is not an exhaustive
Git security audit.

The index lock coordinates normal Git writers, not a hostile process that ignores
locks and races filesystem metadata. Remote readback is point in time; another
authorized actor can later change the branch. Abrupt death and escaped descendants
retain the [process-lifecycle](PROCESS_LIFECYCLE.md) limits. Global Git settings
remain unchanged.

## Isolation check

The HTTP server reaches agentic execution only through twelve actions in
`src/shim`. `tests/invariant_guard.rs` checks `src/server`, `src/shim`, `src/llm`,
and `src/common` against the agentic tree in both directions. It checks literal
agentic paths and parses `use` bindings, including grouped, nested, renamed,
inline-module, and function-local imports. Comments do not trigger the guard.
Child exit codes are the entire
interface: `0` ok, `2` failed, `3` env/config, `4` write refused. A non-zero
child exit is HTTP 200 with `ok=false`; only shim failures map to 400/502/504
and a disabled layer to 409.

Model output is judged before it lands; see [Bounded edits](#bounded-edits).
`writer.rs` is a different gate: it guards the single executable GitHub write
op (`gh pr create`, always `--draft`). Quoted YAML `"true"` does not enable a
gate.
