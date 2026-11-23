# Release controls

The `release` workflow runs when a **Bundle** run (`bundle.yml`) on a `main`
push completes successfully. Bundle runs on every `main` push; a newer push
replaces a pending Bundle run, never the newest one. The plan job ignores a
Bundle run that failed, was not a push, or came from another branch or
repository.

The planner (`scripts/release-plan.py`) takes the Bundle run's commit and
compares its tree with the latest regular release:

- **Identical tree:** every build and publication is skipped, even when commit
  IDs differ after a squash merge.
- **Superseded:** the Bundle commit is no longer `main`'s tip. The run skips
  without failing; the tip's own green Bundle run triggers its release.
- **Changed tree:** the run verifies, builds and publishes the next patch
  version (`v0.1.0` to `v0.1.1`). Documentation and workflow changes count.

The planned tag sets the app's About and macOS bundle versions; Cargo manifests
stay unchanged and bundled `Resources/COMMIT` identifies the source. For a
local release candidate, run `scripts/package-desktop.sh` with
`CGAH_RELEASE_VERSION=vMAJOR.MINOR.PATCH`.

Bundle and release both stage CLI artifacts through `scripts/package-release.sh`
(build, checksum, extract, smoke-test the extracted CLI). Backend CI must pass
before CLI packaging and the universal macOS desktop build. The full Windows
test suite is advisory there, as on `main` pushes: Windows is not in the
release bundle. `Windows MCP Job Object lifecycle (required)` still fails the
run. Release packaging (not Bundle) neither restores nor saves shared build
caches. Once both packaging jobs and the downloaded checksums pass, the workflow
rechecks the version and publishes a public release, marked Latest, with all
assets. Existing releases and tags are never overwritten. `preflight` stops a
token that cannot create releases before anything builds.

## Publication delay

Only the `publish` job uses the `release` environment, so its protection rules
apply to publication alone. Set the delay in **Settings → Environments →
release**: a wait timer (for example 60-120 minutes), no required reviewer,
and optionally deployment branches limited to `main` (tags then need their own
rule). With a required reviewer, every release waits for that approval.

`publish` jobs share the concurrency group `release-publish` with
`cancel-in-progress: true`: a newer run reaching `publish` cancels one still
waiting on the timer, so the newest verified commit is the one published. A
release lands only when no newer run reaches `publish` within the timer. A
superseded tag release can be re-run from its run page.

## Run now or preview

In GitHub, open **Actions → Release - Plan, verify and publish → Run workflow**:

- Leave **publish** unchecked for a read-only plan: the run summary shows the
  candidate tag, source commit, changed flag and `should_release: false`. A
  preview works on any branch and builds nothing.
- Select `main` and check **publish** to release `main` now, for example after
  a failed run. It requires a green Bundle run at that commit; an unchanged
  tree still skips, and no other branch can publish this way.
- For a deliberate major or minor version, push an unused stable tag such as
  `v0.2.0` at a reviewed commit already on `main`; the planner refuses a tag
  GitHub reports `ahead` of or `diverged` from `main`. Tag releases run the
  same pipeline. Prerelease tags are rejected.

The planner requires an existing latest regular `vMAJOR.MINOR.PATCH` release;
API errors and occupied versions fail explicitly. After an upload or
publication failure, inspect any draft `gh` left and its assets before
recovering; reruns refuse collisions and never overwrite uploads.

## Verification limits

Released builds carry Apple Silicon and Intel executables and pass automated
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
`GITHUB_EVENT_NAME=workflow_run` shows what a green Bundle run on the tip would
decide.
