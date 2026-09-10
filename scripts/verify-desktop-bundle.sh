#!/usr/bin/env bash
set -euo pipefail
app="${1:?Pass the absolute or relative .app path}"
codesign --verify --deep --strict "$app"
plutil -lint "$app/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")" == com.cgfixit.agent-harness ]]
for binary in cgagentharness cg-agent-harness-desktop; do
  path="$app/Contents/MacOS/$binary"
  [[ -x "$path" ]]
  [[ "$(lipo -archs "$path")" == arm64 ]]
  codesign --verify --strict "$path"
  # Only system libraries are acceptable runtime linkage. Rust/SDK are build
  # prerequisites, never a runtime dylib dependency from the checkout/Homebrew.
  if otool -L "$path" | tail -n +2 | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/Library/)'; then
    echo 'Unexpected non-system dynamic dependency.' >&2; exit 1
  fi
done
for resource in prepare-cargo.py icon.icns DESKTOP.md DESKTOP_ACCEPTANCE.md PROCESS_LIFECYCLE.md COMMIT; do
  [[ -s "$app/Contents/Resources/$resource" ]]
done
"$app/Contents/MacOS/cgagentharness" --help | grep -q serve
set +e
"$app/Contents/MacOS/cgagentharness" agentic --config /nonexistent/cgah-packaging/config.yaml status >/dev/null 2>&1
code=$?
set -e
[[ "$code" == 3 ]] || { echo "Packaged worker dispatch returned $code; expected 3." >&2; exit 1; }
echo 'Bundle resources, arm64 architecture, system linkage, signatures and worker dispatch passed.'
