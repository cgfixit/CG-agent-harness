#!/usr/bin/env bash
# Local PR-body gate against .github/PULL_REQUEST_TEMPLATE.md minimums.
#
# Usage:
#   scripts/check-pr-template.sh PATH/TO/body.md
#   gh pr view --json body -q .body | scripts/check-pr-template.sh -
#   CGAGENTHARNESS_PR_BODY_FILE=body.md scripts/check-pr-template.sh
#   CYCLAW_PR_BODY_FILE=body.md scripts/check-pr-template.sh   # fallback alias
#
# Exit 0 = ok; exit 1 = missing required sections.
# Git hooks cannot intercept GitHub API / gh pr create bodies — agents and
# humans should run this before opening a PR. The script and CI
# (.github/workflows/pr-template-check.yml) enforce the same rules, in
# lock-step, including the Cursor footer strip below.
#
# The core-path rule is mirrored from that workflow too: when the change
# touches src/shim/, the guard or header layers, writer, sandbox, workspace,
# or the shipped config, the body must mention an invariant. The changed-file
# list comes from CGAGENTHARNESS_PR_FILES (newline-separated) when set,
# otherwise from the working tree against the merge base with
# CGAGENTHARNESS_PR_BASE (default origin/main). With neither source the rule
# is reported as skipped rather than silently passed.
set -euo pipefail

input="${1:-${CGAGENTHARNESS_PR_BODY_FILE:-${CYCLAW_PR_BODY_FILE:-}}}"
if [[ -z "$input" ]]; then
  printf '%s\n' \
    "usage: scripts/check-pr-template.sh <body.md|->" \
    "   or: CGAGENTHARNESS_PR_BODY_FILE=body.md scripts/check-pr-template.sh" \
    "   or: CYCLAW_PR_BODY_FILE=body.md scripts/check-pr-template.sh  # fallback alias" \
    >&2
  exit 2
fi

if [[ "$input" == "-" ]]; then
  body="$(cat)"
elif [[ -f "$input" ]]; then
  body="$(cat "$input")"
else
  printf 'check-pr-template: file not found: %s\n' "$input" >&2
  exit 2
fi

# Cursor cloud agents append an HTML footer. The first line that is exactly
# `<!-- CURSOR_AGENT_PR_BODY_END -->` (leading and trailing space, tab,
# vertical tab, or form feed on that line only; CR is removed first) is the
# cut. That line and everything after it are ignored. Every check below —
# required headings, the merge-order heading, `## ELI5` as the last heading,
# the core-path invariant statement, the 40-character floor, and the Last
# updated stamp as the last non-blank line — reads only the text above the
# cut. Text below the marker cannot satisfy any of those rules. A marker
# before a required section makes that section missing. A stamp below the
# marker is discarded and fails. With no such line the body is unchanged.
# `<!-- CURSOR_AGENT_PR_BODY_BEGIN -->` is not a footer and is left in place.
if stripped="$(printf '%s\n' "$body" | tr -d '\r' | awk '
  {
    t = $0
    gsub(/^[ \t\v\f]+|[ \t\v\f]+$/, "", t)
    if (t == "<!-- CURSOR_AGENT_PR_BODY_END -->") { found = 1; exit }
    print
  }
  END { if (!found) exit 2 }
')"; then
  body="$stripped"
fi

fail=0
missing=()

# Matches read here-strings, never `printf | grep -q`: grep -q exits on the
# first match, a still-writing printf then takes SIGPIPE, and pipefail turns
# a present section into a reported-missing one under load.
require_header() {
  local label="$1"
  local pattern="$2"
  if ! grep -Eiq "$pattern" <<<"$body"; then
    missing+=("$label")
    fail=1
  fi
}

# Align with template + advisory CI loose matching, but require the
# CGagentHarness-named sections that Grok Build / agents must fill.
require_header "Proposed changes (or Why/Benefits/Summary)" \
  '^#{1,4}[[:space:]]*(proposed changes|benefits|why|summary|what)\b'
require_header "Types of changes" \
  '^#{1,4}[[:space:]]*types of changes\b'
require_header "Benefits / why" \
  '^#{1,4}[[:space:]]*(benefits|why)\b'
require_header "Risks to monitor" \
  '^#{1,4}[[:space:]]*risks?([[:space:]]*(to[[:space:]]*monitor|impact))?\b'
require_header "Checklist" \
  '^#{1,4}[[:space:]]*checklist\b'
require_header "Suggested merge order of open PRs" \
  '^##[[:space:]]+suggested merge order of open prs[[:space:]]*$'

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
shipped_template="$repo_root/.github/PULL_REQUEST_TEMPLATE.md"
body_unix="$(printf '%s\n' "$body" | tr -d '\r')"
if ! grep -Eq '^## ELI5[[:space:]]*$' <<<"$body_unix"; then
  missing+=("ELI5 (## ELI5 must be the last section heading)")
  fail=1
