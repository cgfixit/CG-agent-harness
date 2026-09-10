# Parity execution state

Assignment: implement all 48 original non-RAG actions in dependency order,
preserving the merged desktop and the credential-free local default. No phase
or action is complete merely because this inventory exists.

Base: `8cf699a92ecbcce7cbfd8d774eff4aaa5975e3b1` (fetched Rust main).
Reference: `a414ba86ebf5f3c8bb901466c16b4f015bbc79c9` (fresh CyClaw clone).
Both repositories had no open PRs at inspection. Operator checkouts are untouched.

Environment: Apple M5 Pro, 51,539,607,552 bytes RAM, arm64, macOS 26.6.2
(25G83), Command Line Tools. Homebrew Rust is 1.98; checks explicitly use the
installed rustup backend 1.88 and desktop 1.90. Ollama inventories exact
`qwen3.8:27b`, GGUF qwen35 27.3B Q4_K_M, digest
`22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643`.
No model was downloaded, converted or replaced; inventory is not inference.

Clean backend baseline: `PATH=$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin
RUSTUP_AUTO_INSTALL=0 CARGO_NET_OFFLINE=true SKIP_LIVE=1 scripts/verify-local.sh`
exited 0: formatting, Clippy, 167 tests and release. Native execution was outside
the outer tool sandbox to permit Seatbelt and Unix sockets. Local cargo-deny is
absent (the script explicitly skipped it); exact-main CI deny passed in
[CI run 34534491953](https://github.com/cgfixit/CG-agent-harness/actions/runs/34534491953).
Desktop Rust 1.90 formatting, Clippy and both policy tests passed locally.
The unchanged backend's eight public protocol tests passed. Exact-main
[desktop CI](https://github.com/cgfixit/CG-agent-harness/actions/runs/34534492060)
and CodeQL subsequently passed. Native UI acceptance remains independent.

Native gate: the existing DESKTOP_ACCEPTANCE report remains authoritative for
historical observations. Current read-only macOS authorization checks returned
`AXIsProcessTrusted=false` and `CGPreflightScreenCaptureAccess=false`.
No permissions were changed or prompted. Actual console pixels, chat/stop,
review dialogs, chooser, hostile navigation, close/Dock reopen, active-work
Cmd-Q and accessibility remain unverified. Backend or Chromium tests cannot
close them. The operator has been asked about manual fixture acceptance;
independent backend work may continue under the assignment's explicit override.

Dependency order follows the original phases: 1 shared configuration/credentials/
dispatch/readiness/scheduling; 2 M1–M5; 3 A4–A9; 4 C1–C4; 5 C5–C9;
6 L1–L4/L6–L7/O1–O2; 7 O3–O5; 8 I1–I2; 9 I3–I6; 10 I7–I8;
11 O8–O9; 12 combined acceptance/package. Shared A1/C10/O6/O7 remain open
until all consumers land. Use a new focused branch/PR per coherent change;
document actual stacked dependencies and verify the combined tree.

Implemented A3: extended existing Unix managed dotenv loading to headless
`serve`, using the same private descriptor validation and explicit environment
precedence already used by desktop. On baseline, the guarded memory endpoint
returned 401 with the managed file key; the patched process returns 200 and
rejects the overridden key. Changed implementation/verification files:
`src/server/mod.rs`, `src/server/env_keys.rs`,
`scripts/test-desktop-backend.py`, `docs/DESKTOP.md`, and this ledger/view.
Ten public process tests pass. Acceptance covers fresh minimal environment, absent file, private valid file,
explicit environment precedence, unsafe/symlink/oversized/invalid credential
refusal, secret-free output and unchanged desktop boundary tests. Full backend
gates and exact-commit packaging are recorded before publication.
This does not claim Keychain (A2), Windows loading (O9), or service lifecycle (O7).

Next smallest action: finish A3 publication/package verification, then extend
L5's fallback resolver to require an exact installed tag and bounded inventory
response, sharing the current desktop readiness contract. Keep no-probe default
and existing local fallback selection. A2 and the other 46 actions remain open.

All database/service tests must use disposable environments. No external
Telegram/OpenTweet writes, real Dropbox sync, production database use, persistent
schedule activation, main push, merge or release publication is authorized.
