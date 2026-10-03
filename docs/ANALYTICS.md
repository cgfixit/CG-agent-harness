# Session, token and coding analytics

After sign-in, choose **Analytics** or `/analytics` in the native or browser
console. The overview shows retained session tokens, readable sessions and coding
runs. A `+` marks a truncated run list. **Refresh** rereads history and timestamps
success. **Close** or Escape clears the dialog and restores focus. Account
transitions invalidate cached and late responses. The command also appears in
Commands and `/help`.

Administrators who replaced the bootstrap password and operators can read the
view; auditors cannot. Session counts and tokens are owner-filtered. Spend and
coding-run records are portal-wide. Explicit legacy auth-disabled configurations
retain local access semantics.

## Read the three sections

- **Tokens and cost:** provider/model/UTC-day ledger rows, calls, input/output
  tokens and available USD. Local inference and unknown prices remain unpriced.
  Partial files, skipped rows and stale rates retain the same warnings as
  [Spend](SPEND_AND_NOTIFICATIONS.md). Estimates are not invoices. This tab also
  holds [**Estimate draft**](SPEND_AND_NOTIFICATIONS.md#estimate-a-cloud-draft-and-configure-a-per-call-cap)
  for an unsent cloud message.
- **Sessions:** readable retained session titles, message counts, input/output
  and total tokens, exchange counts and UTC creation dates. Sort by most tokens
  (the default), newest creation or title. Missing creation dates stay unknown.
  Bars show the latest 14 recorded UTC **creation dates**, using existing
  `created_ts` metadata. They do not measure daily activity, file modification
  times or time spent in the app. Unknown dates are counted separately.
- **Code:** listed run outcomes, iteration totals, changed-file occurrences
  summed across runs, and per-run reject codes. A file appearing in two runs
  counts twice; this is neither unique-file nor lines-added/removed analytics.
  Missing metrics, including unreadable records or an older backend, display
  **Unknown**. Disabled or failed coding reads display **Unavailable**.

Arrow keys, Home and End switch section tabs. Each keeps its own filter and
page. Filter displayed values, including session IDs; **Clear filter** restores
all rows. Tables show at most 25 rows per page and the record range. Charts and
overview cover all retained records. Refresh preserves filters and clamps pages
if records shrink. Closing or changing accounts clears filters, sorting and cache.
Tables scroll within fixed Close/Refresh and paging controls. Empty, no-match
and unavailable states are distinct.

Run listing scans at most 4,097 directory entries to detect overflow and returns at most 128
records. A truncated listing is marked partial, and its totals cover only the
listed records. Session totals and ledger totals have different retention and
accounting rules; they are shown separately and never added together.

Opening analytics starts no inference, repository edit, verification, push or PR.
Only **Estimate draft** sends a token-counting request. Run listing can reconcile
stale `running` records to `interrupted`. Analytics adds no files, polling, cloud
calls, chart dependencies or CSP permissions. User labels render as text.

## API and compatibility

`GET /api/analytics/summary` uses the same account, role, same-origin, rate and
CSRF guards as `GET /api/spend/summary`. Supply the existing `X-CyClaw-CSRF` header
with the authenticated session. No new configuration is required.

The response combines these sources:

| Field | Source and meaning |
|---|---|
| `spend` | Unchanged retained spend summary, including completeness and rates |
| `sessions.sessions` | Existing session summaries; no messages, goals or prompt history |
| `status` | Current model/provider and total tokens across those session summaries |
| `session_days` | Ascending `{day, count}` rows from session creation timestamps |
| `sessions_without_created_date` | Sessions with unusable creation timestamps |
| `code` | Parsed run list through the existing shim, with `runs`, `truncated` and `outcomes` counts; or a typed `error` with `code` and `message` |

Run-list rows include `iterations`, `changed_file_count` and `reject_code`
from existing records. File names, diffs and source contents are not added.
Coding-source failure preserves spend/session data and reports an error, not an empty run list. A snapshot-task failure returns
`ANALYTICS_UNAVAILABLE`. Reads are a best-effort composition, not a transaction
across files; concurrent activity may change the next refresh.

On an older backend returning HTTP 404 for the aggregate, the dialog reads
`/api/spend/summary`, `/api/sessions`, `/api/status` and `/api/agent/runs` in
parallel. Other aggregate failures do not trigger fallback. Individual fallback
source errors remain visible; an authentication/permission refusal clears all
sections. CLI-backed requests retain the existing 130-second client deadline,
which allows the shim's 120-second deadline to report its typed failure.
