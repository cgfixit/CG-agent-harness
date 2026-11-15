#!/bin/sh
# Rerun the browser checks through the cgagentharness binary.
# The enter check submits a search box. The qwen check asks the local model to
# change the board. Both fail closed when the visible page is wrong.
set -eu
root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
bin="$root/target/debug/cgagentharness"
sidecar="$root/browser/sidecar.mjs"
if [ ! -x "$bin" ]; then
  echo "missing $bin. Build it with cargo build --features test-support --bin cgagentharness (fixtures serve on 127.0.0.1)" >&2
  exit 3
fi
echo "enter"
"$bin" browser enter --sidecar="$sidecar"
echo "qwen"
"$bin" browser qwen --sidecar="$sidecar"
