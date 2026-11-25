# Browser use and computer use

## What works today

`/browse <site> [https://site ...] <task>` or `POST /api/browser/run` with
`{task, origins}` starts a Chrome task driven by the selected local model.
An authenticated administrator needs literal `browser.enabled: true` (shipped,
restart-only). One task holds the shared model gate. `/browse cancel` stops it.
The first site may be bare (HTTPS implied). A file-like bare first target
requires submitting twice. Additional grants must be explicit URLs.

The shim launches `cgagentharness browser run`, which starts the
[sidecar](../browser/sidecar.mjs) with Playwright 1.65.1, installed Chrome
and a temporary profile. In `browser/` run
`PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --ignore-scripts`.
Desktop requires an absolute `browser.sidecar` path. The sidecar receives
an allowlisted environment without provider or GitHub credentials.
Task text travels in a private file and is hashed for audit.

ARIA observations carry expiring refs, 40 elements and 4,000 characters.
`observe` supports `page` and `find`. A tuned window too small for the task
returns `409 BROWSER_WINDOW` before launch. Shipped limits are 12 turns and
300 seconds, with 20 seconds reserved to report. A deadline reached on a
later model call returns the partial report with `finished: false` and
`report.stopped`. Three consecutive failures end a task.
macOS and Linux use detached jobs. Windows uses the synchronous endpoint.

- Each task grants exact public origins independently of the web allowlist.
  An ungranted redirect from `navigate`, a link or Enter fails, naming its origin.
  Use `/browse https://www.python.org/ read the first sentence` for Python’s
  homepage. Expand **Page request details** for other origins the page requested.
  The host checks each request and redirect against current block policy. The authenticated
  loopback proxy pins public addresses. Private access is fixture-only.
  `navigate` accepts a task URL, granted root, page URL or shown link.
- Playwright routing holds methods other than GET and HEAD, even mid-load
  form posts; `navigate` still returns the page. CDP Fetch repeats it
  browser-wide (unfixtured backup). Text outside the task or page is
  held. Each observation
  checks text fields concurrently. Targeting a secret field or one whose
  attributes could not be read ends the task with a handoff.
- With `browser.takeover.enabled` (shipped true), the job owner can approve
  one request sent mid-action, or one form posted within 5s
  after a click or Enter. `POST /api/browser/jobs/{job_id}/approve`
  needs request ID, body SHA-256, confirmation and reason. Approval binds
  method, URL and body under current policy. Waiting ends at
  `browser.takeover.wait_sec`, the deadline or a `navigate` (held). A
  request Chrome cancels while waiting (the page left) ends as a no and is
  held; a later yes sends nothing. Other holds get no prompt.
- Temporary profiles can survive a kill until the next task or server start.
  Ungranted origins, workers and documents over 16 MiB are blocked.
  WebRTC uses proxied TCP. ARIA provides no screenshot.
  Alerts are reported and dismissed. `/goal` and `/loop` have no browser tools.

[Source](../src/common/browser_host.rs) and
[fixtures](../.github/workflows/README.md) cover the sidecar, guards,
takeover, and the synchronous route's window refusal and deadline report.
Run Chrome fixtures as an unprivileged user with its sandbox on.

## Roadmap: broader browser use

1. Let operators approve a page-initiated request or resume after handoff.
2. Allow cookieless public GET subresources while keeping navigation on granted origins.

## Roadmap: computer use

Native app automation is a policy scaffold. `computer.enabled` ships false
and has no caller. Browser grants confer no Accessibility, screen capture or
desktop input authority. [Scaffold](../src/common/computer.rs).
