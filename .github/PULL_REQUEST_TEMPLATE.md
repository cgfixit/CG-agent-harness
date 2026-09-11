## Branch naming (required for agent-opened PRs)

Before opening a PR, create the head branch with the **driver-matched** prefix.
The allowlist is `TEMPLATE_BRANCH_PREFIXES` in `src/common/identity.rs`; casual / generic names are not a substitute.

| Driver | Branch pattern | Example |
|--------|----------------|---------|
| Claude Code | `claude/<feature>` | `claude/loopback-host-check` |
| Codex | `codex/<feature>` | `codex/verify-dep-guard` |
| Grok Build | `grok/<feature>` | `grok/pr-template-branch-rules` |
| Kimi / Kimi Code | `kimi/<feature>` | `kimi/docs-sync` |
| Legacy CyClaw / MCP prefix | `CyClaw/<feature>-<YYYYMMDD>` or `cyclaw/<feature>` | `cyclaw/harness-timeout` |
| Unknown / harness default | `agent/<feature>` | `agent/harness-browser-parity` |

Rules for agents:
1. Pick the prefix that matches **the tool that is creating the branch**, not a generic label.
2. Do **not** default to `agent/` when the driver is known (Claude → `claude/`, Grok → `grok/`, etc.).
3. `<feature>` must be short, kebab-case, and describe the change (no spaces, no leading `-`).
4. If the branch name is wrong, rename **before** push: `git branch -m <prefix>/<feature>`.

Also allowed (non-feature): `main`, `dependabot/*`, `renovate/*`, `release/*`, `hotfix/*`.

**Base branch: `main`.** Never open a PR against another feature branch (no stacking).
#36–#41 were squash-merged into their predecessors and never reached `main`; the
`base branch is main` check now fails such PRs. Fix with `gh pr edit <n> --base main`.

## Title
**Use this format:**  
`[prefix] - Short descriptive sentence of the change`

**Recommended prefixes (pick the most relevant):**  
`[invariant]` • `[security]` • `[infra]` • `[fix]` • `[docs]` • `[harness]` • `[agentic]` • `[test]` • `[feat]`

Example: `[docs] - Retarget PR template for CG-agent-harness`

---

## Proposed changes
Describe the big picture of your changes here. Explain **why** maintainers should accept this PR.  
If it fixes a bug or resolves a feature request, link the issue.

**Invariant / Governance Impact** (required for any change touching core paths):
- Which guarantee in `INVARIANTS.md` does this change affect (or confirm none)? Call out I6 (process isolation) and the write gates when relevant.
- Provide evidence it is preserved (e.g., server still does not reference `crate::agentic`, `confirm` is never defaulted, write gates still ship closed, clone jail still refuses escapes).
- If you are intentionally relaxing or evolving an invariant, explain the justification and compensating controls.

---

## Types of changes
What types of changes does your code introduce to CGagentHarness (`cgagentharness`)?  
_Put an `x` in the boxes that apply_

- [ ] Bugfix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation Update (if none of the other choices apply)
- [ ] Invariant / Governance refinement (use this for changes that strengthen or evolve I6 isolation, write gates, or harness phases)

**Optional free-text scope note** (recommended):  
Core isolation / write-gate path (`src/shim`, guards, writer, sandbox, workspace, shipped config) | Agentic pipeline | Console / server | Docs + audits | Infrastructure / CI only

---

## Benefits / why
- Why make this change? What is the concrete upside for CGagentHarness operators, contributors, or long-term maintainability?
- How does this improve (or at least not degrade) loopback-only posture, I6 isolation, write-gate strength, or security posture?
- For agentic or harness changes: how does this increase governed capability without weakening the child-process isolation contract or opening a write gate by default?

---

## Risks to monitor
- What are the potential regressions, negative side-effects, or things that need extra attention after merge?
- Could this introduce an in-process server→agentic call, default `confirm` on, weaken a write gate, create a non-loopback bind, or add a network assumption?
- For write-enablement changes: what failure modes exist if a gate is skipped or `reason` becomes optional?
- How will you (or future maintainers) detect drift from the intended behavior?

---

## Checklist
_Put an `x` in the boxes that apply. You can fill these out after creating the PR. If you're unsure about any item, ask before opening the PR._

- [ ] I have read `INVARIANTS.md` and `AGENTS.md`
- [ ] This change preserves I6 process isolation and the write gates (explicit evidence for core-path changes)
- [ ] Quality bar has been run (`cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets`; or `SKIP_LIVE=1 scripts/verify-local.sh`) and passes with no regressions
- [ ] No new external network dependencies or mandatory online LLM assumptions were introduced without explicit justification + local/offline fallback
- [ ] For any agentic/harness/write-path change: `confirm` is never defaulted, `reason` is required, and shipped write gates stay closed unless this PR is intentionally arming one (with justification)
- [ ] Relevant docs (`INVARIANTS.md`, `AGENTS.md`, `README.md`) have been updated if core behavior or topology changed
- [ ] Commit messages follow the title prefix convention above
- [ ] For large or complex changes: before/after invariant notes + `cargo test` evidence is included in "Further comments" or linked
- [ ] PR body was checked with `scripts/check-pr-template.sh` before opening

---

## Further comments
If this is a relatively large, complex, or core-path change, kick off the discussion here in ELI5 technical tone.

**For changes touching `src/shim`, `src/server/guards.rs`, `src/server/headers.rs`, `src/agentic/writer.rs`, `src/agentic/executor/sandbox.rs`, `src/agentic/workspace.rs`, or `assets/config.default.yaml`, include:**
- Explicit before/after invariant statement (which `INVARIANTS.md` guarantee holds and why)
- Test or `scripts/verify-local.sh` evidence
- Any compensating controls or observability added
- A technical summary plus a plain-language (ELI5) summary of what changed, what is at risk, and where to monitor

**Examples of what good "Further comments" look like for core changes:**
- "No change to the I6 boundary. The server still does not reference `crate::agentic`; the only edge remains `src/shim` spawning a child."
- "Write gates still ship closed. `confirm` is not defaulted; `reason` remains required. Clone jail tests still refuse escapes."
- "Docs-only: no code, no config, no CI Windows parking. Invariants untouched."

---

**Notes for contributors (including solo maintainer / multi-agent PRs):**
- Core invariant or write-gate changes require the strongest evidence.
- `src/agentic/` and console-only changes may use a lighter checklist, but still need Benefits + Risks + the relevant items.
- Docs-only PRs may skip some technical checklist rows; Benefits and Risks remain required.
- Prefer squash-and-merge. The final squashed commit message is the permanent record; keep intermediate agent WIP out of `main`.
- Be blunt about impact: if I6, write gates, or loopback-only bind are affected, say so explicitly.
- PRs are draft by default until a human marks them ready.
