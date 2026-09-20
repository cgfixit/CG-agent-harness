# Issue #148 implementation and closure evidence

Audit date: 2026-09-20. Inspected main: `cccb2363cd71c0341960fdd43a290a19c8b58338`.
This is a dated implementation record, not certification of a later release.

**Not safe to close at this snapshot:** the remaining changes are draft PRs,
not merged features. Merge the reviewed changes, require successful checks on
their final heads, and verify their combined result before closing #148.
PDF remains explicitly deferred following the safety spike; DOCX was approved
separately by the operator.

## What was already implemented

| Track | Main implementation | Remaining finding / correction |
|---|---|---|
| Documentation split | #163 | Stale hardcoded latest-release claim remained in setup-guide; replace it with source/artifact provenance guidance. Inbound-reference audit retained historical records. |
| Tool grounding | #166 | Duplicate tool-name inventory and loop prompts advertised tools that were absent from the request. #171 derives descriptions/names from tool schemas and separates web context from callable tools. |
| Fuzzy slash | #164 | Filler removal modified exact command payloads and reasons; generic second-intent splitting could reinterpret saved text. #170 preserves arguments and refuses fuzzy state changes. |
| Natural-language web search | #165 | No additional engine needed. Existing count/quoted-query routing and permission tests pass; no network authority added. |
| Output styles | #167 | Existing presets, session selection, soul/style/policy order and cloud exclusion pass. No replacement implementation needed. |
| Text attachments | #168 | API preview worked, but console preview omitted selected file IDs. #172 uploads once for preview and retains the IDs for the next Send. |
| Optional office attachments | #173 (draft) | Bounded DOCX added after the written dependency/safety spike. PDF deferred. |

All implementation PRs are independent and based on main. None changes workflows,
shipped execution gates, the I6 boundary, confirm/reason requirements, or URL grants.

## Local evidence

Rust 1.88, macOS arm64. Baseline tests passed after using system Python for the
existing native sandbox fixture; the pyenv interpreter's external dylib was an
environment restriction, not a baseline application failure.

| Change | Tested source | Checks |
|---|---|---|
| Slash | `88eb90880df9eff5e2abddb4329330830e34b218` | fmt, all-feature clippy, deny, 558 tests, release |
| Tool inventory | `29d70def9436a7b82c8f20994b095301fbec361a` | fmt, all-feature clippy, deny, 559 tests, release |
| Preview | `298e34669ea7c3b49d679d14662a41766c0c8c62` | full local gate, 557 tests; Node check executes the actual preview helper and covers reuse, failure, busy and session races |
| DOCX | `bd8edc811c19f6799be29577b8624d828f6725a6` | full local gate, 560 tests; fresh offline vendor build with frozen lockfile |

DOCX follow-up `f65947fe23676547eb185209af92a0adeb3cd952` only annotates fixed XML namespace identifiers for
DevSkim; they are identifiers, not requests. Focused parser tests and formatting
were rerun after those comments. No scanner or workflow was weakened.

The combined local-only validation checkout (`022142d`) also passed the full
gate, including 563 tests and a fresh release build. Its temporary integration
branch is not a stacked PR. Computer Use on that combined binary also verified
fuzzy on/off refusal and two DOCX previews followed by Send. Its SHA-256 is
`33e20c519252ffba91ae0086043391f94072395f38a423b1daf1f6a7ece7dc42`.
The standalone DOCX binary used for live acceptance has SHA-256
`28b524c92f530c77c94bc06ecf3abc5c5c77f23fe0da5ae10e636cd08127b60f`.

The full gate is `SKIP_LIVE=1 scripts/verify-local.sh`, with cargo-deny installed
and cloud keys blank. Tests cover owner isolation, cleanup, quotas, input limits,
cloud/loop/coding exclusions, URL policy and write invariants. Platform-specific
suite success does not imply Linux execution on this macOS host; hosted Linux
checks are separate.

Computer Use operated Chrome against isolated loopback homes and synthetic files:

- Exact memory-save text retained filler words and `and search`; search found it.
  Multi-intent consolidation/search suggested separate commands, and fuzzy clear
  was refused. Database inspection corroborated the saved text.
- Controlled-model denial was corrected only when callable web tools were present.
  A loop turn sent no tools and advertised no callable tools. With web off, the
  same denial remained unchanged.
- `/style concise` appeared after the contradictory test soul in `/prompt` and
  changed the next local request; `/style off` reverted the selection.
- Three Markdown files were selected through the native file picker. Repeated
  `/prompt` reused uploads; the next chat request contained all three markers;
  the following turn contained none of their attachment fence text.
- Benign DOCX reached the existing prompt fence. Compressed oversized DOCX showed
  `ATTACHMENT_DOCX: DOCX XML exceeds decoded limit`, stored no new blob and sent
  no model request. Unit/HTTP tests cover entities/DTDs, malformed XML, wrong
  namespaces, polyglots, hidden/deleted runs, ownership and injection scanning.
- Natural-language search reached the existing search path and reported the
  isolated fixture's missing permission policy honestly. Successful live Google
  or SerpAPI results are not claimed; engine routing is covered by fixtures.

A separate live request through the same browser and DOCX release binary used
installed `qwen3.8:27b-mlx`, with concise style selected. It returned exactly
`DOCX_MARKER` (2,188 prompt tokens, 3 completion tokens). No cloud provider was
called. This sampled live response does not certify every model behavior.

This is browser Computer Use, not native packaged WKWebView acceptance or a new
notarized release. Fixture model responses prove request wiring, not model quality.

## Office-format safety decision

The [initial spike](https://github.com/cgfixit/CG-agent-harness/issues/148#issuecomment-5751771221)
used two benign and two hostile PDF/DOCX fixtures, checked licenses and cargo-deny,
and prepared an offline dependency snapshot. `lopdf` did not expose the required
aggregate decoded-output and cooperative parsing controls for this integration;
no PDF parser ships and no subprocess workaround was added.

The [DOCX amendment](https://github.com/cgfixit/CG-agent-harness/issues/148#issuecomment-5751783735)
records the operator's DOCX-only choice. Production uses `zip` 8.6.0 (MIT,
default features disabled, deflate only) and `quick-xml` 0.42.0 (MIT).
The application lockfile SHA-256 is
`530a6ac90fc335589ac494f7a60936b882b71760c185d119a98c60e900ba6148`.
`cargo deny check`, `scripts/prepare-cargo.py`, and a fresh target's
`CARGO_NET_OFFLINE=true cargo --config <snapshot>/config.toml build --frozen`
all succeeded.

Only bounded UTF-8 main-document text is supported. No URLs/relationships,
macros or embedded files are interpreted. No OCR, headers/footers, Word layout
reproduction or style-based visibility resolution. See the DOCX PR's
`docs/CONSOLE.md` for the exact limits; the deadline is cooperative, not OS
preemption. Injection scanning and local-chat-only ownership remain shared.

## Documentation audit and closure checklist

Reviewed 50 inbound-reference hits across README, setup guide, AGENTS,
INVARIANTS, skills, docs and the console. Checked 272 local Markdown links,
including anchors, with no missing targets. Historical acceptance and benchmark
records remain linked and labeled; parity data and protected documents remain.

Before closure: review and merge #170, #171, #172, #173 and this documentation PR;
require green final-head CI; retain the DOCX/PDF scope decision. Do not interpret
a draft PR, local pass, or old release record as proof that main already includes
the changes. CI is verification of record.
