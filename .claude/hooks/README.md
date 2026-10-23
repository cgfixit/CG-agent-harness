# .claude/hooks

Scripts run by the Claude Code hooks declared in [`../settings.json`](../settings.json).
Read hook output before attributing a first-build failure to the product.

- [`session-start.sh`](session-start.sh): calls the idempotent hook installer
  everywhere, preserving an existing hook manager. Only when
  `CLAUDE_CODE_REMOTE=true`, installs missing `pkg-config`/libdbus packages,
  fetches locked Cargo dependencies, exports `CARGO_PROFILE_DEV_DEBUG=0`, and
  starts a locked background Clippy/test-binary prebuild. Logs to
  `${TMPDIR:-/tmp}/cgah-session-start.log`; preparation exits 0 even on failure.

Related: [scripts/ensure-githooks.sh](../../scripts/ensure-githooks.sh),
[docs/GITHOOKS.md](../../docs/GITHOOKS.md).
