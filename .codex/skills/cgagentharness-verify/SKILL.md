---
name: cgagentharness-verify
description: Verify changed Harness CLI, research, browser or desktop behavior with scoped checks and isolated fixtures.
---

# Verify the Harness

Follow `AGENTS.md`. Run the changed entrypoint and inspect outputs and side
effects. Select refusal tests from `cgagentharness-invariant-guard`.

- CLI/server: use an owned temporary home and listener. Match process, binary
  and assets. Exercise benign chat and the changed behavior. Stop owned processes.
- Isolation: use actual Seatbelt or bubblewrap for claims about that backend.
- Research: check PDF extraction or goal/loop citations, completion and refusals.
- Browser: `scripts/chat-browser-acceptance.mjs` drives console Chrome.
  `scripts/browser-use-eval.sh` needs a debug `test-support` build, prepared
  Chrome/Playwright and local Ollama for Qwen. Drive changed `/browse` paths.
- Desktop: use `cgagentharness-release` for identity and native acceptance.
- Models: record live tag and allocation. Label fixture results separately.
- Docs: run `invariant_guard::markdown_stays_on_the_docs_budget`. Regenerate
  changed self-manual inputs and, if output changes, run
  `self_manual::every_chunk_is_clean_under_the_configured_scanner`.

Record SHA, dirty state, architecture, artifact hashes, commands, exits and
gaps. Zero selected tests means NOT RUN. Compare the baseline when attribution
is unclear.
