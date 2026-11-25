# Spend, analytics and completion notifications

## Read spend without mistaking missing data for zero

`/tokens` tracks committed exchanges in the selected session. The home-wide
`logs/spend.jsonl` and `.1` retain provider/model usage and billing outcomes
without message text. Failed or cancelled billed calls can appear only in
the ledger. `coverage: "retained_generations"` and `complete` describe
retained-file parsing. Check `files` and `skipped_rows` for gaps. A complete
summary can still omit old rotations or best-effort writes.

Local usage is unpriced. Missing counts, rates or unknown models are unknown
USD. Provider-reported Grok cost wins when present. The bundled rate table
marks rates older than 30 days stale. Reconcile subtotals with provider bills.

## Analytics: sessions, tokens and coding runs

`/analytics` reads owner-filtered session aggregates and home-wide spend and
coding summaries for administrators and operators. Auditors are excluded.
Session days use creation time. Coding totals cover listed records, with
truncation shown. Analytics reflects separate retained sources.
`GET /api/analytics/summary` excludes message bodies and file names/diffs.
A coding-source failure can leave sessions and spend available.

## Estimate a cloud draft and configure a per-call cap

Choose the cloud model, then **Analytics → Tokens and cost → Estimate draft**.
Claude can send the draft and generation model to token counting. Grok uses
UTF-8 bytes divided by four, which can undercount CJK. Claude falls back to
that estimate on counting failure. No history, memory or attachments leave
with the draft count. Use `POST /api/spend/predict` for the guarded API.

`models.cloud_chat.max_usd_per_call` is null or positive finite USD.
The estimate reserves output at the applicable configured cap. Unknown
rates cannot enforce it. Estimate-based refusal for a heuristic requires
literal `budget_on_heuristic: true`. Generation rechecks before sending and
returns 422 `CLOUD_CHAT_BUDGET` on refusal. Counting and refusal add no
ledger rows. The API accepts a nonblank `message` and optional `model`, uses the
generation gate and can return `CHAT_BUSY`. Claude's counting deadline is
`count_timeout_sec`. Predictions are distinct from retained billing records.

## Configure a completion webhook

Notifications ship off. Merge into private `config.yaml` and restart:

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
```

Use the current account's `user_id`, or `local` with auth off. Legacy global
`webhook_url` cannot supply an owner. Put destination secrets in private
`data/notifications/bearers.json` with
`{"version":1,"tokens":{"build-alerts":"replace-with-secret"}}`.
`CGAGENTHARNESS_WEBHOOK_TOKEN` is a single-owner legacy fallback.

Public receivers need HTTPS. Supported private targets require their exact
URL in `private_url_allowlist` and can use HTTP. The allowlist has room
for eight exact URLs. Link-local and mapped IPv6 receivers refuse.
Subscription, attempt, rate and retention bounds are in
[shipped config](../assets/config.default.yaml). Each send pins validated DNS
and disallows redirects or proxies. The worker checks owner, startup revision,
subscription, URL and credential before and after DNS. Disk edits revoke
authority until restart. Disabled or deleted owners stay revoked until
deliberately regranted. Already-sent requests may finish.

## Durable payload and recovery

Terminal detached coding jobs emit `finished`, `failed` or `cancelled`.
The private durable outbox persists attempts before networking and resumes
eligible rows after restart. Reconciliation covers a crash after job
persistence and before enqueue. Initial enablement can therefore deliver
recent retained terminal jobs under current grants. Version 2 batches carry `batch_id` and events
with `event_id`, `delivery_id`, `job_id`, status and Unix timestamps.
No prompts, repository names, results, owner IDs or credentials are sent.

Deduplicate on `delivery_id`. A timeout or lost acknowledgement can cause
a retry, and batch membership may differ. 2xx completes; 429, 5xx and
network failures may retry within attempt and retention limits. Capacity
evicts terminal rows first. If all are pending, new delivery is refused.
Outbox failures leave job results intact.

## Inspect and replay without rerunning a job

**Job webhooks** or `GET /api/notifications` shows owned delivery metadata.
An administrator can replay an eligible own terminal delivery through
`POST /api/notifications/{delivery_id}/replay` with confirmation and reason.
Revoked, expired or evicted deliveries are ineligible. Replay rechecks
current grants, preserves delivery ID and leaves jobs alone.
Look for `notification_delivery`, `notification_replay`,
`notification_dropped` or `notification_store_failed` in audit.
