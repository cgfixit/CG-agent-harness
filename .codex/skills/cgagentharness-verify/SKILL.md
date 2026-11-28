---
name: cgagentharness-verify
description: Verify changed Harness CLI, research, browser or desktop behavior with scoped checks and isolated fixtures.
---

# Verify the Harness

Follow `AGENTS.md`. Run the changed entrypoint and inspect outputs and side
effects. Select refusal tests from `cgagentharness-invariant-guard`.

- CLI/server: isolate home and listener. Match process, binary and assets.
- Isolation: verify the actual Seatbelt or bubblewrap backend.
- Research: verify URL provenance, extraction, citations and refusals.
- Browser: `node scripts/test-browse-watch.mjs` exercises retry and job
  recovery in the console script. `scripts/chat-browser-acceptance.mjs`
  mocks the API. `node scripts/browse-console-acceptance.mjs <test-support-binary>`
  tests takeover and rediscovery with Chrome/server and a scripted model.
  For sidecar or local-model runs,
  use `scripts/browser-use-eval.sh`. Keep Chrome sandboxed and unprivileged.
- Desktop: use `cgagentharness-release` for identity and native acceptance.
- Models: record the live tag and allocation. Exercise `/api/show`
  responses when verifying Ollama locality.
- Docs: run `invariant_guard::markdown_stays_on_the_docs_budget`. Rebuild
  the manual after input edits. If output changes, run
  `self_manual::every_chunk_is_clean_under_the_configured_scanner`.

Record SHA, artifacts, commands, exits and gaps. Zero tests is NOT RUN.
