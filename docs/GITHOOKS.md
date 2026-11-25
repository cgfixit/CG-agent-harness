# Git hooks security gate

## Install (once per clone)

```sh
bash scripts/ensure-githooks.sh
git config --get core.hooksPath
```

Expect `.githooks` or inspect the existing hook manager. The installer
preserves a configured manager and honors `GITHOOKS_SKIP_INSTALL=1`.
Optional `gitleaks` and `exiftool` add checks. Read notices about skips.

## What runs when

Pre-commit scans the index. Pre-push scans each published commit, including
intermediate commits that add then remove a secret. Blocked classes include
credential material and filenames, session residue, private identifiers,
absolute user paths, ambiguous filenames, invisible control characters and
removed ignore rules. New file-size and media-metadata checks are staged.
Pre-push also guards protected branches, existing tags and rewritten history.
Quoted, colon-containing or control-character paths are refused because
the scanner uses line-oriented Git records. Ordinary space and Unicode
names are supported. The staged author-email check can enforce the user's
private denylist independently from GitHub email privacy policy.

The [gate](../.githooks/_security.sh) loads defaults, repository
[policy](../.githooks/security.conf), user-private configuration at
`${XDG_CONFIG_HOME:-$HOME/.config}/githooks/private.conf`, then ignored
`.githooks/security.local.conf`. Keep identifiers and email patterns in the
private file with mode 0600. Review protected paths and fixture allowances
in the policy.

Staged Rust runs root formatting. Every commit runs `scripts/lint-diff.py
--staged` when available. Agent instruction, dependency and core-path
changes can print reminders for the PR. Coding-pipeline Git pins its hook
path to the null device. GitHub gitleaks uses the immutable PR-base config
and excludes candidate inline allowances. Hook and server-side coverage are
independent. An uninstalled or explicitly skipped hook has no local
coverage. Prefix checks alone miss some unprefixed credentials.
Review the staged files and inspect any gitleaks or metadata skip before
relying on the result.

## Overrides: operator only

| Variable | Acknowledges |
|---|---|
| `HOOK_OPERATOR_ACK=1` | Protected control-file change or removed ignore rule |
| `ALLOW_MAIN_PUSH=1` | Direct protected-branch push |
| `ALLOW_FORCE_WITH_LEASE=true` | Existing tag update or ref rewrite |

Protected-branch deletion always refuses. Agents surface the refusal and
affected diff for the operator. Fix blocked secrets, identifiers, home
paths and invisible characters in the content. Synthetic fixture exceptions
require review under the matching local and hosted policies.
