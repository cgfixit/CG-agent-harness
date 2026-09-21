# Operator memory notes

Operator enable steps for pinned notes and structured memory. Index: [setup-guide.md](../setup-guide.md). Howto: [USER_MANUAL.md](USER_MANUAL.md). Types: [MEMORY_GUIDE.md](MEMORY_GUIDE.md). Contract: [STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md).

## 7.5 Operator memory notes

For the complete save/retrieve/config reference, see
[Memory guide](MEMORY_GUIDE.md). The `/memory save` and completion-suggestion
controls described here require a build containing these changes; they are not
a claim about an older installed release.

There are two memory systems. Pinned notes are shared-home literals. Structured
memory is a separate, enabled-by-default, account-private store. Neither is embeddings,
a vector database or RAG fusion. Optional completion suggestions require human
approval before they become canonical facts.

```text
/memory
/memory add Prefer metric units in examples.
/memory on
/prompt
```

Chat receives a guide to these controls plus current inclusion settings. It has
two read-only web tools when web is enabled: Google keyword search and permitted
URL fetch. It has no shell, `gh`, note-save, permission-editing or credential tools.
It can list notes actually included in its prompt, but cannot save or delete them.
Plain-language requests can invoke the web tools; slash commands run directly in
the console. `/loop` retains tool-free chat continuation.
`/memory add save all session history` stores those exact words, not a summary
or a reference that loads other sessions. Store concrete facts or a reviewed
summary instead. The app saves bounded conversation histories separately.

Adding a note stores it but does **not** enable inclusion. `/memory` displays the
state and note IDs. `/memory forget <id>` deletes one note; `/memory clear`
deletes all notes. `/memory off` retains the notes but excludes them from future
chat prompts. These changes take effect on subsequent chat requests without a
server restart; they affect other sessions using the same home too.

Notes are stored in `<home>/memory/notes.json`. Current code limits are 20 notes,
500 characters per note and 3,000 characters of assembled prompt context; a full
store is not a promise every note fits in the prompt. Empty, oversized and
blocked instruction-override content is rejected. These note limits are code
constants, not documented YAML settings. If memory is on but absent from
`/prompt`, inspect `/memory` for an unreadable store rather than assuming it
loaded. Back up before manual repair.

Pinned-note capability flags (`rag.facts`, episodes, retrieval fusion) remain
false. `/memory on` does not enable structured memory.

#### Structured memory (on in fresh configuration)

Issue #87 through Phase 6 consolidators and Phase 7 eval: account-private
facts, governed proposals, optional bounded episodes, explicit selected-fact
recall, facts-only FTS5, operator-selected episode consolidation into pending
proposals, an optional idle auto-consolidator, and independently gated
completion suggestions for chat/coding. Phase 7 is a local fixture
corpus for measuring those paths — not a reason to flip any gate. Models may
POST a proposal; applying a fact still requires `confirm` and nonempty `reason`
(same mutation rule as other API writes). Episode staging never writes facts,
never injects episode text, and never fails an already-successful chat. File
mode 0600 is access control, not encryption. Every structured-memory gate
ships **true** in fresh configuration. Existing config files and explicit off
overrides remain unchanged; apply the recipe in [MEMORY_GUIDE.md](MEMORY_GUIDE.md)
to enable an existing home. Fresh pinned notes use `memory.enabled: true`.

**Enable (fail-closed).** Every gate uses `flag_is_true`: literal YAML `true`
only. Quoted `"true"` stays off. Administrator-only slash overrides persist
versioned booleans in `<home>/memory/structured_gates.json`; explicit `off`
overrides config `true`. They take effect immediately but
**cannot open the store**. Config file changes need a full quit/relaunch or
`serve` restart.

1. Set `structured_memory.enabled: true` in the active home's `config.yaml` and
   restart. That creates `<home>/memory/structured.sqlite3`. Disabled startup
   does not create or open the database.
