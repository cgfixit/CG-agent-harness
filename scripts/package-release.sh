#!/usr/bin/env bash
# Stage a release binary, optionally ad-hoc codesign it on macOS, archive it,
# write SHA-256 checksums, and verify after a re-read + extract.
#
# This is a loopback CLI/server binary — never a .app bundle. A single-file
# tar.gz plus checksums is the whole packaging contract. Used by both
# `.github/workflows/bundle.yml` and `.github/workflows/release.yml` so the
# two cannot drift.
#
# Usage:
#   scripts/package-release.sh <bin-path> <artifact-stem> [dist-dir]
#
# Example:
#   scripts/package-release.sh target/release/cgagentharness cgagentharness-linux-x86_64
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  printf 'usage: %s <bin-path> <artifact-stem> [dist-dir]\n' "$0" >&2
  exit 2
fi

BIN=$1
STEM=$2
DIST=${3:-dist}

if [[ ! -f "$BIN" ]]; then
  printf 'package-release: binary not found: %s\n' "$BIN" >&2
  exit 1
fi

mkdir -p "$DIST"
STAGED="$DIST/$STEM"
cp "$BIN" "$STAGED"
chmod +x "$STAGED" 2>/dev/null || true

os=$(uname -s | tr '[:upper:]' '[:lower:]')
if [[ "$os" == darwin* ]] && command -v codesign >/dev/null 2>&1; then
  # Ad-hoc signature only (--sign -). Not a Developer ID / notarized build.
  codesign --force -s - "$STAGED"
  codesign --verify --verbose "$STAGED"
fi

hash_file() {
  local f=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$f" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$f" | awk '{print $1}'
  else
    printf 'package-release: no sha256sum/shasum on PATH\n' >&2
    exit 1
  fi
}

write_checksum() {
  local f=$1
  local dir base hash
  dir=$(dirname "$f")
  base=$(basename "$f")
  hash=$(hash_file "$f")
  # Two spaces: GNU sha256sum / `shasum -a 256 -c` compatible.
  printf '%s  %s\n' "$hash" "$base" >"$dir/$base.sha256"
}

verify_checksum() {
  local f=$1
  local dir base expected actual
  dir=$(dirname "$f")
  base=$(basename "$f")
  expected=$(awk '{print $1}' "$dir/$base.sha256")
  actual=$(hash_file "$f")
  if [[ "$expected" != "$actual" ]]; then
    printf 'package-release: checksum mismatch for %s\n' "$base" >&2
    printf '  expected %s\n  actual   %s\n' "$expected" "$actual" >&2
    exit 1
  fi
}

write_checksum "$STAGED"

ARCHIVE_NAME="${STEM}.tar.gz"
ARCHIVE="$DIST/$ARCHIVE_NAME"
tar -czf "$ARCHIVE" -C "$DIST" "$STEM"
write_checksum "$ARCHIVE"

# Re-read both checksums, then extract and confirm the payload is intact.
verify_checksum "$STAGED"
verify_checksum "$ARCHIVE"

VERIFY=$(mktemp -d)
trap 'rm -rf "$VERIFY"' EXIT
tar -xzf "$ARCHIVE" -C "$VERIFY"
EXTRACTED="$VERIFY/$STEM"
if [[ ! -f "$EXTRACTED" ]]; then
  printf 'package-release: extract did not produce %s\n' "$STEM" >&2
  exit 1
fi
if [[ "$(hash_file "$EXTRACTED")" != "$(hash_file "$STAGED")" ]]; then
  printf 'package-release: extracted binary hash drifted from staged copy\n' >&2
  exit 1
fi
if [[ "$os" == darwin* ]] && command -v codesign >/dev/null 2>&1; then
  codesign --verify --verbose "$EXTRACTED"
fi

printf 'package-release: staged %s (+ .sha256) and %s (+ .sha256)\n' "$STEM" "$ARCHIVE_NAME"
