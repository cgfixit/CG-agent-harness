# Desktop acceptance

## Secure portal development acceptance (2026-09-12)

The `codex/secure-web-research-auth-tls` implementation based on `44a205e`
has separate backend, real-browser and native evidence. These observations do not
replace the historical artifact-specific results below or certify distribution.

- Backend: 222 all-target/all-feature tests passed with no skips, including real
  HTTPS, account/role/migration/revocation, content policy and native sandbox tests.
- Desktop: four unit tests passed for ownership/navigation, exact leaf validation,
  and Security.framework name/expiry/substitution refusal.
- Real Chrome with mock APIs: existing chat suite plus minimal anonymous status,
  forced password replacement, API Keys masked save/clear and logout passed.
- Native WKWebView, isolated home: actual HTTPS page load without a trust
  interstitial, admin/admin restricted login, password replacement, normal portal
  access, masked credential save, restart with saved/active `startup_file` status,
  key clearing with restart-needed status, and logout clearing private panels
  passed. Certificate bytes and the replaced password persisted across restart.
  HTTP/2 authority uses the same strict
  origin boundary as HTTP/1 Host. No certificate was installed in system trust.
- Local-model release smoke: HTTPS account login/password replacement, negative
  auth/CSRF/Host controls, real child dispatch and exact `pong` passed with the
  installed `qwen3.8:27b-mlx` tag (850 prompt + 1 completion tokens).

Public provider credentials, live cloud inference, external-browser trust
installation, Developer ID/notarization, native Intel execution, and macOS 12
interaction remain unverified. Native file chooser, external-link confirmation,
active-job quit and accessibility/scale acceptance are not established by the
login/key observations. The evidence artifact for this work records exact final
bundle digests and restart checks. These initial observations used development
bundles before final documentation/dependency synchronization; the PR and handoff
record final committed source and subsequent verification separately.


## Issue #32 candidate — September 2026

PRs #35–#38 implement status/verification, prompt/persona controls, runtime skill
selection and explicit goal-to-coding staging. The documentation PR follows them.
The following older records are historical; they do not establish acceptance of
the new controls. Exact candidate SHA, universal ZIP checksum, CI results and
local native evidence are attached to the stack's handoff and issue #32 comment.

The deterministic `scripts/chat-browser-acceptance.mjs` test now covers the
actual console in Chrome with its CSP and local mock HTTP APIs: persona editor,
preview/save confirmation, goal/session commands, continuation/cancellation and
budgets, skill selection/check staging, and goal staging/refresh recovery.
Rust tests independently exercise the real guarded APIs and mock model requests.
These tests do not count as native WKWebView interaction.

For native acceptance of this candidate, use an isolated home and exercise:
`/soul status`, `/soul edit` and preview/confirmed save; `/prompt`; `/skill use`
and clear; `/goal` and chat continuation/stop; `/goal stage`, request review and
`/goal task` recovery. Execute coding only in a deliberately prepared disposable
fixture with explicit reason/confirmation. Preserve all unrelated app instances
and operator homes. Keep any unavailable native interaction evidence explicit.


## Universal release verification — 2026-09-10

