#!/usr/bin/env bash
# Cloud-only SessionStart hook: get a fresh Claude Code cloud container ready
# for `cargo clippy` / `cargo test` without the agent paying for it mid-task.
#
# What it does (all idempotent; a warm container finishes in about a second):
#   1. Installs pkg-config + libdbus-1-dev (the keyring crate's libdbus-sys build
#      script fails without them; CI installs the same packages).
#   2. Runs `cargo fetch --locked`, which also makes rustup install the
#      toolchain pinned in rust-toolchain.toml (1.88 + clippy + rustfmt).
#   3. Exports CARGO_PROFILE_DEV_DEBUG=0 for the session, so debug builds do not
#      fill the container's disk (full debug target/ reached ~28 GB at link time).
#   4. Detaches a background prebuild of the artifacts scripts/verify-local.sh
#      uses (clippy, then the test binaries) so the first real run is warm.
#      Cargo's own build-directory lock makes an agent's cargo wait for it
#      instead of racing it.
#
# Exit code is always 0: this hook prepares, it must never block a session.
set -uo pipefail

[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0
cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null)}" 2>/dev/null || exit 0

export CARGO_PROFILE_DEV_DEBUG=0
LOG="${TMPDIR:-/tmp}/cgah-session-start.log"

prebuild() {
  # One prebuild at a time; a second invocation exits immediately.
  exec 9>"${TMPDIR:-/tmp}/cgah-prebuild.lock"
  flock -n 9 || exit 0
  # Same flags as verify-local.sh and CI: clippy args are part of the cache key,
  # so a prebuild without `-D warnings` leaves the real run ~80 s of re-linting.
  cargo clippy --all-targets --all-features --locked -- -D warnings
  cargo test --all-targets --locked --no-run
}

if [ "${1:-}" = "prebuild" ]; then prebuild; exit 0; fi

if [ -n "${CLAUDE_ENV_FILE:-}" ] && ! grep -qs 'CARGO_PROFILE_DEV_DEBUG' "$CLAUDE_ENV_FILE"; then
  echo 'export CARGO_PROFILE_DEV_DEBUG=0' >> "$CLAUDE_ENV_FILE"
fi

{
  if ! pkg-config --exists dbus-1 2>/dev/null; then
    SUDO=""; [ "$(id -u)" -ne 0 ] && SUDO="sudo -n"
    ( $SUDO apt-get update -qq && DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y -qq pkg-config libdbus-1-dev ) &
  fi
  cargo fetch --locked -q &
  wait
} >>"$LOG" 2>&1

# Prebuild needs the packages above, so it starts only after they are in place.
setsid nohup bash "$0" prebuild >>"$LOG" 2>&1 < /dev/null &
exit 0
