# Spend and completion notifications

Operator setup and interpretation for retained spend, the Spend dashboard and
optional job-completion webhooks. Start with [installation](INSTALL.md) and
[accounts](ACCOUNTS.md); coding authority remains governed by the
[coding pipeline](CODING_PIPELINE.md).

## Build requirements

The installed source must contain the corresponding changes:

| Capability | Required change |
|---|---|
| Retained-summary integrity and completeness metadata | [PR #176](https://github.com/cgfixit/CG-agent-harness/pull/176) |
| Read-only Spend console view | [PR #177](https://github.com/cgfixit/CG-agent-harness/pull/177) |
| Bounded MCP stderr diagnostics | [PR #178](https://github.com/cgfixit/CG-agent-harness/pull/178), described in [process lifecycle](PROCESS_LIFECYCLE.md#mcp-stdio-diagnostics) |
| Optional completion webhooks | [PR #179](https://github.com/cgfixit/CG-agent-harness/pull/179) |

A green draft PR is not a release. These docs depend on those changes being
merged before release. Verify the bundle's `Contents/Resources/COMMIT` or your
source checkout; the Cargo package version alone does not identify capabilities.
The spend ledger and persisted schedules predate these four changes.

The `/analytics` dialog reuses this ledger summary alongside session and coding-run
metrics. It keeps the same completeness and pricing semantics; see
[Analytics](ANALYTICS.md).

## Read spend without mistaking missing data for zero

Open **Spend**, then **Refresh** to read `GET /api/spend/summary`. Previous/Next
show at most 100 provider/model/day groups per page. Closing the view, pressing
Escape or logging out clears its page state without deleting the ledger.
An older summary response without completeness fields is shown as unknown.

The ledger lives in the active home at `logs/spend.jsonl` and the previous
rotation `logs/spend.jsonl.1`. It records provider/model, timestamps, token usage
when reported and billing outcome, without prompt or response text. This is a
shared-home operational ledger, not a per-account invoice. Rotation retains a
bounded window: the default rotation threshold is 8 MiB per file, while the
summary reader refuses files larger than 16 MiB.

The summary exposes `coverage: "retained_generations"`, `complete`, `files` and
`skipped_rows`. File statuses mean:

| Status | Interpretation |
|---|---|
| `read` | The file was read; malformed rows are counted separately |
| `missing` | That generation does not exist; missing previous generation is normal |
| `unreadable` | The file could not be read |
| `too_large` | The file exceeds the bounded summary read limit |
| `invalid_utf8` | The file cannot be interpreted as UTF-8 |

`complete: false` means retained evidence was skipped, including malformed rows,
unreadable/oversized/invalid files, or a missing current file with a previous
file present. Two absent files yield a complete empty retained summary.
**Complete means the retained files were summarized, not lifetime billing
coverage or an atomic snapshot across rotation.** Ledger writes are best-effort.

Local inference is unpriced. Missing token counts, unknown models or unavailable
rates are unknown, never zero USD. Grok provider-reported cost ticks take
precedence when present; otherwise supported models use the bundled rate table.
Rates older than 30 days are marked stale. A displayed USD subtotal covers only
priced evidence; consult the provider's own billing records for reconciliation.

## Estimate a cloud draft and configure a per-call cap

Select `grok` or `claude` with `/model use`, type an ordinary message without
sending it, then choose **Spend → Estimate draft**. The response reports the
model, input estimate source, reserved output, dated rate and cap decision.
Local inference remains unpriced. Closing the dialog or logging out clears the
estimate; editing the draft requires a fresh estimate. This feature requires a
build exposing `POST /api/spend/predict`, beyond the original ledger-only view.

Claude sends the draft to its fixed `messages/count_tokens` endpoint using the
same model/message body as generation. The default deadline is 2 seconds across
headers and body; errors, malformed or oversized responses fall back to the
labelled heuristic. Grok uses rounded-up UTF-8 bytes/4 without a counting call.
**Bytes/4 can undercount CJK.** The local Qwen calibration is not a cloud
estimator. Neither history, attachments, memory, skills nor web context is sent.
Provider credentials and enabled provider configuration are still required.

Merge into `models.cloud_chat` in the active home's YAML and restart:

```yaml
models:
  cloud_chat:
    count_timeout_sec: 2
    max_usd_per_call: null
    budget_on_heuristic: false
```

`count_timeout_sec` accepts 0.1–10 seconds. A null or absent cap leaves calls
ungated; a configured cap must be a finite positive USD number. The estimate
uses the existing exact-model rate table and reserves **all** configured
`max_tokens` output tokens without cache credit. Unknown rates stay unpriced
and cannot enforce a cost cap. With known rates, a Claude vendor estimate can
refuse generation above the cap. Applying the cap to Grok or Claude fallback
requires explicit literal `budget_on_heuristic: true`; quoted `"true"` is off.
A stale rate is shown as stale, not automatically refreshed. Estimates are not
a guaranteed bill ceiling or a monthly spending limit.

Every cloud chat rechecks immediately before generation. A refusal returns
`CLOUD_CHAT_BUDGET` (HTTP 422); change the message/cap deliberately before retrying.
Counting and refusal do not append ledger rows. The authenticated, CSRF-guarded
API accepts `{"message":"your draft","model":"grok"}`; omitting `model` uses
the current selection. Messages must be nonblank and at most 32,768 characters.
It shares the generation gate and returns `CHAT_BUSY` during another model
operation. Audit entries contain estimate metadata, never draft text or keys.

Provider contract: [Claude token counting](https://platform.claude.com/docs/en/build-with-claude/token-counting).

## Configure a completion webhook

Notifications are **disabled by default**. In the active home's `config.yaml`,
configure a receiver you control and explicitly enable the literal YAML boolean:

```yaml
notifications:
  enabled: true
  webhook_url: "https://receiver.example.com/harness-completions"
  private_url_allowlist: []
```

Replace the example URL before enabling. Quoted `"true"` does not enable a gate.
For bearer authentication, save `CGAGENTHARNESS_WEBHOOK_TOKEN` through the
administrator **API Keys** control or the active home's private `.env` file.
The optional token must be printable non-space ASCII, at most 4096 bytes.
Explicit process environment values take precedence, including an empty value.
Fully restart the backend or Cmd-Q/relaunch the app after changing any webhook
setting or token. Saving the key does not activate or test a receiver.

Destination policy is separate from web-content permissions. Public receivers
require HTTPS unless their exact URL is explicitly granted. Loopback, RFC1918
and IPv6 unique-local destinations require an exact entry in
`notifications.private_url_allowlist` (at most eight URLs); an exact grant can
also permit HTTP. Use HTTPS when sending a bearer. Link-local and mapped IPv6
addresses remain refused even with a grant. All DNS answers are checked (at most
32) and pinned for each attempt. Proxy inheritance and redirects are disabled;
a redirect is not followed to another destination.

All settings below live under `notifications`; defaults are usually sufficient:

| Setting | Default | Accepted range |
|---|---:|---:|
| `queue_capacity` | 128 | 1–512 events |
| `batch_size` | 16 | 1–32 events |
| `batch_interval_sec` | 30 | 1–3600 seconds |
| `max_attempts` | 3 | 1–3 attempts per batch |
| `retry_delay_sec` | 2 | 1–60 seconds |
| `timeout_sec` | 5 | 1–30 seconds per attempt, including DNS |

## Payload and delivery limits

Only terminal detached coding jobs produce events: `finished`, `failed` and
`cancelled`, whether started manually or by a schedule. Chat replies and recovered
`interrupted` jobs do not. A schedule refused before job creation has no event.
Repeated finish/cancel operations do not enqueue the same transition again.

The JSON envelope contains a version, a batch ID and minimal events, for example:

```json
{
  "version": 1,
  "batch_id": "0123456789abcdef0123456789abcdef",
  "events": [
    {
      "job_id": "example-job-id",
      "status": "finished",
      "created_at": 1790000000.0,
      "finished_at": 1790000010.0
    }
  ]
}
```

Timestamps are Unix seconds. Events contain no prompts, goals, repository names,
results or errors. `X-CGAgentHarness-Batch-ID` carries the same batch ID; the
optional bearer goes in `Authorization`. No receiver response body is logged.

HTTP 2xx completes delivery. HTTP 429, 5xx and transport/DNS/timeout failures may
retry, at most `max_attempts` total. Other responses stop delivery. Retries retain
the batch ID; a lost response can cause duplicates, so receivers should
deduplicate it. The queue and batches are bounded and in memory. Overflow,
exhausted retries or process exit can lose notifications. There is no durable
outbox, restart replay or guarantee that every job produces a delivered event.

Inspect `notification_delivery` audit records for attempt, event count, success,
coarse result code and retry disposition; `notification_dropped` reports queue
unavailability. Destination URLs, bearer values and receiver bodies are not
included. Inspect the retained job with `/agent job <id>` for the authoritative
outcome. Delivery failure never changes that outcome; do not rerun a coding job
just to resend a notification. See [job and schedule recovery](CONSOLE_JOBS.md#schedules-and-completion-notifications)
and [troubleshooting](TROUBLESHOOTING.md).
