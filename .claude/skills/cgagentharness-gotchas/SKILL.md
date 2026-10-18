---
name: cgagentharness-gotchas
description: Session-tested traps for working on CG-agent-harness. Load before install/verify, desktop packaging, Chrome acceptance CI, clippy/toolchain fights, or when something hangs or looks green for the wrong reason.
---

# CG-agent-harness gotchas

Session-process traps for this repo. Canonical contracts and the standing traps live in `AGENTS.md` /
`INVARIANTS.md`; this file holds only what they do not: the "looks green for the wrong reason" list.
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
- **Also seen:** a *non*-startup flake in the `chat-browser-acceptance.mjs`
  assertion that the `saved-DEEPAGENT_API_KEY` input is empty after saving: it
  still held the typed value after a refresh.
  Observed once in eleven local runs; the same commit then passed ten straight,
  so do not attribute it to your diff on n=1. Mechanism unconfirmed. The one
  candidate worth checking first: the script launches Chrome without
  `--password-store=basic` or any autofill-disabling flag, and the field is
  `type="password"`. Confirm before "fixing" it.
- **Evidence:** CI gotcha / #43.

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
  process you started (`scripts/smoke-ollama.sh` pattern).
- **Evidence:** `scripts/smoke-ollama.sh`.

## 6. `github.paginate` result consumed as `{data}`

- **Symptom:** A workflow step dies with
  `TypeError: Cannot read properties of undefined (reading 'find')`, and the
  check fails on *every* PR rather than intermittently.
- **Wrong fix:** Wrap the access in `?.` and move on, or revert to the
  unpaginated call that only ever saw page 1 of 30.
- **Right fix:** `github.rest.issues.listComments(...)` resolves to `{data: [...]}`;
  `github.paginate(github.rest.issues.listComments, ...)` resolves to the **array
  itself**. When switching to `paginate`, change the consumer too —
  `comments.find(...)`, not `comments.data.find(...)`. Audit every
  `await github.paginate` call site in `.github/workflows/` at once; there are
  four, and they must all read the array directly.
- **How to check before pushing:** the `github-script` block can be extracted
  from the YAML and run under stubs (`github`, `context`, `core` are just
  arguments), with `listComments` honouring the 30-per-page default and a marker
  planted on page 2. Cheaper than a CI round trip, and it reproduces both the
  original duplicate-comment bug and the `TypeError`.
- **Evidence:** `.github/workflows/pr-template-check.yml` — its two jobs
  disagreed on this for a while; the `base-branch` job was always correct.

## 7. Ephemeral port handed to a child, and `serve` exits **0** when it is taken

- **Symptom:** Intermittent red in `scripts/test-desktop-backend.py` with
  `AssertionError: 0 is not None : headless startup exited`, or
  `AssertionError: 0 == 0` — neither of which mentions ports.
- **Wrong fix:** Widen a timeout, retry the whole job, or call it a runner
  problem.
- **Right fix:** Binding `:0`, reading the port back and closing the socket is
  inherently advisory — the reservation *must* be released before the child can
  bind, so anything on the runner can take it in between. Retry on a fresh port.
  Two traps make this worse than it looks:
  - **`serve` exits 0 on port-in-use**, printing "CGagentHarness may already be
    running on 127.0.0.1:PORT" to **stdout** (stderr empty), because
    `port_in_use` is checked *before* `.env` is read. So a test asserting a
    non-zero refusal fails, and the path it meant to exercise never runs.
  - **Do not truncate captured output between attempts** if a later assertion
    reads that file. The credential-disclosure `assertNotIn` checks in that
    suite read the child's captured stdout/stderr; truncating means a leak on a
    failed attempt is erased and the check passes on a clean retry.
- **Related discipline:** when you fix one instance of a race, re-audit every
  site you *refactored* as well as the one that was failing. Converting a second
  site to the shared helper without its retry leaves the bug live at a site your
  own commit touched.
- **Evidence:** `scripts/test-desktop-backend.py` (`free_port`,
  `await_headless`, `serve_expecting_refusal`); `ModelFixture` and
  `test_unrelated_listener_is_never_adopted` show the keep-the-listener-open
  variant where that is possible.

## 8. A push does **not** re-trigger the Codex review

- **Symptom:** You address review findings, push, see the PR go green, and merge
  — with the fixes themselves never reviewed.
- **Wrong fix:** Assume the green check covers the new commits, or read a stale
  "Completed" summary row as a verdict on the current head.
- **Right fix:** Codex reviews trigger on *open for review*, *mark draft ready*,
  and an explicit `@codex review` comment, **not** on a push. After pushing a
  fix, tell the operator the fixes are unreviewed and let them decide: never
  comment `@codex review` without their approval. If they approve, check that
  the **Reviewed commit** SHA matches your head. A superseded run can be reported
  as `cancelled` by `cancel-in-progress` concurrency; that is not a failure, but
  it is also not a verdict.
- **Why it matters here:** fixes to review findings are exactly the diffs most
  worth a second pass — they are written fast, under the assumption the problem
  is already understood.
- **Evidence:** the Codex "About Codex in GitHub" block on any PR in this repo.

## 9. The docs audit agent can cite lines that do not exist

- **Symptom:** A drift report says "CONSOLE.md line 15 says sessions are
  written to `sessions/*.json`"; the file has no such line. Applying the edit
  blindly fails, or worse, inserts prose at a guessed location.
- **Wrong fix:** Trust a subagent's line numbers or quoted text as evidence.
- **Right fix:** Every audited claim is a *hypothesis*. Before editing, `grep`
  the quoted anchor in the named file; skip the finding if it does not match.
  Quote code values (config defaults, ranges, alias spellings) from `rg`, never
  from the report. The `doc-sync` skill's "grep it" rule exists for this.
- **Evidence:** 2026-09-16 docs refresh (PR #131): 1 of ~20 findings was
  fabricated; the other 19 were verbatim-verifiable.

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
  on collisions by design. Also note: a green `Bundle` run on the exact main
  SHA is a hard precondition for `schedule`/`workflow_dispatch` releases.
- **Evidence:** 2026-09-16, `v0.1.14` tag push started run 26 while the docs
  PR was being drafted; dispatch run 27 queued behind it.

## Related skills

Load `cgagentharness-project-guidance` for read order; `cgagentharness-config-guard`
before merging config diffs; `cgagentharness-write-policy-redteam` for write/git/argv/jail
loops; `verification-specialist` to break a supplied patch; `fable-protocol` for
evidence-first discipline. Before merging core-path diffs, ask the operator to run
`/cgagentharness-invariant-guard` (it ships `disable-model-invocation: true`).
