# Web commands, multi-source research, and help acceptance

Local verification on 2026-09-21, before committing or publishing the PR.
This is an arm64 development candidate, not a released or notarized application.

## Source and immutable artifact

- Base: `f85e352a77d3af707d0443e2851ff4543b8b19af` from `origin/main`; latest resync added only an unrelated documentation image and preserved candidate edits.
- Source manifest SHA-256, excluding `screenshots/**`: `7fb3a009c5d4f2326d7d9f0fb5e2e2720c6a83be841bce99933cdcc40568b791`.
- Manifest recipe: list tracked plus non-ignored untracked files, exclude `screenshots/**`, sort paths with `LC_ALL=C`, SHA-256 each file with its relative filename, then SHA-256 that output. This includes the two new Rust files and all product/test/documentation changes; screenshot evidence can be added without altering the tested source manifest.
- Console asset SHA-256: `d8ce7a467b2c3e09b446da6b57f33c5e2f51f3413535094520ef17419493d0cd`.
- ZIP SHA-256: `9c951ea2e49919fecad27f0184363d0b6a4ad9886b454cdf2ce56698f53d3a2e`.
- Packaged backend SHA-256: `460516b13447aabd3a05d892eaffc863e88e35c89a5b69f20d99e835d481a07f`.
- Packaged shell SHA-256: `922dda3c2c29784002a2b4a261b7323c6ce34b9c0de4ea31395978392ce7b373`.
- `Resources/COMMIT` records the base SHA and explicitly says `DEVELOPMENT BUILD: uncommitted changes`, because the operator required native acceptance before commit.
- ZIP checksum verified; extracted into a fresh path containing spaces; the extracted app passed architecture, system-linkage, resource, nested-signature, embedded sidecar-digest, and native-worker checks.
- macOS 27.0 / build 26A428 / Apple Silicon arm64. Ad-hoc signed, not Developer ID signed or notarized. Intel execution and other OS versions are not claimed.

## Owned native environment

Computer Use drove the extracted app, not a browser rendering or an installed copy.
The app ran with an explicitly assigned disposable home, default account/TLS
requirements, and a synthetic account initialized through the supported CLI.
No operator API keys, saved conversations, or cloud inference were used as test inputs.
A stale Computer Use helper briefly launched an older bundle without the fixture
environment; its command was refused with `AUTH_REQUIRED` before dispatch. That
process and its child were stopped, the helper was rebound to the exact candidate,
and all acceptance actions below used the disposable home. No GUI authentication
or authority mutation was performed in the accidentally opened instance.

Final test shell PID `99625` owned sidecar PID `99669`; the sidecar listened only
on `127.0.0.1:52146`. The actual installed local model was `qwen3.8:27b-mlx`.
These are observations of this run, not reusable process handles.
The final app was logged out and quit through Computer Use; both owned processes
were then confirmed absent. The headless fixture used only for initial account
setup had already been stopped before native launch.

## Native observations

| Check | Observed result |
|---|---|
| Login and restart | Final package opened its own HTTPS-backed WKWebView and accepted the disposable account; existing synthetic policy survived restart. |
| Help overview | `/help` displayed topics and a short starting list, not the full alphabetical wall. |
| Help filtering and focus | Searching `--group` returned six matching commands. Clicking the research row inserted `/web research `, focused the composer, and did not execute. |
| Original malformed fetch | `/web fetch https://www.veeam.com and its embedded internal links` was refused with exact-URL/research guidance, not forwarded as a bad URL. |
| Fuzzy permission intent | `/web alow https://www.example.com/*` only suggested an editable command. Clicking it did not grant the URL; a subsequent permission check still denied it. |
| Explicit batch grant | One exact `/web allow` command granted/reaffirmed two public path-wildcard rules in group `demo`. |
| Exact-host diagnostic | `https://example.com/` was permitted; `https://www.example.com/` was denied. No host rewriting occurred. |
| Multi-URL fetch | A single command read both `example.com` and `example.org`: two pages read, zero refused/failed/skipped/unvisited. |
| Natural multi-source chat | The original two-Rust-URL reproducer now fetched both real pages and answered with `rustc --version` and `rustc main.rs`. Both source/tool results were visible. Actual aggregate model usage: 10,934 prompt and 361 completion tokens. |
| Dedicated multi-site research | Repeated `--url` plus group `demo` read both example domains and returned a checked quote. Coverage truthfully reported two refused external links and two failed robots requests; no exhaustive-coverage claim. |
| Policy guidance | On the earlier streaming-fixed candidate, in a fresh session, the real local model correctly explained that URL rules and web enablement are shared-home settings, while saved selections are account-private. The final candidate retains those prompt bytes. |
| Terminal parity | Packaged `web --help` and `web research --help` expose the flags/examples. Authenticated terminal `web check` against this app returned the same permitted apex/denied www results. |
| Ten-call ceiling | Final packaged API reports `chat_tool_calls: 10`. Boundary tests accept the shipped/fallback ten, reject eleven/invalid types, execute exactly ten reads before N+1 refusal, and retain a smaller configured three-call streamed-batch cap. Token/time/byte/URL controls are unchanged. |
| Veeam homepage research | The exact homepage was explicitly permitted in group `veeam`. Final `/web research --group veeam --url https://www.veeam.com/ What does Veeam offer according to its homepage? Summarize the main products and benefits.` returned six supported claims with checked exact quotes and no warnings: one page read, 24 unpermitted links refused, one skipped, zero failed. |
| Veeam fetch → inject → summary | Fetched 6,136 source characters, explicitly injected the saved extract, then requested four bullets from only that context with no additional reads. Final real local-model reply summarized the homepage, labelled it not an independent evaluation, and used 4,692 prompt / 175 completion tokens. The native confirmation now explains one-extract scope without inventing a zero character count. |
| Broad cloud search | Earlier ten-call candidate attempted the requested two searches plus two fetches; the first public Google search returned `WEB_GOOGLE_CHALLENGE`. No result was fabricated. The operator explicitly requested skipping that part and continuing; live two-search/two-fetch success and live SerpAPI are **skipped**, not passed. |

