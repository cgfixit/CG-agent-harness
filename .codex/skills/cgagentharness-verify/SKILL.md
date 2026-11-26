---
name: cgagentharness-verify
description: Verify changed Harness CLI, research, browser or desktop behavior with scoped checks and isolated fixtures.
---

# Verify the Harness

Follow `AGENTS.md`. Run the changed entrypoint and inspect outputs and side
effects. Select refusal tests from `cgagentharness-invariant-guard`.

- CLI/server: isolate home and listener. Match process, binary and assets.
- Isolation: verify the actual Seatbelt or bubblewrap backend.
- Research: inspect extraction, citations, completion and refusals.
- Browser: `scripts/chat-browser-acceptance.mjs` uses a mock API.
  `node scripts/browse-console-acceptance.mjs <test-support-binary>` drives
  takeover and rediscovery with real Chrome/server and a scripted model.
  Use `scripts/browser-use-eval.sh` for sidecar fixtures and local-model
  acceptance. Prepare Playwright/Chrome; run unprivileged with sandbox on.
- Desktop: use `cgagentharness-release` for identity and native acceptance.
- Models: record live tag and allocation.
- Docs: run `invariant_guard::markdown_stays_on_the_docs_budget`. Rebuild
  the manual after input edits. If output changes, run
  `self_manual::every_chunk_is_clean_under_the_configured_scanner`.

Record SHA, artifact hashes, commands, exits and gaps. Zero selected tests
means NOT RUN.
