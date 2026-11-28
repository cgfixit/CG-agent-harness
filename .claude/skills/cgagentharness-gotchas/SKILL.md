---
name: cgagentharness-gotchas
description: Session-tested traps for working on CG-agent-harness. Load before install/verify, desktop packaging, Chrome acceptance CI, clippy/toolchain fights, or when something hangs or looks green for the wrong reason.
---

# CG-agent-harness gotchas

Session-process traps for this repo. Canonical contracts and the standing traps live in `AGENTS.md`,
`CLAUDE.md` and `INVARIANTS.md` (the otel re-run rule and the root-refusing Chrome fixtures among them);
this file holds only what they do not: the "looks green for the wrong reason" list.
Paths are relative to the repo root. Skills are repository guidance,
not runtime `/api/skills` plugins.

## 1. Clippy is the wrong binary on some Macs

- **Symptom:** `scripts/verify-local.sh` fails clippy with toolchain/proxy skew;
  rustup's `cargo-clippy` older than Homebrew's.
- **Wrong fix:** `#allow` the warnings or drop `-D warnings`.
- **Right fix:** `CLIPPY="/opt/homebrew/bin/cargo-clippy clippy" scripts/verify-local.sh`;
  bare `cargo-clippy` skips `--all-targets`.
- **Evidence:** `scripts/verify-local.sh`.

## 2. Chrome chat-browser acceptance flakes

- **Symptom:** CI red on chat-browser acceptance; intermittent "Chrome didn't
  start" / session attach failures.
- **Wrong fix:** Delete the acceptance job or weaken assertions until green.
- **Right fix:** Treat as known flake class (issue **#43** — Start Chrome
  reliably). Retry/quarantine with tracking; fix the starter, don't hollow the
  test.
- **Also seen:** a *non*-startup flake where a saved key's `type="password"`
  input in `chat-browser-acceptance.mjs` still held the typed value after a
  refresh (once in eleven runs). It
  now checks `visibleKeys` only.
  First suspect: Chrome launched without `--password-store=basic` or any
  autofill-disabling flag. Confirm before "fixing" it.
- **Evidence:** #43.

## 3. Desktop packaging: lipo / sign / embed-SHA256 order

- **Symptom:** Checksums never match after local resign; Gatekeeper weirdness;
  sidecar digest verification fails.
- **Wrong fix:** Re-sign the sidecar **after** digest embed; mix ad-hoc and
  Developer ID steps without documenting which artifact is which.
- **Right fix:** Order is **lipo → sign → embed-SHA256**. Never re-sign a sidecar
  after digest embed. Ad-hoc (`--sign -`) ≠ Developer ID / notarized. WKWebView
  shell ≠ HTTP console semantics — don't assume fetch/CSRF behavior is identical
  inside the native wrapper.
- **Evidence:** `scripts/package-release.sh` (ad-hoc note); `docs/RELEASING.md`.

## 4. Seatbelt permission errors ≠ app regression

- **Symptom:** Sandbox verify fails with permission denied; patch softens the
  Seatbelt profile or skips sandbox tests.
- **Wrong fix:** Weaken assertions or broaden the profile to silence noise.
- **Right fix:** Reproduce with owned temp `CGAGENTHARNESS_HOME` + unique port.
  Distinguish environment/Seatbelt noise from product bugs. No backend ⇒ exit 3
  remains correct fail-closed behavior.
- **Evidence:** `INVARIANTS.md` judged-before-land; `agentic_foundations` sandbox_* tests.

## 5. Shared listener ≠ this bundle

- **Symptom:** Smoke "passes" against a leftover `serve` on `:8790` from another
  checkout or build.
- **Wrong fix:** Hit the default port and assume it's your binary.
- **Right fix:** Owned temp `CGAGENTHARNESS_HOME`, unique port, kill/own the
  process you started.
- **Evidence:** `scripts/smoke-ollama.sh`.

## 6. `github.paginate` result consumed as `{data}`

- **Symptom:** A workflow step dies with
  `TypeError: Cannot read properties of undefined (reading 'find')`, on
  *every* PR, not intermittently.
- **Wrong fix:** Wrap the access in `?.`, or revert to the unpaginated call
  that saw only page 1.
