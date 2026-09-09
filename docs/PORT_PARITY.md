# Port parity and hardening evidence — 2026-09-09

Acceptance is **incomplete**. This ledger distinguishes source comparison,
fixture verification, and real-model acceptance. No whole-port equivalence claim.

## Pinned baseline

- Rust remote: `https://github.com/cgfixit/CG-agent-harness.git`, default `main`,
  clean baseline `7a29186f0726c2e630fd69c94cf0e7576f1218c6`; no open PRs at start.
  Existing operator checkout was clean at `a16eba2e24618a0b94ff461e096df3d1c4800cac`
  and was not changed. Work uses an isolated fresh clone, `codex/consistent-write-policy`.
- Extraction commit: `b698d648032fe52e17277f1fcfcb14d6a87fb32f` (September 7).
  It names Python modules but records no exact Python source SHA.
- Python comparison: `5cfd882d723f20fbf011b46f73ca1a0cbb5cbaf0`, latest ancestor
  dated before the extraction timestamp; **inferred comparison point, not proven
  extraction provenance**. Current remote reference also pinned:
  `d9b0ad9660cc4697bbe66f4afea17f084a37e6d2`. Never compare moving branches.
  Existing operator CyClaw tree has unrelated changes and was not modified.
- Native target: macOS 26.6.2 (25G83), arm64, Apple M5 Pro, 48 GiB physical memory.
  Initial available disk about 161 GiB; swap used about 1.64 GiB. These are one-time
  observations, not available model memory or performance guarantees.
- PATH resolves Rust/Cargo to Homebrew 1.98.0 under `/opt/homebrew/bin`;
  rustup absent on PATH. `rust-toolchain.toml` requests 1.88 but Homebrew binaries
  do not enforce it. Local results do not establish MSRV 1.88; CI has that gate.
  Git 2.55.0 and authenticated `gh` available after outer sandbox access granted.
- Ollama at `http://127.0.0.1:11434`; exact requested tag `qwen3.8:27b`,
  inventory ID `22130167c4c2`, about 17 GB. Reported architecture `qwen35`,
  27.3B parameters, Q4_K_M, maximum model context 262144, thinking/tools/vision.
  Maximum context is metadata, not a recommended runtime setting.
  Distinct installed `qwen3.8:27b-mlx` ID `5642e97495e1` is not interchangeable.
  No backend conclusion follows from either tag. No models changed or pulled.
- Existing default application home contains config/settings, no `.env`,
  `auth.json`, or `soul.md` at inspection. No private contents copied into this PR;
  all tests use disposable homes, repositories, identities, and local mocks.

## Baseline commands and limits

| Command | Exit / evidence | What it does not establish |
|---|---|---|
| `SKIP_LIVE=1 CLIPPY=/opt/homebrew/bin/cargo-clippy scripts/verify-local.sh` inside outer tool sandbox | 101 at Seatbelt test: `sandbox_apply: Operation not permitted`; fmt/clippy passed | Nested sandbox failure is not an application success |
| Same command outside outer sandbox with `CARGO_NET_OFFLINE=true` | 0; fmt/clippy, 122 tests, release build | Live smoke deliberately omitted; some loop units use unconstrained test backend; real smoke uses a text check and mocked planner |
| `cargo-deny check` (private installation of cargo-deny 0.20.2) | 0: advisories, bans, licenses, sources OK; advisory network access only | Not a complete security audit; no production dependency additions |
| Real CLI approved-record push to disposable bare remote under read mode, writes false, emergency disable 1 | Baseline exit 0, remote branch created, record pushed true | Demonstrates policy defect without touching GitHub |
| Baseline GitHub CI associated with exact baseline SHA | CI, Bundle, CodeQL, DevSkim, gitleaks succeeded | Does not establish local Cargo sandbox readiness or model usability |

The native baseline reported no ignored tests. Hard-sandbox detection can return
success from merely locating `sandbox-exec`; the CI smoke contains an early
return when capability is absent. That must become a required real Cargo gate.

## Ledger

Python paths below refer to the pinned comparison SHA. Rust paths refer to the
baseline plus this PR where explicitly stated. “Verified” applies only to the
listed evidence; “unverified” means not yet executed at the required realism.

