# CG-agent-harness E2E — fresh clone (2026-09-13)

**Repo:** [cgfixit/CG-agent-harness](https://github.com/cgfixit/CG-agent-harness)  
**When:** Sunday, 2026-09-13 (America/New_York)  
**Tree:** brand-new `gh repo clone` at `/workspace/harness-e2e-2026-09-13/src` (not a reuse of yesterday’s worktrees)  
**This report:** `/workspace/cg-agent-harness-e2e-2026-09-13.md`  
**Supporting:** `/workspace/harness-e2e-2026-09-13/findings.md`  
**Legend:** **FACT** = observed this run. **INFERENCE** = interpretation.

No PRs. Isolated `CGAGENTHARNESS_HOME`. `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE=1` on the served binary. No real-account / real-repo writes.

---

## Verdict

| Surface | Result |
|---|---|
| Fresh clone | `main` @ `dddc8f01df0561a4d8fd4d1df113c6c8c9699cec` == tag **v0.1.9** — FACT |
| Latest GitHub release | **v0.1.9** (published 2026-09-13); HEAD matches the tag — FACT |
| rustc / cargo | **1.88.0** (rustup) — FACT |
| `cargo test --no-fail-fast --locked` | **226 passed / 0 failed / 0 ignored** (exit 0) — FACT |
| Extra Linux E2E scripts | desktop-backend **11/11**, chat-browser **pass**, release-plan **17/17** — FACT |
| Source-built binary matrix | **35 pass / 0 fail** (HTTPS + session auth defaults) — FACT |
| macOS GUI / desktop digest / live Ollama | **BLOCKED** on this Linux host — FACT |

**Linux source + cargo + binary path: PASS.** Mac interactive UI still unproven here.

---

## 1. Clone identity — FACT

| Item | Value |
|---|---|
| Path | `/workspace/harness-e2e-2026-09-13/src` |
| Method | `gh repo clone cgfixit/CG-agent-harness` |
| Default branch | `main` |
| HEAD | `dddc8f01df0561a4d8fd4d1df113c6c8c9699cec` |
| `git describe` | `v0.1.9` |
| HEAD == latest release? | **Yes** |

`Cargo.toml` still says `version = "0.1.0"` while the tag is `v0.1.9`. Binary `--version` is `cgagentharness 0.1.0` (**FACT** version-string skew).

---

## 2. Toolchain — FACT

| Tool | Version |
|---|---|
| rustc | `1.88.0` via `/home/box/.cargo/bin` |
| cargo | `1.88.0` |
| `rust-toolchain.toml` | channel `1.88` |

---

## 3. What “E2E” meant this run — FACT

Discovered from README / CI / scripts, then executed:

1. `cargo test --no-fail-fast --locked` (full source suite)
2. `python3 scripts/test-desktop-backend.py` with `CGAH_TEST_BINARY=target/debug/cgagentharness`
3. `node --experimental-websocket scripts/chat-browser-acceptance.mjs`
4. `python3 scripts/test-release-plan.py`
5. `cargo build --release --locked` then local `serve` function matrix
6. Skipped: `scripts/smoke-ollama.sh` (no local model), macOS desktop bundle / digest / GUI, real GitHub writes

---

## 4. `cargo test` — 226 / 0 / 0

| Metric | Value |
|---|---|
| Command | `cargo test --no-fail-fast --locked` |
| Exit | **0** |
| Passed / failed / ignored | **226 / 0 / 0** |
| Failed names | none |
| Log | `/workspace/harness-e2e-2026-09-13/logs/cargo-test.log` |
| Wall | ~4 minutes (11:41–11:45 ET) |

Includes lib + bin unit tests, `tests/*.rs` integrations, and doc-tests. `tests/macos_cargo.rs` ran 0 tests on Linux (cfg-gated). `invariant_guard`: 11 passed. `real_repo_loop` (including `real_repo_run_smoke_end_to_end`): 9 passed.

Env for cargo: empty API keys; `CGAGENTHARNESS_AGENTIC_WRITE_DISABLE` unset so the suite can arm writes the way CI does.

---

## 5. Extra scripts

| Script | Result | Kind |
|---|---|---|
| `scripts/test-desktop-backend.py` | **11/11** exit 0 | FACT |
| `scripts/chat-browser-acceptance.mjs` | **passed=true** exit 0 (Chrome) | FACT |
| `scripts/test-release-plan.py` | **17/17** exit 0 | FACT |
| `cargo test --test invariant_guard` | 11/11 inside full suite | FACT |
| `scripts/check-pr-template.sh` | N/A (needs a PR body) | FACT |
| `scripts/test-verify-desktop-digest.py` | **BLOCKED** — no macOS desktop binary | FACT |
| `scripts/smoke-ollama.sh` | **skipped** — no local Ollama | FACT |

---

## 6. Source-built binary matrix — 35 / 0

| Item | Value |
|---|---|
| Build | `cargo build --release --locked` exit 0 |
| Binary | `target/release/cgagentharness` |
| `--version` | `cgagentharness 0.1.0` |
| Bind | `https://127.0.0.1:8790/` (TLS default on a freshly seeded home) |
| Home | `/workspace/harness-e2e-2026-09-13/runtime/home` |
| Seeded defaults | `auth.enabled: true`, `tls.enabled: true`, `security.api_key_optional: true` |

HTTP does not serve the API on a fresh home. Unauthenticated former open reads return **401 AUTH_REQUIRED**. `GET /api/status` stays 200 with a reduced payload until login. Bootstrap: `admin`/`admin`, then password change via HTML meta CSRF + session cookie.

| Bucket | Count |
|---|---|
| PASS | **35** |
| FAIL | **0** |

Highlights: health via `/api/status`; CSRF 403; authed keys/memory/github/jobs 200; chat 502 without Ollama (guards still passed); 25-way concurrency; 80× status all 200; 80× keys hit 429 as expected; 2MB/10MB chat 422; TERM and KILL restart 200; non-loopback bind refused; bad `Host` 400.

Logs: `logs/binary-matrix-summary.json`, `logs/binary-matrix-run.log`.

---

## 7. vs 2026-09-12 — INFERENCE

| Axis | 2026-09-12 | 2026-09-13 |
|---|---|---|
| Tree | tag **v0.1.3** | **v0.1.9** / `dddc8f01…` |
| cargo | 176 pass / **16 fail** (`AGENTIC_WRITE_REFUSED`) | **226 pass / 0 fail** |
| Binary matrix | 36/0 on the **release asset** (HTTP, many open GETs) | 35/0 on a **source-built** binary (HTTPS + session auth) |
| Serve | HTTP :8790 | HTTPS :8790 on a fresh home |

**INFERENCE:** the v0.1.3 write-gate cargo failures are gone on v0.1.9. Product defaults moved to TLS + login. Count 35 vs 36 is the same ID set with expectations updated for those defaults, not a new hole.

---

## 8. Still BLOCKED

- macOS interactive GUI / Finder-launch acceptance
- Desktop bundle / digest scripts (need `cg-agent-harness-desktop`)
- Live Ollama smoke
- Real GitHub agentic writes (intentionally off)

---

## 9. Counts

| Bucket | Pass | Fail | Blocked |
|---|---|---|---|
| `cargo test` | 226 | 0 | 0 |
| Extra Linux scripts | 3 (11+chat+17) | 0 | desktop-digest, Ollama, Mac GUI |
| Source binary matrix | 35 | 0 | 0 |

Linux E2E predicate: **MET**.
