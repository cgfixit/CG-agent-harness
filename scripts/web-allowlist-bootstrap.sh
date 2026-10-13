#!/usr/bin/env bash
# Grant the operator's default research allowlist through the running portal.
#
# - Uses the supported CLI (`cgagentharness web allow`): every grant is
#   validated, written atomically (0600) and audited. No hand-written JSON.
# - Idempotent: re-granting the same pattern in the same group replaces that
#   rule, so re-running never duplicates or grows the list.
# - Each `allow` line is all-or-nothing; the precheck below refuses to start
#   when web.max_rules cannot hold the whole list.
# - Needs: a running server (scripts/start-web.sh), an admin CLI session
#   (`cgagentharness account login <admin>`), and web.max_rules >= the total.
#
# Env: CGAH_BIN (binary path), CGAH_URL (portal origin; omit to use the CLI default),
#      CGAH_GRANTS_FILE (optional local file of extra "<group>|<patterns>|<seeds>"
#      lines, one per line, '#' comments allowed; keep personal lists there, not here).
#
# The built-in list is technical references only (14 rules). If the total
# exceeds web.max_rules (shipped default 64; homes seeded earlier may keep 32),
# raise it in the home config.yaml and restart the server.
set -euo pipefail
BIN=${CGAH_BIN:-cgagentharness}
url_args=()
[ -n "${CGAH_URL:-}" ] && url_args=(--url "$CGAH_URL")
# ${arr[@]+"${arr[@]}"}: Bash 3.2 (macOS default) treats an empty array as unset under set -u.
web() { "$BIN" web ${url_args[@]+"${url_args[@]}"} "$@"; }
# Each entry: "<group>|<pattern> <pattern>...|<seed> <seed>..."
# Patterns are quoted data here, so the shell never globs the `*`.
GRANTS=(
  "rust|https://doc.rust-lang.org/* https://docs.rs/* https://rustsec.org/*|https://doc.rust-lang.org/book/ https://doc.rust-lang.org/std/ https://doc.rust-lang.org/reference/ https://doc.rust-lang.org/cargo/ https://docs.rs/tokio/latest/tokio/ https://docs.rs/axum/latest/axum/ https://docs.rs/serde/latest/serde/ https://docs.rs/reqwest/latest/reqwest/ https://docs.rs/rusqlite/latest/rusqlite/ https://docs.rs/tantivy/latest/tantivy/ https://rustsec.org/advisories/"
  "code|https://developer.mozilla.org/* https://docs.python.org/* https://learn.microsoft.com/en-us/* https://docs.github.com/* https://docs.ollama.com/*|https://developer.mozilla.org/en-US/docs/Web/HTTP https://docs.python.org/3/library/ https://learn.microsoft.com/en-us/powershell/scripting/overview https://docs.github.com/en/actions https://docs.ollama.com/"
  "tech|https://www.rfc-editor.org/* https://owasp.org/* https://cheatsheetseries.owasp.org/* https://nvd.nist.gov/* https://arxiv.org/*|https://www.rfc-editor.org/rfc/rfc9110.html https://owasp.org/www-project-top-ten/ https://cheatsheetseries.owasp.org/IndexTopTen.html https://nvd.nist.gov/vuln https://arxiv.org/list/cs.CR/recent"
  "ref|https://en.wikipedia.org/wiki/*|https://en.wikipedia.org/wiki/Main_Page"
  # Personal lists: put lines like these in CGAH_GRANTS_FILE instead.
  # At most 32 patterns and 16 seeds per line; a wildcard-host pattern
  # (https://*.example.com/*) needs a seed, and never covers the apex.
  # "onetab|https://example.org/* https://docs.example.net/*|https://example.org/"
)
if [ -n "${CGAH_GRANTS_FILE:-}" ]; then
  [ -r "$CGAH_GRANTS_FILE" ] || { echo "allowlist-bootstrap: cannot read CGAH_GRANTS_FILE" >&2; exit 2; }
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    GRANTS+=("$line")
  done <"$CGAH_GRANTS_FILE"
fi
# Precheck: the CLI must be signed in, and the cap must hold every new rule.
status_json=$(web status) || { echo "allowlist-bootstrap: not signed in or server down; run: $BIN account ${CGAH_URL:+--url $CGAH_URL} login <admin>" >&2; exit 3; }
need=$(STATUS="$status_json" python3 -I - "${GRANTS[@]}" <<'PY'
import json, os, sys
status = json.loads(os.environ["STATUS"])
have = {(r["group"], r["pattern"]) for r in status.get("rules", [])}
new = set()
for entry in sys.argv[1:]:
    group, patterns, _ = entry.split("|")
    for p in patterns.split():
        if (group, p) not in have:
            new.add((group, p))
cap = status.get("max_allow", 32)
total = len(have) + len(new)
print(f"{len(new)} {total} {cap}")
PY
)
read -r new total cap <<<"$need"
echo "allowlist-bootstrap: $new new rule(s); total after run $total; web.max_rules $cap"
if [ "$total" -gt "$cap" ]; then
  echo "allowlist-bootstrap: refusing: $total rules exceed web.max_rules=$cap. Raise it in config.yaml and restart." >&2
  exit 4
fi
for entry in "${GRANTS[@]}"; do
  IFS='|' read -r group patterns seeds <<<"$entry"
  read -r -a pats <<<"$patterns"
  args=(allow "${pats[@]}" --group "$group")
  read -r -a seed_list <<<"$seeds"
  for s in ${seed_list[@]+"${seed_list[@]}"}; do args+=(--seed "$s"); done
  web "${args[@]}" >/dev/null
  echo "  granted: $group (${#pats[@]} pattern(s))"
done
web status | python3 -I -c 'import json, sys
d = json.load(sys.stdin)
print("allowlist-bootstrap: done: %d of %s rules" % (len(d.get("rules", [])), d.get("max_allow")))'