| Python reference | Rust implementation | Intended behavior | Evidence | Status |
|---|---|---|---|---|
| `harness/config.py`, `server.py` | `common/home.rs`, `main.rs` | Loopback startup, isolated home, embedded assets | common-layer and bind-refusal tests; release build | verified (fixtures) |
| `harness/config.py` | `common/config.rs`, `agentic/config.rs` | Validate configuration, closed enablement | common/foundation tests; quoted booleans rejected by typed agentic config | verified (fixtures) |
| Existing-home config/settings migration | `common/home.rs` | Upgrade existing homes without losing state | Seeds only missing files; full upgrade/recovery matrix absent | unverified |
| `harness/sessions.py` | `server/sessions.rs` | Persist sessions and recover corrupt state | chat/session tests; broader corruption scenarios pending | incomplete |
| `harness/server.py`, `schemas.py` | `server/routes/*`, `schemas.rs` | Compatible schemas, errors and timeouts | route tests; write intent deliberately extended in this PR | changed deliberately |
| `harness/agent_policy.py` | `server/agent_policy.rs`, console asset | Authoritative supported checks/defaults | Server-advertised cargo-test/default capabilities; actual Chrome selection tests pass | incomplete |
| `harness/chat_client.py`, `chat_cancel.py` | `llm/openai_chat.rs` | Local chat, complete output, cancel | mock and real Ollama chat/cancel; explicit non-streaming; stop required before content | incomplete |
| `harness/server.py`, model settings | `llm/backend.rs`, `agentic/proposer.rs` | Consistent chat/planner model selection | Separate config paths explicitly shown; exact installed tag exercised in both clients | incomplete |
| Chat usage/reasoning plumbing | `llm/openai_chat.rs`, `agentic/proposer.rs` | Correct usage and supported reasoning parameters | mocked usage plus real none/low effort and usage observations in MAC_ACCEPTANCE.md | unverified |
| `harness/prompts.py`, `skills_view.py` | `server/prompts.rs`, `views.rs`, bundled skills | Skills/persona/context in prompt | panel/prompt fixture tests; real-model influence unverified | incomplete |
| `harness/memory_notes.py` | `server/memory_notes.rs` | Local note CRUD and prompt wiring | panel tests | verified (fixtures) |
| `harness/web_search.py` | `server/web_search.rs` | Explicit optional web with SSRF checks; offline semantics | local mock web tests; offline-mode audit pending | incomplete |
| `agentic/gh_client.py`, `context.py` | matching Rust modules | Read-only GitHub access and selected repo | fake-gh tests; live auth verified, selection workflow pending | incomplete |
| `agentic/real_repo_loop.py` | `agentic/real_repo_loop.rs` | Bounded useful context and planning | Same bounded character ceilings; explicit line windows and hash-bound exact edits; tree/search still pending | incomplete |
| Loop file blocks and workspace writes | `real_repo_loop.rs`, `workspace.rs` | Safe ordinary edits, stale preconditions, atomic proposals | strict parser + snapshot/excerpt tests; >12 KB exact edit and real Cargo correction pass; concurrency/crash limits documented | incomplete |
| `agentic/executor/runner.py`, `hard_sandbox.py` | `executor/runner.rs`, `sandbox.rs` | Actual offline Cargo checks | Baseline serde fails under fresh HOME; prepared locked serde/build-script/unit/doctest now pass native Seatbelt | incomplete |
| `agentic/writer.py`, `deepagent_github/repo_workspace.py` | `writer.rs`, `workspace.rs` | Consistent actual-boundary write controls | Baseline bypass reproduced; new native local-Git matrix covers revocation | changed deliberately |
| Loop finalize, writer | `commands.rs`, `real_repo_loop.rs`, `writer.rs` | Review → approve/commit → separate push → draft publication | Existing smoke + new CLI/API matrix; all three require reason/confirm | changed deliberately |
| Diff rendering | `commands.rs`, console asset | Complete human-readable diff before approval | 20000-character cap remains; browser refuses truncated diff | incomplete |
| Path jail / protected scope | `workspace.rs`, loop policy | Containment and protected landed destinations | proposal scope checks raw/landed paths, denies symlinks; retained parent handles; concurrent rename/leaf-CAS residual remains | incomplete |
| Hard sandbox | `executor/sandbox.rs` | Read/write/network/process confinement | Baseline outside read/.git write reproduced; this PR denies both plus network, cache and candidate writes; process/resource limits remain | incomplete |
| Git execution/finalization | `workspace.rs`, `executor/apply.rs` | No hooks/filters/index contamination | Disposable proof disables hooks; actual later Git operations need further audit | incomplete |
| Child process runner / ops runner | `common/process.rs`, `shim/mod.rs` | Bounded output, time, descendants, resources | bounded Unix capture/deadline and native nested-group cancellation pass; escaped reparenting, memory/disk quotas unresolved | incomplete |
| `harness/server.py`, auth routes | `server/guards.rs`, common auth | Rate → origin → key → CSRF; loopback Host, optional local auth | auth/security-header tests | verified (fixtures) |
| Logger/error/telemetry controls | `common/audit.rs`, env scrubber, cloud proposer | Redaction, opt-outs, no cloud inference by default | common/chat/panel tests; exhaustive leak audit pending | incomplete |
| Optional cloud proposer | `agentic/cloud_proposer.rs` | Gated provider wire formats without real cloud tests | local Grok/Claude mocks; no actual cloud credentials used | verified (mocks only) |
| Python synchronous run routes | `server/agent_jobs.rs`, `routes/agent.rs` | Detached jobs, refresh recovery, stop | actual Chrome asynchronous jobs and refresh recovery pass; startup reconciliation absent | incomplete |
| Run store / cleanup | `agentic/run_store.rs`, `commands.rs` | Durable lifecycle and controlled cleanup | fixture record guards; crash/restart recovery not established | incomplete |
| Python CLI/ops coverage | `agentic/cli.rs`, `shim/mod.rs`, views | Honest exposed capabilities | whitelist/invariant/surface scans | verified (structural) |
| Python install scripts | packaging scripts and GitHub workflows | Apple Silicon package with assets/self-location | native arm64 archive/checksums/assets/shim self-location pass; ad-hoc signing only; preparation script absent from binary archive | unverified |
| RAG, terminal execution, fs/sql/netconnect, native desktop | no implementation | Excluded extraction scope | extraction commit explicitly omits RAG/terminal/connectors | intentionally omitted |