else
  eli5_tail="$(printf '%s\n' "$body_unix" | awk '
    /^## ELI5[[:space:]]*$/ { buf = ""; seen = 1; next }
    seen { buf = buf $0 ORS }
    END { printf "%s", buf }
  ')"
  if grep -Eq '^#{1,6}[[:space:]]+' <<<"$eli5_tail"; then
    missing+=("ELI5 must be the last section heading")
    fail=1
  fi
fi
last_nonempty="$(printf '%s\n' "$body_unix" | sed -e 's/[[:space:]]*$//' | awk 'NF { line = $0 } END { print line }')"
filled_stamp='^Last updated: [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2} ET$'
placeholder_stamp='^Last updated: YYYY-MM-DD HH:MM ET$'
stamp_ok=0
if grep -Eq "$filled_stamp" <<<"$last_nonempty"; then
  stamp_ok=1
elif [[ "$input" != "-" && -f "$input" ]]; then
  input_abs="$(cd "$(dirname "$input")" && pwd)/$(basename "$input")"
  if [[ "$input_abs" == "$shipped_template" ]] \
    && grep -Eq "$placeholder_stamp" <<<"$last_nonempty"; then
    stamp_ok=1
  fi
fi
if [[ "$stamp_ok" -ne 1 ]]; then
  missing+=("Last updated timestamp (last line, YYYY-MM-DD HH:MM ET)")
  fail=1
fi

# CI measures the trimmed body; a whitespace-padded stub must not pass here
# and fail there.
trimmed="${body#"${body%%[![:space:]]*}"}"
trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
if [[ "${#trimmed}" -lt 40 ]]; then
  missing+=("Body too short (< 40 chars)")
  fail=1
fi

# Core-path rule (same file set as pr-template-check.yml).
# `CGAGENTHARNESS_PR_FILES`, when set, is the changed-file list in place of
# `git diff`. It does not skip the template comparison: that comparison runs
# below whether or not the variable is set.
core_pattern='^(src/shim/|src/server/guards\.rs$|src/server/headers\.rs$|src/agentic/writer\.rs$|src/agentic/executor/sandbox\.rs$|src/agentic/workspace\.rs$|assets/config\.default\.yaml$)'
base="${CGAGENTHARNESS_PR_BASE:-origin/main}"
merge_base="$(git -C "$repo_root" merge-base "$base" HEAD 2>/dev/null || true)"
changed=""
files_source=""
if [[ -n "${CGAGENTHARNESS_PR_FILES:-}" ]]; then
  changed="$CGAGENTHARNESS_PR_FILES"
  files_source="CGAGENTHARNESS_PR_FILES"
elif [[ -n "$merge_base" ]]; then
  # --no-renames: a core file moved elsewhere must still surface its source
  # path, not only the destination Git's rename detection would report.
  # Untracked, non-ignored files are appended: `git diff` omits a new core
  # file until it is added, but CI's pulls.listFiles sees it once committed,
  # so the local gate must count it now to predict CI. Do not `|| true`: an
  # empty list with files_source set would skip the core-path rule while
  # claiming it ran.
  if changed="$(git -C "$repo_root" diff --name-only --no-renames "$merge_base")" \
    && untracked="$(git -C "$repo_root" ls-files --others --exclude-standard)"; then
    changed="$changed"$'\n'"$untracked"
    files_source="git diff against $base merge base plus untracked files"
  fi
fi
# Template comparison runs on every invocation. `CGAGENTHARNESS_PR_FILES`
# only chooses the changed-file list; it does not turn this comparison off.
# CI fetches the template on every run (pr-template-check.yml). Compare
# against the copy the BASE branch ships (the tip of $base, not the merge
# base), so a template changed on main after the fork is still the one
# judged. A branch that edits the template itself must not turn deleted
# boilerplate into contributor-written evidence. Only a base ref that does
# not resolve at all (shallow clone, unfetched remote) falls back to the
# working tree, and that fallback is reported. A base ref that resolves but
# lacks the template is CI's failure case and fails closed here too: a
# missing template is a visible warning and a non-zero exit, never a pass.
template_path=".github/PULL_REQUEST_TEMPLATE.md"
template_text=""
template_source=""
template_missing=""
if git -C "$repo_root" rev-parse --verify -q "$base^{commit}" >/dev/null; then
  # Resolve the blob through ls-tree, not `git show base:path`: Git for
  # Windows' bash rewrites a colon-joined argument like that one before git.exe
  # sees it. Do not set MSYS_NO_PATHCONV: `git -C "$repo_root"` needs the
  # /tmp-style path translated. git's own error is kept in the message, so a
  # failure names its cause.
  tree_line=""
  if tree_line="$(git -C "$repo_root" ls-tree "$base" -- "$template_path" 2>&1)" \
    && blob="$(awk 'NR == 1 && $2 == "blob" { print $3 }' <<<"$tree_line")" \
    && [[ -n "$blob" ]] \
    && template_text="$(git -C "$repo_root" cat-file blob "$blob" 2>&1)"; then
    template_source="template at $base"
  else
    template_missing="$template_path is absent at $base; refusing to pass (CI fails the same way)"
    [[ -n "$tree_line" ]] && template_missing+=" [git: ${tree_line%%$'\n'*}]"
  fi
