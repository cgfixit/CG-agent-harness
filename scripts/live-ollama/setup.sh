#!/usr/bin/env bash
# Start a pinned Ollama on 127.0.0.1:11434 with two small pinned models, for the
# live fingerprint (scripts/smoke-ollama.sh with FINGERPRINT_OUT). Every
# download is checked against a pinned SHA-256. The window is fixed with
# OLLAMA_CONTEXT_LENGTH so every machine loads the same one; speeds still
# differ, and the fingerprint does not record them.
#   scripts/live-ollama/setup.sh <dir>     # Linux x86_64 or macOS
# LIVE_OLLAMA_HOST (default 127.0.0.1:11434) picks the loopback address; use
# another port on a machine whose own Ollama already listens there, and pass
# MODEL_URL=http://<that address> to scripts/smoke-ollama.sh.
set -euo pipefail
cd "$(dirname "$0")/../.."
DIR="${1:?usage: scripts/live-ollama/setup.sh <dir>}"
mkdir -p "$DIR/bin" "$DIR/models" "$DIR/gguf"
DIR="$(cd "$DIR" && pwd)"

OLLAMA_VERSION="v0.12.6"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) ASSET="ollama-linux-amd64.tgz"; SUM="de82adce2ab79235115d511ff22fcb099ac53b67127870f12b80198c033ec0a1" ;; # DevSkim: ignore DS173237 because this is the release's published SHA-256, not a secret.
  Darwin-*) ASSET="ollama-darwin.tgz"; SUM="60e652314f08cd88c85e45f6b48211fb189664c0386d4ebf14bc7223bb5504c6" ;; # DevSkim: ignore DS173237 because this is the release's published SHA-256, not a secret.
  *) echo "unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

sha256_of() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
fetch() { # url sha256 dest
  if [ ! -f "$3" ] || [ "$(sha256_of "$3")" != "$2" ]; then
    curl -fsSL --retry 3 -o "$3.part" "$1"
    [ "$(sha256_of "$3.part")" = "$2" ] || { echo "checksum mismatch for $1" >&2; rm -f "$3.part"; exit 1; }
    mv "$3.part" "$3"
  fi
}

OLLAMA="$DIR/bin/ollama"
if [ ! -x "$OLLAMA" ]; then
  fetch "https://github.com/ollama/ollama/releases/download/$OLLAMA_VERSION/$ASSET" "$SUM" "$DIR/$ASSET"
  if [ "$ASSET" = ollama-darwin.tgz ]; then
    # Flat archive: the binary loads the libraries beside it.
    tar -xzf "$DIR/$ASSET" -C "$DIR/bin"
  else
    # bin/ and lib/; GPU runtimes are skipped (functional check, not a benchmark).
    tar -xzf "$DIR/$ASSET" -C "$DIR" --exclude='lib/ollama/cuda*' --exclude='lib/ollama/rocm*'
  fi
  rm -f "$DIR/$ASSET"
fi

export OLLAMA_HOST="${LIVE_OLLAMA_HOST:-127.0.0.1:11434}" # DevSkim: ignore DS162092 because the pinned Ollama must listen on loopback only.
case "$OLLAMA_HOST" in 127.0.0.1:*) ;; *) echo "LIVE_OLLAMA_HOST must be 127.0.0.1:<port>" >&2; exit 1 ;; esac # DevSkim: ignore DS162092 because this refuses any non-loopback address.
API="http://$OLLAMA_HOST/api" # DevSkim: ignore DS137138 because Ollama serves plain HTTP on loopback only (checked above).
if ! curl -fsS -m 2 "$API/version" >/dev/null 2>&1; then
  OLLAMA_MODELS="$DIR/models" OLLAMA_CONTEXT_LENGTH=8192 OLLAMA_KEEP_ALIVE=30m \
    nohup "$OLLAMA" serve >"$DIR/serve.log" 2>&1 &
  for _ in $(seq 1 100); do curl -fsS -m 2 "$API/version" >/dev/null 2>&1 && break; sleep 0.3; done
fi
curl -fsS "$API/version"; echo

# name  repo  revision  file  sha256
while read -r name repo rev file sum; do
  fetch "https://huggingface.co/$repo/resolve/$rev/$file" "$sum" "$DIR/gguf/$file"
  { printf 'FROM %s\nTEMPLATE """' "$DIR/gguf/$file"; cat scripts/live-ollama/qwen25.template; printf '"""\nPARAMETER stop "<|im_end|>"\n'; } >"$DIR/gguf/$name.Modelfile"
  "$OLLAMA" create "$name" -f "$DIR/gguf/$name.Modelfile" >/dev/null
  curl -fsS "$API/show" -d "{\"model\":\"$name\"}" |
    python3 -c 'import json,sys;caps=json.load(sys.stdin).get("capabilities") or [];assert "tools" in caps,caps;print(sys.argv[1],caps)' "$name"
done <<'MODELS'
fp-qwen25-05b:q4km Qwen/Qwen2.5-0.5B-Instruct-GGUF 9217f5db79a29953eb74d5343926648285ec7e67 qwen2.5-0.5b-instruct-q4_k_m.gguf 74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db
fp-qwen25-15b:q4km Qwen/Qwen2.5-1.5B-Instruct-GGUF 91cad51170dc346986eccefdc2dd33a9da36ead9 qwen2.5-1.5b-instruct-q4_k_m.gguf 6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e
MODELS
