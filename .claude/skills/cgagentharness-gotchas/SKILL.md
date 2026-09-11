---
name: cgagentharness-gotchas
description: Session-tested traps for working on CG-agent-harness. Load before install/verify, desktop packaging, Chrome acceptance CI, clippy/toolchain fights, write-gate debugging, or when something hangs or looks green for the wrong reason.
---

# CG-agent-harness gotchas

Session-process traps for this repo. Canonical contracts live in `AGENTS.md` /
`INVARIANTS.md`; this file is the "looks green for the wrong reason" list.
Paths are relative to the CG-agent-harness root. Skills are repository guidance,
not runtime `/api/skills` plugins.

## 1. Server imports `crate::agentic`

- **Symptom:** `tests/invariant_guard.rs` fails; or a "cleanup" PR "shares types"
  across the console and the pipeline.
- **Wrong fix:** Make the agentic module a library dependency of the server, or
  call pipeline functions in-process "just for this route."
- **Right fix:** Keep I6. Put server-side behavior in `src/server`; cross only
  through `src/shim` spawning `current_exe() agentic <action>` with the ACTIONS
  whitelist. Exit codes `0/2/3/4` remain the interface.
- **Evidence:** `INVARIANTS.md` I6; `AGENTS.md` traps; `tests/invariant_guard.rs`.

## 2. Deduplicating intentional boundary copies

- **Symptom:** Drift PR removes duplicated `RUN_ID_PATTERN`, timeout constants, or
  the check-profile table "to DRY."
- **Wrong fix:** Share the type/module across `server` and `agentic`.
- **Right fix:** Leave the duplication; keep the sync tests green
  (`invariant_guard::duplicated_constants_still_agree` and peers).
- **Evidence:** `AGENTS.md` traps.

## 3. Quoted YAML `"true"` arms a gate

- **Symptom:** Operator sets `enabled: "true"` (quoted) and expects writes; or a
  test assumes string `"true"` is on.
- **Wrong fix:** Teach operators that quotes are fine; loosen `flag_is_true`.
- **Right fix:** Unquoted YAML booleans only. Quoted `"true"` is **OFF** for every
  gate. Keep that parser contract.
- **Evidence:** `AGENTS.md`; `README.md` security defaults; shipped-config tests.

## 4. CSRF placeholder / header rename "cleanup"

- **Symptom:** Console or middleware renames `__CYCLAW_CSRF_TOKEN__`,
  `__CYCLAW_CSP_NONCE__`, or `X-CyClaw-CSRF` to a "harness" spelling.
- **Wrong fix:** Rename for branding consistency.
- **Right fix:** Keep placeholders and header name verbatim — contractual from
  the port. `invariant_guard::console_asset_is_verbatim_with_both_placeholders`.
- **Evidence:** `AGENTS.md` traps; `INVARIANTS.md` guard chain.

## 5. Tests assert developer `GROK_API_KEY`

- **Symptom:** Local suite fails on a clean CI image, or passes only because a
  real key is in the environment.
- **Wrong fix:** Require the key in test setup; commit a dummy that looks real.
- **Right fix:** Tests must not assert key presence. CI blanks
  `GROK_API_KEY` / `ANTHROPIC_API_KEY` / `DEEPAGENT_API_KEY`. Mirror that in
  `scripts/verify-local.sh`.
- **Evidence:** `AGENTS.md`; `README.md`; `scripts/verify-local.sh`.

## 6. scrypt / unoptimized dev builds make tests crawl

- **Symptom:** Auth/key derivation tests take forever in debug; people propose
  lowering scrypt `n`.
- **Wrong fix:** Reduce `n=2^17` or delete the cost.
- **Right fix:** Keep `n=2^17`. Keep `[profile.dev.package."*"] opt-level=3` —
  load-bearing for test time.
- **Evidence:** `AGENTS.md` traps.

## 7. Clippy is the wrong binary on some Macs

- **Symptom:** `scripts/verify-local.sh` fails clippy with toolchain/proxy skew;
  rustup's `cargo-clippy` older than Homebrew's.
- **Wrong fix:** `#allow` the warnings or drop `-D warnings`.
- **Right fix:** `CLIPPY=/opt/homebrew/bin/cargo-clippy scripts/verify-local.sh`.
- **Evidence:** `AGENTS.md`; `cgagentharness-verify` skill (when landed).

