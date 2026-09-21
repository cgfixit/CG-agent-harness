# Issue 191 acceptance record — 2026-09-21

This is a dated verification record, not installation guidance. Operator settings
and limitations are in [CONSOLE.md](CONSOLE.md#local-history-compaction).

## Source and artifact

The implementation follows [issue 191's four-phase plan](https://github.com/cgfixit/CG-agent-harness/issues/191#issuecomment-5755009546).
Local main and the implementation branch were synced to `de5d7c5e357c4122a35d391b4ef41e88adc4e58c`.
The tested bundle was built from `9ee6c98c35b94968118aa89da5b3a9652bde1f64`
plus this PR's uncommitted runtime changes. The intervening main commit only
changed `finetune/README.md`; subsequent acceptance edits only changed docs.
The SHA-256 of `git diff --binary 9ee6c98 -- src assets` for the tested runtime
is `8e98ae1b63d4673ef2cb67ea1096585981e8c5e572a171ecdb3efb789c7afbb7`.

Host: Apple Silicon, macOS 27.0 (26A428). Backend Rust 1.88; desktop Rust 1.90.
The arm64 development app is ad-hoc signed and not notarized.

| Artifact | SHA-256 |
|---|---|
| Packaged backend | `e7991eacfad864c02d9efcfe31649d0b9dcc52c5af8f0218a949f01c4a0f8a35` |
| Native shell | `8c5a57a70f9be2fec8cdae78f65d59fe77a9c616298761d145badf0096dbb147` |
| App ZIP | `5a955f5acb90ecf904601fa5793930974de28b15ac1190e799d8f25d42dacd7a` |

Packaging used `CARGO_PROFILE_RELEASE_STRIP=none CGAH_ALLOW_DIRTY=1
scripts/package-desktop.sh`. The ordinary stripped build hit macOS 27's
`mis-aligned LINKEDIT string pool` loader failure, tracked in
[rust-lang/rust#157750](https://github.com/rust-lang/rust/issues/157750).
The environment-only workaround leaves repository build settings unchanged.
Bundle resource, architecture, system-linkage, signature and worker-dispatch
checks passed.

## Automated checks

| Check | Result |
|---|---|
| Clean base `cargo test --all-targets` | 604 passed, no failures or ignored tests |
| Changed backend `cargo test --all-targets` | 610 passed, no failures or ignored tests |
| Backend and desktop fmt / strict Clippy | Passed |
| Backend and desktop `cargo deny --all-features check` | Passed; existing duplicate-dependency warnings remain |
| Desktop `cargo test --all-targets` | 4 passed |
| `scripts/test-desktop-backend.py` with the packaged backend | 11 passed |
| Doc-sync invariant and registered-route checks | 13 passed |

The two-cycle summary regression failed on the original implementation, then
passed with full prior summaries preserved. Additional checks cover doubled
reply reservations at boot and projection, 2x observed CJK usage with web on
and off, repeated compaction, missing usage, failed-call rollback, restart
persistence, summary-budget clamps, and first-call versus aggregated web usage.
The optional idle worker remains deferred exactly as the issue plan permits.

## Native and browser interaction

Computer Use drove the actual WKWebView app and Chrome using separate disposable
homes. Accounts were provisioned through the CLI. Native HTTPS login used the
owned leaf certificate; the browser fixture used loopback HTTP. No operator home
or installed app was replaced.

- Both interfaces triggered compaction against a synthetic compatible endpoint.
  Each stored history shrank from 18 to 6 messages while keeping its first user
  turn and goal. Captured summary requests retained the whole previous summary
  and used the configured 1024-token ceiling; saved calibration stayed at 2.
- Both file choosers uploaded a synthetic Markdown attachment whose relevant
  passage occurred after 28 KB of unrelated text. Initial and subsequent chats
  included the late passage in captured model requests. Pins survived native
  restart and browser reload.
- Owner-scoped notes were ingested through their API, then retrieved through
  ordinary native and browser chat. Captured requests contained the notes
  marker. The console has no dedicated notes-corpus upload control.
- Chrome refused pinned attachments on cloud and `/loop` surfaces with
  `ATTACHMENT_SURFACE_FORBIDDEN`. The automated suite separately covers the
  agent surface, DOCX parsing, upload bounds and cross-owner restrictions.
- After switching the disposable homes to the installed Ollama model, native
  chat returned `LIVE_OLLAMA_191_OK` (1642 input / 10 output tokens). Chrome
  answered in the restored attachment session (2318 / 23); its saved calibration
  reset to the new endpoint/model with ratio 1.
- Native Computer Use then triggered real Ollama compaction of a separate
  synthetic CJK-heavy history: 26 stored messages became 6, and the audit's
  projected budget fell from 24638 to 7796 against a 19000 threshold. The model
  summarized the previous cobalt-valve decision and answered it correctly.
  The first request and goal remained intact; the successful exchange saved a
  calibration ratio of 1.05054 and 21489 input / 116 output tokens, including
  the summary call. This is one live functional sample, not a quality benchmark.

The exact installed tag was `qwen3.8:27b-mlx`, digest
`5642e97495e1a088883805981563dcdc4a040c2f53388b7a41d1f24d3622cf7e`.
Ollama reported safetensors, nvfp4, and a loaded `context_length` of 32768.
No model was downloaded, renamed or replaced, and no cloud inference was used.
Both owned harness processes and the synthetic model server were stopped, and
the dedicated Chrome tab was closed. The existing Ollama service was preserved.

## Recent-feature checks and limits

The merged #194 dataset builder generated 48 examples (43 train / 5 validation),
and its mock smoke completed an authenticated packaged-backend chat. The native
and browser compatible-endpoint checks above establish chat routing. They do
not establish successful adapter training/fusion or a live fine-tuned planner.
Main's base-model warning is retained and reflected in both fine-tuning guides.

The merged #196 attachment/notes paths passed the interactions and automated
checks above. The separately tracked notes count-limit and atomic-manifest
findings remain deferred in [#197](https://github.com/cgfixit/CG-agent-harness/issues/197)
and [#198](https://github.com/cgfixit/CG-agent-harness/issues/198).

Doc-sync covered README, the compaction invariant, console/install settings and
fine-tuning guidance against the current source. Historical desktop acceptance
records remain historical. This pass does not certify Intel/older-macOS UI,
release signing/notarization, every application workflow, or model answer quality.
