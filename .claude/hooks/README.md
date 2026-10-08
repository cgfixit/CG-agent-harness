# .claude/hooks

Scripts run by Claude Code hooks declared in `../settings.json`.
`session-start.sh` prepares the Rust toolchain, libdbus and the build cache at
session start. Keep hooks idempotent and quiet on success; a failure here shows
up as a broken first build, so read the hook output before debugging the code.
