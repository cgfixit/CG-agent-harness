# Browser use and computer use

## What works today

`/browse <site> [https://site ...] <task>` or `POST /api/browser/run` with
`{task, origins}` starts a Chrome task driven by the selected local model.
An authenticated administrator needs literal `browser.enabled: true` (shipped,
restart-only). One task holds the shared model gate. `/browse cancel` stops it.
The first site may be bare (HTTPS implied). A file-like bare first target
requires submitting twice. Additional grants must be explicit URLs.

The shim starts `cgagentharness browser run` with an allowlisted environment.
The child starts [the Playwright sidecar](../browser/sidecar.mjs) with installed
Google Chrome and a temporary profile. Run
`PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --ignore-scripts` in `browser/`.
Use an absolute `browser.sidecar` path for the desktop app.
The sidecar inherits no provider or GitHub credentials. Paths and queries
from grant inputs are stripped before handoff. Task text is sent via a
private temporary file and the audit keeps its SHA-256.

ARIA observations carry expiring element refs, 40 elements and 4,000 characters
at a time. `observe` supports `page` and `find`. Actions include navigate,
click, type, Enter, select, scroll and wait. One granted tab can replace the
active page. A tuned window below 13K fails with `BROWSER_WINDOW`. Shipped
limits are 12 model turns and 300 seconds per task. The child reserves 20 seconds
to report its steps. Three consecutive refused/failed actions end a task.
The console uses detached jobs on macOS and Linux. Windows falls back to
the synchronous endpoint, with no takeover.

- Grants are public exact origins from the request. The host checks every
  request and redirect against current block policy. The authenticated
  loopback proxy pins public addresses. Private access is fixture-only.
  `navigate` accepts a task URL, granted root, page URL or shown link.
- GET and HEAD can proceed under policy. Other methods are held by page and
  browser-level interception. Text outside the task or page is held. Password
  and one-time-code fields hand off.
- With `browser.takeover.enabled` (shipped true), the owner can approve a
  held request from the action through
  `POST /api/browser/jobs/{job_id}/approve` with request ID, matching SHA-256, explicit confirmation and a reason.
  The pass binds method, URL and body hash. The approved request is sent once under current policy. The wait is bounded
  by `browser.takeover.wait_sec` and the task deadline. Background holds are
  reported separately. An unsupported handoff (for example a password
  field) ends the task. Only URLs inside the grant can replace the active
  page. Ungranted origins in the report contain no paths.
- Temporary profiles can survive a kill until the next task or server start.
  Cross-origin assets and workers are blocked. ARIA provides no screenshot or
  canvas view. Playwright routing and browser-wide CDP Fetch enforce
  request holds. The task profile confines WebRTC to proxied TCP.
  Documents above 16 MiB refuse mid-stream. Alerts are reported and
  dismissed. `/goal` and `/loop` use
  search/fetch tools only.

[Source](../src/common/browser_host.rs) and
[fixtures](../.github/workflows/README.md) cover the sidecar, guards and
takeover. Run Chrome fixtures as an unprivileged user with its sandbox on.

## Roadmap: computer use

Native app automation is a policy scaffold. `computer.enabled` ships false
and has no caller. Browser grants confer no Accessibility, screen capture or
desktop input authority. [Scaffold](../src/common/computer.rs).