2. For existing homes, turn the following sub-gates on as needed, in config **or** via slash
   (store must already be open):
   - `structured_memory.episode_capture` / `/memory capture on` — stage
     metadata-only episodes after a successful chat exchange.
   - `structured_memory.explicit_recall` / `/memory recall on` — allow
     operator-selected facts into `/prompt` after owner/active/revision
     revalidation.
   - `structured_memory.retrieval` / `/memory retrieval on` — allow bounded
     FTS search and per-request force-include.
   - `structured_memory.auto_retrieval` / `/memory auto-retrieve on` —
     Automatic path. When this is on **and** retrieval is
     on, chat may FTS the user message and inject rechecked top-k without a
     per-request flag. Ships true for fresh homes; turn it off to require explicit retrieval.
   - `structured_memory.consolidation` / `/memory consolidation on` — allow
     `/memory consolidate <episode-id...>` to turn selected episodes into
     pending proposals only. Ships true for fresh homes.
   - `structured_memory.auto_consolidation` / `/memory auto-consolidate on` —
     bounded idle worker that reuses the manual consolidator. Requires
     consolidation (AND). Ships true for fresh homes. Feature-off starts no worker. Chat
     wins the generation gate. Pending proposals only.
   - `structured_memory.auto_suggest_chat` / `/memory auto-suggest-chat on` —
     queue the completed current chat turn for pending suggestions. Requires capture.
   - `structured_memory.auto_suggest_coding` / `/memory auto-suggest-coding on` —
     queue successful coding runs for pending suggestions. Requires capture.
3. `/memory on` remains pinned-note inclusion only and never opens these gates.

**Summarize, consolidate, review** (enabled defaults; approval before fact writes):

```text
/memory remember Prefer metric units in examples. :: My standing preference
```

This explicitly confirms attaching your bounded, scanned sentence to your latest
completed episode; it requires a visible reason and an open store. It writes no
facts and starts no consolidation. Use the returned episode id:

```text
/memory consolidation on
/memory consolidate <episode-id> [episode-id...]
```

That claims the local generation gate, sends only those episode summaries to
the local model (no tools, no web, no recalled facts), and writes pending
proposals. It does **not** create or update facts. Review and apply use the
Memory panel: `/memory proposals`, expand the full proposal, then Apply or
Reject with a nonblank reason. Each button confirms the displayed revision
through the existing decide API. Manual selection of summary-less episodes is
still allowed but has worse quality. Auto-consolidation requires an unexpired
`none`/`pending` episode with a nonblank human semantic summary.
The `consolidator-v2` confidence floor is
`structured_memory.min_consolidation_confidence: 0.40` (clamped to 0–1);
omitted model confidence is still accepted. All memory gates ship true in fresh configuration.
Automatic consolidation additionally needs
`/memory auto-consolidate on` and is **AND**ed with `consolidation`. Feature-off
starts no worker. It waits for the generation gate; already-running generation
is not preempted. The installed binary must include the relevant commands.

#### Automatic summaries and insights, with approval

The new completion path is separate from the human-summary auto-consolidator.
Merge these values into the active home's existing config block, then restart:

```yaml
structured_memory:
  enabled: true
  episode_capture: true
  auto_suggest_chat: true
  auto_suggest_coding: true
  suggestion_mode: "both" # "summaries", "insights", or "both"
  consolidation: false
  auto_consolidation: false
  explicit_recall: false
  retrieval: false
  auto_retrieval: false
```

Both source booleans ship **true for fresh homes**, preserve existing explicit off choices, accept only literal config booleans, and
require store + capture. Enable either source alone in config or with
`/memory auto-suggest-chat|auto-suggest-coding on`. Invalid modes, including
non-string YAML values, disable suggestions. Existing slash overrides take
precedence over config: use `/memory capture on` if previously disabled, and set
other overrides to match this recipe. `/memory on` enables none
of them.

After a successful Harness chat turn or coding run, bounded current evidence can
produce pending `session_summary` and/or `insight` proposals. A summary covers
that completed turn/run, not an unseen entire session. Coding uses the initiating
account's instruction and a bounded projection of the completed child result,
not raw tool logs, source files or scans of shared job history. External Codex
chats and old sessions are not imported.

Open **Memory** or `/memory proposals`, Refresh after generation, expand the
full text and review its sources. **Apply and Reject both require your reason.**
Apply saves a private canonical fact through the existing decide API; it never
silently attaches a human episode summary. There is no auto-approval flag.
Saving and later prompt retrieval remain separate operations.

Defaults: `suggestion_max_input_chars: 8000` (shared input/output budget, split
equally), `suggestion_max_queue: 8` waiting jobs plus one active generation,
`suggestion_queue_ttl_secs: 300`, `suggestion_idle_ms: 2000`. Waiting evidence is
redacted, scanned and held only in RAM; restart drops it. Full/expired queues,
model errors, low confidence or store quotas can produce no suggestion without
failing the original successful chat/run. Existing pending capacity defaults to
32 per owner. An active suggestion holds the local generation gate; a concurrent
chat may receive `CHAT_BUSY`. The local generator uses no tools/web/recalled
facts, temperature 0 and max_tokens 1024. Review is still necessary: scanners
and confidence cannot guarantee truth or detect every secret.

#### Hard-save manually while automatic suggestions are off