- **Right fix:** `github.rest.issues.listComments(...)` resolves to `{data: [...]}`;
  `github.paginate(github.rest.issues.listComments, ...)` resolves to the **array
  itself**. Change the consumer too: `comments.find(...)`, not
  `comments.data.find(...)`. Audit every
  `await github.paginate` call site in `.github/workflows/` at once
  (`pr-template-check.yml`, `review-gate*.yml`); all must read the array directly.
- **Check before pushing:** extract the `github-script` block and run it under
  stubs (`github`, `context`, `core` are arguments), with `listComments` paging
  by 30 and a marker on page 2. It reproduces the duplicate-comment bug and
  the `TypeError`.
- **Evidence:** `.github/workflows/pr-template-check.yml` — its comment jobs
  (`comment-base`, `comment-template`) paginate `listComments`; an unpaginated
  read missed a marker on page 2.

## 7. Ephemeral port handed to a child, and `serve` exits **0** when it is taken

- **Symptom:** Intermittent red in `scripts/test-desktop-backend.py` with
  `AssertionError: 0 is not None : headless startup exited`, or
  `AssertionError: 0 == 0`; neither mentions ports.
- **Wrong fix:** Widen a timeout, retry the whole job, or call it a runner
  problem.
- **Right fix:** A port bound on `:0`, read back and closed is only advisory:
  it must be released before the child binds, so anything can take it in
  between. Retry on a fresh port. Two traps:
  - **`serve` exits 0 on port-in-use**, printing "CGagentHarness may already be
    running on 127.0.0.1:PORT" to **stdout** (stderr empty), because
    `port_in_use` is checked *before* `.env` is read. A test asserting a
    non-zero refusal then fails without exercising its path.
  - **Do not truncate captured output between attempts** if a later assertion
    reads that file. That suite's credential-disclosure `assertNotIn` checks
    read the child's captured output; truncating erases a leak from a failed
    attempt, and a clean retry passes.
- **Related discipline:** after fixing one race, re-audit every site you
  *refactored* too: a second site moved to the shared helper without its retry
  keeps the bug.
- **Evidence:** `scripts/test-desktop-backend.py` (`free_port`,
  `await_headless`, `serve_expecting_refusal`); `ModelFixture` and
  `test_unrelated_listener_is_never_adopted` show the keep-the-listener-open
  variant where that is possible.

## 8. A push does **not** re-trigger the Codex review

- **Symptom:** You address review findings, push, see the PR go green, and merge,
  the fixes never reviewed.
- **Wrong fix:** Assume the green check covers the new commits, or read a stale
  "Completed" summary row as a verdict on the current head.
- **Right fix:** Codex reviews trigger on *open for review*, *mark draft ready*,
  and an explicit `@codex review` comment, **not** on a push. After pushing a
  fix, tell the operator the fixes are unreviewed and let them decide: never
  comment `@codex review` without their approval. If they approve, check that
  the **Reviewed commit** SHA matches your head. A superseded run reads
  `cancelled` (`cancel-in-progress`): not a failure, and not a verdict.
- **Evidence:** the Codex "About Codex in GitHub" block on any PR in this repo.

## 9. The docs audit agent can cite lines that do not exist

- **Symptom:** A drift report says "CONSOLE.md line 15 says sessions are
  written to `sessions/*.json`"; the file has no such line. Applying the edit
  fails, or inserts prose at a guessed location.
- **Wrong fix:** Trust a subagent's line numbers or quoted text as evidence.
- **Right fix:** Every audited claim is a *hypothesis*. Before editing, `grep`
  the quoted anchor in the named file; skip the finding if it does not match.
  Quote code values (config defaults, ranges, alias spellings) from `rg`, never
  from the report (the `cg-doc-sync` "quote from `rg`" rule).
