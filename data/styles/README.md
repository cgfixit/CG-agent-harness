# data/styles

Reply style presets embedded by `src/server/style.rs` (`BUILTIN_IDS` and
`BUILTIN_BODIES`). Add a built-in to both tables; a file alone is not embedded.
Operators can overlay their own under the home's `styles/` folder.

- [`code-review.md`](code-review.md): review-shaped replies: findings, severity, evidence.
- [`concise.md`](concise.md): short answers, minimal preamble.
- [`design.md`](design.md): design and architecture discussion framing.
- [`research.md`](research.md): sourced, evidence-first research replies.
- [`soul.md`](soul.md): concise evidence discipline and candid voice from the original persona.
- [`technical.md`](technical.md): precise technical register with concrete references.
- [`troubleshoot.md`](troubleshoot.md): observed versus inferred, one next command.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (`/style`),
[tests/style_presets.rs](../../tests/style_presets.rs).
