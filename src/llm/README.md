# src/llm

Model backends: local OpenAI-compatible servers and native Ollama, the explicit
cloud client, model facts and the spend ledger. Docs:
[MODELS.md](../../docs/MODELS.md), [SPEND_AND_NOTIFICATIONS.md](../../docs/SPEND_AND_NOTIFICATIONS.md).

- [`backend.rs`](backend.rs): `resolve_local_backend` picks primary Ollama or the loopback fallback; reasoning effort.
- [`cloud_chat.rs`](cloud_chat.rs): `CloudChat`: explicit Grok or Claude selection, never a fallback; predicts spend first.
- [`inventory.rs`](inventory.rs): bounded local model inventory for readiness and failover (`InventoryLimits`).
- [`mod.rs`](mod.rs): module list and a test that backend `Debug`/JSON omit the API key.
- [`ollama.rs`](ollama.rs): native `/api/tags`, abortable `pull_model`, `keep_alive` warmup; payloads carry no `num_ctx`.
- [`openai_chat.rs`](openai_chat.rs): `ChatClient`: OpenAI-compatible chat, bounded JSON and SSE, per-call abort.
- [`openai_stream.rs`](openai_stream.rs): bounded SSE decoder; partial text is provisional; `MAX_TOOL_CALLS` ceiling.
- [`profile.rs`](profile.rs): per-model facts from `/api/show` and `/api/ps`, optional speed measurement.
- [`spend.rs`](spend.rs): append-only `logs/spend.jsonl`; USD derived at read time; never stores prompts.

Tests: [tests/model_completion.rs](../../tests/model_completion.rs),
[tests/model_readiness.rs](../../tests/model_readiness.rs),
[tests/ollama_manage.rs](../../tests/ollama_manage.rs), [tests/spend_ledger.rs](../../tests/spend_ledger.rs).
