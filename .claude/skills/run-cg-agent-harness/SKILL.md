---
name: run-cg-agent-harness
description: Build, run, drive, screenshot, and smoke-test the cgagentharness console (Rust axum server + browser UI) on a headless Linux box with a fake local model. Use when asked to run, start, launch, screenshot, or verify the harness/console/server end-to-end.
---

# Run cg-agent-harness

`cgagentharness serve` is a loopback-only axum server (default `127.0.0.1:8790`)
serving a single-page console (`assets/static/harness.html`). Chat needs an
OpenAI-compatible model server; the driver fakes one so nothing is downloaded.
All paths below are relative to the repo root. The driver is
`.claude/skills/run-cg-agent-harness/driver.mjs` (Node 22, no npm deps).

## Prerequisites

Already present in the Claude Code web container: Rust 1.88 via rustup
(`rust-toolchain.toml` pins it; first `cargo` call downloads it), Node 22,
Playwright at `/opt/node22/lib/node_modules/playwright`, Chromium at
`/opt/pw-browsers/chromium`. Tests and the shim spawn real `git`, so set an
identity once:

```bash
git config --global user.email >/dev/null || { git config --global user.email ci@example.invalid; git config --global user.name CI; }
```

## Build

```bash
cargo build --locked            # target/debug/cgagentharness (~2.5 min cold, seconds warm)
```

The driver uses the debug binary by default (`CGAH_BIN` overrides). The repo's
own Python suites (`scripts/test-desktop-backend.py`, `scripts/browser-fixture.py`)
hardcode `target/release/`; build that with `cargo build --release --locked` only
if you need them.

## Run (agent path)

```bash
D=.claude/skills/run-cg-agent-harness/driver.mjs
node $D smoke                                   # up -> 9 API checks through the guard chain -> down; JSON summary, exit 1 on any FAIL
node $D shot out.png /status /tools 'hello'     # up -> real browser runs each console command -> full-page screenshot -> down
node $D serve                                   # up and stay up; prints base, home, CSRF token, a ready-made curl; Ctrl-C tears down
CGAH_BASE=http://127.0.0.1:8790 node $D smoke   # drive an already-running server: model-agnostic checks, no /api/agent/run probe (8 checks)
```

What `up` does: starts a fake OpenAI-compatible model on `127.0.0.1:18434`
(`CGAH_MODEL_PORT`), writes a disposable home (`CGAH_HOME` to keep one) whose
`config.yaml` is the shipped default with `base_url`/`model` rewritten to the
fake, blanks the cloud key env vars, runs `serve --port 8790` (`CGAH_PORT`), and
polls `GET /` until ready. `shot` prints the last 1200 chars of the console
stream to stdout so you can assert on it without opening the PNG.

Talking to the API by hand (every `/api/*` operator route needs both headers):

```bash
B=http://127.0.0.1:8790
CSRF=$(curl -s --noproxy '*' $B/ | grep -o 'name="csrf-token" content="[^"]*"' | sed 's/.*content="//;s/"$//')
curl -s --noproxy '*' -H "X-CyClaw-CSRF: $CSRF" -H "Origin: $B" $B/api/status
curl -s --noproxy '*' -H "X-CyClaw-CSRF: $CSRF" -H "Origin: $B" -H 'Content-Type: application/json' \
  -X POST $B/api/chat -d '{"session_id":"","message":"hello harness"}'
```

`session_id: ""` auto-creates a session. The console's slash commands map to
these routes; `GET /api/tools` returns the full command -> route diagram.

## Direct invocation (no server)

The agentic pipeline is a hidden subcommand the server spawns as a child. It
needs an explicit `--config` (it does not derive it from the home) and a home
that `serve` has already seeded; the driver seeds one when `CGAH_HOME` is set:

```bash
mkdir -p /tmp/cgah-home && CGAH_HOME=/tmp/cgah-home node $D smoke
CGAGENTHARNESS_HOME=/tmp/cgah-home target/debug/cgagentharness agentic --config /tmp/cgah-home/config.yaml status
```

Prints the agentic status table and `agentic.enabled is false ... nothing to do`,
exit 0. Without `--config` it exits 3 (`cannot read config file`).
For a pure function change, prefer `cargo test --test <file> <name>` (see Test).

