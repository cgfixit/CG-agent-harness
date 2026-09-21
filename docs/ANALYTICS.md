# Session, token and coding analytics

Enter `/analytics` in the native or browser console. **Refresh** rereads retained
history; **Close** or Escape clears the dialog. Logout and login transitions
invalidate cached results, including responses that arrive after the transition.
The command is in the alphabetical Commands pane and `/help`.

The view is available to administrators who have replaced the bootstrap password
and to portal operators. Auditors cannot read it. Sessions, spend and coding runs
are shared portal resources, not private per-account histories. Existing explicit
legacy auth-disabled configuration keeps its existing local access semantics.

## Read the three sections

- **Tokens and cost:** provider/model/UTC-day ledger rows, calls, input/output
  tokens and available USD. Local inference and unknown prices remain unpriced.
  Partial files, skipped rows and stale rates retain the same warnings as
  [Spend](SPEND_AND_NOTIFICATIONS.md). Estimates are not invoices.
- **Sessions:** readable retained session titles, message counts, input/output
  and total tokens, and exchange counts, sorted by total tokens descending.
  Bars show the latest 14 recorded UTC **creation dates**, using existing
  `created_ts` metadata. They do not measure daily activity, file modification
  times or time spent in the app. Unknown dates are counted separately.
- **Code:** listed run outcomes, iteration totals, changed-file occurrences
  summed across runs, and per-run reject codes. A file appearing in two runs
  counts twice; this is neither unique-file nor lines-added/removed analytics.
  Missing metrics, including unreadable records or an older backend, display
  **Unknown**. Disabled or failed coding reads display **Unavailable**.

Each table displays at most 100 rows per page. Previous/Next pages all three
tables together; a shorter table can be empty on a later page. Run listing scans
at most 4,097 directory entries to detect overflow and returns at most 128
records. A truncated listing is marked partial, and its totals cover only the
listed records. Session totals and ledger totals have different retention and
accounting rules; they are shown separately and never added together.

Opening the view starts no inference, repository edit, verification, push or PR.
The existing run-list operation can reconcile a stale `running` record to
`interrupted`; analytics retains that recovery behavior. No new analytics files,
background polling, cloud calls, charting dependencies or CSP permissions are
introduced. User-controlled labels render as text.

## API and compatibility

`GET /api/analytics/summary` uses the same account, role, same-origin, rate and
CSRF guards as `GET /api/spend/summary`. Supply the existing `X-CyClaw-CSRF` header
with the authenticated session. No new configuration is required.

The response composes the existing sources:

| Field | Source and meaning |
|---|---|
| `spend` | Unchanged retained spend summary, including completeness and rates |
| `sessions.sessions` | Existing session summaries; no messages, goals or prompt history |
| `status` | Current model/provider and total tokens across those session summaries |
| `session_days` | Ascending `{day, count}` rows from session creation timestamps |
| `sessions_without_created_date` | Sessions with unusable creation timestamps |
| `code` | Parsed run list through the existing shim, with `runs`, `truncated` and `outcomes` counts; or a typed `error` with `code` and `message` |

Run-list rows now include `iterations`, `changed_file_count` and `reject_code`
from existing records. File names, diffs and source contents are not added.
A coding-source failure leaves spend and session data available; it never
becomes an empty successful run list. A snapshot-task failure returns
`ANALYTICS_UNAVAILABLE`. Reads are a best-effort composition, not a transaction
across files; concurrent activity may change the next refresh.

On an older backend returning HTTP 404 for the aggregate, the dialog reads
`/api/spend/summary`, `/api/sessions`, `/api/status` and `/api/agent/runs` in
parallel. Other aggregate failures do not trigger fallback. Individual fallback
source errors remain visible; an authentication/permission refusal clears all
sections. CLI-backed requests retain the existing 130-second client deadline,
which allows the shim's 120-second deadline to report its typed failure.
