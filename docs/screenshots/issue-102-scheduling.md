# Scheduling Computer Use evidence

2026-09-21, Safari private local fixture at `http://127.0.0.1:57688`.
Backend SHA-256: `1e25adfbb5ef3f0aede49b57b5f97adfc27e9898036b7d83f305f7c4b5e5efe4`.
Synthetic accounts and goal; coding gates remain closed. No live provider, native
packaged app or real repository mutation is claimed by this browser evidence.

- `cron-preview-safari.png`: reviewed goal and explicit reason/checkbox, five weekday
  occurrences for `0 9 * * MON-FRI` in `America/New_York`, matching UTC times.
- `cron-activated-safari.png`: successful one-shot preview activation; owned row visible.
- `cron-cancelled-safari.png`: Cancel schedule preserves the row with cancelled status.

The walkthrough caught transparent dialog styling and per-keystroke schedule-list
requests. Both were repaired before these screenshots. A later text-only change
replaces the previous activation message after cancellation; that final artifact was subsequently covered by the integrated
acceptance linked below.

Backend fixtures separately verify DST gaps/folds, leap dates, invalid cron,
legacy ownership/cadence migration, no catch-up after restart, backward/forward
clock shifts, stale poll snapshots, persistence refusal, preview binding/expiry,
current owner revocation and existing run-gate overlap refusal.

Final integrated acceptance is now recorded in [final acceptance](issue-102-final-acceptance.md).
