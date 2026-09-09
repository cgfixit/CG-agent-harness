# Console job workflow

The server advertises the default check, allowed profiles, planner model, polling
interval and supported job capabilities at `/api/agent/checks`. The console
stages a request without a check override; today the server selects `cargo-test`.
`/agent checks <profile>` sets an explicit supported override. `/model use`
selects chat only and reports the separately configured planner model.

`/agent confirm <reason>` submits to `/api/agent/jobs` and immediately retains
its job ID in the URL fragment. `/agent job <id>` resumes monitoring after a
refresh; re-enter the API key, which is never persisted in browser storage.
`/agent jobs` lists this server process's retained jobs. Failed CLI results are
failed jobs, with their error output retained. Completed results display the
run record and verification output.

`/agent cancel` discards only a staged request. `/agent stop <id>` cancels the
active request; **descendant processes may survive**. Job state is currently
in memory: server restart loses job handles, and startup reconciliation and
live per-check progress are not implemented. Inspect persistent run records
and surviving processes before additional writes. A stop response is not proof
of process-tree termination. Chat output remains non-streaming.

Inspect `/agent status <run id>` and the complete diff before
`/agent approve <run id> <reason>`. Approval commits locally. Push and draft
publication require their own explicit `/agent push` and `/agent publish`
actions with reasons. A truncated diff cannot satisfy console review.

## Reproduce browser acceptance on macOS

Requires a built release binary, Python 3, Node with built-in WebSocket support,
Google Chrome and an already installed, explicitly chosen Ollama model. The
fixture helper creates a new directory, local bare remote, isolated application
home, synthetic API key and a local `gh` adapter. It prepares locked Cargo
inputs offline. It never calls the real GitHub CLI or pulls models.

First inspect `ollama list`; use the exact installed identifier. In one terminal:

```sh
cargo build --release
python3 scripts/browser-fixture.py /tmp/cgah-browser-acceptance \
  --model qwen3.8:27b --endpoint http://127.0.0.1:11434/v1 --port 8792
```

The destination must not exist. In another terminal, from the checkout:

```sh
CGAH_TEST_BASE_URL=http://127.0.0.1:8792 \
CGAH_TEST_HOME=/tmp/cgah-browser-acceptance/home \
CGAH_TEST_API_KEY_FILE=/tmp/cgah-browser-acceptance/api-key \
CGAH_TEST_FIXTURE=disposable-arithmetic \
node scripts/browser-acceptance.mjs
```

The test starts a fresh headless Chrome profile, checks authentication and CSRF,
stages an explicit large-file window, verifies server defaults/check selection,
submits an asynchronous job, refreshes, restores authentication, reviews the
complete expected one-expression diff, approves a local commit, separately
pushes to the local bare remote, and distinguishes staged and active cancellation.
Missing Chrome, model failure, failed verification or wrong diff fails the test.
No npm dependency or Chrome sandbox bypass is used. Stop the fixture server with
Ctrl-C; its disposable directory remains for inspection.

The model endpoint must itself enforce the desired network policy. A localhost
URL alone does not prove offline inference. This test does not establish remote
GitHub publication, startup recovery or descendant termination.

## Recorded acceptance

On the target Apple M5 Pro/48 GiB, macOS 26.6.2, actual Chrome exercised the flow
with installed `qwen3.8:27b`, a separate Ollama process with Seatbelt non-loopback
network denial, and real offline sandboxed Cargo. This is real local-model
browser evidence, separate from Rust route tests. The operator's normal Ollama
process and model files were unchanged.
