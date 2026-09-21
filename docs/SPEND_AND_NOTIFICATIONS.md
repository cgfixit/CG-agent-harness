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

The original spend and best-effort notification changes above are on main.
The current source adds the versioned durable outbox described below. Check the
installed bundle's `Contents/Resources/COMMIT` or source checkout; the Cargo
package version alone does not identify capabilities. `GET /api/notifications`
and version-2 event envelopes identify this delivery interface.

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

Notifications are **disabled by default**. The active home's `config.yaml` needs
an explicit destination owner, subscriptions and enablement. Obtain the account's
stable `user_id` from administrator account management; username is not an owner ID.
Only deliberately auth-disabled homes use `local`.

```yaml
notifications:
  enabled: true
  schema_version: 1
  destinations:
    - id: build-alerts
      owner: user_REPLACE_WITH_CURRENT_ACCOUNT_ID
      enabled: true
      url: "https://receiver.example.com/harness-completions"
      events: [finished, failed, cancelled]
      use_bearer: true
      rate_per_minute: 20
  webhook_url: ""
  private_url_allowlist: []
```

Replace the example values before enabling. There are at most eight destinations;
IDs are unique, 1–64 ASCII letters/digits/underscores/hyphens. Subscriptions are
explicit and limited to terminal job states. Each destination's rate is 1–120
batches/minute. A legacy global `webhook_url` must be migrated deliberately; the
backend refuses an enabled nonempty legacy URL instead of assigning its authority
to an arbitrary account. There is no portal endpoint that lets an account choose
another owner or grant a destination. The local operator configures those grants;
portal users inspect and replay only their own retained deliveries.

Quoted `"true"` never enables the master gate. For `use_bearer: true`, save
`CGAGENTHARNESS_WEBHOOK_TOKEN` through administrator **API Keys** or the private
managed `.env`. The token must be printable non-space ASCII, at most 4096 bytes.
An explicit environment value takes precedence, including an empty value. The
single managed bearer is shared only by destinations that explicitly select it;
use receiver-side authorization appropriate to those grants. No signing-secret
or payload-content adapter is enabled implicitly. Credentials are absent from the
outbox, status responses, event payloads and content-free delivery audit records.

All settings require restart. Before each attempt/replay, the worker rereads the
bounded regular configuration file and rechecks the current account, exact startup
destination revision, subscription/grant and selected credential. Disk changes can
revoke startup authority but cannot add it. Removing/changing a destination or
disabling/deleting/demoting its owner stops subsequent attempts; an already in-flight
request may finish. Malformed config refuses delivery. The ordinary 22-key config
reload allowlist remains unchanged. These are outbound requests only: no listener,
callback commands, tunnel or public memory API is created.

Public receivers require HTTPS. Loopback, RFC1918 and IPv6 unique-local receivers
need an exact URL in `notifications.private_url_allowlist` (maximum eight).
Explicitly granted private receivers may use HTTP; public-address HTTP is refused
even if the URL was listed. Prefer HTTPS for bearer confidentiality. Link-local
and mapped IPv6 remain refused. Each attempt validates every DNS answer (maximum
32) and pins the result; proxies, connection reuse, implicit client retries and
redirects are disabled. Neither receiver bodies nor destination URLs enter audit.

| Setting | Default | Accepted range |
|---|---:|---:|
| `queue_capacity` | 128 | 1–512 retained delivery rows |
| `batch_size` | 16 | 1–32 events per POST |
| `batch_interval_sec` | 30 | 1–3600 initial batching delay |
| `poll_interval_sec` | 1 | 1–60 outbox scan interval |
| `max_attempts` | 3 | 1–3 attempts per delivery/replay |
| `retry_delay_sec` | 2 | 1–60 exponential backoff base |
| `max_backoff_sec` | 300 | 1–3600 seconds before jitter |
| `jitter_percent` | 20 | 0–50% additional random delay |
| `retention_sec` | 604800 | 60–2592000 seconds |
| `max_replays` | 3 | 0–10 explicit replay cycles |
| `timeout_sec` | 5 | 1–30 seconds including DNS |

## Durable payload and recovery

Only `finished`, `failed` and `cancelled` detached jobs produce completion events.
Chat, recovered `interrupted` jobs and schedules refused before job creation do
not. Events remain metadata-only; richer fields are not implemented. A batch is:

```json
{
  "version": 2,
  "batch_id": "<digest of delivery IDs in this batch>",
  "events": [{
    "event_id": "<stable digest of owner-bound terminal metadata>",
    "delivery_id": "<stable event/destination/revision digest>",
    "job_id": "<job ID>",
    "status": "finished",
    "created_at": 1790000000.0,
    "finished_at": 1790000010.0
  }]
}
```

Timestamps are Unix seconds. Prompts, goals, transcripts, memory, source, repository
names, results, raw errors and owner identities are absent. The batch ID is also
sent as `X-CGAgentHarness-Batch-ID`. **Deduplicate by delivery ID**, because retry
batch membership can change. Explicit replay preserves that ID and event ID.

The private `data/notifications/outbox.json` uses schema 1 and atomic replacement
at mode `0600` (directory `0700` on Unix). Attempts and per-destination rate timing
persist before networking; successful/failed responses are persisted afterward.
Restart resumes retained pending deliveries. Retained terminal jobs are reconciled
to cover a crash after job persistence but before enqueue; their stable IDs suppress
retained duplicates. First enabling a destination can therefore deliver recent
retained terminal jobs for its owner and selected subscriptions.

This is **at-least-once delivery with finite retries and retention**, not an
exactly-once guarantee. A timeout or crash after receiver acceptance may resend.
A crash during the last allowed attempt can leave an exhausted failure whose
receipt is unknown; inspect the receiver before explicit replay. File data is
synced before replacement and Unix directory ordering is synced. Power-loss
behavior still depends on the storage/filesystem. Run one backend per home.

HTTP 2xx completes delivery. HTTP 429, 5xx, DNS and transport/timeouts may retry;
other statuses fail. Attempts, backoff, jitter, rate and payload size (64 KiB) are
bounded. Expired rows are pruned. Under capacity pressure the oldest terminal row
can be evicted; if all rows are pending, new deliveries are refused and audited.
Reconciliation can resend an evicted terminal record still present in retained
jobs; the stable delivery ID lets the receiver deduplicate it. Outbox write failures
and overflow never change a job result. Bounded retention cannot guarantee delivery
of every lifetime job.

## Inspect and replay without rerunning a job

Open **Deliveries** and **Refresh status**. The panel lists only the acting owner's
destinations and retained metadata, with state, attempts, replays and coarse result
codes. It exposes no destination URL, token or job result. A failed/delivered row
can be replayed with a reason and explicit checkbox; current authority and finite
replay/retention limits still apply. Pending, revoked, expired and evicted deliveries
cannot be replayed. A revoked destination revision needs a new deliberate grant,
not a replay bypass.

The guarded APIs are `GET /api/notifications` and
`POST /api/notifications/{delivery_id}/replay` with `{"reason":"…","confirm":true}`.
Foreign IDs return `DELIVERY_NOT_FOUND`; a machine memory key grants neither route.
Replay returns `job_restarted: false` and never creates or modifies a coding job.
Audit events `notification_delivery`, `notification_replay`, `notification_dropped`,
`notification_evicted` and `notification_store_failed` record bounded metadata.
The retained job remains authoritative for the coding outcome.
