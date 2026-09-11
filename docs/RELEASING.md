# Release controls

The `release` GitHub Actions workflow checks `main` daily at **08:17 UTC**
(4:17 a.m. Eastern in summer, 3:17 a.m. in winter). GitHub can delay scheduled
runs; this is a daily check, not an exact delivery deadline. Schedules become
active when the workflow is merged into the default branch.

It compares the source tree with the latest regular release. Identical trees
skip all builds and publication, including when commit IDs differ after a
squash merge. A changed tree selects the next patch version: `v0.1.0` becomes
`v0.1.1`. Changes to documentation or workflows also count as source changes.
The application/Cargo version is not automatically edited; the release tag and
bundled `Resources/COMMIT` identify the build.

Backend CI must pass before clean CLI packaging and the reusable universal
macOS desktop build. Both packaging jobs must pass, then downloaded checksums
are verified. The workflow rechecks the version, creates a draft with all
assets, and publishes it as a regular Latest release. Existing releases and
tags are never overwritten. Publication uses the event's exact source commit,
so commits merged during a build wait for a later run.

## Change the timing

Edit `on.schedule` in `.github/workflows/release.yml` through a PR. The five
cron fields are minute, hour (UTC), day of month, month, day of week. For
example, `17 8 * * *` checks daily at 08:17 UTC; `17 8 * * 1` checks Mondays.

PRs already build downloadable artifacts through **Bundle**. Opening a PR
does not publish its unmerged code. To release after every merge instead of
once daily, replace the schedule with `push.branches: [main]` alongside the
existing `push.tags`, and extend the planner's push handling to use the main
version-selection path. Do not merely add the trigger: the current planner
intentionally accepts tag pushes only. Daily batching is the configured policy.

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
API errors and occupied candidate versions fail the run instead of being
reported as unchanged. Concurrent release workflow runs are serialized.
If uploading or final publication fails, inspect the resulting draft and its
assets before manually recovering it; reruns deliberately refuse a collision.
There is no upload-with-overwrite fallback.

## Verification limits

Daily builds include both Apple Silicon and Intel executables and automated
bundle/backend tests. They are ad-hoc signed, not Developer ID signed or
notarized. CI does not claim a new interactive GUI acceptance test on the
operator's Mac for each daily release. Follow the repository release skill
for a deliberate native acceptance pass when desktop behavior changes.

To validate planner changes locally:

```sh
python3 scripts/test-release-plan.py
actionlint
zizmor .github/workflows
```

Planner tests also run in the workflow-lint PR check and before release planning.
