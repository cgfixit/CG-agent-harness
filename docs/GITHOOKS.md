# Git hooks security gate

A `pre-commit` and `pre-push` gate that catches secrets, private data and
ask-first actions for anyone committing through `git` in an installed clone.

It is a speed bump, not a boundary. CI secret scanning (`gitleaks.yml`) is the
backstop, and branch protection is separate GitHub configuration. Pull-request
CI reads `.gitleaks.toml` from the immutable base commit, so a candidate ignore
file or an inline `gitleaks:allow` comment cannot suppress that scan.

## Install (once per clone)

```sh
bash scripts/ensure-githooks.sh         # or: git config core.hooksPath .githooks
git config --get core.hooksPath         # verify the effective path
```

The setup script preserves an existing hook manager, honors
`GITHOOKS_SKIP_INSTALL=1` and exits successfully even if installation fails, so
read its output. `install-githooks.sh` replaces the local hook path instead.
Optional tools: `gitleaks` (entropy detection of unprefixed secrets) and
`exiftool` (media metadata). Without them the gate runs its own checks and says
what it skipped.

## Files

| File | Tracked | Role |
|---|---|---|
| `.githooks/pre-commit`, `.githooks/pre-push` | yes | Entry points that call the gate. |
| `.githooks/_security.sh` | yes | Scanner and policy engine; Bash 3.2 compatible (stock macOS). |
| `.githooks/security.conf` | yes | This repo's blocked files, protected paths, fixture allowlist and checks. |
| `${XDG_CONFIG_HOME:-$HOME/.config}/githooks/private.conf` | **never** | Your identifiers (PII patterns, personal email), shared by installed clones. |
| `.githooks/security.local.conf` | **never** (gitignored) | Same, for one clone only. |

Load order: gate defaults, `security.conf`, `private.conf`, then
`security.local.conf`. Later files append to (`+=`) or replace any value.

## What runs when

**Blocking.** The commit or push is refused.

