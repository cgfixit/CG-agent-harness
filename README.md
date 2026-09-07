# CGagentHarness

[![CI](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/cgfixit/CG-agent-harness/actions/workflows/ci.yml)

Loopback-only agentic coding harness. One Rust binary.

This is **not** CyClaw. CyClaw is the offline-first RAG soul agent
([cgfixit/CyClaw](https://github.com/cgfixit/CyClaw)). This repo is the
`harness/` console plus the `agentic/` real-repo pipeline, ported from that
Python stack: same security posture, no RAG, no corpus, no terminal, no
fsconnect / sqlconnect / netconnect.

- Console: `http://127.0.0.1:8790/` (`assets/static/harness.html`, served verbatim)
- Chat: local OpenAI-compatible model (Ollama on `127.0.0.1:11434` by default)
- Pipeline: clone → plan → patch → hard-sandbox verify → human decide → commit → push → draft PR
- Isolation: the HTTP process never calls the pipeline in-process. It only
  spawns `cgagentharness agentic …` as a child (`src/shim`). Exit codes
  `0 / 2 / 3 / 4` are the whole interface.

Status: `0.1.0`. Rust 1.88. MIT. Bind is loopback-only. Every write gate
ships **closed**.

## Project summary

CGagentHarness is a single-binary, loopback-only coding console and
governed GitHub write pipeline. You chat with a local model in the
browser; when you arm it, the same binary can clone a repo, propose a
patch, verify it in a hard sandbox, and open a draft PR — only after a
human reviews a digest-bound diff.

It exists because CyClaw's Python `harness/` + `agentic/` layer is the
part worth extracting: the console, the child-process I6 boundary, the
clone jail, and the write gates. Everything else (RAG, soul, Telegram,
fsconnect) stays in CyClaw. If you want an offline knowledge agent, use
CyClaw. If you want a local coding harness that cannot reach the
pipeline except through `src/shim`, use this.

Shipped defaults do nothing to a repository. `agentic.enabled`,
`deepagent_github.enabled`, and `allow_git_write_tools` are false.
Unset `CGAGENTHARNESS_API_KEY` → guarded routes 401. Non-loopback bind
is refused.

---

## Prerequisites

| Need | Why |
|---|---|
| Rust 1.88+ (`rust-toolchain.toml`) | Build |
| Ollama on `127.0.0.1:11434` with a chat model | Console chat and local planner |
| `gh` ≥ 2.40.0, logged in | Real-repo pipeline only |
| `openssl` (or any CSPRNG) | Generate `CGAGENTHARNESS_API_KEY` |

Default model tag in config is `qwen3.8:27b-mlx`. Change
`models.local_llm.model` if that is not what you run.

---

## Quick start (chat only)

```bash
git clone https://github.com/cgfixit/CG-agent-harness.git
cd CG-agent-harness
cargo build --release

export CGAGENTHARNESS_API_KEY="$(openssl rand -hex 20)"
./target/release/cgagentharness serve
# http://127.0.0.1:8790/