Leave `structured_memory.enabled: true`. Use `/memory auto-suggest-chat off`,
`/memory auto-suggest-coding off`, and `/memory auto-consolidate off` (or set the
config values false and restart). Capture can
also stay off. Then use:

```text
/memory save For repository example, use metric examples. :: Reviewed standing preference
```

This command explicitly confirms the private fact write and requires the visible
nonblank reason after `::`. It uses the existing facts API, with no model call,
episode requirement or automatic consolidation. Natural language such as “save
this to memory” does not execute a write. `/memory add <literal note>` is the
existing shared-home pinned-note alternative; `/memory remember` only attaches
a human episode summary and does not save a fact.

To stop all derived capture while keeping manual facts, also close
`/memory capture off`. To avoid opening structured storage entirely, set
`enabled: false` and restart. Existing files are not deleted. **These settings do
not disable ordinary chat-history saving:** successful exchanges still persist
in account-owned `sessions/*.json` and bounded recent messages are used for the next
chat in that session. There is no structured-memory flag for disabling that
session persistence. `/clear` clears the display; Sessions Clear has its separate
deletion flow. It cancels waiting/in-flight chat suggestions but keeps existing
facts/proposals under the documented deletion rules.

#### Memory types at a glance

| Type | Saved by | Used by |
|---|---|---|
| Shared chat history | Successful exchanges | Selected session's bounded recent context |
| Shared pinned notes | `/memory add` | `/memory on` prompt inclusion |
| Shared persona | Soul editor / reviewed soul apply | `/soul on` prompt inclusion |
| Private episodes | Capture; optional human `remember` summary | Provenance and episode consolidation, never prompt injection |
| Private pending proposals | Explicit proposals or opted-in local generators | Human Memory-panel review; never recalled |
| Private canonical facts | `/memory save`, fact API, or approved proposal | Separately enabled selection/FTS retrieval |
| Facts-only FTS index | Derived transactionally from facts | Lexical candidate search; no embeddings |
| Shared coding records | Existing job/run lifecycle | Operational evidence, not automatic memory injection |

**Search ≠ inject.** `/memory search <query>` (or
`GET /api/structured-memory/search`) returns a small top-k of candidates. It
does not write session selection and does not inject. Retrieval off is a 409,
not a silent empty inject.

**Force-include one prompt.** `/memory retrieve <query>`, or `retrieve: true`
with optional `retrieve_query` on `/api/chat` and `/api/prompt/preview`. That
flag is the explicit pick for that request: bounded FTS, recheck survivors,
inject under the Phase 4 budget. Hits do not stick on the owned session.

**Prompt budget.** Combined pinned-note + selected-fact body is 3000 characters
(`structured_memory.pinned_prompt_chars` / `selected_fact_prompt_chars`, default
1500/1500 when both are present). Unused reserved capacity is not transferred.
When only one source is present it may use the full 3000.

**Not in this tree:** embeddings, vector DB, RAG fusion, or episode FTS.
Status can report `retrieval: true` or `consolidation: true` while fusion
and RAG stay false. `auto_consolidation` ships true for fresh homes and still
requires consolidation. Feature-off starts no worker.

Complete type/flag/bounds overview: [docs/MEMORY_GUIDE.md](MEMORY_GUIDE.md).
The next-task [historical memory benchmark plan](MEMORY_BENCHMARK_PLAN.md) follows the
existing web benchmark fixture/local-model split and scores summary faithfulness
separately from durable-insight precision. It is a plan, not a live-model result.
Day-to-day operator howto: [docs/USER_MANUAL.md](USER_MANUAL.md). Contract,
rollout order, and Phase 7 bars:
[docs/STRUCTURED_MEMORY.md](STRUCTURED_MEMORY.md). Use pinned notes for
shared-home preferences; use structured facts only after explicit review.
Clearing session history keeps derived episodes unless you confirm
`delete_derived_episodes`. Before flipping any later gate, re-run the Phase 7
fixture corpus on the source/build you intend to use (the earlier v0.1.12 record is in
[historical issue #87 close-out](memory/ISSUE_87_CLOSEOUT.md)):

```text
GROK_API_KEY="" ANTHROPIC_API_KEY="" DEEPAGENT_API_KEY="" \
  cargo test --locked --test structured_memory_phase7 -- --nocapture
cargo run --locked --example structured_memory_eval -- /tmp/phase7-report.json
```

Suggested order: store → capture → explicit recall → FTS → manual
consolidation → auto consolidation. `/memory on` never opens these gates.
Disable (`/memory … off` or config literal `false`) stops new
reads/writes/workers without deleting the database; export and purge remain.
