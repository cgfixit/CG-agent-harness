# assets/static

The console served by `cgagentharness serve`. The asset is served verbatim: the
placeholders `__CYCLAW_CSRF_TOKEN__` and `__CYCLAW_CSP_NONCE__` and the
`X-CyClaw-CSRF` header name are contractual (`src/server/console.rs`,
`src/server/headers.rs`).

- [`harness.html`](harness.html): the single-page console (chat, panels, slash
  commands, settings), including Memory outcome polling and retained proposal-review
  forms, with inline script and styles.
- [`auth_admin.js`](auth_admin.js): the sign-in, password and account-admin
  script loaded by the console.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (operator view),
[docs/API_ROUTES.md](../../docs/API_ROUTES.md), the `scripts/test-*.mjs`
browser checks in [scripts/](../../scripts/README.md).
