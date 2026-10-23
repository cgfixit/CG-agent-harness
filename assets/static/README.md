# assets/static

The console embedded by `cgagentharness serve`. Keep the contractual
`__CYCLAW_CSRF_TOKEN__`, `__CYCLAW_CSP_NONCE__` and `X-CyClaw-CSRF` names:
`src/server/mod.rs` substitutes the process CSRF token, and
`src/server/console.rs` inserts a fresh CSP nonce per response.

- [`harness.html`](harness.html): the single-page console (chat, panels, slash
  commands, settings), including Memory outcome polling and retained proposal-review
  forms, with inline script and styles.
- [`auth_admin.js`](auth_admin.js): the sign-in, password and account-admin
  script loaded by the console.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (operator view),
[docs/API_ROUTES.md](../../docs/API_ROUTES.md), the `scripts/test-*.mjs`
browser checks in [scripts/](../../scripts/README.md).
