# Issue 102 automation acceptance

Verified on 2026-09-21 before committing or drafting this PR. Base:
`cd55c3f73e7b5877a2ce763f8d5342ca70a6b36b` (merged PR #207).
The separate gateway candidate was combined locally for integration acceptance;
this automation branch contains no inbound gateway implementation.

## Final artifacts and Computer Use

Automation binary SHA-256:
`1e8af5530216df927c10af2996281f5781a7533393a508fa7364158959fc159b`.
Final combined binary SHA-256:
`3f8eb280b5934ba297d99b0c48b42a163d843d54925f1c53753a9ccc4298b318`.
Safari was operated through Computer Use against disposable loopback homes with
synthetic accounts, a fake model, a synthetic completed job and an owned receiver.
All coding write gates remained closed. Browser credentials were not saved.

- [Final account reset](automation-final-account-reset-safari.jpg): after setting
  an unpublished cron expression to `15 7 * * TUE` and timezone `America/Chicago`,
  logging out and signing into a second account resets interval to 3600, cron to
  `0 9 * * MON-FRI`, timezone to UTC, reason, confirmation and reviewed goal.
  The second account has no saved schedules. This is the final automation binary.
- [Final combined isolation](combined-final-isolation-safari.jpg): the combined
  artifact shows the second account's empty schedule inventory, default timing,
  no reviewed goal and disabled activation.

The following combined walkthrough used binary
`e866e262cc5ed3ec202267b08f045caad4b728428dcf0712c67f7c5bc3124811`.
It preceded the final form-timing reset fix; the backend behavior is unchanged.

- [Cron preview](combined-cron-preview-safari.png): review a staged goal and five
  calendar occurrences with an explicit timezone before activation.
- [Cron cancellation](combined-cron-cancelled-safari.png): activate the reviewed
  preview, then cancel; the owned row remains cancelled and cannot fire.
- [Delivery replay](combined-delivery-replayed-safari.png): a receiver's initial
  503 becomes a successful explicit replay. Exactly two receiver requests carry
  identical event/delivery IDs and only six metadata fields. The single completed
  job remains unchanged, and its private result marker is absent from requests.
- [Owner isolation](combined-owner-isolation-safari.png) and
  [delivery isolation](combined-delivery-isolation-safari.png): the second account
  sees only its own session and no administrator schedules or delivery records.

Earlier phase-specific screenshots and artifact hashes remain documented in
[ownership](OWNERSHIP.md), [scheduling](SCHEDULING.md) and [deliveries](DELIVERIES.md).
They are historical evidence, not screenshots of the final binary.

## Deterministic acceptance

- Current main baseline: 650 tests in 47 suites passed.
- Automation candidate: 671 tests in 48 suites passed.
- Combined automation and gateway: 682 tests in 49 suites passed.
- Strict Clippy, formatting, cargo-deny, actionlint and the review-reset,
  schedule-review, delivery-review and analytics JavaScript contracts passed.
- Final combined native desktop sidecar: 11 lifecycle/auth/home-lock tests passed.
- Tests cover foreign owner read/search/export/chat/cancel/clear, explicit legacy
  adoption, cron DST gaps/folds, restart/clock/overlap/cancellation, one-use preview,
  durable outbox recovery/dedup/timeout/revocation/pressure and bounded redaction.

PR #207 already supplies exact-head Linux confinement/service and Windows Job
Object acceptance: [run 35608075769](https://github.com/cgfixit/CG-agent-harness/actions/runs/35608075769),
head `42f681a512f805709506387bb9e782c847559b87`. New-PR hosted CI is separate
from this local evidence and must pass before merge. No live-provider billing,
packaged/notarized GUI, public receiver or remote gateway acceptance is claimed.
