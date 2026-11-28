---
name: cgagentharness-gotchas
description: Diagnose misleading Harness install, packaging, browser, toolchain, sandbox, CI or release results.
---

# Diagnose misleading results

## Wrong compiler or incomplete local checks

Confirm rustup Cargo, rustc and Clippy match the root or desktop
`rust-toolchain.toml`. A `CLIPPY` override in `scripts/verify-local.sh` must
match rustc. Use a task-specific `CARGO_TARGET_DIR` for incompatible artifacts.
Read `verify-local.sh` output for skipped cargo-deny or live checks.

## Console and model-driven browser sessions

Use the fixture map in `cgagentharness-verify`. For `/browse`, check child
launch, Playwright, Chrome and the model separately. Check whether a blocked
load failed the action or only the page request. The watcher retries
network errors, 429 and 5xx within its retry limit. Reload or return to the
originating session to resume a waiting job. Server restart loses the job record.
Packaged apps need an absolute `browser.sidecar`.

## Sandbox denial and listener races

An outer sandbox can deny process creation or a loopback bind before assertions.
Use a disposable `CGAGENTHARNESS_HOME`, verify the candidate PID and listener,
and stop only owned processes. Linux refuses without usable bubblewrap by
default. Opt-in `linux-netns` isolates the network only. Verify the actual backend.

In `scripts/test-desktop-backend.py`, `free_port` closes the reservation before
the bind. `src/server/mod.rs::port_in_use` can print an already-running notice
and exit zero before loading provider keys. Inspect `await_headless` and
`serve_expecting_refusal`. Retry identified collisions on a fresh port with
bounded attempts and retain captured output. Keep a fixture listener open where
possible, as in `ModelFixture` and `test_unrelated_listener_is_never_adopted`.
When changing a shared helper, recheck all callers.

## GitHub checks and reviews

`docs impact` runs on the PR head and the merge-queue candidate.
`ci.yml`'s required gate checks which jobs may skip per event. Check the
Windows suite selection separately. Use `tests/ci_docs_only.rs` for the gates.

`github.rest.issues.listComments` wraps results in `data`.
`github.paginate(github.rest.issues.listComments, ...)` returns the array.
Exercise pagination in the extracted `pr-template-check.yml` script.

Compare reviewed SHA and unresolved threads with the advisory
`.github/workflows/review-gate.yml`. Resolving a thread sends no webhook.
Re-run the gate after the final resolution when authorized. Treat cancellation
as missing review evidence.

## Console status and release

Trace help in `src/server/views.rs` and dispatch in `assets/static/harness.html`.
If an audit view has missing lines, check `audit_gap` in `/api/status` or
`/api/diag`. An expired flush and a refused record are separate cases.

Use `cgagentharness-release` for Bundle provenance and release collisions.