## 8. Chrome chat-browser acceptance flakes

- **Symptom:** CI red on chat-browser acceptance; intermittent "Chrome didn't
  start" / session attach failures.
- **Wrong fix:** Delete the acceptance job or weaken assertions until green.
- **Right fix:** Treat as known flake class (issue **#43** — Start Chrome
  reliably). Retry/quarantine with tracking; fix the starter, don't hollow the
  test.
- **Evidence:** CI gotcha / #43.

## 9. Desktop packaging: lipo / sign / embed-SHA256 order

- **Symptom:** Checksums never match after local resign; Gatekeeper weirdness;
  sidecar digest verification fails.
- **Wrong fix:** Re-sign the sidecar **after** digest embed; mix ad-hoc and
  Developer ID steps without documenting which artifact is which.
- **Right fix:** Order is **lipo → sign → embed-SHA256**. Never re-sign a sidecar
  after digest embed. Ad-hoc (`--sign -`) ≠ Developer ID / notarized. WKWebView
  shell ≠ HTTP console semantics — don't assume fetch/CSRF behavior is identical
  inside the native wrapper.
- **Evidence:** `cgagentharness-release` packaging discipline; `scripts/package-release.sh` (ad-hoc note).

## 10. Seatbelt permission errors ≠ app regression

- **Symptom:** Sandbox verify fails with permission denied; patch softens the
  Seatbelt profile or skips sandbox tests.
- **Wrong fix:** Weaken assertions or broaden the profile to silence noise.
- **Right fix:** Reproduce with owned temp `CGAGENTHARNESS_HOME` + unique port.
  Distinguish environment/Seatbelt noise from product bugs. No backend ⇒ exit 3
  remains correct fail-closed behavior.
- **Evidence:** `INVARIANTS.md` judged-before-land; `agentic_foundations` sandbox_* tests; `cgagentharness-verify`.

## 11. Shared listener ≠ this bundle

- **Symptom:** Smoke "passes" against a leftover `serve` on `:8790` from another
  checkout or build.
- **Wrong fix:** Hit the default port and assume it's your binary.
- **Right fix:** Owned temp `CGAGENTHARNESS_HOME`, unique port, kill/own the
  process you started (`scripts/smoke-ollama.sh` pattern).
- **Evidence:** `scripts/smoke-ollama.sh`; verify skill.

## 12. Draft PR / template / driver prefix skipped

- **Symptom:** CI template check fails; review bots bounce; mergeability unclear.
- **Wrong fix:** Force a ready PR to skip draft discipline; invent a free-form body.
- **Right fix:** Draft PR, one concern, driver-prefixed branch (`claude/`,
  `codex/`, `grok/`, `kimi/`, `agent/`), body from
  `.github/PULL_REQUEST_TEMPLATE.md` after `scripts/check-pr-template.sh`.
  Core-path diffs need an explicit invariant statement. Skill selection still
  does **not** authorize push/merge/release.
- **Evidence:** `AGENTS.md` quality bar.

## 13. Write-gate debugging "just set confirm default true"

- **Symptom:** Publish/write path 4xx; agent proposes defaulting `confirm` or
  making `reason` optional.
- **Wrong fix:** Default `confirm` on; drop reason; OR the kill switch.
- **Right fix:** Keep `confirm` never defaulted, `reason` required, kill switch
  AND-ed only (`CGAGENTHARNESS_AGENTIC_WRITE_DISABLE`). Debug which gate refused
  with the exit-code API (`4` = write refused).
- **Evidence:** `INVARIANTS.md` write gates; `writer_gates_in_order_and_plan_integrity`.

## Related skills

Load `cgagentharness-project-guidance` for read order;
`cgagentharness-invariant-guard` / `cgagentharness-config-guard` before merging
core-path or config diffs; `cgagentharness-write-policy-redteam` for write/git/
argv/jail hardening loops; `verification-specialist` to adversarially verify a
supplied patch; `cgagentharness-parity` for CyClaw↔harness ledger work;
`cgagentharness-verify` / `cgagentharness-release` / `cgagentharness-optimize`
for those jobs; `fable-protocol` for evidence-first discipline.