Tested implementation: [432c11d](https://github.com/cgfixit/CG-agent-harness/commit/432c11d163f0b5416de0bcbf0c121a3371f28413),
based on main [010d361](https://github.com/cgfixit/CG-agent-harness/commit/010d361db6960e57e77b4e0d3ac7746ac0e0e31f).
The extracted universal ZIP launched through Launch Services from a path with spaces
on the Apple M5 Pro / macOS 26.6.2 Mac below, with disposable homes and shipped
write gates closed. No operator home or cloud provider was used.

| Check | Result |
|---|---|
| arm64 app and owned backend | OS sampling identified ARM64 for both; owned loopback listener and local `qwen3.8:27b` chat returned `UNIVERSAL_READY` |
| Intel app and owned backend | OS sampling identified X86-64 (translated) for both under existing Rosetta; same local chat succeeded |
| Owner termination | SIGTERM of each test app was followed by app and owned-backend exit; native Cmd-Q interaction was not tested |
| Package | Both architecture slices, system-only linkage, resources, strict nested ad-hoc signatures, ZIP extraction and worker dispatch passed |
| Automated checks | Clean-main 167 backend tests; root fmt/Clippy; two desktop policy tests and desktop fmt/Clippy; 10 packaged-backend tests on each architecture; backend and both-architecture desktop dependency policies; actionlint and zizmor passed |
| Project guidance | Three `.codex/skills` entrypoints passed the Codex skill validator |

This establishes local launch and backend operation for both slices. It does not
establish native Intel hardware compatibility, older macOS acceptance, WKWebView
content/focus/dialog interaction, Developer ID signing or notarization. Those
remain separate from the historical native interaction gate below. The initial
smoke script missed a live listener because lsof resolved its address to a hostname;
rerunning with numeric addresses fixed the test without an application change.

## Earlier arm64 acceptance record — 2026-09-10

**The native interaction gate is unmet.** The app launches on the actual Mac
and its owned packaged backend completes local inference and sandboxed coding.
Accessibility access is unavailable; System Events timed out and own-window
capture failed. Operator confirmation of actual console content and interaction
is pending. No Chromium test, HTTP fixture or bundle build is counted as a
WKWebView interaction pass.

## Baseline and environment

Fetched main: `2b62afcd7c030f719c391ec019eb3b3311fea680`; unchanged at the
pre-publication fetch; no overlapping open PRs. Original checkout preserved.
MacBook Pro Mac17,9, Apple M5 Pro (18 cores), 48 GB, arm64, macOS 26.6.2 (25G83).
Xcode Command Line Tools available. Backend Rust 1.88; desktop Rust 1.90.
No valid signing identity: arm64 ad-hoc signing, no notarization.

Baseline: native backend fmt/Clippy, 159 tests, release and dependency checks pass.
Homebrew Rust 1.98 reports an existing newer Clippy lint at `src/agentic/git.rs:180`;
no lint suppression or baseline source change was made. Outer tool sandbox
prevents nested Seatbelt and Unix socket binding; native tests ran outside it.

Current implementation gates: 163 backend tests including real Seatbelt/process
cleanup, fmt, Clippy and release pass on 1.88. Two desktop tests pass on 1.90;
six public sidecar tests pass. Backend and desktop dependency policy passes;
no advisory exceptions were added. Desktop dependency policy covers the shipped
Apple Silicon target. Exact commit/build checksums accompany the delivered
artifact; development observations below used explicitly marked dirty builds.

## Evidence matrix

| Scenario | Evidence and result | Native interaction status |
|---|---|---|
| Finder-style launch | `open -n` with minimal PATH and explicit disposable home starts native app and its child; actual owned listener verified | Window metadata observed; console pixels/operator confirmation pending |
| Moved bundle / spaces | Bundle and fixture moved to `/private/tmp/CG Desktop Acceptance/Applications/CG Agent Harness.app`; listener and local workflows work | Content/interactions pending |
| Local Qwen chat | Packaged backend returned exact requested text, HTTP 200, 11.38 s, model `qwen3.8:27b` | Backend pass; webview send/cancel pending |
| Coding | Packaged worker used actual local model, one iteration, about 8.02 s; native offline Cargo check passed; only `a - b` → `a + b` changed | Backend pass; webview input/diff display pending |
| Decisions | Complete expected fixture diff inspected by automation; explicit local approval, separate local bare push, separate mock PR transport passed | Test-only automation; not operator review |
| Negative policy | Missing reason/confirmation refused in fixture; existing revocation, accepted-tree, jail and gate regressions pass | Backend pass; native dialogs pending |
| Worker dispatch | Absolute bundled backend runs `agentic`, no desktop library in worker; bundle worker-mode exit check and actual coding passed | No extra app process needed for coding |
| Ownership | Challenge/PID/protocol checks, proof-not-API-key, wrong-origin/CSRF rejection, unrelated listener never adopted, same-home conflict, EOF release pass | Public process tests pass |
| App crash | Exact task app PIDs killed; owned backends exit on pipe EOF; unrelated listeners on 11434, 8790 and 8787 survive | Native process pass, idle case |
| Interrupted recovery | Durable job unit tests; public worker lease test keeps live state then reconciles to interrupted after owner exit; racing completion preserved | Webview recovery/list/discard pending |
| Native cancellation | Existing required macOS test cancels real sandbox check plus wrapper, preserves unrelated sibling | Cmd-Q choice during actual app work pending |
| Webview boundaries | Native navigation policy unit test and backend guards pass; local Setup capability scope reviewed; no native capability for external console | Hostile navigation/new-window/redirect/clipboard/file-chooser interaction pending |
| Startup/setup | Missing key initialization, literal dotenv, unsafe credential/protocol refusal, fixed helper, no developer-tool runtime linkage checked | Native setup buttons and degraded-state presentation pending |
| Window semantics | Close-hide, Dock focus, second-instance notification and active-work quit choice implemented | Operator close/reopen/Cmd-Q/keyboard/focus/scale pending |
| Persistence | Backend storage remains outside bundle; job restart and run-state tests pass | Session/settings/notes app relaunch test recorded separately with final artifact; UI pending |
| Signing/package | App/ZIP/DMG built; arm64, resources, system linkage, strict nested ad-hoc signatures and worker dispatch pass | Gatekeeper/distribution on another Mac pending |
| Runtime egress | Inference endpoint and publication fixture are local; no cloud key or real GitHub publication used by fixture | No complete process-tree packet capture; independent Ollama egress not proven absent |

## Live model and resource observation

Ollama endpoint reports version 0.33.3. Exact model `qwen3.8:27b`:
GGUF, qwen35 family, 27.3B parameters, Q4_K_M, digest
`22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643`.
Ollama reports 18,357,514,402 resident/VRAM bytes and context length 32768.
The installed `qwen3.8:27b-mlx` tag was inventoried but not used in this fixture;
its suffix is not used as proof of backend implementation. No model was pulled,
replaced or converted.

During the representative coding interval, app RSS was 106,864 KiB and backend
RSS 10,800 KiB. Swap used was 1,657.19 MiB in both pre-run and observed snapshots.
These are samples, not peaks or a responsiveness benchmark. The immediate job
acknowledgement and native app process survival do not prove smooth scrolling
or interactive responsiveness. Chat was 11.38 s; the next coding run was about
8.02 s, with the model already resident.

A development instance under Documents waited in an `open` syscall before
spawning its backend. Moving the same complete app and a fresh fixture outside
Documents succeeded. File-access mediation or replaced-bundle state is a
reasonable hypothesis, not a proven cause. No global privacy/security setting
was changed. The shipped setup text directs the operator to check a macOS
file-access prompt and quit/move/reopen rather than reset their home.

## Reproduction and pending operator work

Use the exact delivered bundle and its `Contents/Resources/COMMIT`, not a newer
checkout. [DESKTOP.md](DESKTOP.md) lists automated build and verification commands.
Create a fresh fixture with `scripts/browser-fixture.py NEW_DIRECTORY --prepare-only
--model EXACT_INSTALLED_TAG`, then launch the packaged app with that disposable
home. This helper prepares a local bare remote and mock gh transport; it does
not authorize real GitHub fixture publication. Do not paste its API key into
logs, screenshots, process arguments or reports.

With the operator, verify: actual console display with HTTPS, account login, forced password replacement and an empty optional key; chat and stop;
sessions/goals/model selection/notes/persona; keys/auth/skills/tools; staged agent
run, complete diff and separately authorized fixture decisions; plan/PR file
inputs; external-link confirmation, invalid schemes and hostile rendered text;
close and Dock reopen, double launch, Cmd-Q keep/cancel while a check runs;
keyboard shortcuts, scrolling, selection, Markdown/code, scaling and accessibility.
Mark each outcome only after interaction. Existing unsupported streaming and
unprovable escaped-descendant cleanup remain limitations, not passing features.

For the historical desktop delivery below, one draft PR against main carried
the change; that statement does not apply to the later issue #32 stack. Exact pushed-head CI and final artifact checksums are
reported in the handoff. Do not mark the PR ready or merge while native acceptance
is pending.
