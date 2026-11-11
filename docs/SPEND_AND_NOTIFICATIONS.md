# Spend, analytics and completion notifications

Retained spend, Analytics and optional completion webhooks build on the
[installation](INSTALL.md), [account](SECURE_RESEARCH.md#accounts-and-roles)
and [coding](CODING_PIPELINE.md) guides.

## Read spend without mistaking missing data for zero

The header's **session · tokens** is the selected session's persisted prompt
plus completion tally, restored after restart; `/tokens` shows its breakdown.
Unavailable tallies display `—`, not zero. Analytics and `/status` hold
all-session aggregates.

Session tallies count committed chat exchanges, including web-tool rounds and
compaction attempts. They are not billing records: calls that fail or are
cancelled after billing, and research or coding operations, can enter the
ledger without a saved exchange.

The active home stores `logs/spend.jsonl` and its previous rotation,
`logs/spend.jsonl.1`. Rows hold provider, model, timestamps, reported token
usage and billing outcome, never prompt or response text. This shared-home
ledger is not a per-account invoice. Files rotate at 8 MiB by default; the
summary reader refuses files above 16 MiB.

The summary exposes `coverage: "retained_generations"`, `complete`, `files` and
`skipped_rows`; each file's status is:

- `read`: summarized; malformed rows are counted separately.
- `missing`: normal for the previous generation.
- `unreadable`, `too_large` or `invalid_utf8`: not summarized.

`complete: false` means retained evidence was skipped: malformed rows,
unreadable/oversized/invalid files, or a missing current file with a previous
file present. Two absent files yield a complete empty summary.
**Complete means the retained files were summarized, not lifetime billing
coverage or an atomic snapshot across rotation.** Ledger writes are best-effort.

Local inference is unpriced. Missing token counts, unknown models or unavailable
rates are unknown, never zero USD. Grok's provider-reported cost takes
precedence; otherwise supported models use the bundled rate table, with rates
older than 30 days marked stale. A USD subtotal covers only priced evidence;
reconcile against the provider's billing.

## Analytics: sessions, tokens and coding runs

Choose **Analytics** or `/analytics`. The overview shows retained session
tokens, readable sessions and coding runs; `+` marks a truncated list.
Administrators and operators can read it, auditors cannot; sessions are
owner-filtered while spend and coding runs are portal-wide. **Close**, Escape
or an account change clears filters, sorting and cache.

- **Tokens and cost:** the ledger rollup (`GET /api/spend/summary`) in pages of
  at most 25 provider/model/day groups, plus **Estimate draft**.
- **Sessions:** titles, message and exchange counts, input/output/total tokens
  and UTC creation dates, sorted by tokens (default), creation date or title.
  Bars show the latest 14 recorded `created_ts` dates, not activity.
- **Code:** run outcomes, iteration totals, per-run reject codes, and
  changed-file occurrences summed across runs (a file in two runs counts
  twice). Missing metrics show **Unknown**; disabled or failed coding reads
  show **Unavailable**.

Arrow keys, Home and End switch sections; each keeps its own filter and pages
of at most 25 rows. Charts cover all retained records. Run listing scans at most 4,097 directory entries
and returns at most 128 records; a truncated listing's totals cover only
listed records. Session and ledger totals have different retention rules and
are never added.

Opening Analytics starts no inference or repository operation; only **Estimate
draft** sends a token-counting request, and run listing can reconcile stale
`running` records to `interrupted`.

`GET /api/analytics/summary` sits behind the same guards as
`GET /api/spend/summary`. It returns `spend`,
`sessions.sessions` (no messages, goals or prompts), `status`, `session_days`,
`sessions_without_created_date` and `code` (`runs`, `truncated`, `outcomes`;
rows add `iterations`, `changed_file_count` and `reject_code`, never file
names, diffs or source; or a typed `error`). A coding-source failure keeps
spend and session data; a snapshot-task failure returns
`ANALYTICS_UNAVAILABLE`. The composition is best-effort, not a transaction
across files. An authentication or permission refusal clears all sections.
CLI-backed requests keep a 130-second client deadline, past the shim's 120.

## Estimate a cloud draft and configure a per-call cap

Select `grok` or `claude` with `/model use`, type without sending, then choose
**Analytics → Tokens and cost → Estimate draft**. It reports model, estimate
source, reserved output, dated rate and cap decision; an edited draft needs a
new estimate.

Claude sends the generation model and message body to its fixed
[`messages/count_tokens`](https://platform.claude.com/docs/en/build-with-claude/token-counting)
endpoint; the two-second default deadline covers headers and body, and errors
or malformed or oversized responses use the labelled fallback. Grok uses
rounded-up UTF-8 bytes divided by four without a counting call, which **can
undercount CJK**. History, attachments, memory, skills and web context are not
sent.

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
uses the exact-model rate table and reserves **all** configured `max_tokens`
output tokens without cache credit. Unknown rates stay unpriced and cannot
enforce a cap. With known rates, a Claude vendor estimate can refuse
generation above the cap; applying the cap to Grok or the Claude fallback
requires literal `budget_on_heuristic: true`. Estimates are not a guaranteed
bill ceiling or a monthly spending limit.

Every cloud chat rechecks immediately before generation. A refusal returns
`CLOUD_CHAT_BUDGET` (HTTP 422); change the message or cap before retrying.
Counting and refusal append no ledger rows. The CSRF-guarded API accepts
`{"message":"your draft","model":"grok"}`; omitting `model` uses the current
selection. Messages must be nonblank and at most 32,768 characters. It shares
the generation gate and returns `CHAT_BUSY` during another model operation.
Audit entries contain estimate metadata, never draft text or keys.

## Configure a completion webhook

Notifications are **disabled by default**. The active home's `config.yaml` needs
an explicit destination owner, subscriptions and enablement. The owner is the
account's `user_id` from `GET /api/auth/users` (administrator) or
`GET /api/auth/whoami`, not a username; only auth-disabled homes use `local`.

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

At most eight destinations; IDs are unique, 1–64 ASCII
letters/digits/underscores/hyphens. Each destination's rate is 1–120
batches/minute. An enabled nonempty
legacy global `webhook_url` is refused rather than assigned to an arbitrary
account. Only the local operator configures grants; portal accounts can
inspect and replay their own retained deliveries. Queue, batch, retry,
retention and timeout bounds are listed with their ranges under
`notifications:` in the shipped YAML.

Quoted `"true"` never enables the master gate. With `use_bearer: true`, each
destination has its own bearer in private
`data/notifications/bearers.json` (mode `0600` on Unix):

```json
{"version":1,"tokens":{"build-alerts":"replace-with-a-destination-secret"}}
```

This plaintext file is independent of `security.allow_plaintext_key_file`
(managed keys in `.env`). It is limited to 64 KiB and must be an owner-only
single regular file: no symlink/reparse point, no macOS extended ACL entries.

The secret is printable non-space ASCII, at most 4096 bytes, held in process
memory for the outbound `Authorization` header and compared by SHA-256 on
every attempt. Replacing one destination's secret revokes that destination's
pending rows (`authority_revoked`) and leaves every other destination alone.
`CGAGENTHARNESS_WEBHOOK_TOKEN` remains only as a legacy bootstrap, and only
when every enabled bearer destination belongs to one owner; a second owner's
bearer destination is refused at startup unless `bearers.json` has its own
secret. There is no signing-secret or payload-content adapter.
Credentials are absent from the outbox, status responses, event payloads and
delivery audit records.

Disabling or deleting an account cancels that owner's schedules and records the
owner in `data/notifications/revoked-owners.json`; that owner's destinations stop
and other owners stay enabled. Re-enabling the account does not restore the
grants; edit the configuration and restart.

All settings require restart. Before each attempt or replay, the worker rereads
the private config and rechecks the account, startup destination revision,
subscription, grant and credential: disk changes can revoke startup authority,
not add it. Destination changes or owner disablement, deletion or demotion
stop later attempts; an active request may finish. Malformed config refuses
delivery. There is no listener, only outbound requests.

Public receivers require HTTPS. Loopback, RFC1918 and IPv6 unique-local receivers
need an exact URL in `notifications.private_url_allowlist` (eight entries) and
may use HTTP (HTTPS still protects the bearer). Public HTTP, link-local and
mapped IPv6 are refused. Each attempt validates and pins at most 32 DNS answers,
with no proxies, connection reuse, implicit retries or redirects. Audit
omits receiver bodies and URLs.

## Durable payload and recovery

Only `finished`, `failed` and `cancelled` detached jobs produce completion events;
chat, recovered `interrupted` jobs and schedules refused before job creation do
not. A metadata-only batch is:

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
batch membership can change; replay preserves that ID and the event ID.

Private `data/notifications/outbox.json` is replaced atomically (mode 0600, in
a mode-0700 Unix directory). Attempts and rate timing persist before
networking; responses persist afterward. Restart resumes pending rows.
Reconciliation covers a crash between job persistence and enqueue, with stable
IDs suppressing retained duplicates, so first enablement can deliver recent
retained jobs for the owner and selected subscriptions.

Delivery is **at least once with finite retries and retention**. A timeout or
crash after acceptance may resend, and a crash on the last attempt can leave an
exhausted row with unknown receipt: inspect the receiver before replay.

HTTP 2xx completes delivery. HTTP 429, 5xx, DNS and transport/timeouts may retry;
other statuses fail. Attempts, backoff, jitter, rate and payload size (64 KiB) are
bounded, and expired rows are pruned. Under capacity pressure the oldest
terminal row can be evicted; if all rows are pending, new deliveries are
refused and audited. Reconciliation can resend an evicted terminal record
still in retained jobs. Outbox write failures and overflow never change a job
result, and bounded retention cannot guarantee delivery of every job.

## Inspect and replay without rerunning a job

Open **Job webhooks** and **Refresh status**; the header button appears only while
notifications are enabled. The panel lists only the acting owner's destinations
and retained metadata (state, attempts, replays, coarse result codes), never a
destination URL, token or job result. A failed or delivered row can be replayed
with a reason and checkbox, within current authority and the replay
and retention limits. Pending, revoked, expired and evicted deliveries cannot
be replayed, and a revoked destination revision needs a new grant.

The guarded APIs are `GET /api/notifications` and
`POST /api/notifications/{delivery_id}/replay` with `{"reason":"…","confirm":true}`.
Foreign IDs return `DELIVERY_NOT_FOUND`; a machine memory key grants neither route.
Replay returns `job_restarted: false`: it never creates or modifies a coding job.
Audit events `notification_delivery`, `notification_replay`, `notification_dropped`,
`notification_evicted` and `notification_store_failed` record bounded metadata.
