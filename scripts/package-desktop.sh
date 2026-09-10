#!/usr/bin/env bash
# Apple Silicon local application package. No signing credentials or downloads.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Requires an Apple Silicon Mac.' >&2; exit 2; }
[[ "${1:-}" == '' || "${1:-}" == --dmg ]] || { echo 'Usage: package-desktop.sh [--dmg]' >&2; exit 2; }
if [[ -n "$(git status --porcelain --untracked-files=normal)" && "${CGAH_ALLOW_DIRTY:-0}" != 1 ]]; then
  echo 'Commit the reviewed change before packaging, or use CGAH_ALLOW_DIRTY=1 for a clearly marked development artifact.' >&2
  exit 2
fi
export MACOSX_DEPLOYMENT_TARGET=12.0
export RUSTUP_AUTO_INSTALL=0
cargo build --release --locked
# Sign before the shell embeds the exact sidecar digest. Do not re-sign the
# copied sidecar afterward or its build-time identity check will fail.
codesign --force --sign - --options runtime target/release/cgagentharness
(cd desktop && cargo build --release --locked)
mkdir -p dist
stage="$(mktemp -d "$PWD/dist/.desktop-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/CG Agent Harness.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/cgagentharness "$app/Contents/MacOS/"
cp desktop/target/release/cg-agent-harness-desktop "$app/Contents/MacOS/"
cp desktop/Info.plist "$app/Contents/Info.plist"
cp desktop/icons/icon.icns scripts/prepare-cargo.py "$app/Contents/Resources/"
cp docs/DESKTOP.md docs/DESKTOP_ACCEPTANCE.md docs/PROCESS_LIFECYCLE.md "$app/Contents/Resources/"
git rev-parse HEAD > "$app/Contents/Resources/COMMIT"
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  echo 'DEVELOPMENT BUILD: uncommitted changes' >> "$app/Contents/Resources/COMMIT"
fi
codesign --force --sign - --options runtime "$app/Contents/MacOS/cg-agent-harness-desktop"
codesign --force --sign - --options runtime "$app"
scripts/verify-desktop-bundle.sh "$app"
# These paths contain only generated bundles, never application homes.
rm -rf 'dist/CG Agent Harness.app'
mv "$app" 'dist/CG Agent Harness.app'
ditto -c -k --sequesterRsrc --keepParent 'dist/CG Agent Harness.app' dist/CG-Agent-Harness-macos-arm64.zip
if [[ "${1:-}" == --dmg ]]; then
  hdiutil create -volname 'CG Agent Harness' -srcfolder 'dist/CG Agent Harness.app' -ov -format UDZO dist/CG-Agent-Harness-macos-arm64.dmg
fi
(cd dist && shasum -a 256 CG-Agent-Harness-macos-arm64.zip > SHA256SUMS)
if [[ "${1:-}" == --dmg ]]; then
  (cd dist && shasum -a 256 CG-Agent-Harness-macos-arm64.dmg >> SHA256SUMS)
fi
echo "Packaged $PWD/dist/CG Agent Harness.app (arm64, ad-hoc signed, not notarized)"
