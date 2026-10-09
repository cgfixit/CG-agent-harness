# Operator manual

Start with [setup](../README.md), [console commands](CONSOLE.md),
[account roles](SECURE_RESEARCH.md#roles), and the
[coding pipeline](CODING_PIPELINE.md) for staging, monitoring and reviewed publication.

## Review state and logout

Logout, session switches, `/clear`, and clearing history discard staged coding
requests, diff reviews, reviewed PR bodies and persona proposals. Review them
again before confirming, approving, publishing or applying. Late responses cannot
restore reviews. Clearing the view stops monitoring but preserves submitted jobs
and server records. Use `/agent jobs` or `/agent runs` to resume review.

## Keyboard, zoom and screen readers

Text follows browser zoom. In the sidebar tab list, arrow keys, Home and End
move; Enter or Space opens a tab. The conversation is a polite live log, marked
busy while a reply streams. The paperclip beside Send attaches files.

## Spend and completion notifications

Open **Analytics** → **Tokens and cost** for retained provider/model/day usage.
Refresh rereads the ledger; Previous/Next show 25 groups per page. Unknown usage
and local inference remain unpriced. Check completeness and stale-rate warnings.
Close, Escape and logout clear the view without erasing the ledger.

Choose **Estimate draft** for an unsent cloud message. Claude sends the text
for token counting; Grok estimates bytes/4. The preview reserves full output,
warns about CJK and unknown/stale rates, and shows the optional per-call cap.
Count-only requests do not change the ledger. See [estimation and caps](SPEND_AND_NOTIFICATIONS.md#estimate-a-cloud-draft-and-configure-a-per-call-cap).

Completion webhooks are default-off and restart-only. **Job webhooks** appears
while enabled. A persistent private outbox sends identifiers, status and timestamps
with finite retries and no job content. Owned delivery status and confirmed replay
recheck current grants. Deduplicate by stable delivery ID. See
[setup, crash recovery and replay limits](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).

## Reload web and API limits

After editing supported limits in the active home's YAML, administrators can
choose **Reload limits**. Invalid or restart-only changes preserve all running
limits and explain the refusal. Unix SIGHUP invokes the same backend operation.
See [the allowlist and recovery](CONFIG_RELOAD.md).

## Memory

Pinned notes and structured memory: [memory guide](MEMORY_GUIDE.md).

## Session, token and code analytics

After sign-in, choose **Analytics** or `/analytics` for ledger usage, session
tokens/messages, creation-date bars and coding outcomes. Administrators and
operators can read it; auditors cannot. Sessions are owner-filtered; spend and
coding records are shared. Refresh rereads data. Close, Escape and account
transitions clear the dialog. Disabled coding and incomplete ledgers stay explicit.
Only **Estimate draft** starts a token-counting request; opening analytics starts
no inference or repository action.

Arrow keys switch tabs, each with its own filter and 25-row page. Sessions sort
by tokens, creation date or title. Charts cover all retained records, including
filtered rows. See [API fields, retention and interpretation](ANALYTICS.md).
