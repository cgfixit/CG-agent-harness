# Release controls

A successful [Bundle](../.github/workflows/bundle.yml) on a `main`
push triggers [release](../.github/workflows/release.yml).
`release-plan.py` skips superseded tips and trees identical to the latest
regular release. A changed tree, docs included, plans the next patch.
Backend CI gates packaging. The full Windows suite is advisory here;
Windows MCP lifecycle stays required. Extracted CLI smoke tests and
checksums gate publication. Existing tags and releases cannot be overwritten. The planner needs the
latest regular stable release. The planned version sets the app's About
and bundle version. `CGAH_RELEASE_VERSION=vMAJOR.MINOR.PATCH` builds
a local candidate with that version.

## Publication delay

Only `publish` enters the `release` environment, where its wait timer and
reviewer rules apply. A newer job entering `release-publish` cancels an older
one waiting. After the wait `publish` replans: once `main`'s workflow
files differ from its commit, GitHub refuses the tag, so it ends green;
the newer tip's run releases. Builds can overlap before publication.

## Run now or preview

- Dispatch with **publish** unchecked for a plan without builds.
- Select `main` and **publish** to release a changed tree whose commit has
  a green Bundle.
- A new stable `vMAJOR.MINOR.PATCH` tag on a reviewed commit already on
  `main` runs the same pipeline. Ahead, diverged or prerelease tags refuse.

Outside Actions, `python3 scripts/release-plan.py` plans read-only
with authenticated `gh`. Inspect retained drafts and assets after a
publication failure.

## Verification limits

Mac packages are universal and ad-hoc signed. Check `Resources/COMMIT`
for source provenance. Interactive native acceptance is separate from CI.
For planner changes run `python3 scripts/test-release-plan.py` and
`scripts/lint-workflows.sh`.
