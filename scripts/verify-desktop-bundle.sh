#!/usr/bin/env bash
set -euo pipefail
app="${1:?Pass the absolute or relative .app path}"
architecture="${2:-arm64}"
case "$architecture" in
  arm64) expected=arm64 ;;
  universal) expected='arm64 x86_64' ;;
  *) echo 'Expected architecture: arm64 or universal' >&2; exit 2 ;;
esac
codesign --verify --deep --strict "$app"
plutil -lint "$app/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")" == com.cgfixit.agent-harness ]]
for binary in cgagentharness cg-agent-harness-desktop; do
  path="$app/Contents/MacOS/$binary"
  [[ -x "$path" ]]
  actual="$(lipo -archs "$path" | tr ' ' '\n' | LC_ALL=C sort | paste -sd ' ' -)"
  [[ "$actual" == "$expected" ]] || { echo "Unexpected architectures: $actual (expected $expected)" >&2; exit 1; }
  codesign --verify --strict "$path"
  # Only system libraries are acceptable runtime linkage. Rust/SDK are build
  # prerequisites, never a runtime dylib dependency from the checkout/Homebrew.
  if otool -arch all -L "$path" | awk '/^[[:space:]]/ {print $1}' | grep -Ev '^(/usr/lib/|/System/Library/)'; then
    echo 'Unexpected non-system dynamic dependency.' >&2; exit 1
  fi
done
# After the bundle codesign: staged sidecar bytes must still be the digest
# compiled into the shell (CGAH_BACKEND_SHA256). Re-signing the sidecar after
# embed would fail this check.
sidecar="$app/Contents/MacOS/cgagentharness"
shell="$app/Contents/MacOS/cg-agent-harness-desktop"
digest="$(shasum -a 256 "$sidecar" | awk '{print $1}')"
python3 -c 'import pathlib, sys; d = sys.argv[1].encode(); b = pathlib.Path(sys.argv[2]).read_bytes(); raise SystemExit(0 if d in b else 1)' \
  "$digest" "$shell" \
  || { echo "Staged sidecar SHA-256 is not the digest compiled into the shell (post-bundle codesign)." >&2; exit 1; }
for resource in prepare-cargo.py icon.icns DESKTOP.md DESKTOP_ACCEPTANCE.md PROCESS_LIFECYCLE.md COMMIT; do
  [[ -s "$app/Contents/Resources/$resource" ]]
done
"$app/Contents/MacOS/cgagentharness" --help | grep -q serve
set +e
"$app/Contents/MacOS/cgagentharness" agentic --config /nonexistent/cgah-packaging/config.yaml status >/dev/null 2>&1
code=$?
set -e
[[ "$code" == 3 ]] || { echo "Packaged worker dispatch returned $code; expected 3." >&2; exit 1; }
echo "Bundle resources, $architecture architecture, system linkage, signatures and native worker dispatch passed."
