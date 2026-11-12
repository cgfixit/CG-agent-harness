# Release controls

The `release` workflow checks `main` at **08:17 and 20:17 UTC**; GitHub can
delay [scheduled runs](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).

It compares the source tree with the latest regular release:

- **Identical tree:** every build and publication is skipped, even when commit
  IDs differ after a squash merge.
- **Changed tree:** still skipped unless that exact source SHA has a successful
  **Bundle** run (`bundle.yml`). A green Bundle unblocks the next scheduled or
  manual release, which takes the next patch version (`v0.1.0` to `v0.1.1`).
  Documentation and workflow changes count.

The planned tag sets the app's About and macOS bundle versions; Cargo manifests
stay unchanged and bundled `Resources/COMMIT` identifies the source. For a
local release candidate, run `scripts/package-desktop.sh` with
`CGAH_RELEASE_VERSION=vMAJOR.MINOR.PATCH`.

Bundle and release both stage CLI artifacts through `scripts/package-release.sh`
(release build, named artifact, sha256, extract-verify). Backend CI must pass
before CLI packaging and the universal macOS desktop build. Release packaging
(not Bundle) neither restores nor saves shared build caches. Once both packaging jobs and the
downloaded checksums pass, the workflow rechecks the version, creates a draft
with all assets and publishes it as the regular Latest release. Existing
releases and tags are never overwritten. Publication uses the event's exact
source commit, so commits merged during a build wait for a later run.

Publication needs no manual approval: the `publish` job declares no
environment, so verification, both builds and the source recheck are the only
review. `preflight` stops a token that cannot create releases before anything
builds.

## Change the timing

Edit `on.schedule` in `.github/workflows/release.yml` through a PR; cron is in
UTC, so `17 8 * * 1` checks Mondays at 08:17.

**Bundle** runs only on pushes to `main` and publishes nothing. Releasing after
every merge would mean replacing the schedule with `push.branches: [main]` and
extending the planner's push handling, which accepts tag pushes only.

## Run now or preview

In GitHub, open **Actions → Release - Plan, verify and publish → Run workflow**:

- Leave **publish** unchecked for a read-only plan: the run summary shows the
  candidate tag, source commit, changed flag and `should_release: false`. A
  preview works on any branch and builds nothing.
- Select `main` and check **publish** to run the verified pipeline now. An
  unchanged tree still skips, and no other branch can publish this way.
- For a deliberate major or minor version, push an unused stable tag such as
  `v0.2.0` at a reviewed commit already on `main`; the planner refuses a tag
  GitHub reports `ahead` of or `diverged` from `main`. Tag releases run the
  same pipeline. Prerelease tags are rejected.

The planner requires an existing latest regular `vMAJOR.MINOR.PATCH` release;
API errors and occupied versions fail explicitly. Publication runs share one
concurrency group: the running release keeps its slot and only the newest
pending run is retained. Previews use separate groups, so they neither wait
behind a release nor replace a queued one. After an upload or publication
failure, inspect the draft and its assets before recovering; reruns refuse
collisions and never overwrite uploads.

## Verification limits

Scheduled builds carry Apple Silicon and Intel executables and pass automated
bundle and backend tests. They are ad-hoc signed, not Developer ID signed or
notarized, and CI runs no interactive GUI acceptance on a Mac: do a native
acceptance pass (the Codex release skill) when desktop behavior changes.

To validate planner changes locally:

```sh
python3 scripts/test-release-plan.py
actionlint
zizmor .github/workflows
```

Planner tests also run in the workflow-lint PR check and before release
planning. Outside Actions, `python3 scripts/release-plan.py` previews the next
release of `main` read-only (needs `gh auth login`);
`GITHUB_EVENT_NAME=schedule` shows what the next scheduled run would decide.
