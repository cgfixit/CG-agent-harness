#!/usr/bin/env bash
# macOS application package. Toolchains/targets must already be installed.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Requires an Apple Silicon Mac.' >&2; exit 2; }
architecture=arm64
dmg=0
for argument in "$@"; do
  case "$argument" in
    --universal) architecture=universal ;;
    --dmg) dmg=1 ;;
    *) echo 'Usage: package-desktop.sh [--universal] [--dmg]' >&2; exit 2 ;;
  esac
done
if [[ -n "$(git status --porcelain --untracked-files=normal)" && "${CGAH_ALLOW_DIRTY:-0}" != 1 ]]; then
  echo 'Commit the reviewed change before packaging, or use CGAH_ALLOW_DIRTY=1 for a clearly marked development artifact.' >&2
  exit 2
fi
export MACOSX_DEPLOYMENT_TARGET=12.0
export RUSTUP_AUTO_INSTALL=0
if [[ "$architecture" == universal ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target "$target"
  done
  mkdir -p target/release
  lipo -create target/aarch64-apple-darwin/release/cgagentharness target/x86_64-apple-darwin/release/cgagentharness -output target/release/cgagentharness
else
  cargo build --release --locked --target aarch64-apple-darwin
  mkdir -p target/release
  cp target/aarch64-apple-darwin/release/cgagentharness target/release/cgagentharness
fi
# Sign before the shell embeds the exact sidecar digest. Do not re-sign the
# copied sidecar afterward or its build-time identity check will fail.
codesign --force --sign - --options runtime target/release/cgagentharness
if [[ "$architecture" == universal ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd desktop && cargo build --release --locked --target "$target")
  done
  mkdir -p desktop/target/release
  lipo -create desktop/target/aarch64-apple-darwin/release/cg-agent-harness-desktop desktop/target/x86_64-apple-darwin/release/cg-agent-harness-desktop -output desktop/target/release/cg-agent-harness-desktop
else
  (cd desktop && cargo build --release --locked --target aarch64-apple-darwin)
  mkdir -p desktop/target/release
  cp desktop/target/aarch64-apple-darwin/release/cg-agent-harness-desktop desktop/target/release/cg-agent-harness-desktop
fi
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
scripts/verify-desktop-bundle.sh "$app" "$architecture"
# These paths contain only generated bundles, never application homes.
rm -rf 'dist/CG Agent Harness.app'
mv "$app" 'dist/CG Agent Harness.app'
archive="CG-Agent-Harness-macos-$architecture"
ditto -c -k --sequesterRsrc --keepParent 'dist/CG Agent Harness.app' "dist/$archive.zip"
if [[ "$dmg" == 1 ]]; then
  hdiutil create -volname 'CG Agent Harness' -srcfolder 'dist/CG Agent Harness.app' -ov -format UDZO "dist/$archive.dmg"
fi
(cd dist && shasum -a 256 "$archive.zip" > SHA256SUMS)
if [[ "$dmg" == 1 ]]; then
  (cd dist && shasum -a 256 "$archive.dmg" >> SHA256SUMS)
fi
echo "Packaged $PWD/dist/CG Agent Harness.app ($architecture, ad-hoc signed, not notarized)"
