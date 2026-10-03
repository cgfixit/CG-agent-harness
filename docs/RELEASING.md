# Release controls

The `release` workflow checks `main` at **08:17 and 20:17 UTC**
(04:17/16:17 New York in summer, 03:17/15:17 in winter). GitHub can delay
scheduled runs. The default-branch workflow owns this schedule. See [schedule events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).

It compares the source tree with the latest regular release. Identical trees
skip all builds and publication, including when commit IDs differ after a
squash merge. A changed tree still skips unless that exact source SHA already
has a successful **Bundle** workflow run (`bundle.yml`, `conclusion=success`).
A green Bundle unblocks the next scheduled or manual release. A changed tree
then selects the next patch version, such as `v0.1.0` to `v0.1.1`. Documentation
and workflow changes count. The application/Cargo version is not automatically
edited; the release tag and bundled `Resources/COMMIT` identify the build.

Both Bundle and release stage CLI artifacts through `scripts/package-release.sh`
(release build, named artifact, sha256, extract-verify). Backend CI must pass before clean CLI packaging and the reusable universal
macOS desktop build. Both packaging jobs and downloaded checksums must pass. The workflow rechecks the version, creates a draft with all
assets, and publishes it as a regular Latest release (`gh release edit --latest`).
Existing releases and tags are never overwritten. Publication uses the event's
exact source commit, so commits merged during a build wait for a later run.

## Change the timing

Edit `on.schedule` in `.github/workflows/release.yml` through a PR. Cron fields
are minute, hour (UTC), day of month, month, day of week. For example,
`17 8 * * 1` checks Mondays at 08:17 UTC.

PR **Bundle** runs produce downloadable artifacts without publishing releases. To release after every merge instead of
the 12-hour check, replace the schedule with `push.branches: [main]` alongside the
existing `push.tags`, and extend the planner's push handling to use the main
version-selection path. Adding only the trigger fails because the planner accepts tag pushes only.

## Run now or preview

In GitHub, open **Actions → release → Run workflow**:

- Leave **publish** unchecked for a read-only plan. The run summary shows the
  candidate tag, source commit, changed flag, and `should_release: false`.
  Preview is allowed on a feature branch and does not build or publish.
- Select `main` and check **publish** to run the verified release pipeline now.
  An unchanged tree still skips. Other branches cannot publish this way.
- For an intentional major/minor version, push an unused stable tag such as
  `v0.2.0` pointing at the desired reviewed commit. Tag releases run the same
  verification pipeline. Prerelease tags are rejected by this regular-release
  workflow; create previews deliberately using a separate manual process.

The planner requires an existing latest regular `vMAJOR.MINOR.PATCH` release.
API errors and occupied versions fail explicitly. Release runs are serialized.
After upload/publication failure, inspect the draft and assets before recovery;
reruns refuse collisions and never overwrite uploads.

## Verification limits

Scheduled builds include both Apple Silicon and Intel executables and automated
bundle/backend tests. They are ad-hoc signed, not Developer ID signed or
notarized. CI does not claim a new interactive GUI acceptance test on the
operator's Mac for each scheduled release. Follow the repository release skill
for a deliberate native acceptance pass when desktop behavior changes.

To validate planner changes locally:

```sh
python3 scripts/test-release-plan.py
actionlint
zizmor .github/workflows
```

Planner tests also run in the workflow-lint PR check and before release planning.
