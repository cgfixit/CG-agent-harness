# Streaming chat and cancellation

The console requests `POST /api/chat` with `Accept: text/event-stream`. Existing
clients that omit this header retain the JSON response and HTTP error contract.
Both modes use the same authentication, CSRF, generation gate, bounded web-tool
runner, memory selection and completed-exchange persistence.

Each SSE `data` field contains one JSON event:

- `{"type":"delta","text":"..."}`: provisional model text.
- `{"type":"done","data":{...}}`: the existing complete chat response, including
  session identity, usage, tally, web events and memory metadata.
- `{"type":"error","status":502,"error":{"detail":{...}},"headers":[]}`:
  a typed failure after SSE response headers have been sent. The event carries
  the application status and any retry headers; the HTTP stream itself is 200.

The console displays text as it arrives and replaces it with the complete answer
on `done`. Failed/cancelled partial text is removed and is not stored as a successful
exchange. Model streams require a completion reason and `[DONE]`; truncated or
malformed responses fail. Usage is collected from the final usage event. A model
that ignores streaming and returns valid JSON remains compatible, with one final
answer instead of incremental text.

`/loop stop` (`POST /api/chat/cancel`) cancels an ordinary chat or loop turn. Closing the streaming response
also aborts its task, releases generation/loop claims, and drops the model HTTP
request and any web read. Each direct ChatClient call has its own cancellation
record; completing one cannot clear another's handle. The model's actual GPU
scheduler may react later to the disconnected socket; immediate GPU release is
not claimed.

Streaming tool calls are accumulated and validated before dispatch. Tools remain
bounded read-only web search/fetch. Current owner and URL permissions are checked
before each text delta derived from web evidence and again before final delivery.
Revocation cannot retract text already displayed or sent to the model.

Model traffic remains bounded by the existing 4 MiB response ceiling and request
deadline. A bounded channel applies downstream backpressure, and the deadline
also covers time waiting for the reader. The single-generation policy remains.

Verification: `cargo test --test chat_stream`, the SSE decoder unit test, and
`node scripts/chat-browser-acceptance.mjs` cover completion, fragmented UTF-8,
concurrent-client cancellation, disconnect, malformed output, tool refusal and
provisional browser rendering. Synthetic fixtures establish protocol behavior,
not the quality or cancellation scheduling of a real model.
