# data/styles

Reply style presets embedded by name in `src/server/style.rs` (`BUILTIN_IDS`).
Adding a preset means adding it to that table; a file alone is not catalogued.
Operators can overlay their own under the home's `styles/` folder.

- [`code-review.md`](code-review.md): review-shaped replies: findings, severity, evidence.
- [`concise.md`](concise.md): short answers, minimal preamble.
- [`design.md`](design.md): design and architecture discussion framing.
- [`research.md`](research.md): sourced, evidence-first research replies.
- [`soul.md`](soul.md): concise evidence discipline and candid voice from the original persona.
- [`technical.md`](technical.md): precise technical register with concrete references.

Related: [docs/CONSOLE.md](../../docs/CONSOLE.md) (`/style`),
[tests/style_presets.rs](../../tests/style_presets.rs).
