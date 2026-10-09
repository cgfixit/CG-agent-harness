# assets

Files compiled into the binary with `include_str!`: `config.default.yaml` (every
tunable; the only place defaults live), `soul.default.md`, `skills_registry.json`,
`skills/` (bundled runtime skills) and `static/` (the console). Because they are
product assets, their Markdown needs no `DOCS_BUDGET` row. `config.default.yaml`
is a core path: read [INVARIANTS.md](../INVARIANTS.md) before editing it.
