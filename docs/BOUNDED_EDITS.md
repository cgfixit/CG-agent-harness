# Bounded existing-file edits

Declare each file needed by the task with `--read-file src/lib.rs`, or select
an inclusive line window with `--read-file 'src/lib.rs#L590-L630'`. The same
selector can be staged with `/agent read src/lib.rs#L590-L630`. A `#Lstart-Lend`
suffix is reserved for this interface. Select one window per file. Missing,
unsafe, oversized and omitted selections are reported explicitly.

The existing ceilings remain 4,000 characters per file and 12,000 total, with a
256,000-byte internal file-read ceiling. The exact displayed span is retained
separately from headings. These are character budgets, not a claim of exact
Qwen token counts. Context includes the full-file SHA-256 and a completeness
flag. A later attempt takes a fresh snapshot; having written a file earlier
never authorizes blind replacement. Predeclare new paths when corrections may
need to read them in later attempts.

For existing large files the local planner can emit:

```text
=== EDITS ===
{"edits":[{"path":"src/lib.rs","sha256":"<provided hash>","old":"<unique displayed text>","new":"<replacement>"}]}
=== END EDITS ===
```

Use JSON escapes and one edit per file. `old` must be nonempty, unique in the
whole original (including overlapping matches), and contained in the actual
displayed excerpt. Hash mismatch, hidden/ambiguous text, duplicate destinations,
invalid JSON, mixed block formats and incomplete trailing markers reject the
whole proposal. `FILE` blocks remain available for new files or fully displayed
current originals. They cannot overwrite a partially viewed file.

All final replacement content is scanned and checked against aggregate write
budgets before application. Response bytes are bounded by the existing handoff
ceiling; large original files still count toward the final write budget. Raw,
canonical and landed destinations pass protected-path policy, including Unicode
and case equivalents. Proposal writes refuse symlink ancestors/leaves. Protected
tests and build configuration remain protected; inline existing tests may be
read, and operator-authored regression PRs are the reviewed mechanism for new
protected tests. No tests-directory exemption was added.

Every file's existence/content precondition is checked before staging. Retained
parent directory capabilities avoid following a newly substituted symlink while
installing a leaf. Replacements are staged, current originals rechecked, and
renamed; ordinary later application errors roll back earlier replacements. A
failed rollback is fatal, quarantines the run and preserves its recovery backup.
Checks see the resulting batch only after successful application. Failed Cargo
checks include bounded stdout and stderr in the next attempt's feedback.

This is not crash-atomic multi-file commit or atomic compare-and-swap against an
adversarial concurrent writer: comparison and rename are separate syscalls, and
an externally relocated directory remains reachable through its open handle.
Do not concurrently edit an active disposable clone. Empty newly-created parent
directories may remain after staging failure. Complete tree/search tooling and
exact tokenizer budgeting remain open work.

Evidence: `tests/exact_edits.rs` drives a >12 KB Rust source edit near line 600,
real offline Seatbelt Cargo failure with the actual assertion in feedback, then
a successful exact correction preserving every unrelated byte. Other regressions
cover multi-file edits, stale state, protected aliases, budget and parser refusal.
Workspace unit tests exercise rollback after an actual first rename. Scripted
planner responses prove execution semantics; real-Qwen acceptance is separate.
