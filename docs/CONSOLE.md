# Chat, soul, skills and slash commands

This page covers chat, sessions, soul, skills, style, and commands.
See the [index](../README.md), [memory setup](MEMORY_GUIDE.md), and
[web operations](SECURE_RESEARCH.md#search-fetch-and-research).

Chat continuation and coding jobs differ: `/loop` follows a session goal,
while `/agent` drives the [coding pipeline](CODING_PIPELINE.md). A goal, skill,
persona, or reply never authorizes commands, commits, pushes, or publication.

## What you can do

| Capability | How it works |
|---|---|
| Chat and sessions | Create, rename, and revisit separate conversations with saved messages and token counts. New Session replaces the transcript; the confirmed Clear my session history control deletes saved conversations. Derived structured-memory episodes remain unless the operator also confirms that cascade. |
| Persona, memory and prompt context | Inspect `/prompt`, manage shared `soul.md`, select session prompt skills, and save literal `/memory` notes. `/memory on` includes only those notes. Account-private structured facts, proposals, episodes, facts-only FTS, and consolidation use separate gates. Models propose but never approve. Search does not inject. Only `selected_facts`, `/memory retrieve`, `retrieve`, or `auto_retrieval` adds facts. Episodes never enter prompts, and consolidation writes only pending proposals. |
| Chat model selection | Select an installed local tag, or select `grok` or `claude` after an administrator saves its key and restarts. The unvalidated name persists. Cloud chat sends only the new message; local history, memory, skills, and web context stay local. `/loop` refuses cloud selection (`CLOUD_CHAT_LOOP`). [Cloud defaults](INSTALL.md#tunables-you-may-want-to-know-about). |
| Chat continuation | `/goal` and `/loop` provide bounded follow-up turns with request limits, completion-token budgets, cancellation, and optional auto-continue. |
| Tool visibility and use | `/skills` and `/tools` distinguish registered adapters from readiness and execution evidence. Console commands invoke backend operations through fixed, validated interfaces; model prose does not become an arbitrary shell command. |
| Chat web tools | Natural-language Google search and permitted URL fetch. API Keys accepts `SERPAPI_API_KEY`; no active key selects public Google. Actual tool outcomes and source links appear in chat. |
| Web research | Grant exact or wildcard URL access for bounded discovery and BM25 search. `/web research` returns local-model answers with checked quotes, usage, and partial coverage. Fresh homes enable web with an empty allowlist; selection and injection are account scoped. |
| Accounts and API Keys | Signed out, only a sign-in card shows; fresh `admin` / `admin` requires password replacement. Administrator, Portal operator and Auditor permissions are enforced on API reads and writes. Administrators manage masked saved/active credentials in API Keys. |
| Coding loop | Stage a task and files or plan, then confirm an isolated run that proposes bounded edits, runs fixed sandboxed checks, and feeds results into later attempts. |
| Review and publication | Inspect retained run status and diffs, approve the reviewed tree for a local commit, then separately push and publish a draft PR with a reviewed repository template. |
| Analytics | After sign-in, Analytics or `/analytics` opens filterable, paged Tokens, Sessions, and Code sections; sessions also sort. Completeness warnings persist. [Interpretation and API](ANALYTICS.md). |
| Spend | Analytics' Tokens and cost tab: read-only retained provider/model/day usage, available USD, completeness warnings, pagination and **Estimate draft**. [Build requirements and interpretation](SPEND_AND_NOTIFICATIONS.md). |
| Completion webhooks | Metadata-only notifications for terminal detached jobs, off until configured; the **Job webhooks** button shows only then. Delivery grants no job or repository authority. [Setup](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook). |
| Recovery | Rediscover retained jobs and runs after reopening. Worker leases distinguish active work from interrupted runs; reopening does not automatically resume work or replay a publication. |

## 7. Chat, soul, skills and goals

Fresh chat has no assigned repository or automatic coding skills. When web is
enabled, local chat can dispatch only the bounded read-only `web_search` and
`web_fetch` tools; `/loop` remains tool-free. Chat cannot inspect local files or
certify live wiring. Use command results as evidence and the coding workflow for
execution.

Existing homes retain repositories and skill files; chat neither loads nor edits
legacy coding skills automatically. For an isolated test, start `/session new`,
use `/skill clear`, inspect `/memory` and `/soul status`, then `/prompt`. A new
session still uses enabled shared notes and persona.

Enter slash commands in chat. `/help` lists installed commands. Begin with
`/status`, `/model`, `/skills all`, and `/tools`; registration is not
[readiness](#77-tools-and-connectors-available-versus-catalog-only).

### 7.1 Sessions and bounded chat continuation

```text
/session new Setup check
/goal Explain this project's test strategy
/goal
```

Send a question and confirm a reply. `/tokens` and the header's
**session · tokens** count show the selected session's tally
([interpretation](SPEND_AND_NOTIFICATIONS.md#read-spend-without-mistaking-missing-data-for-zero)).

Creating or switching sessions replaces the visible transcript, stops chat and
the browser's generation wait, discards delayed replies, and clears staged
coding or persona reviews. Switching restores retained messages without adding
duplicates. A new session has no messages, goal, or prompt skills. Persona and
enabled notes remain shared within the home. Your account's web selection and
injected context survive session changes. Reset controls have different scopes:

| Intent | Control | Retained data |
|---|---|---|
| Clear the visible output | `/clear` | Saved messages, session ID and goal remain; later chat still receives recent history. Hidden staged reviews and loop state clear. |
| Start a separate conversation | **+ new session** or `/session new <title>` | Old sessions, shared persona/notes/model selection, and your account's web selection remain. |
| Delete my saved conversations | **Clear my session history**, immediately below **+ new session** | Shared notes/persona/web, model configuration, coding runs and audit records remain. |

In the dialog, choose **Delete my session history** to confirm or **Cancel**.
Confirmation stops active chat, clears the conversation, and deletes your saved
sessions, goals, skill selections, and token totals. It retains other accounts,
unassigned legacy records, and unreadable files. Late replies cannot recreate a
deleted session. Afterward, send a message or use `/session new`. If storage
fails, resolve the reported problem before retrying because deletion may be partial.

Deletion is not secure erasure. Backups, model-service copies, and text loaded in
other clients remain; close or refresh those clients separately.

In the composer, Up recalls older prompts and Down moves forward or restores the
draft. The active session's local JSON log retains the latest 50 persisted
prompts across restart. Legacy logs seed the list from user messages, and
compaction does not erase it. Session switches and logout reset navigation. The
browser stores nothing persistently. Failed submissions and unsaved slash
commands do not enter prompt history.

The session store retains at most 500 messages per conversation. Local prompt
history uses that retained window; [compaction](#local-history-compaction) reduces
overflow against the configured token budget while preserving the goal and recent messages. There is
no separate 20-message/8,000-character history clip. Stored token totals can
therefore exceed the compacted context sent to the model.

Runtime `sessions/` directories, named `.CGagentHarness/` homes and dotenv files
are Git-ignored in this repository. Avoid `git add -f` for private files: ignore
rules do not remove files already tracked by Git and are not global Git policy.
A custom home with another name can expose notes, web extracts, or coding
records. Keep the whole home outside the checkout. Before publication, these
checks must show matching ignore rules and no tracked runtime files:

```bash
git check-ignore .CGagentHarness/config.yaml example-home/sessions/example.json .env
git ls-files -- ':(glob)**/sessions/**' ':(glob)**/.CGagentHarness/**' '.env' '.env.*' ':!.env.example'
```

```text
/loop 3
/loop
/loop stop
/goal clear
```

`/loop 3` runs up to three follow-up turns toward the goal (default three,
maximum five), each restating 240 goal characters. Manual mode pauses after each
turn; `/loop` continues. `/loop auto` toggles automatic continuation, before
starting or to resume. `/loop stop` cancels during generation or cooldown. Goal
clear, session switching, rate limits, failures, token budgets and a
90%-identical reply stop it. A `GOAL_DONE` line (even `**GOAL_DONE**`) stops it
too, but is only model advice.

This loop does not edit files, run skills as programs, or perform coding checks.
The goal persists; continuation counters and auto state are page state.
Refresh/restart does not resume an unattended loop. Goal-to-coding execution is
an explicit separate workflow in [goal staging](CODING_PIPELINE.md#94-stage-a-session-goal-as-a-coding-task).

#### Search and export session transcripts

Search and export are guarded APIs, not `/session` subcommands.
`POST /api/sessions/search` accepts `{"query":"exact phrase"}` and returns owned
session snippets. Its request-local Tantivy RAM index covers at most 200 sessions
and 2,000,000 bytes. Queries allow 200 characters; indexing uses up to 32 terms.
Results contain at most 16 hits, two per session, with 160-character snippets.
Overlap finds case-insensitive literal phrases across 1,200-character chunks. Search runs on
the blocking pool, writes no web cache, and sends nothing off-machine. Rebuilding per
query is accepted within these bounds; measure representative latency before
introducing a cache (#258).

`GET /api/sessions/{session_id}/export` returns Markdown and writes it to
`<home>/exports/{id}.md`. Both APIs enforce owner and CSRF. Export includes
bounded attachment metadata without bodies or filenames.

### 7.2 Default soul, effective prompt and persona editing

**Fresh homes seed the bundled CG Agent persona and enable the soul toggle.**
The bundled response contract asks for plain text in short paragraphs and uses
Markdown only when requested. It guides the model but does not rewrite output.
Existing homes, custom files and deliberate deletions are preserved. Missing soul
means no persona file loaded; the base general-chat prompt still operates.
Planning and coding use the same public shipped communication guidance, beneath
their governed output contracts. Private local persona edits are not implicitly
sent to a cloud coding provider. The two seeded coding skills are
optional context and are not injected automatically. A missing persona is not fetched
from CyClaw or Codex automatically.

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
`Cache-Control: no-store`. Review the preview before sharing. It is not a
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
applying it and returns an ID. This accepts text you supply, including
model-authored text; it does not automatically generate a new personality.

```text
/soul review <proposal-id>
/soul apply <proposal-id> <reason>
```

Review first. Apply confirms the displayed revision with the supplied reason;
there is no second dialog. `/soul reject <proposal-id> <reason>` preserves the
active persona. A changed base, changed proposal, or decided proposal is refused.
Backup and proposal storage holds 32 records without automatic deletion.

Before replacement, Apply records proposal and content hashes in
`soul-pending-apply.json`, then removes the marker after saving proposal status.
Under the persona lock, startup and the next persona operation reconcile matching
candidate content as `applied`, unchanged base content as `pending`, and unrelated
content as `interrupted`. Recovery writes no persona text. Review an interrupted
record and create a new proposal if needed.

Unreadable or unwritable recovery storage blocks startup and persona operations. Before manual
repair, preserve `soul.md`, `soul-history/`, and `soul-pending-apply.json`; do not
delete the marker blindly. Recovery covers process interruption, not power loss
or external concurrent edits. Older unmarked interruptions need manual review.

### 7.3 Runtime skills and Codex development skills

| Kind | Location / selection | What it does |
|---|---|---|
| Seeded coding context | `<home>/skills/ponytail` and `karpathy-guidelines` | Available through explicit `/skill use ponytail karpathy-guidelines`; not automatically loaded |
| Optional runtime prompt skill | `<home>/skills/<id>/SKILL.md`; `/skill use <id>` | Adds bounded context to this session's chat |
| Fixed check | `/skill check:cargo-test` | Selects a known check for an already staged coding request; no immediate execution |
| Governed catalog entry | `/skills all` | Inventory only unless an implemented adapter says otherwise |
| Codex development skill | Repository `.codex/skills` or Codex personal skill directory | Guides Codex maintaining the repository; not automatically an app runtime skill |
| Claude Code development skill | Repository `.claude/skills` | Guides Claude Code maintaining the repository; not an app command |

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
counts, and SHA256 hashes. It can differ from current files or selection and does
not prove execution. Use `/agent job <id>` or `/agent status <run-id>` for coding
evidence. A missing, empty, unreadable, or invalid selected file blocks chat;
restore it or use `/skill clear`. Clearing removes optional prompt bodies,
including seeded selections, but does not delete files.

For repository maintenance, use the development-skill entrypoints in
[AGENTS.md](../AGENTS.md), such as `$cgagentharness-optimize` or
`$fable-protocol`; Claude Code has corresponding slash commands. If an agent does
not list one, reference its checkout `SKILL.md`. Copying a file does not load it.
Development skills neither enter app chat nor authorize publication.

### 7.4 Customize response style

Use soul for your usual voice across sessions; use an optional runtime skill for
an explicitly selected task style. Neither changes the app's permissions or gives
chat access to a repository. Start with `/soul edit` and adapt this example:

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

For a session-specific alternative, create `<home>/skills/concise/SKILL.md` using
the file format in [runtime skills](#73-runtime-skills-and-codex-development-skills) and put the desired writing rules in its body.
Select `/skill use concise`, then inspect `/prompt`. To combine it with another
skill, supply both IDs in the same command. Avoid contradictory rules in soul,
selected skills and memory: a “give full detail” instruction can conflict with a
“one sentence only” instruction.

For customization, compare three disliked responses with your preferred rewrites
and explain each difference. Use observable rules such as "answer before
explanation" or "no repeated closing summary" instead of "sound human."

**Style presets.** `/style <name>` selects a session preset;
`/style off` is the default and `/style` prints
the active preset. Shipped: `code-review`, `concise`, `design`,
`research`, and `technical`
([`data/styles/`](../data/styles/)). An overlay at
`$CGAGENTHARNESS_HOME/styles/<name>.md` wins. The selector creates a session if needed.
Prompt order is soul, style, then fixed policy, so style cannot
override the contract. `/prompt` reports load failures and omits an unreadable,
empty, missing, or scanner-refused overlay.

`concise` is brief, `technical` traces mechanism, `research` grades evidence,
`code-review` leads with blockers at file:line, `design` weighs tradeoffs. These model
instructions do not deterministically rewrite or limit
output. An explicit format request wins. `off` removes only style; soul remains.
Changes affect future turns without rewriting history. Compare the same question,
model, and settings in a new session; a loaded indicator alone proves no prose effect.

**The `unslop` planner probe.** This optional local coding-planner probe is
not a chat preset. It excludes proposed file bodies when possible,
then records a response hash and counts 16 fixed case-insensitive phrases such
as "delve" and "game-changer." It may nudge another ordinary loop iteration but
never rewrites an answer, forces an iteration, or applies to cloud planners. A
candidate can pass with hits; phrase matching is not a quality score.

If you already use the governed local coding workflow and want that probe,
merge this into the existing active home's `config.yaml`, then restart:

```yaml
unslop:
  enabled: true
  metrics_path: "logs/unslop.jsonl"
```

It ships disabled. Metrics follow bounded JSONL policy. There is no configurable
phrase list or `/unslop` command. Use persona or skills for chat; this setting
adds no chat rewrite and does not justify repository writes.
See [the current probe](../src/agentic/unslop.rs) and
[its loop integration](../src/agentic/real_repo_loop.rs).

### 7.7 Tools and connectors: available versus catalog-only

`/tools` separates route registration, known enablement, and readiness. Listing
does not probe a model, invoke GitHub, or arm writes. `last_result: null` means
no invocation history; registered or legacy `wired` does not prove readiness.
`/tools mcp` shows external MCP declarations, read/write grants, network policy,
and explicit process-group or unrestricted Windows Job Object exceptions. It does not execute a tool or prove that
the selected sandbox is available. See [MCP capabilities](MCP_CLIENT.md).
`/connectors` displays connector inventory; `/registry` is another inventory
view. None of these commands installs a connector or proves its prerequisites
are ready. The registry's tools array is not a replacement for `/tools`.

| Capability | Current configuration / action | Actual boundary |
|---|---|---|
| Local model | `models.local_llm` in `config.yaml`; `/model` and `/model use <name>` | Chat uses a configured loopback service; selecting a tag does not download it |
| Public web text | Chat web tools and `/web` controls in [SECURE_RESEARCH.md](SECURE_RESEARCH.md#search-fetch-and-research) | Google listings, permitted URL fetch, page research and optional context injection |
| GitHub coding | Explicit `agentic.repo`, gates and prerequisites in [coding pipeline](CODING_PIPELINE.md); `/github` reports status | Separate governed child-process workflow; a chat reply does not execute Git commands |
| Local file context for coding | Stage `/agent read <repo-relative-path[#Lx-Ly]>` before confirmation | Bounded reads from the governed repository clone; no general Mac filesystem mount |
| `fsconnect` | Not implemented in this app | No slash command, datasource picker, filesystem indexing or YAML enable switch |
| `netconnect` | Passive CLI `status` and `devices`; `/net` read-only aliases; gates ship false | LAN panel is GET `/api/netconnect` (scope, passive devices, tier state). No scan button. Empty scope refuses armed tiers. `/web` still applies |
| `sqlconnect` | Not implemented in this app | No database connection configuration, query tool or ingestion workflow |
| `github-public` catalog entry | Inventory only | Does not provide an independent public-repository connector |
| `openai-compatible` catalog entry | Inventory only as a connector | Separately configured local compatible model/fallback paths exist; the row is not an activation control |
| Runtime prompt skill | Home skill file plus `/skill use` | Prompt context, not an executable plugin or permission grant |

Upstream names do not establish support. There is no `/fsconnect` or
`/sqlconnect`; `/netconnect` is a read-only `/net` alias, not a scan control.
See [netconnect](netconnect.md). Copying settings does not implement a connector,
and the Setup folder chooser prepares offline Cargo inputs rather than chat file
access. Remaining connector work is tracked in issues, checked against the
[current connector inventory](../src/server/views.rs).

### 7.8 Slash-command quick reference

Angle brackets mean "replace with your value"; square brackets mark optional
arguments; `|` separates alternatives. The Commands pane folds the alphabetical
catalog by family under a search box, a topic filter and a short list of easily
mistyped forms; `/help all` prints the same catalog, and `/help` opens a topic
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
chat, toggle gates or write notes. Unknown subcommands are refused. Retype the
intended command after reviewing a suggestion. Exact `/memory retrieve <query>`
starts a model turn with explicit retrieval; `/memory search <query>` only lists
candidates. Use `/memory` for status and `/help` for syntax. Exact save/remember
still require `:: <reason>`; exact clear/forget still delete pinned notes, so
inspect the command before sending.

Typed slash commands require the server parser; a parser failure or invalid reply
leaves the command unexecuted, so retry or inspect the pane. Pending parser
responses are discarded after a session/account change or clearing the transcript.
Exact `/loop stop` still cancels chat continuation while the parser is unavailable;
extra words disable that local shortcut. Inspect results after every
state-changing command. The [coding pipeline](CODING_PIPELINE.md) describes
confirmation and persistence.

| Command family | Supported use |
|---|---|
| `/agent [help]` | Inspect coding commands; full workflow below |
| `/analytics` | Open retained session, token and coding-run [analytics](ANALYTICS.md); no inference or repository operation |
| `/api`, `/api set <KEY> <value>`, `/api clear <KEY>` | Inspect, save or clear a managed credential; prefer the API Keys password fields; [persistence](INSTALL.md#8-persistence-optional-keys-and-recovery) |
| `/clear` | Clear visible output, staged coding/review state and chat continuation; saved chats, notes, persona and web context remain |
| `/connectors` | Connector catalog; does not activate connectors |
| `/github` | Read-only agentic GitHub status |
| `/goal`, `/goal <text>`, `/goal clear` | Inspect, set or clear the saved session goal |
| `/goal stage <branch>`, `/goal task` | Explicitly stage coding from a goal or inspect its task linkage |
| `/harness` | List retained optimizer runs; does not start optimization |
| `/help [topic\|search words\|all]` | Searchable topic guide, full command catalog and insertable examples |
| `/loop [n]`, `/loop auto`, `/loop stop` | Bounded chat continuation; `auto` toggles; exact `stop` also cancels a streaming chat turn |
| `/memory`, `status`, `on`, `off`, `add <note>`, `forget <id>`, `clear` | Shared pinned notes; [operator memory notes](MEMORY_GUIDE.md#pinned-notes) |
| `/memory capture\|recall\|retrieval\|auto-retrieve\|consolidation\|auto-consolidate\|auto-suggest-chat\|auto-suggest-coding on\|off` | Administrator-only structured-memory gate overrides; `on` or `off` is required. `/memory on` stays pinned notes |
| `/memory consolidate <episode-id> [episode-id ...]` | Selected-episode consolidation into pending proposals; does not apply facts |
| `/memory proposals` | Open the Memory panel; Apply/Reject requires review and a reason |
| `/memory remember <sentence> :: <reason>` | Confirm a semantic summary on the latest completed episode; no fact write |
| `/memory facts`, `/memory retrieve <query>`, `/memory search <query>` | List your active facts, force fact retrieval for this chat prompt, or inspect candidates without injection |
| `/memory save <text> :: <reason>` | Confirm an immediate private fact write |
| `/model`, `/model list`, `/model use <name>\|grok\|claude` | Inspect/list/select a local chat model or explicit cloud provider; does not select the coding planner |
| `/net status`, `/net devices` | `status` is scope and gates and loads no collector. `devices` lists passive neighbors. Exact aliases: `/netconnect`, `/lan`, `/scan`, `/ports`, `/speed`. A near-miss only suggests. `ports`, `diag`, `watch`, and `device` do not scan or control a device |
| `/prompt` | Private preview of the next chat system prompt |
| `/registry` | Combined skill, tool and connector catalog |
| `/session`, `list`, `info`, `new [title]`, `rename <title>`, `use <id>` | List sessions, inspect the current session, create, rename or reopen one |
| `/skill`, `status`, `use <id...>`, `clear` | Inspect, replace or clear session prompt-skill selection |
| `/skill check:<profile>` | Select one fixed check for an already staged coding request |
| `/skills [all\|help\|<name>]` | Inspect wired skills, the full catalog including optional prompt files, help, or skills matching a display name (which can differ from the `/skill use` directory ID) |
| `/soul`, `status`, `on`, `off`, `edit`, `history`, `propose` | Inspect, toggle or open persona editing/proposal flows |
| `/soul apply <id> <reason>`, `reject <id> <reason>`, `review <id>` | Review a retained proposal before explicitly applying or rejecting its exact revision |
| `/status` | Runtime status; operational details require account access |
| `/style`, `/style <name>`, `/style off` | Inspect/select a session output style, or clear it without changing soul.md |
| `/tokens` | Current session token tally |
| `/tools [all\|help\|mcp\|<name>]` | Inspect registered tools, the full catalog including unwired entries, help, external MCP grants, or one named tool |
| `/users` | Administrator account panel; [accounts](SECURE_RESEARCH.md#accounts-and-roles) |
| `/web [status\|help]`, `/web on`, `/web off` | Inspect shared-home rules; administrators enable/disable web |
| `/web allow <pattern> [pattern ...] [--group name] [--seed URL ...]`, `/web deny <id-or-pattern>` | Administrator grants are atomic; repeat `--seed` per concrete seed. Every seed must fit a requested rule |
| `/web check <URL> [URL ...] [--group name]` | Exact permission diagnostic, with no network request or grant |
| `/web cancel`, `/web fetch <URL> [URL ...] [--group name]`, `/web forget`, `/web inject` | Cancel research, batch-read exact permitted URLs, clear or select your last extract |
| `/web pages [--group name] <query>`, `/web research [--group name] [--url URL ...] <question>` | Permitted-page discovery or local-model research; repeat `--url` per starting point; no new authority |
| `/web search [--count 1..10] [--engine google\|pages] [--group name] <query>` | Google listings by default; a group selects permitted-page search |

Web commands support the flags above; `--help` opens help without mutation.
Named web flags accept `--flag value` or `--flag=value`. Quote whole arguments
when needed. In queries `-tokio` is text; use `--` before words starting `--`. Legacy
`group=name` and single-pattern `allow PATTERN GROUP SEED` remain accepted.
Unknown, repeated singleton, or missing-value flags refuse; fetch takes URLs,
not appended prose.
Memory `save` and `remember` split text from the required reason at the final `::`.

Common misspellings such as `/memroy` and `/memory cler` suggest `/memory` and
`/memory clear` without executing either. Ambiguous `/skil` offers both `/skill`
and `/skills`; edit the input and submit the intended command yourself. Supported
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
| `/agent read <repo-relative-path[#Lx-Ly]>` or `clear` | Stage up to 8 bounded file-context declarations, or clear them. A local planner may later emit `=== READ path ===` (cap 6, next iteration, same jail + basename deny-list); that is not a slash command. Denied basenames (`.env`, keys, credentials, …) refuse operator and model READ alike. Optional [repository retrieval](CODING_PIPELINE.md#optional-repository-retrieval) can add bounded local-only excerpts; it starts off and shows per-step provenance in run status. A cloud planner refuses model-requested and automatic retrieval reads. The deny-list is not a secret scanner; the jail is not a secrets control. |
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
The command implementation is in [the console](../assets/static/harness.html), and
fixed profiles are in [agent policy](../src/server/agent_policy.rs).

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
but share that local prompt budget (`source=notes_corpus`). Cloud chat, `/loop`,
and `/agent` refuse attachment IDs or live pins with
`ATTACHMENT_SURFACE_FORBIDDEN` and never receive notes-corpus bytes. Preview
proves only local-chat context.
Unsupported formats are refused. Clipped sections explicitly say they are incomplete.

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
A two-second cooperative deadline is checked on ZIP reads/seeks and XML events;
this is bounded in-process parsing, not OS preemption or a hard real-time
scheduler. Prompt clipping is separate and remains explicitly labeled.

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

`/loop stop` (`POST /api/chat/cancel`) cancels the account's chat or loop turn.
Closing the stream aborts its task, releases claims, and drops model and web
requests. Each ChatClient call has an independent cancellation record. Socket
disconnect does not claim immediate GPU release.

Tool calls are accumulated and validated before dispatch. Owner and URL
permission are rechecked before each evidence-derived delta and final delivery.
Revocation cannot retract delivered text. Model traffic has a 4 MiB ceiling and
request deadline; a bounded channel applies backpressure within that deadline.

`cargo test --test chat_stream`, the SSE decoder unit test, and
`node scripts/chat-browser-acceptance.mjs` cover completion, fragmented UTF-8,
concurrent-client cancellation, disconnect, malformed output, tool refusal and
provisional browser rendering. Synthetic fixtures establish protocol behavior,
not the quality or cancellation scheduling of a real model.

### Local history compaction

Before local chat or `/loop`, the harness estimates system text, stored messages,
the new message, tool definitions, and reply reservation. Above
`chat.compact_prompt_tokens` (default 24000), it summarizes middle turns while
preserving the first user message, goal, and recent tail
(`chat.compact_keep_messages`, default 8, range 2–40). Prior summaries remain
intact; other middle turns clip at 800 characters. Summary input is at most
24000 characters and must fit its calibrated call budget. Otherwise compaction
fails without changing history.

Reply reservation is `max_tokens` for resolved Ollama with explicit
`reasoning_effort: "none"`, or twice that otherwise. The threshold floor adds
4096 prompt tokens and calibrated tool definitions, capped at 30000 (scaled to
a verified window). Web chat
also moves toward `web.total_tokens - reservation`, leaving two replies, while
its dispatcher checks each call independently. Startup accepts at most 25904
reply tokens on the first path or 12952 on the second for chat and `/loop`.

This doubled reservation is a conservative harness policy, not a claim that
every provider counts reasoning separately. For example, OpenAI documents
`max_completion_tokens` as including reasoning tokens, while compatible local
servers differ in supported parameters ([OpenAI](https://developers.openai.com/api/reference/resources/chat),
[Ollama](https://docs.ollama.com/api/openai-compatibility)). The harness retains
its existing `max_tokens` request field.

Initial input estimate is UTF-8 bytes divided by four. Positive prompt usage from
successful local replies trains a model/backend ratio within 1–3. Increases apply
immediately; decreases use a half-weight moving average. Only the initial web-tool
prompt trains it; summary, cloud, and missing usage do not. Session calibration
survives restart and resets to 1 when model or endpoint changes. It cannot predict
language shifts, same-name model replacement, or ratios above 3. Verify the model
window as described in [MODELS.md](MODELS.md).

`compaction.summary_max_tokens` defaults to 768 and clamps to 128–2048, including
in older homes. Tune it in home `config.yaml`, then restart. Compaction is inline.
Empty, failed, truncated, or cancelled output commits neither summary nor
calibration. A prompt that still exceeds the limit returns
`CHAT_PROMPT_TOO_LARGE` (422) naming the bounding setting (`details.limit_source`),
the turn's reply budget (`models.local_llm.max_tokens`, or
`api.harness_loop_rate_limit.max_tokens` for `/loop`) and only remedies that can help.
A new session cannot help when the system prompt and reply reservation fill the limit.
