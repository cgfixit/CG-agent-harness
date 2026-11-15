# Browser use and computer use

Status as of 2026-11-15. The local model
(`models.local_llm.model`, `qwen3.8:27b-mlx-cg` on a 32K window) drives a real
Chrome. Computer use (native apps) is a skeleton that nothing calls.

## What works today

`/browse <site> [site ...] <task>` in the console, or `POST /api/browser/run`
with `{task, origins}`. Administrator only, refused unless `browser.enabled` is
literal `true` (ships `false`, restart-only), one task at a time, stopped after
`browser.timeout_sec` (default 300). A bare domain means `https://`.

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
   `select` (a listed option of a select box), `select_tab`, `follow_link` and
   `finish`. A link that opens a new tab replaces the page: one tab per task.
   Older observations are dropped to fit 32K. Three refused actions in a row
   end the task.

Boundaries, enforced in code:

- Origins come only from the request; pages and the model cannot add one.
  Origins on the shared web block list, or that cannot be checked, are refused.
- The sidecar's loopback proxy is the egress check. The task's fresh profile
  keeps WebRTC to proxied TCP, so a page cannot send UDP around it (Chrome
  153 ignored the command-line switch). A granted name must
  resolve to public addresses (`localhost` is refused), and a loopback or
  private IP origin is refused; only a `test-support` build lets the fixtures
  use `127.0.0.1`.
- The block list is checked on the granted origin before launch, again before
  every model turn, and once more before the report is returned.
- A task holds the shared model gate, so it never overlaps chat or research.
- Clicks may run. Every request except GET and HEAD is held for the operator:
  a held page load answers 204 so the page stays, a held `fetch` fails.
- Password and one-time-code fields hand off and are never typed. The console
  lists each held request and hand-off after the answer.

Verified by running: fixtures `browser`, `browser enter` (a new tab replaces
the page; search submits; a select takes an option; password hands off; a POST
form and a script POST are held; WebRTC sends no UDP) and `browser-verify`;
Qwen listed the games behind Start → Games on cgfixit.com (2026-11-14).

## Honest limits

- Grants are exact origins. Scripts and styles from a CDN origin are blocked,
  so many large sites render partly.
- Path-level block-list rules are not enforced inside a granted origin.
- The model sees the first 40 elements and 4,000 characters of text; long
  pages need it to navigate deeper. No screenshots yet, so canvas and
  image-only UIs are opaque.
- `scroll` and `wait` exist in the policy but are not model tools.
- A hand-off or hold ends the task: no approve button and no resume.
- Holding every `fetch` POST can break sites that save state in the
  background, such as chat widgets or autosave.
- Page text is untrusted input. Grants and holds bound what it can cause; an
  answer can still be wrong.
- `/goal` and `/loop` do not have browser tools. A goal that says "/browse" runs
  web search and fetch instead.
- The unit tests added for this feature have not run locally (operator rule);
  CI runs them.

## Roadmap: broader browser use

In rough order:

1. Takeover: approve a listed held request or hand-off and resume the session.
2. Subresource policy: let GET subresources load from public origins without
   cookies while navigation stays on granted origins.
3. Vision: send the latest screenshot to the model (`qwen3.8` lists the
   `vision` capability) and retire older images like observations.
4. Model tools for `scroll` and `wait`, and element paging past 40.
5. Path-level block rules inside the proxy.
6. Browser tools for `/goal` and `/loop`, behind their own gate and the same
   grants.

## Roadmap: computer use

A browser grant never authorizes Accessibility, screen capture or input. The
policy kernel carries over unchanged: per-observation refs, trusted element
facts, allow, hand-off or refuse. Only the driver behind
`browser_host::Driver` changes.

**Option A, with Kimi Computer Use as the driver.** Kimi Computer Use
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

**Option B, hand the task to Kimi Code.** The harness passes the whole task
to the Kimi Code agent (`kimi acp`) and receives its result. This is the
quickest route to general computer use, but Kimi's model, not the harness
policy, decides each step, and the task leaves the machine for Kimi's
service. It would need its own opt-in and an explicit warning.

**Option C, no Kimi.** A first-party macOS Accessibility driver, a small
signed helper the harness owns, behind the same `Driver` seam. Most work,
fewest outside dependencies.

The first pilot for any option stays attended: one packaged app, one window,
fixture data, operator takeover, and one result checked outside the model.