## Ordered next work

1. Phase 1: close consistent write policy, keep reason/confirmation explicit,
   regression-test actual operations and revoked policy, draft PR with this ledger.
2. Phase 2: reproduce real Cargo inside Seatbelt, prepare dependencies separately,
   isolate writable build/cache state; then test read/write/.git/child/resource boundaries.
3. Phase 3: bounded exact edits with hash/context preconditions and atomicity;
   protect canonical and landed destinations. Never unprotect tests wholesale.
4. Phase 4: server defaults, jobs/recovery/diff controls and browser tests;
   process-tree cancellation separately. Correct streaming claims.
5. Phase 5: independent chat/planner, then disposable end-to-end Qwen acceptance,
   measured cold/warm/cleanup/interruption behavior with explicit network isolation.
6. Phase 6: reconcile every row, existing-home doctor, retained features, package QA.

Each dependent concern gets a documented stacked draft PR. No merge, release,
cloud inference, CyClaw edits, model changes, or global configuration changes.

## Phase 1 verification

Final `CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh`: exit 0,
fmt + exact all-target/all-feature clippy + 126 tests + release build. Dependency
audit passed separately against unchanged Cargo.lock. No ignored tests reported;
live Qwen acceptance and actual browser automation remain unverified.
`tests/write_policy.rs` contains four real-Git/CLI/API tests without a sandbox skip,
including direct PR-writer revocation, scope/budget revocation, missing policy,
separate push and read-only diff without executable Git extensions.
The original bypass now exits 4 and creates no remote ref. Existing local mocked
planner → real sandbox text check → approval → push → mock draft publication
smoke still passes. This is not real-model end-to-end acceptance.

## Phase 2: prepared Cargo and native filesystem boundary