- **Evidence:** 2026-09-16 docs refresh (PR #131): 1 of ~20 findings was
  fabricated.

## 10. Memory gate slash aliases are not the config key names

- **Symptom:** Docs (or a doc fix) write `/memory auto-retrieval on` or
  `/memory auto-consolidation on`; the console rejects them.
- **Wrong fix:** Derive the alias from the `structured_memory.*` key.
- **Right fix:** The console alias map is `gateMap` in
  `assets/static/harness.html` and the help line in `src/server/views.rs`:
  `capture|recall|retrieval|auto-retrieve|consolidation|auto-consolidate|
  auto-suggest-chat|auto-suggest-coding`. The HTTP body for
  `POST /api/structured-memory/gates` takes the *config* name
  (`episode_capture`, `explicit_recall`, `auto_retrieval`, ...) instead.
  Two spellings, two surfaces; grep the one you are documenting.

## 11. Do not cut a release the maintainer already cut

- **Symptom:** Asked to "cut a release after the PR", you dispatch
  `release.yml` with `publish=true`; it fails with
  "Candidate tag already exists" or "Release already exists (including drafts)".
- **Wrong fix:** Retry, delete the draft, or push a tag by hand.
- **Right fix:** Before any dispatch, list `release.yml` runs and releases. A
  tag push (`push` event, `head_branch: vX.Y.Z`) or a pending
  `workflow_dispatch` on the same SHA means the release is already in flight;
  watch that run instead. The planner (`scripts/release-plan.py`) fails closed
  on collisions. A green `Bundle` run on the exact main
  SHA is a hard precondition for `workflow_run`/`workflow_dispatch` releases.
- **Evidence:** 2026-09-16, `v0.1.14` tag push started run 26 while the docs
  PR was being drafted; dispatch run 27 queued behind it.

## 12. PR CI is not main CI

- **Symptom:** A green PR is mistaken for desktop or packaged-app evidence.
- **Right fix:** Every PR requires full Windows and the focused lifecycle
  and credential job. Docs-only PRs skip the Linux and macOS test legs. A
  Unix-sandbox test carries `#[cfg(unix)]`; cross-check with `--target
  x86_64-pc-windows-gnu` and a `CARGO_TARGET_DIR` outside the repo. `desktop/`
  compiles only on macOS, and only when `desktop-check.yml`'s classify regex
  matches: a new `include_bytes!` or `#[path]` input must join it.
- **Evidence:** PR #985 (`tests/real_repo_loop.rs`); PR #968 (`prepare-cargo.py`).

## 13. A fixture that is kinder than the real peer

- **Symptom:** MCP SSE tests pass; a real Python-SDK server times out.
- **Right fix:** The fixture answered JSON-RPC in the POST body; the SDK answers
  `202 Accepted` and replies on the open GET stream. Run a protocol client once
  against the reference server, then teach the fixture that shape
  (`PostReply::OverStream`, `tests/mcp_client.rs`).
- **Evidence:** PR #976.

## 14. A fixture home's short `config.yaml` turns shipped features off

- **Symptom:** a smoke home with only `auth`/`tls` keys refuses `/browse` or
  memory; it looks like a regression.
- **Right fix:** a missing key is not the shipped value. `serve` WARNs with
  every omitted ship-true key; set those, or start from the shipped default.
- **Evidence:** 2026-11-26 smoke: 34 named, `browser.enabled` among them.

## 15. One `CARGO_TARGET_DIR` shared by worktrees links a stale binary

- **Symptom:** a fix "does nothing", or a fixture passes on code it lacks;
  `cargo build` takes a second.
- **Right fix:** worktrees write the same artifact names and freshness is by
  mtime, so another worktree's newer build reads as yours. Use one target
  directory per worktree, or `touch` sources first; `grep -c <new string>
  <binary>` before trusting a run. A stale `invariant_guard` reads the other
  tree's files (`CARGO_MANIFEST_DIR`).
- **Evidence:** 2026-11-27, PR #1205.

## 16. `docs-impact.py` passes before the commit exists

- **Symptom:** local `Docs impact check passed`; CI's `docs impact` is red.
- **Right fix:** it compares `--head HEAD` with the base, so uncommitted edits
  are not in it. Commit, then run it; a missing receipt goes in a
  trailer-only commit.
- **Evidence:** PRs #1204 and #1210.

## Related skills

Load `cgagentharness-project-guidance` for read order; `cgagentharness-config-guard`
before merging config diffs; `cgagentharness-runtime-invariant-check` before pushing
a core-path PR; `cgagentharness-write-policy-redteam` for write/git/argv/jail
loops; `verification-specialist` to break a supplied patch; `fable-protocol` for
evidence-first discipline. Before merging core-path diffs, ask the operator to run
`/cgagentharness-invariant-guard` (it ships `disable-model-invocation: true`).
