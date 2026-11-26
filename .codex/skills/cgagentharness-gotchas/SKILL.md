---
name: cgagentharness-gotchas
description: Diagnose misleading Harness install, packaging, browser, toolchain, sandbox, CI or release results.
---

# Diagnose misleading results

Read the corresponding trap in `AGENTS.md`, then locate the failure or apparent
success in the actual command path.

## Wrong compiler or incomplete local checks

Confirm rustup Cargo, rustc and Clippy match the root or desktop
`rust-toolchain.toml`. A `CLIPPY` override in `scripts/verify-local.sh` must
match rustc. Use a task-specific `CARGO_TARGET_DIR` for incompatible artifacts.
For slow scrypt, check the dependency optimization setting in `Cargo.toml`.
A green `verify-local.sh` can include skipped cargo-deny or live checks. Read
its output and use scoped commands for ordinary changes.

## Console and model-driven browser sessions

Use the fixture map in `cgagentharness-verify`. For `/browse`, check child
launch, Playwright, Chrome and the model separately. Distinguish a refused
redirect from a blocked request on the page. After reload or a session switch,
the console can rediscover a waiting job. After server restart, its record
is gone and the console reports it unavailable. Packaged apps need an
absolute `browser.sidecar`; see `cgagentharness-release`.

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

`docs impact` runs on the PR head; `docs-impact-repair.yml` handles repairs.

`github.rest.issues.listComments` wraps results in `data`.
`github.paginate(github.rest.issues.listComments, ...)` returns the array.
Exercise pagination in the extracted `pr-template-check.yml` script.

Compare reviewed SHA and unresolved threads with the advisory
`.github/workflows/review-gate.yml`. Resolving a thread sends no webhook.
Re-run the gate after the final resolution when authorized. Treat cancellation
as missing review evidence.

## Console command mapping and release collisions

Trace help in `src/server/views.rs` and dispatch in `assets/static/harness.html`.
For example, `gateMap` maps `/memory auto-retrieve` to `auto_retrieval` at
`POST /api/structured-memory/gates`.

Use `cgagentharness-release` for same-SHA Bundle evidence and existing runs.
Preserve tags and assets after ambiguous publication. `scripts/release-plan.py`
rejects collisions.
