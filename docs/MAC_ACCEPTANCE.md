# Native local-model acceptance record

Date: 2026-09-09. Host: Apple M5 Pro, 48 GiB unified memory, macOS 26.6.2
(build 25G83), arm64. Rust/Cargo 1.98.0 from Homebrew; rustup is not on PATH.
The pinned Rust 1.88 minimum is separately exercised in CI. Native and CI
results are distinct evidence.

## Exact installed model and inference boundary

`ollama list` identified `qwen3.8:27b`, digest
`22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643`.
The separately installed `qwen3.8:27b-mlx` has a different digest; no equivalence
is assumed. No model was pulled, replaced or renamed.

Ollama 0.33.3 reported GGUF, qwen35, 27.3B parameters and Q4_K_M. A separate
Ollama process used the existing model directory read-only, a disposable home,
`OLLAMA_NO_CLOUD=1`, one loaded model, one parallel request and context 8192.
Its native runtime log selected Metal on the Apple M5 Pro. This backend evidence
comes from execution, not a tag suffix. `/api/ps` reported context 8192 and
17,435,386,182 bytes for the loaded model allocation. That is not total process
RSS or a claim that all unified memory is available.

A Seatbelt profile denied all network and then allowed only loopback inbound
and outbound. A non-loopback connection probe under the same profile failed;
loopback inventory and inference succeeded. The operator's original daemon was
left unchanged. Verification used the separate Cargo Seatbelt profile with
network denied entirely. Engineering GitHub/dependency access ran outside these
application execution profiles. No cloud model request or real cloud credential
was used. A localhost URL or Cargo's offline flag alone is not isolation proof.

## Measurements

These are individual controlled observations, not performance benchmarks. Other
engineering builds ran during the session; load/cache state changes latency.

| Request | Wall time | Reported tokens / result |
|---|---:|---|
| Cold small completion, reasoning `none` | 18.499 s | 53 prompt / 32 completion; `stop` |
| Warm repeat | 1.274 s | 53 / 32; 49 cached prompt; `stop` |
| One-token output budget | 0.472 s | 21 / 1; `length`, incomplete output |
| Reasoning `low`, 96-token budget | 1.815 s | 49 / 41; reasoning present, `stop` |
| Reasoning `none`, 96-token budget | 0.299 s | 21 / 6; no reasoning, `stop` |
| Large-file CLI edit through real Cargo | 23.404 s | One proposal, pending human decision |
| Two-file deliberate failure then correction | 22.518 s | Two proposals; Cargo exit 101 then 0 |
| Harness chat first / repeated turn | 6.480 / 0.313 s | 2027 prompt / 2 completion each |
| In-flight chat cancellation | 1.008 s | HTTP 502, typed cancelled result |

The `low` and `none` responses verify those parameters for this installed model
and Ollama version. Structured-output mode, all effort levels, and maximum
context are not established. The observed model metadata maximum is not the
configured runtime context. See [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility),
[context configuration](https://docs.ollama.com/context-length), and
[offline/local model configuration](https://docs.ollama.com/faq).

A loaded-state memory snapshot showed 51% free by `memory_pressure -Q` and about
1,681 MiB swap used, unchanged from the initial snapshot. These are system-wide
snapshots, not attributable model peaks. Conservative fixture settings were
context 8192, planner timeout 180 s, planner output 1024 tokens, chat output
256 tokens, one request at a time. They are acceptance settings, not a global
recommendation or an automatic change to the operator's configuration.

## Workflow evidence

A disposable repository contained a >12 KB Rust file with a controlled
subtraction defect and an unchanged addition test. Explicit lines near 600
supplied bounded context. Real Qwen produced an exact edit; actual sandboxed
Cargo passed. The complete one-expression diff was inspected before local
approval/commit. A separately authorized push reached a disposable bare remote.
Draft publication used a local GitHub adapter and is **not real GitHub evidence**.

Actual Chrome repeated the same pipeline through the server, with authentication,
CSRF, check selection, asynchronous acknowledgement, refresh recovery, complete
diff review, local approval, separate local push and cancellation commands.
`scripts/browser-fixture.py` and `scripts/browser-acceptance.mjs` reproduce it;
see `CONSOLE_JOBS.md`. The standard quality script's skipped live smoke does not
replace these separately recorded real executions.

A second fresh fixture explicitly requested two wrong expressions on the first
attempt, then correction from unchanged Cargo assertions. Real Qwen changed
both files, actual Cargo failed (101), and the second real proposal passed (0).
Only the intended two files changed. This controlled exercise establishes that
failure feedback reaches the model; it is not a success-rate benchmark.

The release packaging script verified archive and binary SHA-256 checksums,
re-extraction and the ad-hoc macOS signature. The extracted arm64 binary served
its embedded console and spawned itself through the shim successfully. A real
one-token model response returned the new typed truncation error. No Developer
ID, notarization or release publication is claimed. The Cargo preparation helper
is still a checkout script and is absent from the binary-only archive.

Native verification-time cancellation across observed separate process groups now
passes in PR #21, including preservation of an unrelated sibling process. Owned
acceptance servers and the separate Ollama daemon were stopped; no owned PIDs
remained, and the normal Ollama listener remained available. Escaped/reparented
work, abrupt server death, startup reconciliation, resource peak/cleanup stress
and broader configuration/setup parity remain incomplete.
Automated correction regressions and the separate real Qwen exercise are
distinct evidence. No broad usability,
security, parity or release-readiness claim follows from these samples.


## Legitimate real-GitHub acceptance

The installed local model generated a README-only setup correction from an
explicit line window against pinned main `7a29186f0726c2e630fd69c94cf0e7576f1218c6`.
One iteration took 28.630 seconds. The harness ran actual offline sandboxed
`cargo fmt --check`; the complete candidate then passed the repository's native
full quality gates (122 baseline tests, fmt, clippy, release build).

After the complete one-paragraph diff and clean staged index were reviewed,
local approval created a commit with pushed=false. A separate explicit push
created `codex/installed-model-selection`; a separate publication used the actual
completed PR template through `--body-file`. Readback is recorded in draft
[PR #22](https://github.com/cgfixit/CG-agent-harness/pull/22).
This standalone README correction has no runtime dependency on the hardening
stack, although the acceptance used that stack's binary. No main push, merge,
release, force-push or cloud inference occurred.
