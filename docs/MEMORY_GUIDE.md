# Memory: save, suggest, review and retrieve

## What counts as memory

| Surface | Scope and use |
|---|---|
| Session history | Account-owned exchanges. The selected session contributes bounded recent messages. |
| Pinned notes | Shared-home `memory/notes.json`, included when `/memory on`. |
| Uploaded notes | Account-owned Markdown, text or PDF in **Memory → Manage uploaded notes**. Local prompt retrieval uses excerpts. |
| Persona | Shared-home `soul.md`, controlled by `/soul on\|off`. |
| Structured memory | Account-owned facts, pending proposals and episodes. Only eligible facts enter prompts. |

Recalled text is untrusted background. See the
[structured-memory contract](STRUCTURED_MEMORY.md) and [commands](CONSOLE.md#78-slash-command-quick-reference).

## Pinned notes

`/memory add <note>` saves literal text. `/memory on` enables inclusion in
later turns across sessions. `/memory forget <id>` removes one note and
`/memory clear` removes all. `/memory off` excludes notes while keeping them.
The store holds 20 notes of up to 500 characters each. Assembled content is
capped at 3,000 characters and shares that budget with structured facts.
Check `/memory` and `/prompt` for status and inclusion.

## Gates and configuration

### Enable in an existing home

Set `structured_memory.enabled: true` in the home `config.yaml` and restart.
Configure the needed [sub-gates](STRUCTURED_MEMORY.md#gates) and check
`GET /api/structured-memory` for effective settings and `config_drift`.
Live administrator overrides persist and take precedence over sub-gate YAML.
Fresh homes seed all structured-memory gates on. Older homes keep their settings.

## Automatic suggestions and review

Store, capture and the chat or coding source gate must be on.
`suggestion_mode` is `summaries`, `insights` or `both`. The exact local model
and mode must be listed in `suggestion_supported`. The seeded
`qwen3.8:27b-mlx-cg` supports all three.

Open **Memory** or `/memory proposals`. Review text, source, action and any
affected fact. Enter a reason and use **Save memory** or **Dismiss**.
The HTTP decision requires current revision, `confirm: true`, `reason`
and boolean `apply` on `POST /api/structured-memory/proposals/{id}`.
A stale revision requires another review. Check **Suggestion activity** for failures, expiry or
empty results. Queued input is RAM-only. See
[completion suggestions](STRUCTURED_MEMORY.md#completion-suggestions).

## Save a fact manually

`/memory save <fact> :: <reason>` confirms a private `manual` fact write.
It requires the store. Capture and generation can stay off. Ordinary chat text cannot confirm a save;
with suggestions enabled it may result in a proposal.

## Summarize and consolidate episodes

`/memory remember <summary> :: <reason>` attaches a human summary to the
latest completed episode. `/memory consolidate <episode-id...>` proposes
facts from selected episodes using the local model. Review the result.
Idle consolidation requires summarized episodes. Completion suggestions
can create `session_summary` proposals without filling the episode's
human summary. Dismissing or saving a proposal changes its review state
only, and a saved proposal can create or mutate a fact.

## Retrieve and inspect

- **Use in this session** selects a fact ID and revision.
- `/memory search <query>` lists FTS candidates without injecting them.
- `/memory retrieve <query>` force-includes eligible matches for one chat.
- Gated auto retrieval can match meaningful terms in a local message.
  Retrieval is lexical. Owner, active state, revision and budget are rechecked.
- `/prompt` previews `structured_facts.injected` and `dropped`.

## Turn automation or the store off

Use `/memory capture off`, `/memory auto-suggest-chat off`,
`/memory auto-suggest-coding off`, `/memory auto-consolidate off` and
`/memory auto-retrieve off` as needed. `enabled: false` plus restart closes
the store without deleting it. Sessions Clear removes owned uploaded notes
and cancels chat suggestions. The optional `delete_derived_episodes` also
removes eligible episodes. Facts, proposals and pinned notes survive.