elif [[ -f "$repo_root/$template_path" ]]; then
  template_text="$(cat "$repo_root/$template_path")"
  template_source="working-tree template"
  printf 'check-pr-template: warning: %s does not resolve; comparing against the working-tree template (CI uses the base branch copy)\n' "$base" >&2
else
  template_missing="PR template not readable; refusing to pass"
fi
if [[ -n "$template_missing" || -z "$template_text" ]]; then
  template_missing="${template_missing:-PR template not readable; refusing to pass}"
  printf 'check-pr-template: warning: %s\n' "$template_missing" >&2
  missing+=("$template_missing")
  fail=1
fi

if [[ -z "$files_source" ]]; then
  printf 'check-pr-template: core-path rule skipped (set CGAGENTHARNESS_PR_FILES or fetch %s)\n' "$base" >&2
elif grep -Eq "$core_pattern" <<<"$changed"; then
  # The template itself says "invariant" in its headings and checklist, so
  # only contributor-written lines (those not copied verbatim from the
  # template) can satisfy the statement requirement.
  # Only the "Invariant / Governance Impact" section counts (its heading must
  # start the line, so prose that merely mentions the phrase is not a
  # heading), after folding
  # `- [x]` to `- [ ]` and collapsing whitespace on both sides. Lines copied
  # from the template are dropped; on the heading line itself the template's
  # own words are removed and at least eight characters must remain, so a
  # concise statement on that line passes while a reworded heading does not;
  # only that remainder (never the heading's own "Invariant") is judged.
  # Without that heading, any non-template line mentioning an invariant
  # counts. Light edits inside the template's instruction text are not
  # detected; a reviewer reads the section either way.
  # This comparison uses the template loaded above, including when
  # CGAGENTHARNESS_PR_FILES supplied the file list.
  if [[ -n "$template_text" ]]; then
    fold_boxes() { sed -E 's/^([[:space:]]*- \[)[xX](\])/\1 \2/; s/[[:space:]]+/ /g; s/^ //; s/ $//'; }
    contributed="$(awk '
      NR == FNR { tmpl[$0] = 1; next }
      {
        line = $0; low = tolower(line)
        heading = (low ~ /^[#*_ ]*invariant \/ governance impact/)
        if (!insec) { if (heading) { insec = 1; seen = 1 } else { if (!(line in tmpl) && low ~ /invariant/) other[n++] = line; next } }
        else if (line ~ /^(---|#)/) { insec = 0; next }
        if (line in tmpl) next
        if (heading) {
          r = low
          gsub(/invariant \/ governance impact/, "", r)
          gsub(/\(required for any change touching core paths\):?/, "", r)
          s = r; gsub(/[*: ]+/, "", s)
          if (length(s) >= 8) print r
        } else print line
      }
      END { if (!seen) for (i = 0; i < n; i++) print other[i] }
    ' <(printf '%s\n' "$template_text" | fold_boxes) <(printf '%s\n' "$body" | fold_boxes))"
    # The statement must name a guarantee from INVARIANTS.md (or say none),
    # not merely occupy the section. Wording beyond that is for the reviewer.
    guarantee='invariant|\bnone\b|\bi6\b|process isolation|guard chain|write[ -]gate|clone jail|judged|approval|secret|redact|detached|csrf|sandbox|loopback|weaker than'
    if ! grep -Eiq "$guarantee" <<<"$contributed"; then
      missing+=("Invariant / Governance Impact statement (a core path changed; say which invariant and why it holds)")
      fail=1
    fi
  fi
fi

if [[ "$fail" -ne 0 ]]; then
  printf '%s\n' \
    "check-pr-template: PR body is missing required sections from" \
    "  .github/PULL_REQUEST_TEMPLATE.md" \
    "" \
    "Missing:" \
    >&2
  for m in "${missing[@]}"; do
    printf '  - %s\n' "$m" >&2
  done
  printf '%s\n' \
    "" \
    "Fill the full template before: gh pr create / GitHub connector create_pull_request" \
    "Grok Build branch prefix: grok/<feature>" \
    "Title format: [prefix] - Short descriptive sentence" \
    >&2
  exit 1
fi

# A pass names the template that was compared. The old success line defaulted
# to "template not compared" whenever the core-path branch did not set
# template_source, including when CGAGENTHARNESS_PR_FILES listed no core path.
# That report is not a pass.
if [[ -z "$template_source" ]]; then
  printf 'check-pr-template: warning: template was not compared; refusing to pass\n' >&2
  exit 1
fi
printf 'check-pr-template: OK — required template sections present (%s; %s)\n' "${files_source:-core-path rule skipped}" "$template_source"
exit 0
