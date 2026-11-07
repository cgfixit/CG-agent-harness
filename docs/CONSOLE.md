# Chat, soul, skills and slash commands

See [setup](../README.md), [memory](MEMORY_GUIDE.md), and
[web operations](SECURE_RESEARCH.md#search-fetch-and-research).

Chat continuation and coding jobs differ: `/loop` follows a session goal,
while `/agent` drives the [coding pipeline](CODING_PIPELINE.md). A goal, skill,
persona, or reply never authorizes commands, commits, pushes or publication.

## What you can do

| Capability | How it works |
|---|---|
| Chat and sessions | Create, rename, and revisit separate conversations with saved messages and token counts. New Session replaces the transcript; the confirmed Clear my session history control deletes saved conversations. Derived structured-memory episodes remain unless the operator also confirms that cascade. |
| Persona, memory and prompt context | Inspect `/prompt`, manage shared `soul.md`, select session prompt skills, and save literal `/memory` notes. `/memory on` includes only those notes; account-private structured memory has separate gates, and models propose but never approve ([memory guide](MEMORY_GUIDE.md)). |
| Chat model selection | Select local tags, `local`, `grok/xai`, or `claude/anthropic`. Cloud requires an enabled provider, active key and confirmation; restart resets to local. `/model status` shows key presence and profile sources. Only new text leaves; history, memory, skills and web context stay local. `/loop` refuses cloud (`CLOUD_CHAT_LOOP`). [Profiles](MODELS.md). |
| Chat continuation | `/goal` and `/loop` provide bounded follow-up turns with request limits, completion-token budgets, cancellation, and optional auto-continue. |
| Tool visibility and use | `/skills` and `/tools` distinguish registered adapters from readiness and execution evidence. Console commands invoke backend operations through fixed, validated interfaces; model prose does not become an arbitrary shell command. |
| Chat web tools | Natural-language keyword search (public Google, SerpAPI or Tavily, per `web.search.provider` and the saved key) and permitted URL fetch, with the opt-in Firecrawl fallback for exact reads. Tool outcomes and source links appear in chat ([search providers](SECURE_RESEARCH.md#google-search)). |
| Web research | Grant exact or wildcard URL access for bounded discovery and BM25 search. `/web research` returns local-model answers with checked quotes, usage, and partial coverage. Fresh homes allow only Google search; selection and injection are account scoped. |
| Accounts and API Keys | Fresh homes open one form for the private setup code and new password; completed homes show sign-in. Closing setup leaves a lock card. Administrator, Portal operator and Auditor permissions are enforced on API reads and writes. Administrators manage masked saved/active credentials in API Keys. |
| Coding loop | Stage a task and files or plan, then confirm an isolated run that proposes bounded edits, runs fixed sandboxed checks, and feeds results into later attempts. |
| Review and publication | Inspect retained run status and diffs, approve the reviewed tree for a local commit, then separately push and publish a draft PR with a reviewed repository template. |
| Analytics and spend | After sign-in, Analytics or `/analytics` opens filterable, paged Tokens and cost, Sessions and Code sections: read-only retained usage, available USD, completeness warnings and **Estimate draft**. [Interpretation and API](SPEND_AND_NOTIFICATIONS.md#analytics-sessions-tokens-and-coding-runs). |
| Completion webhooks | Metadata-only notifications for terminal detached jobs, off until configured; the **Job webhooks** button shows only then. Delivery grants no job or repository authority. [Setup](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook). |
| Recovery | Rediscover retained jobs and runs after reopening. Worker leases distinguish active work from interrupted runs; reopening does not automatically resume work or replay a publication. |

Text follows browser zoom. In the sidebar tab list, arrow keys, Home and End move
and Enter or Space opens a tab. Drag its right edge to resize; drag far left to
hide it, or focus the divider and use the same keys. The top-left **>>** restores
it. Width lasts until reload. The conversation is a polite live log, marked busy
while a reply streams.

## 7. Chat, soul, skills and goals

Fresh chat has no assigned repository or automatic coding skills. When web is
enabled, local chat can dispatch only the bounded read-only `web_search` and
`web_fetch` tools, including local `/loop` turns. Chat cannot inspect local
files or certify live wiring; use command results as evidence, the coding
workflow for execution.

Existing homes retain repositories and skill files; chat neither loads nor edits
legacy coding skills. For an isolated test, start `/session new`, `/skill clear`,
inspect `/memory` and `/soul status`, then `/prompt`. A new
session still uses enabled shared notes and persona.

Enter slash commands in chat; `/help` lists them. Begin with `/status`,
`/model`, `/skills all`, and `/tools`; registration is not
[readiness](#77-tools-and-connectors-available-versus-catalog-only).

### 7.1 Sessions and bounded chat continuation

```text
/session new Setup check
/goal --web off Explain the attached test strategy
/goal
```

Send a question and confirm a reply. `/tokens` and the header's
**session · tokens** count the tally
([interpretation](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero)).

Creating or switching sessions replaces the visible transcript, stops chat and
the generation wait, discards delayed replies, and clears staged
coding or persona reviews. Switching restores retained messages without
duplicates. A new session has no messages, goal, or prompt skills. Persona and
enabled notes stay shared; your account's web selection and
injected context survive session changes. The HTML editor button, beside the web-policy globe, previews the draft for the chat open when clicked, as a separate page that runs the draft's own style and script but cannot call this console. Reset controls differ in scope:

| Intent | Control | Retained data |
|---|---|---|
| Clear the visible output | `/clear` | Saved messages, session ID and goal remain; later chat still receives recent history. Hidden staged reviews and loop state clear. |
| Start a separate conversation | **+ new session** or `/session new <title>` | Old sessions, shared persona/notes/model selection, and your account's web selection remain. |
| Delete my saved conversations | **Clear my session history**, immediately below **+ new session** | Shared notes/persona/web, model configuration, coding runs and audit records remain. |

In the dialog, choose **Delete my session history** to confirm or **Cancel**.
Confirmation stops active chat, clears the conversation, and deletes your saved
sessions, goals, skill selections, and token totals, retaining other accounts,
unassigned legacy records, and unreadable files. Late replies cannot recreate a
deleted session. Afterward, send a message or use `/session new`. If storage
fails, resolve the reported problem before retrying; deletion may be partial.

Deletion is not secure erasure: backups, model-service copies, and text loaded
in other clients remain.

In the composer, Up recalls older prompts; Down moves forward or restores the draft. Every submitted line enters history, refused commands included; `/api`
keeps only its key name and `/loop` turns are skipped. The session log keeps the
latest 50 across restart; legacy logs seed it and compaction keeps it. Session switches and logout reset navigation; the browser stores nothing.

The session store retains at most 500 messages per conversation, dropping the
oldest whole exchanges after the first exchange or its summary. A model reply
that opens with `[session-compacted]` is stored with a leading space, so only
compaction writes a summary. Local prompt
history uses that window; [compaction](#local-history-compaction) reduces
overflow against the configured token budget, preserving the goal and recent messages. Stored token totals can therefore
exceed what is sent to the model.

Runtime homes, `sessions/` directories and dotenv files are Git-ignored in this
repository, and the [hook gate](GITHOOKS.md#cgagentharness-specifics) blocks
them; keep a custom home outside the checkout and never `git add -f` private files.

```text
/loop 3
/loop 3 compare hvac vendors
/loop auto --web fetch 2 summarize the release notes
/loop stop
/goal clear
```

Bare `/loop` defaults to three turns within the configured cap.
[`loop.*` limits](CONFIG_RELOAD.md) reload; `api.harness_loop_rate_limit.max_tokens`
bounds each reply. When both configured caps are zero or absent, `/status`
and loop preflight warn: set a finite cap, then **Reload limits**. The warning
never blocks; requested turn counts still apply.
Text after the count sets the goal; otherwise a goal-less loop takes the last
prompt. Manual mode pauses between turns; `/loop` continues. `/loop auto` toggles automatic continuation; turning it
on with a goal (or last prompt) to follow starts the loop after 5 s, and so does
`/goal <text>` while auto is on. `/loop auto` with a count, mode or goal turns
auto on and starts after the same pause, storing nothing until then. A turn
that needed compaction continues the same way automatically. `/loop stop`
cancels generation, cooldown or a pending start. Goal clear, session switching, token budgets, a 90%-identical reply or
`loop.max_failures` consecutive failed turns (default 2; auto retries once) stop it. A `GOAL_DONE` line (even `**GOAL_DONE**`) stops it
too, but is only model advice.

With `server_loop` in `/api/status`, auto loops outlive tabs. The console reads
`GET /api/loop/events/<run_id>`, reattaches using `/api/loop/status?session_id=`,
and stops through `/api/loop/stop`. `POST /api/loop/start` accepts `session_id`,
optional `goal`/`web_mode`, and limits. Turns persist under the home's `loops/`.
Runs recheck account authority each turn and retain bounded, revalidated
[web evidence references](SECURE_RESEARCH.md#chat-web-tools) through compaction.
Manual, older-server, compaction-started and attachment loops remain browser-driven.

This loop does not edit files, run skills as programs, or run coding checks.
`--web` on `/goal` or `/loop` sets the chat tools of every turn while the goal is set; without it the goal takes `web.goal_web_default` (ships `all`, restart-only). `fetch` excludes search, `off` excludes web context and tools. Search listings never grant URL permission; URL permissions still apply. Preflight shows the mode.
A server restart does not resume a loop; goal-to-coding execution
is the separate [goal staging](CODING_PIPELINE.md#94-stage-a-session-goal-as-a-coding-task) workflow.

#### Search and export session transcripts

The Sessions pane's **Search my transcripts** box and per-row **Export .md**
buttons call guarded APIs, not `/session` subcommands.
`POST /api/sessions/search` takes `{"query":"exact phrase"}` and searches a
request-local Tantivy RAM index of at most 200 owned sessions and 2,000,000
bytes. Queries allow 200 characters; indexing uses up to 32 terms. At most 16
hits, two per session, 160-character snippets, literal case-insensitive
phrases across 1,200-character chunks. Search writes no web cache, sending
nothing off-machine; per-query rebuilding is accepted (#258). A hit opens its
session; session or account switches discard results.

`GET /api/sessions/{session_id}/export` returns Markdown, written to
`<home>/exports/{id}.md`; the pane offers a download. Both enforce owner and
CSRF; export includes bounded attachment metadata without bodies or filenames.

### 7.2 Default soul, effective prompt and persona editing

**Fresh homes leave the global persona unset.** The soul toggle remains available;
without `soul.md`, no persona is loaded and the interface displays **unset**.
Existing files and deliberate deletions are preserved. Select `/style soul` for
concise evidence-first communication without creating a global persona. The
original reference remains in `assets/soul.default.md`.
Planning and coding use the same public shipped communication guidance, beneath
their governed output contracts; private persona edits are not implicitly sent
to a cloud coding provider.

```text
/soul status
/prompt
/soul edit
```

`/soul status` reports enabled, present, loaded, truncated, and a safe failure
reason. `/prompt` opens a private snapshot of the next chat system prompt:
general-chat header, an operator-command guide and current inclusion settings,
selected optional prompt skills, enabled persona, the active output style, session
goal, explicitly injected web context while web is enabled, and enabled memory
notes. The API also reports source load state and limits; no credentials file is
included. Preview and editor responses require the existing API guards and use
`Cache-Control: no-store`. Review the preview before sharing; it is not a
transcript or the coding planner's prompt.

To create or change persona in the editor:

1. Enter bounded persona text, for example: “Use concise explanations. State
   assumptions and list the evidence needed to verify a proposed fix.”
2. Supply a reason for the edit and select **Preview prompt**.
3. Review the preview, select the confirmation checkbox and **Save persona**.
4. Close the editor, use `/soul on` if needed, then `/soul status` and `/prompt`
   to verify the next chat sees the text.

Edits save to `<harness home>/soul.md`; default maximum is 8,000 characters
(`personality.soul_max_chars`). Empty, oversized, invalid and critical instruction-
override content is refused. A stale revision cannot overwrite newer content:
reload the editor and review again. The fixed contract is not editable through
this dialog, and persona text never authorizes code execution.
`/soul off` disables inclusion without deleting the file.

**History and proposals:** replacement is atomic and saves the previous content
as a private SHA256-named backup. `/soul history` lists backup revisions; the
guarded API `GET /api/soul/document?version=<revision>` reads one for deliberate
restoration through the same preview/confirmed-edit process. `/soul propose` opens
a proposal-only editor: **Create proposal only** stores proposed text without
applying it and returns an ID. It accepts supplied or model-authored
text; it does not generate a new personality.

```text
/soul review <proposal-id>
/soul apply <proposal-id> <reason>
```

Review first. Apply confirms the displayed revision with the supplied reason;
there is no second dialog. `/soul reject <proposal-id> <reason>` preserves the
active persona. A changed base, changed proposal, or decided proposal is refused.
Backup and proposal storage holds 32 records without automatic deletion.

Apply records proposal and content hashes in `soul-pending-apply.json` before
replacement and removes the marker after saving proposal status.
Under the persona lock, startup and the next persona operation reconcile matching
candidate content as `applied`, unchanged base content as `pending`, and unrelated
content as `interrupted`. Recovery writes no persona text; review an interrupted
record and create a new proposal if needed.

Unreadable recovery storage blocks startup and persona operations: preserve
`soul.md`, `soul-history/` and the marker before manual repair. Recovery covers
process interruption, not power loss or concurrent external edits.

### 7.3 Runtime skills and Codex development skills

| Kind | Location / selection | What it does |
|---|---|---|
| Seeded coding context | `<home>/skills/ponytail` and `karpathy-guidelines` | Available through explicit `/skill use ponytail karpathy-guidelines`; not automatically loaded |
| Optional runtime prompt skill | `<home>/skills/<id>/SKILL.md`; `/skill use <id>` | Adds bounded context to this session's chat |
| Fixed check | `/skill check:cargo-test` | Selects a known check for an already staged coding request; no immediate execution |
| Governed catalog entry | `/skills all` | Inventory only unless an implemented adapter says otherwise |

To add optional context, create a local file in the **actual active home**, e.g.
`skills/review-notes/SKILL.md`. Use a normal directory and file within the home;
links cannot escape the skills-directory boundary. Example content:

```markdown
---
name: Review notes
---
When explaining a change, identify its observable behavior and a targeted check.
```

Then select its **directory ID**, not its display name:

```text
/skills all
/skill use review-notes
/skill status
/prompt
```

IDs allow lowercase ASCII letters, digits, hyphens and underscores, up to 80
characters. Selection replaces the session's optional list; supply all wanted IDs
in one `/skill use <id...>`. Up to four are retained, with defaults of 6,000
characters per body and 16,000 total. Frontmatter is stripped. These limits live
under `personality.prompt_skill_max_chars` and `personality.prompt_skills_total_chars`.

After chat, `/skill status` reports the last successful snapshot's IDs, character
counts, and SHA256 hashes; it can differ from current files and does not prove
execution. A missing, empty, unreadable, or invalid selected file blocks chat; restore it
or `/skill clear`. Clearing removes optional prompt bodies, seeded selections
included, but not files.

Repository development skills under `.claude/`, `.codex/` and `.github/` are
indexed in [AGENTS.md](../AGENTS.md#project-skills); they never enter app chat.

### 7.4 Customize response style

Use soul for your usual voice across sessions; an optional runtime skill styles
an explicitly selected task. Neither changes permissions or gives chat repository
access. Start with `/soul edit` and adapt this example:

```text
Lead with the answer. Use plain, direct language.
Default to 1–3 short paragraphs; expand when requested.
Use bullets for steps or comparisons, not every response.
Avoid praise, filler introductions and repeated summaries.
Distinguish verified results from assumptions and proposed actions.
Never sacrifice accuracy or necessary uncertainty for brevity.
```

Save through the [soul review flow](#72-default-soul-effective-prompt-and-persona-editing),
enable `/soul on`, and inspect `/prompt`. Compare a factual, troubleshooting, and
detailed question in `/session new Style check`; `/clear` is not a fresh session.
Style instructions guarantee neither length nor correctness.

For a session-specific alternative, put the writing rules in a
[runtime skill](#73-runtime-skills-and-codex-development-skills) and select it.
Avoid contradictory rules across soul, selected skills and memory.

**Style presets.** `/style <name>` selects a session preset;
`/style off` is the default (shown as Default) and `/style` prints
the active preset. Shipped: `code-review`, `concise`, `design`, `extract`,
`handoff`, `research`, `soul`, `technical`, `troubleshoot`, `verify`
([`data/styles/`](../data/styles/)). An overlay at
`$CGAGENTHARNESS_HOME/styles/<name>.md` wins. Selecting creates a session if needed.
Local chat and `/prompt` also carry a live status block and bundled-manual excerpts
(`diagnostics.self_knowledge`, `live_state`); [contract](../INVARIANTS.md#self-knowledge-never-widens-authority).
Prompt order is soul, style, then fixed policy; style cannot
override the contract. `/prompt` reports load failures and omits an unreadable,
empty, missing, or scanner-refused overlay.

`concise` is brief, `technical` traces mechanism, `research` grades evidence,
`code-review` leads with blockers at file:line, `design` weighs tradeoffs,
`troubleshoot` ranks causes with one next command, `handoff` briefs another
agent, `verify` weighs one claim, `extract` uses supplied documents only.
These instructions guide, not rewrite, output; an explicit format request wins.
`off` removes only style; soul remains. Changes affect future turns only.
Compare across sessions; a loaded indicator proves no prose effect.

**The `unslop` planner probe** is not a chat preset: an optional local
coding-planner probe (`unslop.enabled: true` plus `metrics_path:
"logs/unslop.jsonl"`, restart) that counts 16 fixed phrases such as "delve" in
proposals and may nudge another iteration. It never rewrites an answer, applies
to cloud planners, or scores quality; there is no phrase list setting or
`/unslop` command ([probe](../src/agentic/unslop.rs)).

### 7.7 Tools and connectors: available versus catalog-only

`/tools` separates route registration, known enablement, and readiness. Listing
does not probe a model, invoke GitHub, or arm writes. `last_result: null` means
no invocation history; registered or legacy `wired` does not prove readiness.
`/tools mcp` shows external MCP declarations, read/write grants, network policy,
and process-group or unrestricted Windows Job Object exceptions. It does not execute a tool or prove the selected sandbox is available. See [MCP capabilities](MCP_CLIENT.md).
`/connectors` displays connector inventory; `/registry` is another. Neither
installs a connector or proves its prerequisites ready; the registry's tools
array is not a replacement for `/tools`.

| Capability | Current configuration / action | Actual boundary |
|---|---|---|
| Local model | `models.local_llm` in `config.yaml`; `/model` and `/model use <name>` | Chat uses a configured loopback service; selecting a tag does not download it |
| Public web text | Chat web tools and `/web` controls in [SECURE_RESEARCH.md](SECURE_RESEARCH.md#search-fetch-and-research) | Google listings, permitted URL fetch, page research and optional context injection |
| GitHub coding | Explicit `agentic.repo`, gates and prerequisites in [coding pipeline](CODING_PIPELINE.md); `/github` reports status | Separate governed child-process workflow; a chat reply does not execute Git commands |
| Local file context for coding | Stage `/agent read <repo-relative-path[#Lx-Ly]>` before confirmation | Bounded reads from the governed repository clone; no general Mac filesystem mount |
| `netconnect` | CLI `status`, `devices`, and budgeted `diag`; `/net` aliases; gates ship false | LAN panel is GET `/api/netconnect` (scope, passive devices, tier state). No scan button. `/net diag` calls GET `/api/netconnect/diag`. Empty scope refuses armed tiers. `/web` still applies |
| Log diagnostics | `/diag`; `diagnostics.logs_enabled` ships true; admin or `portal_auditor` | Bounded, redacted tails of allowlisted `<home>/logs/` files; one excerpt staged for the next local turn ([diagnostics](TROUBLESHOOTING.md#diagnostics)) |
| `fsconnect`, `sqlconnect` | Not implemented in this app | No slash command, datasource picker, filesystem indexing, database connection or YAML enable switch |
| `github-public`, `openai-compatible` catalog entries | Inventory only | Neither is a connector or an activation control; local model and fallback paths are configured separately |
| Runtime prompt skill | Home skill file plus `/skill use` | Prompt context, not an executable plugin or permission grant |

Upstream names do not establish support; copying settings does not implement a connector,
and the Setup folder chooser prepares offline Cargo inputs, not chat file
access. Remaining connector work is tracked in issues against the
[current connector inventory](../src/server/views.rs).

### 7.8 Slash-command quick reference

Angle brackets mean "replace with your value"; square brackets mark optional
arguments; `|` separates alternatives. The Commands pane folds the catalog
by family under a search box, topic filter and easily mistyped forms; `/help all` prints the same catalog, and `/help` opens a topic
overview filtered by a topic key or search words. A row's `?` opens the command's
manual: synopsis, arguments, examples, notes and related commands. Commands,
examples, suggestions and **Insert into composer** only insert the fixed prefix
(`/agent approve `), never execute.
Unknown flags, `--dry-run` and `--confirm` cannot authorize an action. These are
console commands, not a shell: nothing is expanded, substituted or run as a
script. Every command must be one line without control characters, U+FEFF, zero-width
spaces or bidi overrides; the parser refuses them before tokenization can split a line
differently from the console.

Memory commands require an exact `/memory` root and subcommand (case-insensitive;
ordinary spaces are allowed). `/mem`, typos and conversational forms such as
`/memory please retrieve preferences` only suggest; they never read memory, start
chat, toggle gates or write notes. Unknown subcommands are refused. Exact `/memory retrieve <query>`
starts a model turn with explicit retrieval; `/memory search <query>` only lists
candidates. Use `/memory` for status and `/help` for syntax. Exact save/remember
still require `:: <reason>`; exact clear/forget still delete pinned notes, so
inspect the command before sending.

Typed slash commands require the server parser; a parser failure leaves the
command unexecuted, and pending parses are discarded on a session, account or
transcript change. Exact `/loop stop` still cancels continuation while the
parser is unavailable. Inspect results after every state-changing command.

| Command family | Supported use |
|---|---|
| `/agent [help]` | Inspect coding commands; full workflow below |
| `/analytics` | Open retained session, token and coding-run [analytics](SPEND_AND_NOTIFICATIONS.md#analytics-sessions-tokens-and-coding-runs); no inference or repository operation |
| `/api`, `/api set <KEY> <value>`, `/api clear <KEY>` | Inspect, save or clear a managed credential; prefer the API Keys password fields; [persistence](INSTALL.md#8-persistence-optional-keys-and-recovery) |
| `/clear` | Clear visible output, staged coding/review state and chat continuation; saved chats, notes, persona and web context remain |
| `/connectors` | Connector catalog; does not activate connectors |
| `/diag [status]`, `/diag audit\|spend\|unslop\|errors [flags]`, `/diag inject\|forget` | Read allowlisted logs or stage one excerpt; aliases `/diagnostics`, `/logs`; [diagnostics](TROUBLESHOOTING.md#diagnostics) |
| `/github` | Read-only agentic GitHub status |
| `/goal`, `/goal [--web off\|fetch\|all] [text]`, `/goal clear` | Inspect, set or clear the saved session goal and its web mode |
| `/goal stage <branch>`, `/goal task` | Explicitly stage coding from a goal or inspect its task linkage |
| `/harness` | List retained optimizer runs; does not start optimization |
| `/help [topic\|search words\|all]` | Searchable topic guide, full command catalog and insertable examples |
| `/loop [auto] [--web off\|fetch\|all] [n] [goal text]`, `/loop stop` | Goal-driven chat continuation; goal text sets the goal first; `auto` toggles and starts after 5 s; exact `stop` also cancels a streaming chat turn |
| `/memory`, `status`, `on`, `off`, `add <note>`, `forget <id>`, `clear` | Shared pinned notes; [operator memory notes](MEMORY_GUIDE.md#pinned-notes) |
| `/memory capture\|recall\|retrieval\|auto-retrieve\|consolidation\|auto-consolidate\|auto-suggest-chat\|auto-suggest-coding on\|off` | Administrator-only structured-memory gate overrides; `on` or `off` is required. `/memory on` stays pinned notes |
| `/memory consolidate <episode-id> [episode-id ...]` | Selected-episode consolidation into pending proposals; does not apply facts |
| `/memory proposals` | Open the Memory panel; Apply/Reject requires review and a reason |
| `/memory remember <sentence> :: <reason>` | Confirm a semantic summary on the latest completed episode; no fact write |
| `/memory facts`, `/memory retrieve <query>`, `/memory search <query>` | List your active facts, force fact retrieval for this chat prompt, or inspect candidates without injection |
| `/memory save <text> :: <reason>` | Confirm an immediate private fact write |
| `/model [status]`, `/model list`, `/model use <name>` | Inspect/list/select a local chat model or explicit cloud provider; does not select the coding planner |
| `/net status`, `/net devices` | Read-only scope and gates, or passive neighbors. Aliases and refused tier words: [netconnect](netconnect.md) |
| `/prompt` | Private preview of the next chat system prompt |
| `/registry` | Combined skill, tool and connector catalog |
| `/session`, `list`, `info`, `new [title]`, `rename <title>`, `use <id>` | List sessions, inspect the current session, create, rename or reopen one |
| `/skill`, `status`, `use <id...>`, `clear` | Inspect, replace or clear session prompt-skill selection |
| `/skill check:<profile>` | Select one fixed check for an already staged coding request |
| `/skills [all\|help\|<name>]` | Inspect wired skills, the full catalog including optional prompt files, help, or skills matching a display name (which can differ from the `/skill use` directory ID) |
| `/soul`, `status`, `on`, `off`, `edit`, `history`, `propose` | Inspect, toggle or open persona editing/proposal flows |
| `/soul apply <id> <reason>`, `reject <id> <reason>`, `review <id>` | Review a retained proposal before explicitly applying or rejecting its exact revision |
| `/status` | Runtime status and unlimited-loop warning; account access required |
| `/style`, `/style <name>`, `/style off` | Inspect/select a session output style, or clear it without changing soul.md |
| `/tokens` | Current session token tally |
| `/tools [all\|help\|mcp\|<name>]` | Inspect registered tools, the full catalog including unwired entries, help, external MCP grants, or one named tool |
| `/users` | Administrator account panel; [accounts](SECURE_RESEARCH.md#accounts-and-roles) |
| `/web [status\|help]`, `/web on`, `/web off` | Inspect shared-home rules; administrators enable/disable web |
| `/web allow <pattern> [pattern ...] [--group name] [--seed URL ...]`, `/web deny <id-or-pattern>` | Administrator grants are atomic; repeat `--seed` per concrete seed. Every seed must fit a requested rule |
| `/web mode allowlist\|denylist`, `/web block <pattern-or-host> [...]`, `/web unblock <id-or-pattern>` | Administrator. The header globe (red X) switches modes after a confirmation dialog. `denylist` reads every public URL except blocked rules; a host with no path blocks it and its subdomains. Both lists persist across switches |
| `/web check <URL> [URL ...] [--group name]` | Exact permission diagnostic, with no network request or grant |
| `/web cancel`, `/web fetch <URL> [URL ...] [--group name] [--crawl]`, `/web forget`, `/web inject` | Cancel research; fetch permitted URLs (`--crawl` follows links); clear/select extracts |
| `/web job [id]`, `/web jobs` | Show the newest or one owned detached research job, or list them; jobs outlive the tab, not a restart ([research](SECURE_RESEARCH.md#fetch-discovery-and-research)) |
| `/web pages [--group name] <query>`, `/web research [--group name] [--url URL ...] <question>` | Permitted-page discovery or local-model research; repeat `--url` per starting point; no new authority |
| `/web search [--count 1..10] [--engine google\|pages] [--group name] <query>` | Google listings by default; a group selects permitted-page search |

Web value flags accept `--flag value` or `--flag=value`; `--help` opens help
without mutation. In queries `-tokio` is text; put `--` before words starting
`--`. Legacy `group=name` and `allow PATTERN GROUP SEED` remain accepted.
Unknown, repeated or missing-value flags refuse; fetch takes URLs, not prose.
Memory `save` and `remember` split text from the required reason at the final `::`.

Misspellings only suggest: ambiguous `/skil` offers both `/skill` and
`/skills`; edit the input and submit the intended command yourself. Supported
conversational prefixes preserve the following query words: `/web please search
The Who` becomes `/web search The Who`. Existing normalization still collapses
repeated whitespace. Unsupported fixed arguments such as `/memory clear --help`
remain unexecuted; use the help commands above to inspect syntax.

The `/agent` family operates the separate coding workflow:

| Command | Effect |
|---|---|
| `/agent run <branch> <instruction>` | Stage a request for inspection; does not start it |
| `/agent checks [profiles]` | List fixed profiles, or select space/comma-separated profiles |
| `/agent iterations <n>` or `clear` | Set a staged iteration cap from 1–10, or restore the configured default |
| `/agent pr <number>` or `/agent issue <number>`; `clear` | Select one context source; choosing one clears the other |
| `/agent plan` or `/agent plan clear` | Open a file chooser for a supplied plan, or clear it; does not generate a plan |
| `/agent read <repo-relative-path[#Lx-Ly]>` or `clear` | Stage up to 8 bounded file-context declarations, or clear them. A local planner may later emit `=== READ path ===` (cap 6, next iteration; not a slash command). Denied basenames (`.env`, keys, credentials, …) refuse operator and model reads alike; a cloud planner refuses model-requested and [retrieval](CODING_PIPELINE.md#optional-repository-retrieval) reads. The deny-list is not a secret scanner. |
| `/agent cancel` | Clear the staged request; does not stop an already submitted job |
| `/agent confirm <reason>` | Explicitly submit the staged request through the execution gates |
| `/agent jobs`, `/agent job [job-id]`, `/agent stop [job-id]` | List, monitor or request cancellation; an omitted ID uses the retained job from the URL |
| `/agent runs`, `/agent status <run-id>` | List runs or inspect a run and its diff |
| `/agent approve <run-id> <reason>` | Review then approve the exact candidate; repeat only after reading a newly displayed diff |
| `/agent reject <run-id>` | Reject a candidate |
| `/agent push <run-id> <reason>` | Separately authorize pushing the approved commit |
| `/agent pr-body <run-id>` | Choose a reviewed PR body file |
| `/agent publish <run-id> <reason>` | Separately authorize PR publication with the selected body |
| `/agent discard <run-id>` | Clean up a terminal run's owned clone |

Fixed check profiles map to these commands; selection does not install their
prerequisites or run them immediately. Native Cargo verification additionally
uses the prepared offline sandbox described in [coding pipeline](CODING_PIPELINE.md).

| Profile | Fixed command |
|---|---|
| `cargo-test` (default) | `cargo test --quiet` |
| `cargo-clippy` | `cargo clippy --all-targets -- -D warnings` |
| `cargo-fmt` | `cargo fmt --check` |
| `pytest` | `python3 -m pytest -q --tb=short` (`python` on Windows) |
| `ruff` | `python3 -m ruff check --select E,F,I,B,C4,UP,S .` (`python` on Windows) |

Arbitrary shell check commands are not accepted. Python profiles need their own
prepared tools/dependencies; the Cargo preparation helper does not install them.

### Text-file attachments and prompt preview

The paperclip beside Send chooses up to three `.txt`, `.md`, `.json`, `.csv`, or
`.log` files, each at most
15 MiB. `/prompt` uploads and previews their fenced, untrusted text; persona
Preview uses the same pending files. Previews reuse uploads, and the next message
sends their IDs once. Failed preview retains them; session or account change
clears them. Uploads count toward the home quota until session or owner cleanup.

Sessions retain at most 12 owner-scoped blob IDs. The next local turn may
BM25-search them and inject matching passages into the attachment fence.
`.md` and `.txt` notes ingested at `/api/notes-corpus` use a separate home jail
but share that local prompt budget (`source=notes_corpus`). Local loops include
these sources; cloud chat and `/agent` refuse attachment IDs or live pins with
`ATTACHMENT_SURFACE_FORBIDDEN` and never receive notes-corpus bytes. `/session unpin` clears session references, keeping blobs.
Loop startup reports live pins, web availability, uploads.
A repeating refusal (no URL permitted, keyless search, no budget for a round)
stops continuation; one failed read is reported and the loop continues
until two turns' reads all failed. A browser-driven loop waits out a
rate limit's `retry_after` (up to 300 s; `/loop stop` ends the wait); a
server-side run counts it as a failed turn.
Unsupported formats are refused; clipped sections say they are incomplete.

The notes corpus has multipart ingest, list, and delete
[routes](API_ROUTES.md#chat-model-and-chat-sessions), but no console uploader.
A local message supplies the query; `/prompt` shows assembled context. Notes are
separate from structured facts and pinned notes. Ingest honors
`notes_corpus.max_files_per_request` (default 8, range 1–16). The HTTP body
cap is that count times `max_file_bytes` plus 256 KiB of multipart overhead.
Attachment uploads still cap at three files per request.

### DOCX attachments

The file picker also accepts `.docx`. Only UTF-8 main-document text is read:
paragraphs, tables, explicit tabs and breaks. Directly hidden/deleted runs and
field instructions are excluded. Headers, footers, images/OCR, style-based
visibility, macros and embedded files are not interpreted; relationships and
URLs are never followed. Extraction is a bounded text view, not a reproduction
of Word's layout. PDF remains unsupported.

DOCX uses the same 15 MiB/file, three-file request, home quota, owner checks,
injection scan, private UUID storage and local-chat-only fence as text uploads.
Magic bytes, package layout, content type and XML namespaces must agree. The
reader refuses ZIP64/split archives, ambiguous entries, trailing payloads,
custom entities/DTDs, non-UTF-8 XML and malformed or empty documents. Fixed
safety ceilings: 256 parts, 256 KiB directory, 16 MiB declared expansion,
64 KiB content-types XML, 1 MiB document XML/output, and 128 XML levels.
A two-second cooperative deadline bounds parsing in process. Prompt clipping is
separate and remains explicitly labeled.

### Streaming chat and cancellation

The console requests `POST /api/chat` with `Accept: text/event-stream`; clients
without it retain JSON responses and HTTP errors. Both modes share authentication,
CSRF, generation gates, web tools, memory selection, and exchange persistence.

Each SSE `data` field contains one JSON event:

- `{"type":"delta","text":"..."}`: provisional model text.
- `{"type":"done","data":{...}}`: the existing complete chat response, including
  session identity, usage, tally, web events and memory metadata.
- `{"type":"error","status":502,"error":{"detail":{...}},"headers":[]}`:
  a typed failure after SSE response headers have been sent. The event carries
  the application status and any retry headers; the HTTP stream itself is 200.

The console shows deltas, then replaces them with the `done` answer. Failed or
cancelled partial text is removed and not persisted. Streams require a completion
reason and `[DONE]`; malformed or truncated output fails. The final event supplies
usage. Valid JSON fallback remains compatible but arrives as one answer.

`/loop stop` (`POST /api/chat/cancel`) cancels the account's chat or browser-driven loop turn; a server-side run is halted with `POST /api/loop/stop`.
Closing the stream aborts its task, releases claims, and drops model and web
requests. Each ChatClient call has an independent cancellation record. Socket
disconnect does not claim immediate GPU release.

Tool calls are accumulated and validated before dispatch. Owner and URL
permission are rechecked before each evidence-derived delta and final delivery.
Revocation cannot retract delivered text. Model traffic has a 4 MiB ceiling and
request deadline; a bounded channel applies backpressure within that deadline.

`tests/chat_stream.rs` and `scripts/chat-browser-acceptance.mjs` pin the
protocol with synthetic fixtures, not a real model's quality or cancellation timing.

### Local history compaction

Before local chat or `/loop`, the harness estimates the next prompt plus
one reply. Above
`chat.compact_prompt_tokens` (default 24000), it summarizes middle turns,
keeping the first user message, goal, and whole recent exchanges (at most
`chat.compact_keep_messages`, seeded 10, and `chat.compact_tail_percent`, 25% of the
trigger). With `chat.compact_follow_window: true`, an `auto_tune`-verified
window above 32768 scales the trigger within `chat.compact_max_prefill_sec`
(seeded 1500) of measured prefill; the seed uses false. The summarizer reads goal and first message, keeps the prior summary
whole, clips other turns at 800 characters within its budget, else history
stays. The model gets a lossy note yielding to
exact messages; unsourced codes become `[unverified]`.

Reply reservation is `max_tokens` for resolved Ollama with explicit
`reasoning_effort: "none"`, or twice that otherwise: a conservative
margin, not provider accounting. The trigger counts one reply on every
backend; a doubled reservation can still compact sooner, at the hard limit. Startup accepts at most 25904 reply tokens on
the first path or 12952 on the second for chat and `/loop`. The trigger floor,
web-chat tightening and the calibrated bytes/4 estimate
are specified in
[INVARIANTS.md](../INVARIANTS.md#local-chat-compaction-preserves-goal-and-the-first-user-turn).
Calibration cannot predict language shifts or ratios above 3; verify the
model window per [MODELS.md](MODELS.md).

`compaction.summary_max_tokens`: seeded 2048, missing-key fallback 768, clamped 128–2048.
Edit home `config.yaml`, then restart. Compaction is inline;
empty, failed, truncated, or cancelled output commits neither summary nor
calibration. Past the trigger a turn may still send up to the hard limit, the
prompt cap (web chat: the web room): an empty or cut-off summary, or too little
to summarize, sends it uncompacted, and the response's `compaction_skipped`
says why. A new message alone must fit the trigger. A prompt over its limit returns
`CHAT_PROMPT_TOO_LARGE` (422) naming the bounding setting (`details.limit_source`),
the turn's reply budget (`models.local_llm.max_tokens`, or
`api.harness_loop_rate_limit.max_tokens` for `/loop`) and only remedies that can help.
A new session cannot help when the system prompt and reply reservation fill the limit.