## Run (human path)

```bash
CGAGENTHARNESS_HOME=/tmp/cgah-home target/debug/cgagentharness serve --port 8790
```

First run seeds `config.yaml`, `sessions/`, `data/` etc. into the home. Open
`http://127.0.0.1:8790/`. Useless headless, and chat fails unless that
`config.yaml` points at a real Ollama (`models.local_llm.base_url` / `model`).

## Test

```bash
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" cargo test --all-targets --locked
cargo test --test invariant_guard --locked                       # 11 source-scan tests, ~0.02s after build
cargo test --test real_repo_loop real_repo_run_smoke_end_to_end -- --nocapture   # one test
```

## Gotchas

- **`curl --noproxy *` unquoted is a shell glob.** It expands to filenames in
  the cwd, curl treats them as URLs, and you get nginx 301 pages from the
  container's proxy mixed into your output. Always `--noproxy '*'`. Node's
  `fetch` ignores the proxy env, so the driver is unaffected.
- **Two `Origin` headers pass the guard.** `enforce_same_origin` reads the first
  `Origin` value only; a second `-H Origin:` on the curl line is silently
  ignored, which looks like a bypass and is not. Send one.
- **Console load logs one 503.** `GET /api/auth/setup-status` returns
  `AUTH_DISABLED` because `auth.enabled: false` ships. Harmless.
- **`POST /api/agent/run` validates before it gates.** Missing `branch` or
  `commit_message` is a 422 `VALIDATION_ERROR`; only a fully-formed body reaches
  the 409 `AGENTIC_DISABLED` banner. Both are correct with shipped config.
- **`GET /api/github/status` is not a stub.** It goes through `src/shim`, spawns
  `cgagentharness agentic status` as a child, and returns its exit code. It is
  the cheapest live proof that the I6 boundary works.
- **Playwright is global, not resolvable from the repo.** ESM `import('playwright')`
  fails; the driver imports the absolute path
  `/opt/node22/lib/node_modules/playwright/index.mjs` (`PLAYWRIGHT_MODULE`
  overrides). Chromium needs `--no-sandbox` as root.
- **The console calls `onSend()` on a global.** The driver fills `#input` and
  evaluates `onSend()`; it waits on `#stream` growing and `window.inflightChat`
  clearing rather than a fixed sleep, because chat is async.
- **`scripts/chat-browser-acceptance.mjs` does not run the real server.** It
  serves `harness.html` from a mock API and needs `/usr/bin/google-chrome`
  (`CHROME_BIN=/opt/pw-browsers/chromium` works). Use the driver for the real thing.
- **Desktop crate (`desktop/`) does not build on Linux.** It is a Tauri 2 shell
  on Rust 1.90 (separate toolchain download); `cargo check` fails on
  `gdk-3.0.pc` missing. See Troubleshooting for how far the apt route got.
  Packaging is macOS only (`scripts/package-desktop.sh`).

## Troubleshooting

- `serve exited 1` with `refusing to bind` -> non-loopback host or port < 1024.
  Only `127.0.0.1`/`localhost`, port 1024-65535.
- `config.default.yaml no longer matches the sed pattern` from the driver ->
  someone changed the shipped `base_url`/`model` literals; update `makeHome()`.
- `403 CSRF_TOKEN_INVALID` -> token is per-process; re-fetch `GET /` after any
  restart. `403 CROSS_ORIGIN_BLOCKED` -> `Origin` must be exactly `http://127.0.0.1:<port>`.
- `binary missing` -> `cargo build --locked`.
- Port 8790 busy -> `CGAH_PORT=8791 node $D ...`, and `pgrep -fl target/debug/cgagentharness`
  to find a leaked server; the driver SIGTERMs then SIGKILLs its own child on exit.
- `cd desktop && cargo check` -> `The system library gdk-3.0 required by crate
  gdk-sys was not found`. Tried `apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev
  libayatana-appindicator3-dev librsvg2-dev`: the container's apt index is stale
  and the fetch 404s (`xdg-desktop-portal ... Not Found`) without an
  `apt-get update`, which the network policy did not allow to complete here.
  Not verified on Linux; treat the desktop crate as macOS-only for now.