| Check | pre-commit | pre-push |
|---|---|---|
| Credential-shaped strings: provider prefixes and private-key headers, plus `gitleaks` when installed | staged diff | every commit being published |
| Credential and runtime-state filenames (any case): `.env*`, keys, keystores, `*.db` and its renamed copies, agent session residue (`.aider*`, `settings.local.json`, `*.har`, shell histories) | staged files | every commit being published |
| Ambiguous filenames: Git-quoted paths (quotes, backslashes, control characters) or colons | all changed paths | every commit being published |
| Your identifiers from `private.conf` | staged diff | every commit being published |
| Absolute home-directory paths (`/Users/<name>/`, `/home/<name>/`, `C:\Users\<name>\`); placeholder users pass | staged diff | every commit being published |
| Invisible bidi-override and Unicode tag characters (Trojan Source; hidden instructions in agent files) | staged diff | every commit being published |
| New file over `SEC_MAX_NEW_FILE_KB` (default 2048; `SEC_VIDEO_GLOBS`: `SEC_MAX_NEW_VIDEO_KB`); GPS/author metadata in media (with `exiftool`) | staged | — |
| Removed `.gitignore` rule | staged | every commit being published |
| Personal address in author/committer (`SEC_AUTHOR_EMAIL_DENY`) | every commit | — |
| Protected control file changed or deleted | staged | reminder only |
| Push to, or deletion of, `main`/`master` | — | yes |
| Existing tag update; non-fast-forward branch push | — | yes |

Pre-push scans each commit, not the tip against the base: a key added in one
commit and deleted in the next is still published, so it is refused. That also
catches `--no-verify` commits and commits from hookless clones. Tag annotation
chains and blob/tree targets are scanned too. Spaces and ordinary Unicode names
pass; the ambiguous names above would break the scanners' line records.

**Reminders.** Printed, never blocking. Answer them in the PR body.

| Reminder | Trigger |
|---|---|
| Agent instruction surface changed | `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `SKILL.md`, `.cursorrules`, `.claude/`, `.codex/`, `.cursor/`, `.github/skills/` |
| Dependency or build surface changed | requirements, constraints, `pyproject.toml`, `Cargo.toml`, `package.json`, Dockerfile, compose, workflows |
| Secure-by-design questions | the diff adds a route, outbound call, exec or deserialization path, persisted model, file write or delete, config input, telemetry, or HTML sink |
| Exposure-prone line | TLS verification off, `0.0.0.0`, wildcard CORS, pipe-to-shell, `--no-verify`, `chmod 777`, a secret-named value in a log call |
| `.gitignore` no longer covers a probe path | `SEC_IGNORE_PROBES` |

Content reminders skip tests, docs, examples and Markdown; filename reminders
still apply. Push suppresses ordinary reminders but repeats protected-control notices.

## Overrides: operator only

| Variable | Allows |
|---|---|
| `HOOK_OPERATOR_ACK=1` | a change to or deletion of a protected path, or a removed `.gitignore` rule |
| `ALLOW_MAIN_PUSH=1` | a direct push to a protected branch |
| `ALLOW_FORCE_WITH_LEASE=true` | an existing-tag update or other ref rewrite |

Protected-branch deletion is always refused, including with `ALLOW_MAIN_PUSH=1`.

An agent never sets these. When the gate refuses, the agent stops, shows the
operator the message and the diff, and waits; the operator reruns the command
with the variable, for example `HOOK_OPERATOR_ACK=1 git commit`. The variable
shows in the agent's transcript, so a self-granted override is detectable.

Secrets, blocked files, identifiers, home paths and invisible characters have
no override: fix the content. For a confirmed synthetic fixture, local hooks
accept `gitleaks:allow` or `SEC_ALLOW_REGEX` / `SEC_BLOCKED_EXCEPT` in protected
`security.conf`; CI needs a reviewed `.gitleaks.toml` exception already on the
PR base.

## Privacy setup

Create the private config at the path above and `chmod 600` it:

```sh
SEC_PII_REGEX='your-email-localpart@|555-867-5309|Your Full Name|internal\.employer\.example'
SEC_AUTHOR_EMAIL_DENY='@(personal-isp\.example|gmail\.com)'
```

The identifier list is itself private data, so it is never tracked. On GitHub,
also turn on *Keep my email addresses private* and *Block command line pushes
that expose my email*, the server-side version of the author check.

## Known limits

The gate sees text, not intent; review owns the rest.

- `git commit --no-verify` skips pre-commit; `git push --no-verify` skips pre-push.
- A clone that never set `core.hooksPath` has no gate.
- An agent can set an override variable itself; the gate makes that a visible rule violation, not an impossible one.
- Regex detection misses low-entropy secrets with no known prefix, and flags look-alike fixtures.
- A secret or personal file read in an agent session has already gone to that agent's vendor.

## CGagentHarness specifics

Branch naming and PR title rules stay in `scripts/check-pr-template.sh` and CI.
`.githooks/security.conf` adds:

- **Blocked files:** `.githooks/security.local.conf` and the operator-home files `.gitignore` lists (`auth.json`, `soul.md`, `sessions/`, `tls/`, `memory/`, `logs/`, `data/agentic/` and the rest of `SEC_BLOCKED_GLOBS`), so `git add -f` or a home inside the checkout cannot publish them.
- **Protected (ask first):** `.gitleaks.toml`, `deny.toml` (root and `desktop/`), plus the template's `.githooks/*` and agent tool settings.
- **Core-path reminder:** a change under the [AGENTS.md](../AGENTS.md) core paths prints a reminder that the PR body needs an invariant statement.
- **Repo checks:** staged Rust runs `cargo fmt --all -- --check` on the root working tree (not the index or `desktop/`); every commit runs `scripts/lint-diff.py --staged`. Missing tools skip with a notice.
- **Fixture allowlist:** synthetic AWS, GitHub and Anthropic values in named source and test files.

The coding pipeline runs `git` with `core.hooksPath` pinned to the null device
(`src/agentic/git.rs`), so its commits bypass the gate by design; the
`gitleaks` workflow and review cover them.
