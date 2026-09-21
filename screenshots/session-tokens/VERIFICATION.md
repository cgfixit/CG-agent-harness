# Per-session token display and independent spend verification

Verified locally on 2026-09-21 before commit/push/draft publication.

## Reproducer and scope

The existing backend persisted `Session.tally` per session, but the header used
`/api/status.total_tokens`, an all-session aggregate. The new regression failed
on unchanged main with `999 !== 0` for a new session. It passes after the header
reads the selected session's `tokens` and completed chat's `tally` instead.

No backend accounting, ledger format, pricing, retention, authentication, URL
permission, or write-gate implementation changed. Analytics and `/status` retain
their aggregate semantics. Session totals count successfully committed exchanges;
the spend ledger can additionally record billed failures and independent model
operations. These are deliberately different accounting surfaces.

## Exact candidate

- Base: `f85e352a77d3af707d0443e2851ff4543b8b19af` (`origin/main`). Independent of web PR #214.
- Source manifest SHA-256 excluding `screenshots/**`: `712e994024fa4d73f739966b40acdf3e6cd69208b719aff2b34f936d331855dc`.
- Manifest recipe: `git ls-files --cached --others --exclude-standard -- . ':(exclude)screenshots/**'`, sort paths with `LC_ALL=C`, hash each file with its relative filename, then SHA-256 that output.
- Console asset SHA-256: `35042ecf02b4d43b2392d5f25158421df5d93a47f31215dcd45da2ac7884f235`.
- ZIP SHA-256: `29fb7070ff0ff616ae97e362eb96659f5d60908276fa8ba4e041cdc23ba6baa9`.
- Backend SHA-256: `1f16e0be71ac736abf68a674a0cb2079aaaf7325f44165398e12c9a317af7eb2`.
- Shell SHA-256: `b0f3c169e9047cf0fae6649aa0a7296d59cce53d00dd31b88394cf901343c63f`.
- ARM64, macOS 27.0 / 26A428, ad-hoc signed and not notarized.
- `Resources/COMMIT` records the base and `DEVELOPMENT BUILD: uncommitted changes`, because native acceptance was required before commit.
- Packaging used the existing environment-only `CARGO_PROFILE_RELEASE_STRIP=none` workaround for this host's stripped build-time dylib loader issue. No repository toolchain/profile change.

The ZIP checksum passed. It was extracted into a fresh directory containing spaces
and that exact app passed architecture, system linkage, nested signatures,
embedded sidecar digest, resource and worker-dispatch verification.

## Native Computer Use

The extracted app ran with a disposable home and synthetic account initialized
through the supported authenticated API. Default auth and TLS stayed enabled.
The actual installed local model was `qwen3.8:27b-mlx`; no operator sessions or
provider credentials were used, and no cloud inference was requested.

Initial shell/sidecar PIDs: `16423` / `16464`, owned loopback port `54365`.
Restart shell/sidecar PIDs: `18981` / `18991`, owned loopback port `54636`.
These are run observations, not reusable process handles.

| Native action | Observed result |
|---|---|
| Create Token A | Header starts at 0. |
| Real chat in A | 3,577 input + 1 output; header and saved session show 3,578. |
| Create Token B | Header resets to 0 while sidebar preserves A at 3,578. |
| Real chat in B | 3,580 input + 2 output; B shows 3,582, A stays 3,578. |
| Reopen A | Header restores 3,578, not combined total 7,160. |
| Second real chat in A | Adds 3,597 input + 1 output; A reaches 7,176, B stays 3,582. |
| Quit and restart exact app | Both owned initial processes stop; reopening A through Sessions restores 7,176, `/tokens` reports 7,174 input / 2 output / 2 exchanges. |
| Reopen B after restart | Header restores 3,582. |
| Open Spend after restart | Three local calls, 10,754 input and 4 output tokens; correctly labelled Unpriced (local). |
| New session button after restart | New session starts at 0; A and B retain their counts. |
| Polling and logout | Fresh session stays at 0 through a status poll; logout clears the selected session. |

After all three chats, the ledger SHA-256 was
`21668b0f2d3f3e6419acf1749feba365a30ca896503f46382944e61cc9f05960`.
It remained identical through quit/restart, session reopening, `/tokens` and
Spend reads. Session totals and retained ledger sum both equalled 10,758 for
this all-successful three-chat fixture. That equality is not asserted for
failed or independent inference operations.

Final logout and quit were performed through Computer Use; both owned restart
processes were confirmed absent. The ledger hash was still unchanged afterward.

Computer Use coordinate/scroll actions intermittently returned `noWindowsAvailable`;
accessibility-element clicks and keyboard input worked and completed the checks.
No browser scripting or backend writes substituted for these native interactions.

## Automated checks

- Root formatting, Clippy all-targets/all-features with warnings denied, and locked all-target tests passed (346 library tests plus integration targets, including real macOS sandbox checks).
- `test-session-tokens.mjs`: zero/no selection, restored count, missing/unavailable tally, delayed switch/completion/logout/newer-refresh replies passed. Wired into CI.
- Full real-Chrome `chat-browser-acceptance.mjs`: passed, including new-session zero, chat accumulation, switching back, and polling with a deliberately nonzero all-session aggregate. Mock API evidence, not native/backend evidence.
- Review-reset, analytics, delivery, schedule and authentication deadline suites passed.
- Real HTTP spend regression: A 14, B 25, A 23 after another turn; fresh session 0; aggregate 48; three unchanged ledger rows. Empty-text billed failure adds a fourth append-only ledger row, while A stays 23.
- Packaged backend suite: 11 tests passed. Extended restart test proves distinct sessions persist, startup does not replay inference, A accumulates to 24 while B stays 12, fresh session starts at 0 and ledger totals remain 30 input / 6 output over three calls.
- Desktop formatting, Clippy and all four policy tests passed against the packaged backend digest.
- Backend and desktop `cargo-deny 0.20.2`: advisories, bans, licenses and sources passed; pre-existing duplicate/unmatched-license warnings remain.
- `git diff --check`: passed.

Hosted CI and remote screenshot verification are separate publication checks.
This is not a release, notarization, Intel/other-OS native acceptance or live
cloud-billing reconciliation claim. No existing usage is recomputed or migrated.

## Native screenshots

Unretouched captures, containing only synthetic account/chat data.

1. [Session A usage](01-session-a-usage.jpg)
2. [New session B starts at zero](02-new-session-zero.jpg)
3. [Independent session B usage](03-session-b-independent.jpg)
4. [A restored after restart, with breakdown](04-session-a-restored.jpg)
5. [Independent Spend ledger](05-independent-spend-ledger.jpg)

| Screenshot | SHA-256 |
|---|---|
| 01 | `2753e79f17bf008ec449e26717153f8d8260cdb1b0257a7e84515dfe6eb22a0c` |
| 02 | `cf3af3fd6e70fc74d4ceb36e90d7b79d1e4b4bae35e63244ceec6e5ce549ccca` |
| 03 | `d39f9a87ff1479b7905cb5fd7fc3f0f2a4753c6f9616217848aec75e5ac6b70c` |
| 04 | `2cb184f9cf8fe54ed30fb6da9aee9d4533ecb2e67fc9d72b2746503febdbeee0` |
| 05 | `26f2c6027c260023b4be44309c351ad48ea89dbb605287d312bc5ecdb5e32725` |
