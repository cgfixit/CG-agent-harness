# Chat, soul, skills and slash commands

Chat, soul, skills, style, connector catalog, and slash-command tables. Index: [setup-guide.md](../setup-guide.md). Workflow bounds: [CHAT_WORKFLOWS.md](CHAT_WORKFLOWS.md). Memory enable steps: [MEMORY_SETUP.md](MEMORY_SETUP.md). Web: [WEB.md](WEB.md).

`/loop` continues chat toward a session goal. It does not execute repository edits or checks. `/agent` drives the coding pipeline. See [CODING_PIPELINE.md](CODING_PIPELINE.md).

## What you can do

- Discuss supplied code or documents, save a session goal and revisit the
  conversation with its selected prompt context.
- Ask chat to search Google for current links, or to read and summarize an
  authorized URL. A SerpAPI key is optional; the public Google fallback can be
  blocked by JavaScript or CAPTCHA and reports that explicitly.
- Grant a documentation URL, fetch or search its passages, and ask `/web research` for a
  local-model answer with checked quote references and reported coverage. Use
  `/web inject` to include selected evidence in later chat.
- Stage a bounded repository change, inspect the proposed diff and checks, then
  separately approve a commit, push and draft PR when ready.

| Capability | How it works |
|---|---|
| Chat and sessions | Create, rename, and revisit separate conversations with saved messages and token counts. New Session replaces the transcript; the confirmed Clear all session history control deletes saved conversations. Derived structured-memory episodes remain unless the operator also confirms that cascade. |
| Persona, memory and prompt context | Inspect `/prompt`, edit or review proposals for shared `soul.md`, select per-session prompt skills, and save literal operator notes with `/memory`. `/memory on` includes those notes only. Optional structured facts, governed proposals, bounded episodes, facts-only FTS, and manual consolidation (#87) are a separate account-private store with gates on in fresh configuration; models may suggest, not silently write. Search is not inject. Facts enter `/prompt` only after an explicit pick (`selected_facts`, `/memory retrieve`, or `retrieve`) or the separately gated `auto_retrieval` path. Episodes are never injected. Manual consolidation writes pending proposals only. Chat explains these controls; the operator executes them. |
| Chat model selection | Select an exact installed local model tag, or explicitly select `grok` / `claude` after an administrator saves the matching provider key and restarts. Cloud chat sends only the newly typed message; local history, memory, skills and web context stay local. |
| Chat continuation | `/goal` and `/loop` provide bounded follow-up turns with request limits, completion-token budgets, cancellation, and optional auto-continue. |
| Tool visibility and use | `/skills` and `/tools` distinguish registered adapters from readiness and execution evidence. Console commands invoke backend operations through fixed, validated interfaces; model prose does not become an arbitrary shell command. |
| Chat web tools | Natural-language Google search and permitted URL fetch. API Keys accepts `SERPAPI_API_KEY`; no active key selects public Google. Actual tool outcomes and source links appear in chat. No web tools run in `/loop`. |
| Web research | Grant exact/wildcard URL permission for bounded discovery and BM25 passage search; run `/web research` for local-model answers with verified quote references, usage and partial coverage. Fresh web settings are enabled with an empty URL allowlist; selection/injection is account scoped. |
| Accounts and API Keys | Fresh `admin` / `admin` requires password replacement. Administrator, Portal operator and Auditor permissions are enforced on API reads and writes. Administrators manage masked saved/active credentials in API Keys. |
| Coding loop | Stage a repository task and inspect files or a plan; confirm an isolated run that proposes bounded edits, runs fixed check profiles in a hard sandbox, and feeds check results back into later attempts. |
| Review and publication | Inspect retained run status and diffs, approve the reviewed tree for a local commit, then separately push and publish a draft PR with a reviewed repository template. |
| Analytics | The Analytics button or `/analytics` opens an overview and keyboard-accessible Tokens/Sessions/Code sections, each with its own filter and paging; sessions also support sorting. Completeness warnings remain visible. [Interpretation and API](ANALYTICS.md). |
| Spend | Read-only retained provider/model/day usage, available USD, completeness warnings and pagination. See [build requirements and interpretation](SPEND_AND_NOTIFICATIONS.md). |
| Completion webhooks | Optional metadata-only notifications for terminal detached jobs, disabled until configured. Delivery does not grant job or repository authority; see [setup](SPEND_AND_NOTIFICATIONS.md#configure-a-completion-webhook). |
| Recovery | Rediscover retained jobs and runs after reopening. Worker leases distinguish active work from interrupted runs; reopening does not automatically resume work or replay a publication. |

## 7. Chat, soul, skills and goals

Fresh chat starts without an assigned repository or automatic coding skills.
Local chat can use the bounded web tools when enabled; `/loop` stays tool-free. The assistant can explain supplied context but cannot inspect
local files or certify live wiring merely because you ask in chat. Use actual
commands/results for evidence and the separate coding workflow for execution.

Existing homes retain their selected repository and skill files. The new chat
composer stops automatically loading legacy coding skills without modifying them.
To avoid old conversation instructions influencing a test, start `/session new`,
use `/skill clear`, inspect `/memory` and `/soul status`, then `/prompt`. Only
explicitly change notes/persona you want changed; a new session still uses the
home's enabled notes and persona.


Enter slash commands in the chat input, not Terminal. `/help` lists commands
available in the installed version. Begin with `/status`, `/model`, `/skills all`
and `/tools`. Registration is not readiness: an unknown prerequisite or empty
last-result field is not evidence that an operation ran successfully.

### 7.1 Sessions and bounded chat continuation

```text
/session new Setup check
/goal Explain this project's test strategy
/goal
```

Send a short question and confirm a real reply. `/session list` lists saved
sessions; `/session use <id>` reopens one and `/session rename <title>` renames
the current one. `/tokens` shows its token usage; the header's **all sessions**
count is cumulative across saved sessions.

New/session-switch actions replace the visible transcript, stop chat continuation,
discard delayed replies from the previous selection, and clear hidden staged
coding/persona reviews. Saved sessions remain intact; switching restores their
retained messages. A new session has no prior messages, goal, or selected prompt
skills. Persona and enabled notes remain shared within the home. Web selection
and injected context belong to your account and survive your session changes;
other accounts cannot inherit that selection. The reset controls have different scopes:

| Intent | Control | Retained data |
|---|---|---|
| Clear the visible output | `/clear` | Saved messages, session ID and goal remain; later chat still receives recent history. Hidden staged reviews and loop state clear. |
| Start a separate conversation | **+ new session** or `/session new <title>` | Old sessions, shared persona/notes/model selection, and your account's web selection remain. |
| Delete all saved conversations | **Clear all session history**, immediately below **+ new session** | Shared notes/persona/web, model configuration, coding runs and audit records remain. |

For deletion, read the dialog, then choose **Delete all session history** to confirm
or **Cancel** to keep the sessions. Confirmation stops active chat, deletes saved
session files/goals/skill selections/token totals and clears the visible conversation.
A late response cannot recreate a deleted session. Send a new message or use
`/session new` afterward. A storage failure is reported and may leave a partial
deletion; resolve the reported storage problem before retrying.

This is file deletion, not secure disk erasure. Backups, copies retained by your
model service, and transcript content already loaded in other open clients are
outside its scope. Close or refresh those clients separately.

The session store retains at most 500 messages per conversation. Normal chat sends
at most the latest 20 prior messages within 8,000 characters; loop turns use eight
within 4,000 characters. These are message counts, not user/assistant pairs.
Stored history and lifetime token totals can therefore exceed what the model sees.

Runtime `sessions/` directories, named `.CGagentHarness/` homes and dotenv files
are Git-ignored in this repository. Avoid `git add -f` for private files: ignore
rules do not remove files already tracked by Git and are not global Git policy.
An arbitrarily named custom home can still expose notes, web extracts or coding
records outside its ignored `sessions/` directory. Keep the entire home outside
the checkout. Before publishing from a source checkout, these checks should show
matching ignore rules and no tracked runtime files respectively:

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

`/loop 3` starts up to three follow-up chat turns toward the current goal. The
default is three, hard maximum five. Manual mode pauses after each turn; another
`/loop` continues. `/loop auto` toggles automatic continuation; enable it before
starting, or follow it with `/loop` to resume a paused sequence. `/loop stop`
requests cancellation during generation or cooldown. Goal clear and session
switching stop continuation. Rate limits, failures, repeated output and token
budgets can stop it earlier; the displayed `GOAL_DONE` marker is only model advice.

This loop does not edit files, run skills as programs, or perform coding checks.
The goal persists, while continuation counters and auto state are page state.
Refresh/restart does not resume an unattended loop. Goal-to-coding execution is
an explicit separate workflow in [goal staging](CODING_PIPELINE.md#94-stage-a-session-goal-as-a-coding-task).

### 7.2 Default soul, effective prompt and persona editing

**Fresh homes seed the bundled CG Agent persona and enable the soul toggle.**
The bundled response contract asks for human-readable plain text in short
paragraphs, using Markdown only when explicitly requested. This guides the model;
it does not filter or rewrite generated output.
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

`/soul status` distinguishes enabled, present, loaded, truncated and a safe failure
reason. `/prompt` previews the next chat system prompt: general-chat header, selected optional prompt skills, enabled persona, session goal, injected
web context and enabled memory notes. Preview is private context; review it before
sharing. It is not the coding planner's prompt.

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

`/soul off` disables inclusion without deleting the file. Older versions without
the editor allow deliberate manual editing of the active home's `soul.md`; use
`/soul on` to enable it. Upgrade to a build containing the current main controls
to use the guarded editor and proposal recovery.

**History and proposals:** replacement is atomic and saves the previous content
as a private content-addressed backup. `/soul history` lists backup revisions.
`/soul propose` stores proposed text without applying it and returns an ID. This
accepts text you supply, including model-authored text; it does not automatically
generate a new personality.

```text
/soul review <proposal-id>
/soul apply <proposal-id> <reason>
```

Review first; issuing the apply command with a reason sends explicit confirmation
for that exact reviewed revision. There is no additional proposal-apply dialog. To refuse it, use `/soul reject <proposal-id> <reason>` after review.
Rejection preserves the active persona. Changed base content or already-decided
proposals are refused. Backup/proposal storage is limited to 32 records and does
not automatically delete old history. Apply records a private recovery marker before
replacement. Startup and the next persona document/review/edit/proposal operation
reconcile it: matching candidate content becomes `applied`, unchanged base content
stays `pending` and needs a new explicit apply, and unrelated content becomes
`interrupted`. Recovery never writes persona text. Review an interrupted record and
create a new proposal against the current document if still wanted.

If recovery storage is unreadable or cannot be updated, startup and later persona
operations refuse until repaired. Preserve `soul.md`, `soul-history/`, and
`soul-pending-apply.json` before manual repair; do not blindly delete the marker.
This covers process interruption, not power-loss atomicity or concurrent external
file edits. Older interrupted applies without a marker still need manual review.
[Chat workflow details](CHAT_WORKFLOWS.md) explain history retrieval and
restoration through the guarded edit flow.

### 7.3 Runtime skills and Codex development skills

| Kind | Location / selection | What it does |
|---|---|---|
| Seeded coding context | `<home>/skills/ponytail` and `karpathy-guidelines` | Available through explicit `/skill use ponytail karpathy-guidelines`; not automatically loaded |
| Optional runtime prompt skill | `<home>/skills/<id>/SKILL.md`; `/skill use <id>` | Adds bounded context to this session's chat |
| Fixed check | `/skill check:cargo-test` | Selects a known check for an already staged coding request; no immediate execution |
| Governed catalog entry | `/skills all` | Inventory only unless an implemented adapter says otherwise |
| Codex development skill | Repository `.codex/skills` or Codex personal skill directory | Guides Codex maintaining the repository; not automatically an app runtime skill |
| Claude Code development skill | Repository `.claude/skills`; registered shortcuts in `.claude/commands` | Guides Claude Code maintaining the repository; not an app command |

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

After a successful chat, `/skill status` reports included IDs, lengths and hashes.
That is the last successful snapshot, which can differ from current files or
selection. It does not prove script execution. Missing/unreadable selected files
refuse subsequent chat; restore them or `/skill clear`. Clearing selection removes all optional skill bodies from subsequent prompts,
including selected seeded skills; it does not delete their files.

When working on this repository in Codex, invoke a discovered development skill
such as `$cgagentharness-optimize` or `$fable-protocol`; the entrypoints are listed
in [AGENTS.md](../AGENTS.md). Claude Code has corresponding commands such as
`/cgagentharness-optimize` and `/fable-protocol`. If the coding agent does not list
one, explicitly reference its `SKILL.md` in that checkout. Copying a skill file
does not prove it has been loaded. These development instructions are not sent
to app chat automatically and do not authorize publication.

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

Save through the review flow in [soul](CONSOLE.md#72-default-soul-effective-prompt-and-persona-editing), enable `/soul on`, and inspect
`/prompt`. Test with `/session new Style check` so earlier conversation does not
confound the comparison. Try a short factual question, a troubleshooting question
and a request for a detailed explanation. Style instructions guide the model;
they do not guarantee a word count or factual correctness.

For a session-specific alternative, create `<home>/skills/concise/SKILL.md` using
the file format in [runtime skills](#73-runtime-skills-and-codex-development-skills) and put the desired writing rules in its body.
Select `/skill use concise`, then inspect `/prompt`. To combine it with another
skill, supply both IDs in the same command. Avoid contradictory rules in soul,
selected skills and memory: a “give full detail” instruction can conflict with a
“one sentence only” instruction. `/skill clear` removes the session selection;
`/soul off` disables the home-wide persona without deleting it.

The most useful customization input is three actual responses you dislike,
your preferred rewrite of each, and a sentence explaining the difference. Keep
those as manual comparison examples. Describe observable preferences such as
“answer before explanation” or “no repeated closing summary,” rather than only
“sound human.”

**Style presets.** `/style <name>` selects a per-session output-style preset
without editing `soul.md`; `/style off` clears it and `/style` alone prints the
active one. Shipped presets are `concise`, `unslop`, `technical-deep` and
`beginner`; an operator overlay at `$CGAGENTHARNESS_HOME/styles/<name>.md` wins
over the shipped file of the same name. The status bar `style` select does the
same thing. The prompt is composed soul, then style, then the fixed policy
tail, so a preset never overrides the harness contract. `/prompt` prints the
active style and says when a selected overlay could not be loaded (missing,
unreadable, empty, or refused by the injection scanner), in which case the
prompt is composed without it. There is still no automated style evaluation or
rewrite button. `concise` favors a short answer, `beginner` introduces terms with
a worked example, `technical-deep` explains mechanisms and failure modes, and
`unslop` removes inflated wording without discarding useful detail. These are
model instructions, not deterministic filters; compare the same question in new
sessions as described in [Chat workflows](CHAT_WORKFLOWS.md#inspect-and-edit-the-chat-prompt).

**What the `unslop` *planner probe* does:** separately from the `unslop` chat
preset above, it is an optional local coding-planner prose probe, not a chat
output filter. It scans for 16 fixed, case-insensitive phrases
such as “delve,” “game-changer” and “I hope this helps.” It attempts to exclude
proposed file bodies, records hit counts and a response hash, and supplies a
nudge if the ordinary coding loop needs another iteration. A passing candidate
can finish immediately despite phrase hits. It neither rewrites the current
answer nor forces an extra iteration, and it is not used for cloud-provider
planner overrides. Phrase matching is a heuristic, not a quality score.

If you already use the governed local coding workflow and want that probe,
merge this into the existing active home's `config.yaml`, then restart:

```yaml
unslop:
  enabled: true
  metrics_path: "logs/unslop.jsonl"
```

It ships disabled. Metrics use the bounded JSONL logging policy; there is no
user-configurable phrase list or `/unslop` slash command. Do not enable repository
writes just to customize chat prose. For chat, use the persona/skill method above.
A future chat-specific check or explicit rewrite action would need a separate
implementation and tests; enabling this YAML field does not provide either.
See [the current probe](../src/agentic/unslop.rs) and
[its loop integration](../src/agentic/real_repo_loop.rs).

### DOCX attachments

The file picker also accepts `.docx`. Only UTF-8 main-document text is read:
paragraphs, tables, explicit tabs and breaks. Directly hidden/deleted runs and
field instructions are excluded. Headers, footers, images/OCR, style-based
visibility, macros and embedded files are not interpreted; relationships and
URLs are never followed. Extraction is a bounded text view, not a reproduction
of Word's layout. PDF remains unsupported after the issue #148 safety spike.

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

### 7.7 Tools and connectors: available versus catalog-only

`/tools` reports registered operations and capability information. `/tools all`
includes entries that are not wired; `/tools <name>` filters the inventory.
`/tools mcp` shows external MCP declarations, read/write grants, network policy,
and explicit process-group or unrestricted Windows Job Object exceptions. It does not execute a tool or prove that
the selected sandbox is available. See [MCP capabilities](MCP_CLIENT.md).
`/skills` defaults to wired entries, while `/skills all` also shows optional
prompt files and other catalog entries. `/skills <name>` filters the display
name, which can differ from the directory ID required by `/skill use`.
`/connectors` displays connector inventory; `/registry` is another inventory
view. None of these commands installs a connector or proves its prerequisites
are ready. The registry's tools array is not a replacement for `/tools`.

| Capability | Current configuration / action | Actual boundary |
|---|---|---|
| Local model | `models.local_llm` in `config.yaml`; `/model` and `/model use <name>` | Chat uses a configured loopback service; selecting a tag does not download it |
| Public web text | Chat web tools and `/web` controls in [WEB.md](WEB.md) | Google listings, permitted URL fetch, page research and optional context injection |
| GitHub coding | Explicit `agentic.repo`, gates and prerequisites in [coding pipeline](CODING_PIPELINE.md); `/github` reports status | Separate governed child-process workflow; a chat reply does not execute Git commands |
| Local file context for coding | Stage `/agent read <repo-relative-path[#Lx-Ly]>` before confirmation | Bounded reads from the governed repository clone; no general Mac filesystem mount |
| `fsconnect` | Not implemented in this app | No slash command, datasource picker, filesystem indexing or YAML enable switch |
| `netconnect` | Not implemented in this app | No arbitrary network connector; `/web` restrictions still apply |
| `sqlconnect` | Not implemented in this app | No database connection configuration, query tool or ingestion workflow |
| `github-public` catalog entry | Inventory only | Does not provide an independent public-repository connector |
| `openai-compatible` catalog entry | Inventory only as a connector | Separately configured local compatible model/fallback paths exist; the row is not an activation control |
| Runtime prompt skill | Home skill file plus `/skill use` | Prompt context, not an executable plugin or permission grant |

Names from the upstream project do not establish support in this standalone
harness. There are no `/fsconnect`, `/netconnect` or `/sqlconnect` commands to turn
on, and copying upstream connector settings does not implement them. The native
Setup folder chooser prepares offline Cargo inputs; it is not chat filesystem
access. See [port scope](PORT_PARITY.md) and the
[capability ledger](parity/STATUS.md) for remaining work, checked against
[current connector inventory](../src/server/views.rs).

### 7.8 Slash-command quick reference

Angle brackets below mean “replace with your value”; do not type the brackets.
Square brackets mark optional arguments; `|` separates alternatives. The Commands
pane and `/help` share an alphabetical catalog, with related variants together.
Clicking a command fills its root in the input for editing; it does not run it.
These are console commands, not shell commands: there are no GNU-style flags such
as `--help`, `--dry-run` or `--confirm`. Use `/help`, `/agent`, `/web help`,
`/skills help` or `/tools help` to inspect supported syntax.

Memory commands require an exact `/memory` root and subcommand (case-insensitive;
ordinary spaces are allowed). `/mem`, typos and conversational forms such as
`/memory please retrieve preferences` only suggest; they never read memory, start
chat, toggle gates or write notes. Unknown subcommands and pasted multiline/control
characters are refused. Retype the intended command on one line after reviewing a
suggestion. Exact `/memory retrieve <query>` starts a model turn with explicit
retrieval; `/memory search <query>` only lists candidates. Use `/memory` for status
and `/help` for syntax. Exact save/remember still require `:: <reason>`; exact
clear/forget still delete pinned notes, so inspect the command before sending.

Typed slash commands require the server parser. A parser failure or invalid reply
leaves the command unexecuted; inspect the Commands pane or retry. Pending parser
responses are discarded after a session/account change or clearing the transcript. Exact `/loop stop`
remains available to cancel chat continuation, including while the parser is
unavailable. Extra words after that cancellation command do not trigger its local
shortcut. Inspect results after every state-changing command. The detailed sections
above and [coding pipeline](CODING_PIPELINE.md) describe confirmation and persistence.

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
| `/help` | List the same alphabetical command catalog as the sidebar |
| `/loop [n]`, `/loop auto`, `/loop stop` | Bounded chat continuation; `auto` toggles; exact `stop` also cancels a streaming chat turn |
| `/memory`, `on`, `off`, `add <note>`, `forget <id>`, `clear` | Shared pinned notes; [operator memory notes](MEMORY_SETUP.md#75-operator-memory-notes) |
| `/memory capture\|recall\|retrieval\|auto-retrieve\|consolidation\|auto-consolidate\|auto-suggest-chat\|auto-suggest-coding on\|off` | Administrator-only structured-memory gate overrides; `on` or `off` is required. `/memory on` stays pinned notes |
| `/memory consolidate <episode-id> [episode-id ...]` | Selected-episode consolidation into pending proposals; does not apply facts |
| `/memory proposals` | Open the Memory panel; Apply/Reject requires review and a reason |
| `/memory remember <sentence> :: <reason>` | Confirm a semantic summary on the latest completed episode; no fact write |
| `/memory retrieve <query>`, `/memory search <query>` | Force fact retrieval for this chat prompt, or inspect candidates without injection |
| `/memory save <text> :: <reason>` | Confirm an immediate private fact write |
| `/model`, `/model use <name>`, `/model use grok\|claude` | Inspect/select a local chat model or explicit cloud provider; does not select the coding planner |
| `/prompt` | Private preview of the next chat system prompt |
| `/registry` | Combined skill, tool and connector catalog |
| `/session`, `list`, `info`, `new [title]`, `rename <title>`, `use <id>` | List sessions, inspect the current session, create, rename or reopen one |
| `/skill`, `status`, `use <id...>`, `clear` | Inspect, replace or clear session prompt-skill selection |
| `/skill check:<profile>` | Select one fixed check for an already staged coding request |
| `/skills [all\|help\|<name>]` | Inspect registered skills, the full catalog, help or one named skill |
| `/soul`, `status`, `on`, `off`, `edit`, `history`, `propose` | Inspect, toggle or open persona editing/proposal flows |
| `/soul apply <id> <reason>`, `reject <id> <reason>`, `review <id>` | Review a retained proposal before explicitly applying or rejecting its exact revision |
| `/status` | Runtime status; operational details require account access |
| `/style`, `/style <name>`, `/style off` | Inspect/select a session output style, or clear it without changing soul.md |
| `/tokens` | Current session token tally |
| `/tools [all\|help\|<name>]` | Inspect registered tools, the full catalog, help or one named tool |
| `/users` | Administrator account panel; [accounts](ACCOUNTS.md) |
| `/web`, `help`, `on`, `off`, `allow <url-or-pattern> [group] [seed-url]`, `deny <id-or-pattern>` | Inspect fetching; administrators toggle or edit exact/wildcard permission |
| `/web cancel`, `fetch <url>`, `forget`, `inject` | Cancel research, fetch permitted content, clear or include your last selection |
| `/web pages [group=name] <query>`, `research [group=name] <question>` | Search permitted-page passages or run bounded research with citations |
| `/web search [group=name] <query>` | Google results without a group; a nonempty group selects permitted-page search instead |

`group=name` must immediately follow `search`, `pages` or `research`. The `allow`
command instead takes an optional positional group and crawl seed. Memory `save`
and `remember` split text from the required reason at the final `::`.

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

Choose up to three `.txt`, `.md`, `.json`, `.csv`, or `.log` files (15 MiB each).
Run `/prompt` before sending to upload them locally and inspect their fenced,
untrusted text. The persona editor's Preview includes the same pending files.
Repeated previews reuse those uploads; the next ordinary message sends their
IDs once. A failed preview retains the pending IDs. Changing session or account
clears the pending selection, so another session cannot silently reuse it.
Uploads count toward the home quota until session-clear or owner cleanup.

Local chat persists owner-scoped blob ids on the session (cap 12). The next
local turn may BM25-search those blobs and inject matching passages into the
existing attachment fence. Operator `.md`/`.txt` notes ingested at
`/api/notes-corpus` live in a separate home jail and share that same local
prompt budget (`source=notes_corpus`). That local retrieval is not cloud
egress: cloud chat, `/loop`, and `/agent` return
`ATTACHMENT_SURFACE_FORBIDDEN` if attachment ids or live pins are present,
and they never receive notes-corpus bytes. The preview is a local-chat
snapshot, not proof that cloud chat or the coding planner receives that
context.
Unsupported formats are refused. Clipped sections explicitly say they are incomplete.

The notes corpus accepts multipart ingest with `POST /api/notes-corpus`, lists
the owner's documents with `GET /api/notes-corpus`, and deletes one with
`DELETE /api/notes-corpus/{id}`. It has no dedicated console upload control.
A new local message supplies the retrieval query, while `/prompt`
shows the currently assembled local context. Notes remain separate from
structured facts and pinned `/memory` notes. Ingest honors
`notes_corpus.max_files_per_request` (default 8, range 1–16). The HTTP body
cap is that count times `max_file_bytes` plus 256 KiB of multipart overhead.
Attachment uploads still cap at three files per request.

### Local history compaction

Before local chat or `/loop`, the harness estimates the next input (system,
stored messages, new message and any web-tool definitions) plus an effective
reply reservation. Above `chat.compact_prompt_tokens` (default 24000), it
summarizes middle turns, preserving the first user message, goal and recent
tail (`chat.compact_keep_messages`, default 8, clamped 2–40). Earlier summaries
pass intact into the next summarizer; ordinary middle turns are clipped to 800
characters each. Total summary input is at most 24000 characters and also fits
the calibrated summary-call budget. Prior summaries that cannot fit cause an
explicit failure and leave history intact.

The effective reply reservation is `max_tokens` for resolved Ollama with
explicit `reasoning_effort: "none"`, and twice `max_tokens` otherwise. The
threshold floor is that reservation plus 4096 prompt tokens and the calibrated
tool-definition allowance, capped at 30000. Web chat additionally tightens the
threshold toward `web.total_tokens - 2 * reservation`, respecting the same
floor; its dispatcher still checks the independent web budget before each call.
Startup accepts at most 25904 reply tokens on the first path or 12952 on the
second, for both chat and `/loop`.

This doubled reservation is a conservative harness policy, not a claim that
every provider counts reasoning separately. For example, OpenAI documents
`max_completion_tokens` as including reasoning tokens, while compatible local
servers differ in supported parameters ([OpenAI](https://developers.openai.com/api/reference/resources/chat),
[Ollama](https://docs.ollama.com/api/openai-compatibility)). The harness retains
its existing `max_tokens` request field.

The initial input estimate is UTF-8 bytes divided by four. After each successful
local reply, a model/backend-specific ratio learns from numeric positive prompt
usage: increases apply immediately, decreases use a half-weight moving average,
and the factor stays within 1–3. Only the initial prompt of a web tool sequence
trains it; summary and cloud usage do not. Missing usage preserves the previous
ratio. The calibration survives restart with the session and falls back to 1
when the model or endpoint changes. It cannot predict an abrupt language change
before the first reply, identify a model replaced under the same name, or
guarantee a fit for token ratios above 3. Keep the configured model window in
[MODELS.md](MODELS.md) verified.

`compaction.summary_max_tokens` defaults to 768 and clamps to 128–2048, including
for older homes missing the key. Edit the home `config.yaml` and restart to tune
it. Compaction is inline; there is no idle pre-compaction worker. Empty, failed,
truncated or cancelled model output does not commit the candidate summary or
calibration. A prompt that still exceeds the limit returns
`CHAT_PROMPT_TOO_LARGE` (422); use a shorter paste or start a new session.
