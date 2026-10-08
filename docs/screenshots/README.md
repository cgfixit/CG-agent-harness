# Screenshots

Store captures flat here with unique filenames. Record new artifact hashes,
platform, fixture/live distinction and verification limits in the adding PR;
do not add Markdown files. Historical screenshots do not verify newer builds.

## Overview captures

[CG-Agent-1.png](CG-Agent-1.png), [CG-Agent.png](CG-Agent.png),
[CG-Agent-Mobile.png](CG-Agent-Mobile.png), [CG-mobile.png](CG-mobile.png),
[app-ss.png](app-ss.png), [former README hero](image.png).

2026-10-05 loopback-fixture chat: [README hero](console-chat-signed-in-2026-10-05.png)
(1440×900), [390px](console-chat-signed-in-390-2026-10-05.png),
[first-run sign-in](console-sign-in-first-run-2026-10-05.png).

## Native slash-command captures, 2026-09-22

Computer Use exercised the packaged arm64 app's WKWebView with a disposable
home. No generation, repository write or external provider was used; the session
was logged out and the app quit afterward.

- [Guide](slash-single-line-01-native-command-guide.png): single-line requirement.
- [Staged request](slash-single-line-02-native-staged-request.png): nothing ran.
- [Multiline refusal](slash-single-line-03-native-multiline-agent-refused.png): request stayed staged.
- [Exact cancel](slash-single-line-04-native-exact-agent-cancel.png): staged request discarded.

Source `4eb33c5e01ad9a1d309ca1423de54b4dede833a6` plus uncommitted slash-command
changes and the packager build-dependency strip fix; marker
`DEVELOPMENT BUILD: uncommitted changes`.

SHA-256:

- ZIP: `f9ca86b392a166071489648705d0697db0ab4c9c5d211b33807d618fb5854f84`
- Backend: `ffb95c5cda30c065e3be9131e0395d57856760a7535ecabd678c3be2c6c53394`
- Desktop: `c0fde060bb7f3e5102fa277f259356081182bb6c5164be38c6b0750ee6fa85db`
