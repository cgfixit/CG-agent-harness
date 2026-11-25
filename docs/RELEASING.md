# Release controls

A successful [Bundle](../.github/workflows/bundle.yml) from a push to
`main` triggers [release](../.github/workflows/release.yml).
`release-plan.py` skips superseded tips and trees identical to the latest
regular release. A changed tree plans the next patch, including docs changes.
Backend CI gates packaging. The full Windows suite is advisory on this
path. Windows MCP lifecycle remains required. Extracted CLI smoke tests and
checksums gate publication. Existing tags and releases cannot be overwritten. The planner needs the
latest regular stable release. The planned version sets the app's About
and bundle version. Use `CGAH_RELEASE_VERSION=vMAJOR.MINOR.PATCH` to build
a local candidate with that version.

## Publication delay

Only `publish` enters the `release` environment. Its wait timer and reviewer
rules apply there. A newer job entering `release-publish` cancels an older
one still waiting. Builds can overlap before publication.

## Run now or preview

- Dispatch with **publish** unchecked for a plan without builds.
- Select `main` and **publish** to release a changed tree with a green
  Bundle at that commit.
- A new stable `vMAJOR.MINOR.PATCH` tag on a reviewed commit already on
  `main` runs the same pipeline. Ahead, diverged or prerelease tags refuse.

Outside Actions, `python3 scripts/release-plan.py` provides a read-only plan
with authenticated `gh`. Inspect retained drafts and assets after a
publication failure. Collisions fail explicitly on rerun.

## Verification limits

Mac packages are universal and ad-hoc signed. Check `Resources/COMMIT`
for source provenance. Interactive native acceptance is separate from CI.
For planner changes run `python3 scripts/test-release-plan.py` and
`scripts/lint-workflows.sh`.
