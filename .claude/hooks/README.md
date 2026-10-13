# .claude/hooks

Scripts run by the Claude Code hooks declared in [`../settings.json`](../settings.json).
Keep hooks idempotent and quiet on success; a failure here shows up as a broken
first build, so read the hook output before debugging the code.

- [`session-start.sh`](session-start.sh): `SessionStart` hook. Points
  `core.hooksPath` at [`.githooks/`](../../.githooks/README.md), installs
  `pkg-config` and libdbus on cloud containers, runs `cargo fetch --locked` to
  pull the pinned toolchain, exports `CARGO_PROFILE_DEV_DEBUG=0`, and detaches a
  background prebuild of the clippy and test artifacts. Always exits 0.

Related: [scripts/ensure-githooks.sh](../../scripts/ensure-githooks.sh),
[docs/GITHOOKS.md](../../docs/GITHOOKS.md).
