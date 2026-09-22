# Native slash-command acceptance

Captured on 2026-09-22 through Computer Use against the packaged arm64
`CG Agent Harness.app` and its native WKWebView. The app used a disposable home
under `/private/tmp`; the operator home and installed app were not opened.

## Provenance

- Base source: `4eb33c5e01ad9a1d309ca1423de54b4dede833a6`
- Bundle marker: `DEVELOPMENT BUILD: uncommitted changes`
- ZIP SHA-256: `f9ca86b392a166071489648705d0697db0ab4c9c5d211b33807d618fb5854f84`
- Packaged backend SHA-256: `ffb95c5cda30c065e3be9131e0395d57856760a7535ecabd678c3be2c6c53394`
- Packaged desktop SHA-256: `c0fde060bb7f3e5102fa277f259356081182bb6c5164be38c6b0750ee6fa85db`

The development tree contained the slash-command changes shown here plus the
packager-only build-dependency strip fix needed to produce the app. The latter
does not alter the embedded console or slash parser. No model generation,
repository write, or external provider was used.

## Observed cases

| Screenshot | Native observation |
|---|---|
| `01-native-command-guide.png` | The command guide states that commands must be one line without control characters. |
| `02-native-staged-request.png` | Exact `/agent run` stages a harmless request and explicitly reports that nothing ran. |
| `03-native-multiline-agent-refused.png` | Pasting `/agent` plus a newline and confirmation text is refused; the staged request remains visible. |
| `04-native-exact-agent-cancel.png` | Exact single-line `/agent cancel` still dispatches and reports that the staged run was discarded. |

The session was logged out and the native app was quit after capture. Automated
Rust and real-Chrome suites separately exercise server-side separator refusal,
other mutation families, parser failure behavior, and browser paste handling.
