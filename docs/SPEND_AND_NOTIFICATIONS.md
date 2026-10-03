# Spend and completion notifications

Retained spend, Analytics and optional completion webhooks use the existing
[installation](INSTALL.md), [account](SECURE_RESEARCH.md#accounts-and-roles)
and [coding](CODING_PIPELINE.md) requirements.

## Build requirements

Check the installed bundle's `Contents/Resources/COMMIT` or source checkout;
the Cargo package version does not identify these capabilities. Current main
has retained-summary completeness metadata, the read-only Spend view, bounded
MCP diagnostics, and the versioned durable notification outbox.
`GET /api/notifications` and version-2 envelopes identify this interface. See
[process lifecycle](PROCESS_LIFECYCLE.md#mcp-stdio-diagnostics) for MCP diagnostics.

[Analytics](ANALYTICS.md) combines this ledger summary with session/coding
metrics, preserving completeness and pricing semantics.

## Read spend without mistaking missing data for zero

The header's **session · tokens** is the selected session's persisted prompt
plus completion tally. New sessions start at zero; reopening restores their
tally even after restart. `/tokens` shows its breakdown. Unavailable tallies
display `—`, not zero. Analytics and `/status` retain all-session aggregates.

Session tallies count committed chat exchanges, including web-tool rounds and
successful compaction. They are not billing records. Failed-after-billing calls
and independent research or coding operations can enter the ledger without a
saved exchange. Switching sessions does not change the ledger.

Open **Analytics** → **Tokens and cost**, then **Refresh** to read the same rollup
as `GET /api/spend/summary`. Each page has at most 25 provider, model, and day
groups. Closing the view, pressing Escape, or logging out clears page state without deleting the
ledger. Older responses without completeness fields appear as unknown.

The active home stores `logs/spend.jsonl` and its previous rotation,
`logs/spend.jsonl.1`. Rows contain provider, model, timestamps, reported token
usage, and billing outcome, without prompt or response text. This shared-home
ledger is not a per-account invoice. Files rotate at 8 MiB by default; the
summary reader refuses files above 16 MiB.

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

Select `grok` or `claude` with `/model use`, type without sending, then choose
**Analytics → Tokens and cost → Estimate draft**. It reports model, estimate
source, reserved output, dated rate and cap decision. Local inference stays
unpriced. Closing/logout clears estimates; edits require re-estimation. The
build must expose `POST /api/spend/predict`.

Claude sends the generation model and message body to its fixed
`messages/count_tokens` endpoint. The two-second default deadline covers headers
and body; errors, malformed responses, or oversized responses use the labelled fallback.
Grok uses rounded-up UTF-8 bytes divided by four without a counting call.
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
to an arbitrary account. Only the local operator configures grants; portal accounts can inspect/replay
their own retained deliveries, never choose another owner or grant destinations.

Quoted `"true"` never enables the master gate. For `use_bearer: true`, each
destination has its own bearer. Put them in the private file
`data/notifications/bearers.json` (mode `0600` on Unix):

```json
{"version":1,"tokens":{"build-alerts":"replace-with-a-destination-secret"}}
```

The secret is printable non-space ASCII, at most 4096 bytes. It is held in
process memory for the outbound `Authorization` header and compared by SHA-256
on every attempt. Replacing one destination's secret revokes that destination's
pending rows (`authority_revoked`) and leaves every other destination's secret
and deliveries alone. `CGAGENTHARNESS_WEBHOOK_TOKEN` remains only as a legacy
bootstrap, and only when every enabled bearer destination belongs to one owner.
A second owner's bearer destination is refused at startup unless `bearers.json`
has that destination's own secret. The house token is never copied onto another
owner. No signing-secret or payload-content adapter is enabled implicitly.
Credentials are absent from the outbox, status responses, event payloads and
content-free delivery audit records.

Disabling or deleting an account cancels that owner's schedules and records the
owner in `data/notifications/revoked-owners.json`. That owner's destinations
stop. Other owners stay enabled. Turning the account back on does not restore
those grants; edit the configuration and restart to do that deliberately.

All settings require restart. Before each attempt or replay, the worker rereads
the bounded regular config and rechecks the account, startup destination revision,
subscription, grant, and credential. Disk changes can revoke startup authority,
not add it. Destination changes or owner disablement, deletion, or demotion stop
later attempts; an active request may finish. Malformed config refuses delivery.
This creates only outbound requests, with no listener, commands, tunnel, or
public memory API.

Public receivers require HTTPS. Loopback, RFC1918, and IPv6 unique-local receivers
need an exact URL in `notifications.private_url_allowlist`, which accepts eight.
Granted private receivers may use HTTP, though HTTPS is recommended for bearer
confidentiality. Public HTTP, link-local, and mapped IPv6
are refused. Each attempt validates and pins at most 32 DNS answers. Proxies,
connection reuse, implicit retries, and redirects are disabled. Audit omits
receiver bodies and URLs.

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

Private `data/notifications/outbox.json` uses schema 1 and atomic mode-0600
replacement in a mode-0700 Unix directory. Attempts and rate timing persist
before networking; responses persist afterward. Restart resumes pending rows.
Reconciliation covers a crash between job persistence and enqueue, with stable
IDs suppressing retained duplicates. First enablement can therefore deliver
recent retained jobs for the owner and selected subscriptions.

Delivery is **at least once with finite retries and retention**. A timeout or
crash after acceptance may resend. A crash on the last attempt can leave an
exhausted row with unknown receipt; inspect the receiver before replay. File data
and Unix directory ordering are synced, but power-loss behavior still depends on
storage. Run one backend per home.

HTTP 2xx completes delivery. HTTP 429, 5xx, DNS and transport/timeouts may retry;
other statuses fail. Attempts, backoff, jitter, rate and payload size (64 KiB) are
bounded. Expired rows are pruned. Under capacity pressure the oldest terminal row
can be evicted; if all rows are pending, new deliveries are refused and audited.
Reconciliation can resend an evicted terminal record still present in retained
jobs; the stable delivery ID lets the receiver deduplicate it. Outbox write failures
and overflow never change a job result. Bounded retention cannot guarantee delivery
of every lifetime job.

## Inspect and replay without rerunning a job

Open **Job webhooks** and **Refresh status**; the header button appears only while
notifications are enabled. The panel lists only the acting owner's
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
