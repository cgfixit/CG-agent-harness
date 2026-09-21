# Durable delivery Computer Use evidence

2026-09-21; Safari private local fixture at `http://127.0.0.1:57689` and
owned synthetic receiver at `http://127.0.0.1:57690/hook`.
Backend SHA-256: `f41fef23a2ebbd7a7ba49d5b49f34a792e300688c0f0b97268722d63b63dcace`.
No real receiver, provider request, coding execution or public exposure was used.
The terminal job was a labelled synthetic persisted fixture reconciled on startup.

- `delivery-failed-safari.png`: owner-scoped retained failure after the receiver's
  first HTTP 503; reason and explicit replay checkbox are unchecked.
- `delivery-replayed-safari.png`: confirmed replay succeeds after receiver HTTP
  200; the delivery has one replay and the underlying single job remains finished.
- `delivery-revoked-safari.png`: disabling the destination authority on disk,
  without restart, refuses another replay with `DELIVERY_REVOKED`.

Computer Use also verified that an unchecked/unreasoned Replay click stays local.
Receiver capture verified exactly two HTTP requests, identical event/delivery IDs,
exactly six event metadata fields, and no synthetic private job-result text.
The persisted job inventory still contains one unchanged finished job.

Separate guarded HTTP, storage and transport tests prove restart recovery,
job/outbox enqueue reconciliation, timeout-after-acceptance dedup IDs, current
account/destination revocation, owner filtering, confirmed replay, queue pressure,
private-file bounds, malformed/linked-file refusal, stable batches and real pinned
transport to an otherwise unresolvable fixture hostname. The final combined acceptance linked below additionally covers the gateway
and merged MCP lifecycle work together.

Final integrated acceptance is now recorded in [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md).