Depends on Phase 1 commit `ad5d9f224c7c060af00d0c2801a63389e9a48761` (PR #16).
The same M5/macOS/Homebrew installation now runs minimal and serde-bearing
Cargo fixtures inside actual Seatbelt. Build scripts, unit tests and doctests
execute; repeated runs use fresh scratch. Spaces, apostrophes and Unicode in
preparation paths are covered. Missing snapshots/toolchains/vendor sources and
stale locks return setup errors; the loop checks readiness before model work.

Read-only prepared sources replace reliance on the operator's shared Cargo home.
No Rust dependency added; the preparation helper uses Python 3's standard library.
Native tests have no capability skip. CI explicitly fetches locked fixture sources
before its offline Cargo gate. Runtime network remains denied by Seatbelt.

Phase 2 is **partial**: escaped process groups, hung output readers, output
allocation, memory/disk quotas, and later Git extension/index behavior still need
audit/corrections. No equivalent Linux/Windows confinement claim. Real-model and
browser acceptance are not established by these Cargo tests.

Final Phase 2 `CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh`: exit 0,
fmt, all-target/all-feature clippy, 128 tests, release build. No ignored tests;
live smoke explicitly omitted. `cargo-deny check`: exit 0, existing duplicate
crate/unused-license warnings remain; no new root lockfile dependencies.

Initial PR #17 CI correctly failed in a rustup/Xcode doctest: Cargo's target
linker setting did not reach rustdoc, which invoked cc/xcrun outside permitted
cache/SDK paths. Pass the prepared linker via encoded rustdoc flags as well.
The corrected native full quality run again passed 128 tests and release build;
no read/write grants or test assertions were weakened. Exact CI rerun pending.

## Phase 3: exact edits and complete proposal application

Baseline `f777df6579ceafa3ea5ef237da898d9ea9935248`: 43/65 Rust files exceed
4,000 characters. Disposable probes reproduced large-file refusal, accepted
complete prefixes of truncated output, partial writes after a late refusal, and
protected writes through a directory alias. This change adds explicit line
windows, hash/excerpt-bound exact edits, strict complete-response parsing,
whole-proposal preflight and staged application with rollback. See
`docs/BOUNDED_EDITS.md` for the protocol, tests and concurrency/crash limits.

Still incomplete: tree/search interfaces, exact model-token budgeting, crash
recovery and adversarial concurrent rename containment. Protected tests/build
configuration were not unprotected. No new dependencies.

Phase 3 final `CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh`: exit 0,
fmt, exact clippy, 133 tests and release build. `cargo-deny check`: exit 0 with
pre-existing warnings. Native Cargo fixtures run without a skip; live model
acceptance is recorded separately, not counted as a mocked regression success.

## Phase 4: console jobs and authoritative defaults

Depends on Phase 3 `05cb797a791feeaa43fecd2eb90c840a14ae099c` (PR #18).
Actual browser inspection reproduced staged `pytest` against the server's
`cargo-test`. Console submission now uses existing jobs, persists the job ID
in the URL fragment, and resumes after refresh with renewed authentication.
CLI failures produce failed terminal jobs. Staged cancellation and active
request cancellation are distinct and disclose surviving-descendant risk.
Chat selection explicitly reports the separately configured planner model.
See `docs/CONSOLE_JOBS.md` for executable native browser acceptance.

Native quality gates: fmt, exact clippy, 135 tests and release build pass;
`SKIP_LIVE=1` applies only to the standard script's live smoke. Separate actual
Chrome + installed Qwen + real Cargo acceptance passed against disposable Git.
No new dependencies, no key persistence, no CSRF/CSP or rate-limit weakening.

Phase 4 remains partial: durable jobs/startup reconciliation, live check progress,
process-tree cancellation and oversized diff review remain unresolved. Streaming
is explicitly unsupported. These are not established by the browser success.

## Phase 5: model completion contract

At Phase 4 `831846596753e5066bd437883e93a27795f287e0` (#19), real installed
`qwen3.8:27b` returned `finish_reason=length` under a one-token budget. Both
chat and planner regressions reproduced accepting nonempty truncated content,
including a syntactically complete file-block prefix. Both local clients now
require `finish_reason=stop`; planner truncation is an actionable configuration
error before patch parsing. Missing/unknown completion states are refused.
Normal mock responses now explicitly report `stop`; existing assertions remain.
See `docs/MAC_ACCEPTANCE.md` for measured real-model evidence and its limits.

Completion-contract quality gates passed: fmt, exact clippy, 137 tests, release
build and cargo-deny. Real two-file correction passed in two iterations (Cargo
101 then 0), and the extracted arm64 package passed assets, shim self-location,
checksums/ad-hoc signature and real-model truncation checks. No release published.


## Process lifecycle correction (Phases 2/4)

Native repro: a 100 ms deadline waited about two seconds for inherited pipes.
Shared nonblocking Unix capture now bounds stdin/output/drain with cancellation
cleanup and a 4 MiB raw-output ceiling. Native JobStore cancellation stops an
observed separate-group Seatbelt check and preserves an unrelated sibling.
Independent review found ordinary early-exit descendants surviving after leader
reaping; wait-without-reaping now retains group identity through cleanup, with
a regression. Publication capture failures retain indeterminate outcome/audit
semantics and are not retried. No sandbox permission or dependency changes.

Residual limits are in `docs/PROCESS_LIFECYCLE.md`: escaped reparenting, abrupt
server death, general resource quotas and non-Unix parity remain incomplete.

Process correction final quality gates passed: fmt, exact clippy, 145 tests,
release build and cargo-deny. Required native cancellation and separated-scratch
regressions pass; no capability skip or additional sandbox grant.


## Reviewed draft-publication descriptions

Real-repository acceptance exposed a hardcoded one-line draft body. CLI, shim,
API and console now transport bounded explicitly reviewed Markdown. The browser
loads a local completed template file and previews the full body before a
separate publication action. Both subprocess hops use temporary body files.
Missing text is refused; reason/confirm remain separate requirements. The
operator remains responsible for the repository's actual template and content.

Publication-body final gates passed: fmt, exact clippy, 147 tests, release build
and unchanged dependency checks. Actual Chrome/mock-publication byte readback
passed. Real GitHub draft #22 completed the separate commit/push/publication
workflow with an exact-template readback and a README-only change.