The first native candidate exposed a real missing streaming boundary: a real
two-URL prompt produced multiple model tool calls, but the SSE decoder rejected
index 1. A byte-fragmented regression failed with the same error before the fix.
The final decoder reassembles indexed calls within a fixed allocation ceiling;
the dispatcher still enforces the configured total budget before reads. A real
streamed-HTTP regression now completes both tool reads, commits only the final
answer, and rejects an over-budget batch without prefix reads.

The first broad Veeam research answer failed `WEB_CITATION_INVALID`. A local
diagnostic using only those public passages reproduced both whitespace rewriting
and an ellipsis joining noncontiguous fragments. Synthesis guidance now explicitly
requires short contiguous character-for-character quotes; the identical native
question passed afterward. The validator itself was not relaxed: regression tests
still reject invented ellipses, doubled spaces, newlines and wrong citation IDs.
One successful sample is not a general local-model accuracy guarantee.

On the earlier candidate, dedicated research read both example domains and
returned checked quotes while reporting denied external links and failed robots
requests. A Rust-documentation sample produced `WEB_CITATION_INVALID` and safely
withheld the unsupported answer. Separately, the local model declined to fetch
the example domains based on an incorrect prior belief. Model prose is not
permission or reachability evidence; deterministic `/web check`, `fetch`, and
research coverage are the observable boundary. These model-quality limitations
must not be described as general answer-accuracy acceptance.

## Local automated verification

- Root `cargo fmt --all -- --check` and `git diff --check`: passed.
- Root `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- Root `cargo test --locked --all-targets --quiet`: passed after the final streaming, ten-call, injection-feedback and quote-guidance changes, including 351 library tests, real macOS cargo/process checks, and all integration targets.
- New regression coverage: atomic grants and cap refusal; malformed suffix without prefix reads; exact host/group/SSRF permission checks; CSRF; control-character and unknown-flag refusal; inert help/fuzzy changes; exact-phrase quote preservation; multi-call JSON and SSE; malformed/duplicate/excess calls; fresh research despite cached hits; explicit starts narrowing evidence and refusing unauthorized targets before inference.
- `scripts/chat-browser-acceptance.mjs`: passed the full real-Chrome UI suite. Its mocked API fixtures are separate from the real HTTP grammar and native app evidence.
- `test-review-reset.mjs`, `test-delivery-review.mjs`, `test-schedule-review.mjs`, `test-analytics.mjs`, and `test-auth-deadlines.mjs`: passed.
- Desktop formatting, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked`: passed, including all four desktop policy tests against the new backend digest.
- `scripts/test-desktop-backend.py` against the final packaged backend: all 11 tests passed. Local model fixtures, not live inference.
- Backend and desktop dependency policies: advisories, bans, licenses, and sources passed with verified official `cargo-deny 0.20.2`. Existing duplicate/unmatched-license warnings remain. The installed 0.18.4 cannot parse current advisory CVSS4 data; no dependency policy was weakened.
- Packaging used environment-only `CARGO_PROFILE_RELEASE_STRIP=none`: this macOS build rejects stripped build-time dylibs with a misaligned LINKEDIT error. Repository toolchains, release profiles, and signing checks were not changed.

No release, merge, notarization, live SerpAPI result, Windows/Linux runtime,
native Intel run, or general local-model answer-quality claim is implied.

## Screenshots

Unretouched Computer Use screenshots of the final extracted app, using only
synthetic account/session data and public test/documentation URLs.

1. [Help overview](01-help-overview.jpg)
2. [Flag search and inert insertion](02-help-filter-insertion.jpg)
3. [Malformed/fuzzy intent refusal](03-safe-command-refusal.jpg)
4. [Live Veeam research with validated quotations](04-native-veeam-research.jpg)
5. [Explicit injection and homepage summary](05-native-injected-summary.jpg)

| File | SHA-256 |
|---|---|
| `01-help-overview.jpg` | `004693140b3f2da793adeea4192a27e16e517814c92a564cf2c80d989293e43e` |
| `02-help-filter-insertion.jpg` | `e0b1eecfed715389d4dea9e07310d1adb930526af21e191fc994131166be19d3` |
| `03-safe-command-refusal.jpg` | `8f780e3105a25ad8aa0d55f8cb4429b0e5d3973df6e56f7b721f134fe79caa6e` |
| `04-native-veeam-research.jpg` | `743c7940b0d5c67ed70c0c9cae34480bed05c19d9dbcf12561040d2a1ed5e98d` |
| `05-native-injected-summary.jpg` | `9cd3a7d459940d754ac98ca254e440138ef27d5811a7efc186023b88829958bb` |

Remote screenshot-blob verification and hosted CI are separate publication checks,
recorded in the PR/task handoff; the local results above do not imply hosted results.
