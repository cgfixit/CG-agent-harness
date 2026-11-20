#!/bin/sh
# Rerun the browser checks through the cgagentharness binary.
# The guards check proves the sidecar guards by what a loopback server received.
# The enter check submits a search box. The reach check reads a long page that
# renders late. The qwen check asks the local model to change the board. All
# fail closed when the visible page is wrong.
set -eu
root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
bin="$root/target/debug/cgagentharness"
sidecar="$root/browser/sidecar.mjs"
if [ ! -x "$bin" ]; then
  echo "missing $bin. Build it with cargo build --features test-support --bin cgagentharness (fixtures serve on 127.0.0.1)" >&2
  exit 3
fi
echo "guards"
"$bin" browser guards --sidecar="$sidecar"
echo "takeover"
"$bin" browser takeover --sidecar="$sidecar"
echo "enter"
"$bin" browser enter --sidecar="$sidecar"
echo "reach"
"$bin" browser reach --sidecar="$sidecar"
echo "qwen"
"$bin" browser qwen --sidecar="$sidecar"
