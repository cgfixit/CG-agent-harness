# Chat, soul, skills and slash commands

## 7. Chat, soul, skills and goals

Start with `/status`, `/model`, `/tools` and `/prompt`. Chat and bounded
continuation use local context and permitted tools. A reply confers no
shell, file, account or repository authority. `/agent` is the separate
[coding workflow](CODING_PIPELINE.md).

### 7.1 Sessions and bounded chat continuation

A session owns its saved goal, transcript, skills, style, pins and selected
facts. New sessions keep home-wide persona and pinned notes, and the
account's web selection. Switching stops chat and discards stale replies.
`/clear` clears display and staging. **Clear my session history** removes
owned sessions, attachments and uploaded notes after confirmation.
Derived episodes need a separate explicit choice. Private export and
backups may outlive deletion. Session storage keeps the first exchange
or summary plus bounded newer whole exchanges (500 stored message limit).
Composer Up-arrow recall is separate from model history and keeps up to 50
lines. Managed-key commands keep only the key name.

`/goal [--web off|fetch|all] <text>` saves the goal and mode. Use `off`
to exclude web tools/context or `fetch` for reads without search.
`web.goal_web_default` chooses the unspecified mode.

`/loop [auto] [--web off|fetch|all] [n] [text]` continues local chat.
Bare `/loop` defaults to three turns. Text sets a goal. Auto starts after
five seconds, without a count runs to `GOAL_DONE` or a cap, and can be
stopped with exact `/loop stop`. Counted runs ask for a complete answer at
the final turn. A goal change, near-duplicate reply, turn/token cap or
consecutive failures can end a run. `GOAL_DONE` is model advice.
A tuned loop reply respects the local chat reply cap. Both zero/absent
run caps produce a warning. See [reloadable limits](INSTALL.md#reload-limits-without-a-restart).

When `/api/status` advertises `server_loop`, eligible auto runs survive
the tab, persist run state under `loops/` and recheck the owner per turn.
Reattach through `/api/loop/status` and `/api/loop/events/{run_id}`.
A page-driven loop can wait out a bounded rate-limit delay. Server runs
count a rate-limit response as a failed turn.
Attachment and manual loops are browser-driven. Server loops wait up to
two hours for a busy model. Ordinary chat refuses busy immediately.
Restart does not resume loops. To code from a goal use
[goal staging](CODING_PIPELINE.md#94-stage-a-session-goal-as-a-coding-task).

#### Search and export session transcripts

**Search my transcripts** uses `POST /api/sessions/search` with a
query up to 200 characters. It searches bounded owned sessions locally
for literal case-insensitive phrases. The per-owner index bounds
input at 200 sessions and 2,000,000 bytes. It returns at most 16 hits and
two per session. A session or account switch discards stale results. **Export .md** uses
`GET /api/sessions/{session_id}/export` and writes private Markdown under
`exports/`. The browser offers a download. Export includes attachment
metadata only. Both enforce ownership and the guarded API checks.

### 7.2 Default soul, effective prompt and persona editing

Fresh homes leave `soul.md` unset. `/soul edit` previews bounded text,
then saves with reason, confirmation and revision checking. `/soul on`
or `off` controls inclusion. `/soul history` lists administrator backups.
`/soul propose` stages text. Review with `/soul review <id>` before
`/soul apply <id> <reason>` or `/soul reject <id> <reason>`. A stale
revision refuses. A private pending-apply marker supports recovery after
process interruption. Reconciliation compares the saved content with the
candidate and prior revision before deciding applied, pending or interrupted.
Preserve the marker, document and backups during repair. Backups and
proposals have a 32-record storage limit.

`/prompt` previews the next local turn. The system prompt includes
enabled persona, style, selected skills, goal, pinned notes and chosen
web context as applicable. Local chat fences dynamic web receipts,
selected/retrieved facts, manual excerpts, attachments, uploaded notes
and staged diagnostics before the new message. That turn context counts
toward the prompt and is excluded from saved history. Loop turns carry it
in their system prompt. Context is untrusted and reflects current gates.
The bundled self-manual and live status are optional sources, controlled by
`diagnostics.self_knowledge` and `diagnostics.live_state`.
Preview is private, marked `no-store` and scoped to local chat.

### 7.3 Runtime skills and Codex development skills

Place optional prompt skills under `<home>/skills/<id>/SKILL.md` and use
`/skill use <id...>`. This replaces the session's selection, up to four
directory IDs. Selected files must be readable, bounded and valid.
`/skill clear` removes selections. `/skill status` can report the last
successful snapshot's hashes. `/skill check:<profile>` only selects a
fixed profile for an already staged coding request. Repository
[development skills](../AGENTS.md#project-skills) are separate.

### 7.4 Customize response style

`/style <name>` selects a session preset. `/style off` clears it.
See [shipped styles](../data/styles/). A private `styles/<name>.md`
overrides that preset. `/prompt` reports omissions or load errors.
Style changes presentation within policy.

### 7.7 Tools and connectors: available versus catalog-only

`/tools` separates registration, enablement and readiness.
`/tools mcp` shows [declared MCP grants](MCP.md#outbound-client-capabilities)
without calling them. `/connectors` and `/registry` list inventory only.
Local chat and loops can use eligible [search and fetch](WEB_DECISION_ROUTER.md).
Admin or auditor `/diag` reads bounded logs, and an opt-in local chat
tool can use them when web tools are absent and model support allows. Browser tasks, MCP, netconnect and coding each have
their own guarded entry points.

### 7.8 Slash-command quick reference

Commands are single lines parsed by the server. Suggestions and composer
inserts execute nothing. Unknown or malformed fixed commands refuse.
Exact `/loop stop` can cancel even when the parser is unavailable.
Memory requires an exact command. `save` and `remember` split the reason
at the last `::`. For web queries, `--` ends flag parsing.

| Command family | Supported use |
|---|---|
| `/agent [help]` | Coding command help. See the [coding workflow](CODING_PIPELINE.md). |
| `/analytics` | Read session, spend and coding analytics. |
| `/api`, `/api set <KEY> <value>`, `/api clear <KEY>` | Manage a supported credential. |
| `/browse <site> [site ...] <task>` | Administrator browser task on the named sites only. Blocked redirects are reported separately; other ungranted sites appear in **Page request details**. Include a redirect's exact origin in the grant to follow it ([browser use](Browser-Use.md)). |
| `/clear` | Clear display, staged reviews and continuation. |
| `/connectors` | Connector catalog; does not activate connectors |
| `/diag [status]`, `/diag audit\|spend\|unslop\|errors [flags]`, `/diag inject\|forget` | Read allowlisted logs or stage one excerpt. |
| `/github` | Read-only agentic GitHub status |
| `/goal`, `/goal [--web off\|fetch\|all] [text]`, `/goal clear` | Inspect, set or clear the saved goal and web mode. |
| `/goal stage <branch>`, `/goal task` | Stage goal for coding or read task status. |
| `/harness` | List retained optimizer runs; does not start optimization |
| `/help [topic\|search words\|all]` | Command help and searchable catalog. |
| `/loop [auto] [--web off\|fetch\|all] [n] [goal text]`, `/loop stop` | Goal-driven chat continuation; goal text sets the goal first; `auto` toggles and starts after 5 s; exact `stop` also cancels a streaming chat turn |
| `/memory`, `status`, `on`, `off`, `add <note>`, `forget <id>`, `clear` | Shared pinned notes; [operator memory notes](MEMORY_GUIDE.md#pinned-notes) |
| `/memory capture\|recall\|retrieval\|auto-retrieve\|consolidation\|auto-consolidate\|auto-suggest-chat\|auto-suggest-coding on\|off` | Administrator sub-gate override. |
| `/memory consolidate <episode-id> [episode-id ...]` | Propose from selected episodes. |
| `/memory proposals` | Open the Memory panel; Apply/Reject requires review and a reason |
| `/memory remember <sentence> :: <reason>` | Confirm human summary on latest completed episode. |
| `/memory facts`, `/memory retrieve <query>`, `/memory search <query>` | List facts, retrieve for one turn or search without injection. |
| `/memory save <text> :: <reason>` | Confirm an immediate private fact write |
| `/model [status]`, `profile`, `list`, `use <name>` | Inspect or select chat model. |
| `/net status`, `/net devices`, `/net diag` | LAN panel or budgeted diagnostics. |
| `/prompt` | Private preview of the next local chat turn's prompt |
| `/registry` | Combined skill, tool and connector catalog |
| `/session`, `list`, `info`, `new [title]`, `rename <title>`, `unpin`, `use <id>` | List, inspect, create, rename or reopen sessions; `unpin` clears attachment pins |
| `/skill`, `status`, `use <id...>`, `clear` | Inspect, replace or clear session prompt-skill selection |
| `/skill check:<profile>` | Select one fixed check for an already staged coding request |
| `/skills [all\|help\|<name>]` | List or inspect skills. |
| `/soul`, `status`, `on`, `off`, `edit`, `history`, `propose` | Inspect, toggle or open persona editing/proposal flows |
| `/soul apply <id> <reason>`, `reject <id> <reason>`, `review <id>` | Review a retained proposal, then apply or reject its exact revision |
| `/status` | Runtime status and unlimited-loop warning; account access required |
| `/style`, `/style <name>`, `/style off` | Inspect/select a session output style, or clear it without changing soul.md |
| `/tokens` | Current session token tally |
| `/tools [all\|help\|mcp\|<name>]` | Collect tool and capability metadata. |
| `/users` | Administrator account panel; [accounts](SECURE_RESEARCH.md#accounts-and-roles) |
| `/web [status\|help]`, `/web on`, `/web off` | Inspect shared-home rules; administrators enable/disable web |
| `/web allow <pattern> [pattern ...] [--group name] [--seed URL ...]`, `/web deny <id-or-pattern>` | Admin grant/remove. Repeat seeds as needed. |
| `/web mode allowlist\|denylist`, `/web block <pattern-or-host> [...]`, `/web unblock <id-or-pattern>` | Admin mode and blocks. Denylist needs confirmation. |
| `/web check <URL> [URL ...] [--group name]` | Exact permission diagnostic, with no network request or grant |
| `/web cancel`, `/web fetch <URL> [URL ...] [--group name] [--crawl]`, `/web forget`, `/web inject` | Cancel research, fetch, drop or select extract. |
| `/web job [id]`, `/web jobs` | Inspect owned detached research. |
| `/web pages [--group name] <query>`, `/web research [--group name] [--url URL ...] <question>` | Search or research permitted pages. |
| `/web search [--count 1..10] [--engine google\|pages] [--group name] <query>` | Google listings by default; a group selects permitted-page search |

Coding subcommands and the approval workflow are in
[Coding pipeline](CODING_PIPELINE.md#93-run-it-from-the-console).
`/agent checks` lists fixed check profiles: `cargo-test`, `cargo-fmt`,
`cargo-clippy`, `pytest` and `ruff`. Arbitrary shell commands are not accepted.

### Text-file attachments and prompt preview

The picker takes up to three `.txt`, `.md`, `.json`, `.csv`, `.log`, `.docx`
or `.pdf` files, each at most 15 MiB. `/prompt` uploads and previews fenced
text. The next message uses the selected IDs. Owner-scoped pins (up to 12)
supply matching passages to later local turns. `/session unpin` clears
references while keeping blobs. Cloud chat and coding refuse live pins or
attachments (`ATTACHMENT_SURFACE_FORBIDDEN`). Attachment bytes count toward
the home quota until cleanup. A failed preview keeps selected attachments
and a session or account switch clears the picker.

**Memory → Manage uploaded notes** takes owner-scoped `.md`, `.txt` and
`.pdf` batches and supplies local BM25 excerpts. `notes_corpus.enabled: false`
refuses ingest and withholds excerpts while allowing list and delete.
Cleanup can be partial (`notes_cleanup`). Missing blobs are skipped.
Unreadable or changed blobs refuse. Failed unlinks or index saves can
leave quota charged. Restore storage access and retry cleanup for the
same owner. See the [memory guide](MEMORY_GUIDE.md).

### DOCX attachments

The parser reads main-document text, tables and breaks. It excludes hidden
or deleted runs and instructions. It neither follows relationships nor
loads macros or images. Ambiguous ZIP or XML layouts, invalid types and
empty documents refuse within bounded expansion and a two-second deadline.
The parser caps 256 package parts, 16 MiB declared expansion, 1 MiB
document XML/output and 128 XML nesting levels.

### PDF text

Attachments, uploaded notes and permitted web reads use the isolated
`pdf-text-worker` for text layers. No OCR. `# Page N` preserves page
locators. Positional joining recovers nearby inline fragments. Columns
follow stream order. Refetch cached copies after extraction changes.

The worker bounds size, pages, decoded bytes, heap and time. Distinct
refusals include `PDF_NO_TEXT`, `PDF_ENCRYPTED`, `PDF_MALFORMED`,
`PDF_LIMIT`, `PDF_TIMEOUT`, `PDF_BUSY` and `PDF_WORKER_FAILED`.
The worker limits include 200 pages, 8 MiB decoded per stream or page,
32 MiB decoded per document, 1 MiB extracted text, a 768 MiB heap and
five seconds per file. Notes and web also apply their caller-specific
byte bounds.
See [I6](../INVARIANTS.md#i6--process-isolation-between-the-console-and-the-agentic-pipeline).

### Streaming chat and cancellation

`POST /api/chat` returns JSON or SSE with `Accept: text/event-stream`.
`delta` events are provisional. `done` includes the committed answer and
usage. `error` carries application status after HTTP 200 stream headers.
Failed or cancelled text is removed and excluded from history. Malformed
or truncated model streams fail. The owner and URL permissions are
rechecked before evidence delivery. Revocation cannot retract sent text.

`POST /api/chat/cancel` stops a local turn or browser-driven loop turn.
`POST /api/loop/stop` stops a server run. Closing a chat stream aborts
its request and releases claims. Client disconnect does not promise
immediate model-service GPU release. The transport requires a completion
reason and terminal marker; tool calls validate before network dispatch.

### Local history compaction

Local chat and loops summarize middle turns above the estimated prompt
trigger. The goal, first user message and whole recent exchanges survive.
Optional `chat.compact_follow_window` scales the trigger on a tuned larger
window. Shipped defaults are a 24,000-token trigger, up to 10 recent
messages and a 25% verbatim tail budget. That trigger is an estimate, not
the hard limit. Summary failures keep history and calibration intact.
`compaction.summary_max_tokens` ships 2048 (768 if absent). Uncompacted
fallback within budget reports `compaction_skipped`.

The hard limit reserves one reply for resolved Ollama with explicit
`reasoning_effort: none` and two otherwise. Counting includes dynamic turn
context and restored web evidence. An unfit prompt returns 422
`CHAT_PROMPT_TOO_LARGE` with `details.limit_source` and the effective reply
budget. See the [compaction invariant](../INVARIANTS.md#local-chat-compaction-preserves-goal-and-the-first-user-turn)
and [model diagnostics](MODELS.md).
