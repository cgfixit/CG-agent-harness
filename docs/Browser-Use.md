# Browser use and computer use

Status as of 2026-11-20. The local model (the `/model use` selection, else
`models.local_llm.model`; `qwen3.8:27b-mlx-cg` on a 32K window) drives a real
Chrome. Computer use (native apps) is a skeleton that nothing calls.

## What works today

`/browse <site> [https://site ...] <task>` in the console, or
`POST /api/browser/run` with `{task, origins}`. Administrator only, refused unless `browser.enabled` is
literal `true` (ships `true`, restart-only), one task at a time, stopped after
`browser.timeout_sec` (default 300). Without accounts (`auth.enabled` not literal
`true`) the admin check cannot run, so it refuses with `BROWSER_AUTH_REQUIRED`.
Only the first site may be a bare domain (meaning `https://`), so a task word like
`README.md` is never a grant. A bare first word with a file extension
(`setup.py`) runs only when the line is sent twice; `https://` skips that.

How a run works:

1. The server runs `cgagentharness browser run` as a child through the shim's
   runner with an environment allowlist (`browser_host::child_environment`):
   no provider keys or GitHub tokens reach Node or Chrome.
2. The child starts `browser/sidecar.mjs` (Playwright 1.65.1, installed Google
   Chrome, temporary profile). Prepare it once: `PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
   npm ci` in `browser/`. The desktop app needs an absolute `browser.sidecar`.
3. Each observation is Playwright's aria snapshot: real roles, accessible
   names, absolute link targets. `src/common/browser.rs` mints refs that
   expire with every observation, so a stale ref cannot act.
4. The model calls `navigate`, `observe`, `click`, `type`, `press_enter`,
   `select` (a listed option of a select box), `select_tab`, `follow_link`,
   `scroll`, `wait` (1 to 5 s) and `finish`. An observation is 40 elements and 4,000 characters, read after
   the page's fetches settle; `observe` takes `page` for the next part and
   `find` for what contains a word. Older observations are dropped to fit
   32K. Three refused or failed actions in a row, or a reply with no action
   and no answer, end the task. A sidecar call has 75 s; a model call has the
   task's remaining time. The child stops 20 s before `browser.timeout_sec`
   and reports its steps.
5. One tab per task. A new tab replaces the page only if it shows a granted
   page; any other is closed and the page stays.

Boundaries, enforced in code:

- Origins come only from the request; pages and the model cannot add one.
  Only each URL's origin is kept: its path and query never reach argv, the
  audit line, the prompt or the report. The task text reaches the child in a
  private temporary file, never argv; the audit keeps only its SHA-256.
- A page cannot flood the run: a link target over 8,192 characters is
  dropped, the model sees each cut to 300, the title to 300 and the URL to
  8,192; a sidecar reply line over 4 MiB stops the task.
- The sidecar's loopback proxy is the egress check, and it answers only this
  task's Chrome (a per-session credential). The task's fresh profile
  keeps WebRTC to proxied TCP, so a page cannot send UDP around it. A granted
  name must resolve to public addresses (`localhost` is refused), and a
  loopback or private IP origin is refused; only a `test-support` build lets
  the fixtures use `127.0.0.1`.
- The block list is checked on the granted origin (one that cannot be
  checked is refused) before launch, every model turn and the report. Inside a grant, the host judges every request
  and redirect hop before it is sent, so path rules hold; an edit applies to
  the next request and an unreadable policy stops all. On a host a rule
  names, a URL the matcher cannot judge (over 2,048 bytes, an encoded
  separator) is refused.
- A task holds the shared model gate (no chat or research overlap). A tuned
  model loads first; a window under 13K is refused (`BROWSER_WINDOW`).
- Clicks may run, including a link with no target. Every request except GET
  and HEAD is held for the operator: a held page load answers 204 so the page
  stays, a held `fetch` fails. Two layers hold it: Playwright's page route and
  a browser-wide CDP `Fetch` gate, which also sees a SharedWorker's requests.
- Pages cannot start workers (`worker-src 'none'` on every document): a
  worker's WebSocket passes the page-level block and the proxy tunnel.
- Provenance (#901): `navigate` takes only a URL the task names, a granted
  root, a page URL or a shown link; others are refused, reported by origin.
  Typed text not in the task or page is held (GET forms too).
- Password and one-time-code fields hand off and are never typed. A hand-off
  ends the task at once, as does a held page load or a request held during
  the model's own action, until that action loads a new page. Other held
  requests are the page's own: listed, and the task goes on. The console lists
  each, then the origins (never paths) a page asked for outside the grant.

Verified by running: fixtures `browser`, `browser enter` (new tab, search,
select, password hand-off, both kinds of held POST, no WebRTC UDP), `browser reach` (late
content, an ungranted tab closed and named, scroll, wait, an alert's text,
`page` and `find`) and
`browser-verify`. `browser guards` checks by what its loopback server received:
no worker starts, a redirect lands, no held POST arrives, a blocked path gets no request, a 1.14 MB page
gives a capped reply, and the proxy answers 407 without the session credential. CI's
`browser-guards` job runs every fixture, sandbox on.

## Honest limits

- Grants are exact origins. A CDN origin's scripts and styles are blocked,
  so many large sites render partly.
- A block rule stops requests, not content a page loaded before the rule.
- A killed task's profile (cookies, cache) stays until the next task or
  server start.
- No screenshots: canvas and image-only UIs are opaque. `alert` and
  `confirm` text is shown to the model, then dismissed (a confirm as Cancel).
- Held `fetch` POSTs break background saves; one sent during a model
  action still ends the task. Sites needing a web worker lose it.
- Page text is untrusted. Grants and holds bound its effect; an
  answer can be wrong.
- `/goal` and `/loop` have no browser tools: "/browse" in a goal runs web
  search and fetch.

## Roadmap: broader browser use

1. Takeover, not built: approve a listed held request or hand-off and resume.
2. Subresource policy: let GET subresources load from public origins without
   cookies while navigation stays on granted origins.
3. Vision: send the latest screenshot to the model (`qwen3.8` lists the
   `vision` capability) and retire older images like observations.
4. Browser tools for `/goal` and `/loop`, behind their own gate and the same
   grants.

## Roadmap: computer use

A browser grant never authorizes Accessibility, screen capture or input. The
policy kernel carries over unchanged: per-observation refs, trusted element
facts, allow, hand-off or refuse. Only the driver behind
`browser_host::Driver` changes.

**Chosen driver: Kimi Computer Use.** It
(`/Applications/KimiCU.app`, `kimi-cu mcp`, tools checked at 0.7.6) reads the
Accessibility tree and acts without moving the pointer.
`src/common/computer.rs` maps the wire ops to `get_app_state`, `click`,
`type_text`, `press_key` and `scroll` for one granted app and window and
refuses `debug_tap`, `paste`, `launch_app`, `activate_window` and drags. Still
to build: the MCP transport through `common/mcp.rs`, launching the signed
binary (the plugin's wrapper script is world-writable), turning
`get_app_state` into element facts, and a `computer.enabled` gate. Qwen stays
the model, so nothing leaves the machine. Kimi Computer Use is proprietary;
its TCC grants (Accessibility, Screen Recording) belong to the operator.

Not chosen: Kimi Code (`kimi acp`; the task leaves the machine) or a
first-party Accessibility helper.

Any first pilot stays attended: one app, one window, fixture data, operator
takeover, a result checked outside the model.
