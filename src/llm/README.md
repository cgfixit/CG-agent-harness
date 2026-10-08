# src/llm

Model backends behind one `backend.rs` trait: `ollama.rs` (loopback inventory,
pull, warmup; never sends `num_ctx`), `openai_chat.rs` and `openai_stream.rs`
(OpenAI-compatible local servers), `cloud_chat.rs` (Claude, Grok), `profile.rs`
(model limits), `spend.rs` (append-only usage ledger). Docs:
[MODELS.md](../../docs/MODELS.md),
[SPEND_AND_NOTIFICATIONS.md](../../docs/SPEND_AND_NOTIFICATIONS.md).
