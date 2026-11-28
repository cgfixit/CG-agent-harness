---
name: fable-protocol
description: Investigate consequential Harness claims using execution-path evidence, explicit uncertainty and scoped verification.
---

# Evidence-first investigation

1. Define the outcome, scope and evidence required. Test the premise.
2. Trace the production entrypoint, state owner and side effects. For
   research, distinguish an operator URL, a listing, a fetched passage and
   a derived claim. Check current authority after waits and before reuse.
   For pinned egress, trace destination authorization in the caller and
   bundled-root TLS in `src/server/web_search.rs`.
3. Reproduce the reachable behavior on the current revision. Use equivalent
   workloads for a performance claim. Distinguish product, provider and
   environment failures.
4. Preserve affected refusals with `cgagentharness-invariant-guard`. Run the
   changed entrypoint with `cgagentharness-verify` and record any unverified step.

Match each claim to its evidence:

| Evidence | Claim it supports |
|---|---|
| Build or health response | Compilation or liveness |
| Synthetic provider | Protocol handling |
| Real Chrome/server, scripted model | Console and server flow |
| Live-model `/browse` | Model decisions in the tested run |
| Packaged WKWebView | Native behavior on the tested host |
| Cross-build or Rosetta | The named build or translated execution |
| PR or queue candidate SHA and event | Check coverage for that revision and event |
| Audit gap, response and stored state | Missing records and whether the operation committed |

Record SHA, artifact, commands and outcomes. Label observations, inferences
and unknowns. Omit credentials and operator data.
When monitoring, track the published head. Report checks, reviews and
mergeability separately.
