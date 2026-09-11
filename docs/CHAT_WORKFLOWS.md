# Chat, persona, skills and goal controls

Chat continuation and executable coding jobs are separate operations. A goal,
skill, persona, or model reply never grants permission to execute commands,
commit, push, or publish.

Chat starts without an assigned repository and has no model tool dispatcher.
`/skill use ponytail karpathy-guidelines` deliberately adds coding context for a
coding discussion; it does not connect a repository or execute a review. A fresh
home leaves `agentic.repo` empty and refuses coding enablement until an explicit
`owner/name` is supplied. Existing home files/settings are preserved. Start a new
session and inspect enabled persona/notes when comparing behavior with an older
conversation; prior assistant messages can still bias subsequent model replies.

## Session boundaries

`/session new` replaces the transcript and starts a separate message history,
goal, and skill selection. `/session use <id>` restores only that session's
retained messages, without appending duplicate copies. Switching cancels the
browser's current generation wait and ignores late replies from older selections.
Hidden coding/proposal reviews are cleared, so an unseen review cannot remain
armed. Existing saved sessions are not deleted. `/clear` clears the display only.

Shared persona and enabled memory/web context survive new sessions. The header
token count covers all saved sessions; `/tokens` reports the selected session.
`/prompt` exposes shared context; use its explicit toggles when you want to exclude
it. Do not confuse a new conversation with deleting all shared notes/persona.

## See what is available

`/tools <name>` reports route registration separately from known enablement and
readiness. Unknown prerequisites stay unknown: merely listing tools does not
probe a model, invoke GitHub, or arm a write gate. `last_result: null` means the
inventory has no invocation history. The legacy `wired` field is retained for
compatibility and does not prove operational readiness; `invoked` is no longer
set to true merely because a route exists.

`/skills all` shows these runtime types:

| Type | What happens |
|---|---|
| Seeded coding context | `ponytail` and `karpathy-guidelines` are optional selections, like other prompt skills. Frontmatter changes display text, not directory identity. |
| Optional prompt context | `/skill use <id...>` explicitly selects local skill bodies for subsequent chat turns in this session. No script executes. |
| Fixed check | `/skill check:cargo-test` selects a reviewed, named check for an already staged coding request. It executes only through the existing authorized coding job. |
| Governed catalog | Listed without an execution adapter; skill prose cannot invent one. |

The repository's `.codex/skills` and Codex's personal skills are development
instructions for Codex. They are not automatically installed into the app's
runtime skill directory.

Chat is told which application features exist and how the operator invokes them.
It still has no callable tools, including `gh`. A prose request to save or delete
memory does not execute a command. `/memory` lists persistent notes and
`/memory add <note>` saves literal text; it does not archive or summarize sessions.
Included notes are real stored content, not placeholders. Model answers can still
be wrong; only actual command results establish successful execution.

## Inspect and edit the chat prompt

`/prompt` opens a private snapshot of the next chat system prompt. It includes
the general-chat header, an operator-command guide and current inclusion settings,
selected prompt skills, enabled persona, the current
session goal, explicitly injected web context while web is enabled, and enabled
memory notes. It is not a transcript or the coding planner's prompt. The API also reports source
load state and limits. No credentials file is included. Private preview/editor
responses require the existing API guards and use `Cache-Control: no-store`.

The compiled chat scope and execution boundaries remain fixed. Edit the persona with:

1. `/soul status` to see enabled, present, loaded, truncated, and a safe reason.
2. `/soul edit` to open the current document or explicitly create a missing one.
3. Enter persona text and a reason, then **Preview prompt**.
4. Review the content, select the confirmation checkbox and **Save persona**.

The next chat rereads the saved persona when `/soul on` is enabled. Nothing is
silently seeded into a fresh `soul.md`. Empty, oversized, critical instruction-
override patterns, and invalid content are refused. The default limit is
8,000 characters (`personality.soul_max_chars`). Stale editor revisions cannot
overwrite newer content; reload and review after a conflict.

Replacement is atomic and prior content is retained as a private SHA256-named
backup. `/soul history` lists backup revisions. The guarded API
`GET /api/soul/document?version=<revision>` reads one for deliberate restoration
through the same preview/confirmed-edit process. History is limited to 32
backup/proposal records; archive old records deliberately when the limit is
reached. No automatic deletion or migration of the operator's history occurs.

