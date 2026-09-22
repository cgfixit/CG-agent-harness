---
name: doc-sync
description: |
  Actively fix documentation drift by diffing the current tree against the latest
  origin/main (or the active branch's upstream) and rewriting stale prose in
  README.md, AGENTS.md, setup-guide.md, docs/*.md (including USER_MANUAL.md and
  DEPENDENCIES.md), and mirrored .codex docs to match. Unlike
  cgagentharness-doc-sync (which only flags drift and reports findings), this
  skill produces the edits: it fetches the comparison ref, enumerates what
  changed in code/config/routes/CLI since docs were last touched, and updates
  every affected doc in place. Use after a feature branch has drifted from
  main, before a release, or whenever asked to "sync the docs" / "fix doc
  drift" / "update the README and docs".
compatibility: |
  Requires: git, ripgrep
  Context: Cargo project with README.md, AGENTS.md, INVARIANTS.md,
  setup-guide.md, docs/*.md (including docs/USER_MANUAL.md)
disable-model-invocation: true
---

# doc-sync — fix documentation drift against origin

This skill WRITES fixes. If you only want a drift report with no edits, use
`cgagentharness-doc-sync` instead.

## 1. Establish the comparison base

```bash
git fetch origin main
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
BASE=origin/main
# If already on main (or a branch with no upstream divergence worth using),
# compare against the branch's own upstream instead:
git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null && BASE=@{u}
git diff --stat "$BASE"...HEAD -- ':!*.lock'
git log --oneline "$BASE"...HEAD
```

The diff range `$BASE...HEAD` (triple-dot, merge-base form) is the set of
changes this branch introduces beyond what the docs currently describe. If
`HEAD` IS `origin/main` (nothing has diverged), instead diff the last commit
that touched each doc against the current code: `git log -1 --format=%H -- <doc>`
then `git diff <that-sha>..HEAD -- src/ assets/config.default.yaml`.

## 2. Classify what moved

Walk the diff and bucket every change that is doc-relevant:

- New/removed/renamed shim actions (`src/shim/mod.rs` `ACTIONS`,
  `src/agentic/commands.rs::dispatch` match arms)
- New/removed HTTP routes (`src/server/routes/mod.rs::REGISTERED_PATHS`)
- Config keys added/removed/renamed in `assets/config.default.yaml`
- CLI subcommand or flag changes (`src/main.rs`, `--help` output)
- Changed exit codes, guard order, or write-gate names
- Renamed/moved files the docs link to (`docs/*.md`, `setup-guide.md`)
- New scripts under `scripts/` referenced (or that should be referenced) in
  setup/user docs
- Dependency or toolchain version bumps that `docs/DEPENDENCIES.md` states a
  specific number for

Ignore pure refactors, test-only changes, and formatting-only diffs — they
are not doc-relevant.

## 3. Update each affected file

Truth order for what to write: code > `assets/config.default.yaml` >
`INVARIANTS.md` > `AGENTS.md` > `README.md` > `setup-guide.md` > `docs/*.md`.
Never let a lower-precedence doc override what a higher one (or the code
itself) says — if `INVARIANTS.md` and `README.md` disagree after your pass,
`README.md` is wrong, not `INVARIANTS.md`.

- `README.md` — top-level pitch, command list, architecture summary.
- `AGENTS.md` — literal operating manual; keep instructions actionable, not aspirational.
- `setup-guide.md` — short index plus "choose how to run it". Install, models, chat, memory, web, coding, accounts, and troubleshooting live under `docs/` (`INSTALL.md`, `MODELS.md`, `CONSOLE.md`, `MEMORY_SETUP.md`, `WEB.md`, `CODING_PIPELINE.md`, `ACCOUNTS.md`, `TROUBLESHOOTING.md`). Do not fold those pages back into `setup-guide.md`.
- `docs/USER_MANUAL.md` — end-user console/CLI walkthrough; verify UI copy and routes referenced still exist.
- `docs/DEPENDENCIES.md` — leave dependency version numbers to `dep-sync`; only fix structural drift here (new crates worth documenting, removed ones still mentioned).
- Other `docs/*.md` — update only the files whose subject matter the diff actually touches; don't touch unrelated docs.
- Mirrored guidance under `.codex/skills/` and `.claude/skills/*/SKILL.md` — if a doc you just fixed is paraphrased there, fix the mirror in the same pass so the two trees don't re-diverge.

For each edit, prefer a small precise correction over a rewrite. Quote the
current code/config value verbatim rather than re-deriving it from memory —
grep it.

## 4. Verify

```bash
cargo test --locked --test invariant_guard
cargo test --locked registered_paths_are_unique_and_cover_every_router_route
```

Re-grep every route, gate name, and CLI flag you wrote into a doc against the
source you cited it from. Confirm every internal doc link you touched still
resolves (`rg -n '\]\(' README.md AGENTS.md setup-guide.md docs/*.md` and spot
check targets exist).

## 5. Report

List: base ref compared against, files changed and why (one line each),
anything you deliberately left alone and why (e.g. "historical acceptance
record, not a current contract"). Do not claim a doc is now fully in sync if
you only fixed the parts the diff touched — say so explicitly.

Skill selection here does not authorize push/merge — follow this repo's PR
conventions (draft PR, `[docs]` prefix, `scripts/check-pr-template.sh`) for
publishing the result.
