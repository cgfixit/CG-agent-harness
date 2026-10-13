# assets

Files compiled into the binary with `include_str!`. Because they are product
assets, their Markdown needs no `DOCS_BUDGET` row.

- [`config.default.yaml`](config.default.yaml): every tunable and its shipped
  default; the only place defaults live. **Core path**: read
  [INVARIANTS.md](../INVARIANTS.md) before editing. Gates need literal `true`.
- [`soul.default.md`](soul.default.md): preserved reference persona and public coding guidance; not seeded into fresh homes
  (`DEFAULT_SOUL` in `src/common/home.rs`), editable per home from the console.
- [`skills_registry.json`](skills_registry.json): the empty runtime skills
  registry seeded into a fresh home's `data/agentic/`.
- [`skills/`](skills/README.md): the two bundled runtime skills seeded into a
  fresh home.
- [`static/`](static/README.md): the console page and admin script served by
  `cgagentharness serve`.

Related: [docs/CONSOLE.md](../docs/CONSOLE.md), [docs/INSTALL.md](../docs/INSTALL.md#8-persistence-optional-keys-and-recovery)
(which copy of the config takes effect).