## Review proposed persona changes

`/soul propose` opens a proposal-only editor. This accepts text the operator
supplies, including model-authored text; it does not automatically ask a model
to evolve the persona. **Create proposal only** stores a pending revision and
leaves the active persona unchanged.

Use `/soul review <id>` to inspect the exact retained proposal. Then choose
`/soul apply <id> <reason>` or `/soul reject <id> <reason>`. Apply/reject require
explicit confirmation and the reviewed content hash. A changed active persona,
changed proposal, or already-decided proposal refuses application. A rejection
does not alter the active persona.

Persona replacement and proposal-status persistence are separate durable writes.
After a storage error or crash, inspect both records before retrying. A failed
replacement preserves the active file; a crash after successful replacement can
leave proposal status requiring reconciliation. The base-revision check prevents
silently replaying the old proposal against changed content.

## Select runtime prompt skills

Place a skill at `<harness home>/skills/<id>/SKILL.md`. IDs use lowercase letters,
digits, hyphens and underscores, up to 80 characters. Frontmatter `name` is only
a display label. Files and links cannot escape the skills-directory capability.

```text
/skill use review-notes rust-style
/skill status
/prompt
/skill clear
```

Up to four optional IDs are retained per session. Default per-skill clipping is
6,000 characters; the aggregate limit is 16,000, configured by
`personality.prompt_skill_max_chars` and `personality.prompt_skills_total_chars`.
Frontmatter is stripped. No coding skill is injected automatically; clearing
selection also removes selected seeded coding skills from subsequent prompts. Missing, empty, unreadable or invalid selected skills
refuse the turn until restored or cleared, instead of silently dropping context.

After a successful chat, `/skill status` records the included IDs, character
counts and SHA256 hashes. That is the last successful prompt snapshot, which can
differ from the currently selected files. It is not a claim that skill scripts
ran. Fixed checks are restricted to the existing profile table; unknown IDs or
shell text cannot become commands. Use `/agent job <id>` and `/agent status
<run-id>` for actual coding/check results and refusal evidence.

## Turn a goal into a reviewed coding task

```text
/goal Fix the arithmetic bug and pass the test suite
/goal stage codex/arithmetic-fix
/agent read src/lib.rs
/skill check:cargo-test
/agent iterations 1
/agent confirm Reproduce and fix the reviewed arithmetic defect
/goal task
```

Staging persists an exact goal/branch identity and creates no worker. One
iteration is selected by default. Review the request and optional inputs before
confirmation. Existing coding prerequisites, tool allowlists, write gates,
subprocess isolation, time/output limits and sandbox checks apply unchanged.
The server durably associates the stage and declared checks with the job before
releasing the worker. A changed goal, altered branch/instruction or previously
submitted stage is refused. Retrying requires deliberate inspection and staging.

`/goal task` restores an unsubmitted request after refresh or displays retained
progress. Closing the browser does not erase an authorized job. Restarted jobs
that cannot be reattached are marked interrupted, never automatically replayed.
The existing 32-job retention bound applies; missing evidence is reported
unavailable. Each session retains its current stage; older jobs/runs remain in
their respective inventories. Changing chat goals does not cancel existing
coding work: explicitly use `/agent stop <job-id>`.

A checked candidate is **awaiting review**. Inspect the complete diff with
`/agent status <run-id>` and separately approve it with a reason. **Completed
locally** requires the checked-tree acceptance digest and an operator-approved
commit. It does not mean pushed/published or that subjective requirements were
independently understood. Push and PR publication remain separate actions.
`GOAL_DONE` stays an advisory chat marker and never supplies completion evidence.

## Delivery and remaining scope

The historical #35–#41 stack did not deliver all predecessor changes to `main`.
Check current code and the parity ledger before claiming a feature is delivered.
Current repository policy requires every new PR to target `main`; verify its
exact source and checks independently from native application acceptance.

Automatic arbitrary model tool calls, unattended restart/replay, new executable
skill discovery and general soul-evolution scheduling are not implemented.
Phase 5 remains separate: reconcile #30/shared model readiness and implement
connectors/integrations from the canonical [parity ledger](parity/STATUS.md).
Native WKWebView evidence is recorded independently from browser/API tests in
the candidate handoff; see [desktop acceptance](DESKTOP_ACCEPTANCE.md).
