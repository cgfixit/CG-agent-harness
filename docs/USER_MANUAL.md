# Operator manual

Start with [setup](../README.md) and [console commands](CONSOLE.md).
[Accounts](ACCOUNTS.md) explains who can use each surface;
[coding jobs](CONSOLE_JOBS.md) covers staging, monitoring and reviewed publication.

## Review state and logout

Logout, switching sessions, `/clear`, and clearing session history discard the
console's staged coding request, remembered diff reviews, reviewed PR bodies,
and reviewed persona proposals. Load and review them again before confirming,
approving, publishing, or applying a proposal. Responses still arriving from the
previous view cannot restore those reviews. Clearing this view stops its job
monitoring; submitted jobs and saved run records remain on the server. Use
`/agent jobs` or `/agent runs`, then inspect the selected job/run to resume review.

## Spend and completion notifications

Open **Spend** in a supported build to inspect retained provider/model/day usage.
Refresh rereads the ledger; Previous/Next page through at most 100 groups at a
time. Unknown usage and local inference remain unpriced. Check completeness
and stale-rate warnings before interpreting a total. Close, Escape and logout
clear this view; they do not erase the ledger.

For an unsent cloud draft, choose **Estimate draft**. Claude sends that text
for token counting; Grok uses a labelled bytes/4 estimate. The preview reserves
full output, warns about CJK and unknown/stale rates, and shows the optional
per-call cap decision. Local inference is unpriced; count-only requests do not
change the ledger. See [estimation and caps](SPEND_AND_NOTIFICATIONS.md#estimate-a-cloud-draft-and-configure-a-per-call-cap).

Completion webhooks are separately configured, default-off and restart-only.
They send job ID, status and timestamps, with bounded retries and no job content.
The queue is in memory and is not replayed after restart. Follow
[Spend and completion notifications](SPEND_AND_NOTIFICATIONS.md) for build
requirements, setup, payloads and failure handling.

## Reload web and API limits

After editing supported limits in the active home's YAML, administrators can
choose **Reload limits** without stopping the app. Invalid or restart-only
changes preserve all running limits; the result explains the refusal. Unix
SIGHUP reaches the same backend operation. See [the exact allowlist and recovery](CONFIG_RELOAD.md).

## Memory

Pinned notes and structured memory: [memory guide](MEMORY_GUIDE.md).

## Session, token and code analytics

Choose **Analytics** or use `/analytics` in the console to view retained ledger
usage, session token and message counts, creation-date bars and coding-run outcomes. **Refresh** rereads
the data; Previous/Next shows 25 rows in the selected section. Close, Escape and
account transitions clear the dialog. Disabled coding and incomplete ledgers remain
explicit; no inference or repository action is started. Histories are shared
portal resources available to administrators and operators, not auditors. See
[Analytics](ANALYTICS.md) for API fields, retention limits and interpretation.
Section tabs support arrow keys; each keeps its own filter and 25-row page.
Sessions can sort by tokens, creation date or title. The overview and charts
always describe all retained records, including rows hidden by a filter.
