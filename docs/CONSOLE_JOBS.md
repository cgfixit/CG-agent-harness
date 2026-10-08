# Console job workflow

`/api/agent/checks` advertises the default check, allowed profiles, planner
model, polling interval, and job capabilities. A staged request uses the server
default, currently `cargo-test`, unless `/agent checks <profile>` selects a
supported override. `/model use` selects chat only and reports the separate
planner model.

`/agent confirm <reason>` submits to `/api/agent/jobs` and immediately retains
its job ID in the URL fragment. `/agent job <id>` resumes monitoring after a
refresh; sign in again if the account session expired. The optional harness key
may remain empty and cannot restore account access.
`/agent jobs` lists the account's retained jobs, including entries interrupted
by restart. Failed jobs retain CLI error output. Completed jobs show the run
record and verification output.

`/agent cancel` discards a staged request. `/agent stop <id>` cancels an active
request, but **descendant processes may survive**. A bounded private JSON file
stores job state. Restart marks running jobs interrupted and never resumes them.
`/agent runs` lists run records and reconciles released Unix worker leases to
interrupted; unowned legacy records remain unknown. Inspect records and surviving
processes before more writes. A stop response does not prove process-tree
termination. Chat streams over SSE and `/loop stop` cancels a turn; job progress
does not stream. See [streaming chat](CONSOLE.md#streaming-chat-and-cancellation).

Inspect `/agent status <run id>` and the complete diff before
`/agent approve <run id> <reason>`. Approval commits locally. Push and draft
publication require their own explicit `/agent push` and `/agent publish`
actions with reasons. Before publication, complete the repository's actual PR
template in a local Markdown file, then use `/agent pr-body <run id>` to select
and preview it. `/agent publish <run id> <reason>` sends that reviewed text.
Reselect after any file edit; refresh clears staged bodies. A truncated diff
cannot satisfy console review.

## Schedules and completion notifications

Use the **Schedules** button, shown while the coding pipeline is enabled, after
`/goal stage <branch>`. Review the staged
request, enter a reason, explicitly check the authorization box, and choose an
interval or cron calendar. **Preview next 5** shows local timestamps with UTC
offsets and UTC equivalents. **Activate previewed schedule** consumes a short-lived,
owner-bound receipt for that exact request. Editing the request or timing requires
a new preview. Merely opening or previewing the form creates no schedule.

The guarded API uses the same flow: `POST /api/agent/schedules/preview`, then
`POST /api/agent/schedules` with its `preview_id` and the identical body:

```json
{
  "schema_version": 1,
  "schedule": {"kind": "cron", "expression": "0 9 * * MON-FRI", "timezone": "America/New_York"},
  "request": {"goal_stage": {"session_id": "<owned session>", "stage_id": "<reviewed stage>"}, "instruction": "<reviewed instruction>", "branch": "<reviewed branch>", "commit_message": "<reviewed message>", "reason": "<operator reason>", "confirm": true}
}
```

Use the complete goal-stage response; the example is abbreviated. Intervals use
`{"kind":"interval","seconds":3600}` and accept 60–604800 seconds. The legacy
`interval_secs` field requires the same preview; do not supply both forms. Cron
uses five fields, supports lists, ranges, steps, and weekday names, and uses 0 or
7 for Sunday. Either day-of-month or weekday must be literal `*`. Timezone
defaults to UTC and accepts IANA names. Calendars end in 2100.

Rows migrate to schema 1 without changing interval cadence or inventing owners.
Ownerless legacy schedules stay hidden and cannot dispatch. Management is owner
scoped; every occurrence rechecks the current enabled, non-bootstrap operator/admin
owner, reviewed goal, budget, broker and execution/write gates before the
occurrence is consumed. A disabled account, or one that must change its password,
cancels that owner's active rows (`last_dispatch=skipped_revoked`) so they stop
coming due. Disabling or deleting the account does the same immediately, and
does not touch another owner's rows. The existing run
gate prevents overlap; a busy or unbound occurrence is consumed and skipped
(`last_dispatch=skipped_refused`), never queued and never recorded as attempted.
`GET /api/agent/schedules/{schedule_id}` inspects a schedule;
`POST /api/agent/schedules/{schedule_id}/cancel` stops future dispatch. Cancellation
and job registration serialize under the schedule lock. An already started job
needs its separate cancellation control.

Occurrence identity and the next UTC time persist atomically **before** dispatch.
Restart skips elapsed occurrences. The default polling grace is five seconds;
older work is skipped. Forward clock jumps skip missed work, while backward jumps
wait for the persisted UTC time. Nonexistent DST times are skipped; ambiguous
times use the first UTC mapping. There is no catch-up. Dispatch is at most once,
so a crash after consumption can miss work. `last_dispatch=attempted` means a job
handle was accepted. Other values are `skipped_late`, `skipped_refused`,
`skipped_revoked`, and `skipped_downtime`.

The `scheduling` section in `config.default.yaml` documents finite poll, grace,
preview-expiry and inventory settings. They require restart and are outside the
23-key limit-reload allowlist. Existing execution defaults remain closed.

In a [webhook-capable build](SPEND_AND_NOTIFICATIONS.md), optional notifications
cover terminal detached jobs started manually or by schedules: `finished`,
`failed`, and `cancelled`. Repeated finish/cancel calls do not create a second
event. Recovered `interrupted` jobs are not announced. A schedule refused before
job creation has no completion event; inspect `agent_schedule_failed` in audit.
Notifications never approve, retry or change the outcome of a coding operation.
Setup, durable metadata schema, recovery and finite delivery limits are in the
[completion guide](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook).

## Reproduce browser acceptance on macOS

Requires a built release binary, Python 3, Node with built-in WebSocket support,
Google Chrome and an already installed, explicitly chosen Ollama model. The
fixture helper creates a new directory, local bare remote, isolated application
home, synthetic optional metadata key and a local `gh` adapter. It prepares locked Cargo
inputs offline. It never calls the real GitHub CLI or pulls models.

First inspect `ollama list`; use the exact installed identifier. In one terminal:

```sh
cargo build --release --locked
python3 scripts/browser-fixture.py /tmp/cgah-browser-acceptance \
  --model qwen3.8:27b --endpoint http://127.0.0.1:11434/v1 --port 8792 --http
```

The destination must not exist. In another terminal, from the checkout:

```sh
CGAH_TEST_BASE_URL=http://127.0.0.1:8792 \
CGAH_TEST_HOME=/tmp/cgah-browser-acceptance/home \
CGAH_TEST_API_KEY_FILE=/tmp/cgah-browser-acceptance/api-key \
CGAH_TEST_FIXTURE=disposable-arithmetic \
node scripts/browser-acceptance.mjs
```

The fixture explicitly opts into HTTP for this browser coding test and keeps
account authentication enabled. Native/standalone HTTPS is checked separately
by `scripts/test-desktop-backend.py` and native acceptance. The test starts a
fresh headless Chrome profile, replaces the fixture bootstrap password, checks authentication and CSRF,
stages an explicit large-file window, verifies server defaults/check selection,
submits an asynchronous job, refreshes, restores authentication, reviews the
complete expected one-expression diff, approves a local commit, separately
pushes to the local bare remote, loads/previews a PR body through the browser
file chooser, verifies exact mock draft publication, and distinguishes staged
and active cancellation.
Missing Chrome, model failure, failed verification or wrong diff fails the test.
No npm dependency or Chrome sandbox bypass is used. Stop the fixture server with
Ctrl-C; its disposable directory remains for inspection.

The model endpoint must itself enforce the desired network policy. A localhost
URL alone does not prove offline inference. This test does not establish remote
GitHub publication, startup recovery or descendant termination.

## Recorded acceptance

Historical acceptance used Chrome on an Apple M5 Pro with 48 GiB, macOS 26.6.2,
installed `qwen3.8:27b`, Seatbelt denial of non-loopback Ollama traffic, and
offline sandboxed Cargo. It did not alter the operator's Ollama process or model
files. Current security evidence is in [secure research](SECURE_RESEARCH.md);
the historical native matrix is in [desktop acceptance](DESKTOP_ACCEPTANCE.md).


Direct CLI publication requires `--body-file /path/to/reviewed-description.md`
or explicit `--body`, plus a reason and confirmation. Descriptions contain
1–65536 UTF-8 bytes. The server and GitHub invocation use temporary body files,
which preserve newlines and keep text out of process arguments. The operator
must complete and review the repository's PR template.
