#!/usr/bin/env bash
# cgagentharness-otel-hardening verify — clean-tree pass + mutation self-test.
# Pure stdlib Python (tomllib, 3.11+); no cargo, no network. Exit 0 = healthy.
# A checker that cannot fail proves nothing: every rule that can FAIL or WARN
# (T1-T12) is exercised below by a mutation that must flip it, and each
# mutation asserts it actually changed the file first (CyClaw's verify.sh
# carried two silent-no-op sed bugs before that discipline landed).
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
checker="$here/check_otel.py"

# The date the shipped contract was last reviewed (every INVENTORY row's
# `reviewed`). The strict clean-tree run pins it so this script stays
# deterministic; the default run uses today so real staleness still surfaces
# as WARN/exit 0 without breaking the script.
AS_OF="2026-10-03"

echo "== cgagentharness-otel-hardening verify =="

if python3 "$checker" --repo-root "$repo_root" >/tmp/cgah_otel_live.txt 2>&1; then
  echo "clean tree (default): PASS (exit 0)"
else
  echo "clean tree (default): FAIL — the shipped kill-switch contract is broken" >&2
  cat /tmp/cgah_otel_live.txt >&2
  exit 1
fi
if python3 "$checker" --repo-root "$repo_root" --strict --as-of "$AS_OF" >/tmp/cgah_otel_strict.txt 2>&1; then
  echo "clean tree (--strict @ $AS_OF): PASS (exit 0)"
else
  echo "clean tree (--strict @ $AS_OF): FAIL — a WARN-class regression landed" >&2
  cat /tmp/cgah_otel_strict.txt >&2
  exit 1
fi

# Fresh temp tree carrying everything the checker reads. A partial tree would
# fail for the wrong reason and mask what a mutation actually proved.
_mktree() {
  local d; d="$(mktemp -d)"
  mkdir -p "$d/desktop" "$d/finetune"
  cp "$repo_root/Cargo.toml" "$repo_root/Cargo.lock" "$d/"
  cp "$repo_root/desktop/Cargo.toml" "$repo_root/desktop/Cargo.lock" "$d/desktop/"
  cp -r "$repo_root/src" "$d/src"
  cp -r "$repo_root/desktop/src" "$d/desktop/src"
  cp "$repo_root/finetune/requirements.txt" "$d/finetune/"
  echo "$d"
}

fails=0

# _expect NAME EXPECT_RC GREP_PATTERN [extra checker args]; tree is $a.
_expect() {
  local name="$1" expect_rc="$2" pattern="$3" extra="${4:-}"
  local out rc
  out="$(python3 "$checker" --repo-root "$a" --as-of "$AS_OF" $extra 2>&1)"; rc=$?
  if [ "$rc" -eq "$expect_rc" ] && echo "$out" | grep -q "$pattern"; then
    echo "$name: PASS"
  else
    echo "$name: FAIL — expected rc=$expect_rc + /$pattern/, got rc=$rc" >&2
    echo "$out" >&2
    fails=$((fails + 1))
  fi
  rm -rf "$a"
}

# _mutate FILE PYTHON_SNIPPET — applies the snippet and asserts it changed the file.
_mutate() {
  local file="$1" snippet="$2"
  python3 - "$file" <<PYEOF
import sys
path = sys.argv[1]
before = open(path, encoding="utf-8").read()
text = before
$snippet
assert text != before, "mutation was a silent no-op -- update this scenario"
open(path, "w", encoding="utf-8").write(text)
PYEOF
}

# T1: a builder function disappears.
a="$(_mktree)"
_mutate "$a/src/agentic/gh_client.rs" '
text = text.replace("pub fn gh_env(", "pub fn gh_env_removed(", 1)'
_expect "T1 builder-removed mutation" 2 "FAIL  \[T1\].*agentic-gh"

# T2: VALUE flip in the executor env (key survives; only the value oracle sees it).
a="$(_mktree)"
_mutate "$a/src/agentic/executor/runner.rs" '
text = text.replace("(\"GH_TELEMETRY\", \"false\")", "(\"GH_TELEMETRY\", \"true\")", 1)'
_expect "T2 runner value-flip mutation" 2 "FAIL  \[T2\] executor-runner: GH_TELEMETRY"

# T2: dropped pair.
a="$(_mktree)"
_mutate "$a/src/agentic/executor/runner.rs" '
text = text.replace("        (\"DO_NOT_TRACK\", \"1\"),\n", "", 1)'
_expect "T2 runner dropped-pair mutation" 2 "FAIL  \[T2\] executor-runner: missing DO_NOT_TRACK"

# T2: insert-form flip (gh_env uses .insert("K".into(), "V".into())).
a="$(_mktree)"
_mutate "$a/src/agentic/gh_client.rs" '
text = text.replace("env.insert(\"GH_TELEMETRY\".into(), \"false\".into());", "env.insert(\"GH_TELEMETRY\".into(), \"log\".into());", 1)'
_expect "T2 gh_env insert-flip mutation" 2 "FAIL  \[T2\] agentic-gh: GH_TELEMETRY"

# T2: .env()-form flip on the desktop sidecar.
a="$(_mktree)"
_mutate "$a/desktop/src/backend.rs" '
text = text.replace(".env(\"RUSTUP_AUTO_INSTALL\", \"0\")", ".env(\"RUSTUP_AUTO_INSTALL\", \"1\")", 1)'
_expect "T2 desktop sidecar value-flip mutation" 2 "FAIL  \[T2\] desktop-sidecar: RUSTUP_AUTO_INSTALL"

# T2: an unclassified literal pair joins a builder (a proxy pointed at a child).
a="$(_mktree)"
_mutate "$a/src/agentic/executor/runner.rs" '
text = text.replace("        (\"NO_PROXY\", \"*\"),\n", "        (\"NO_PROXY\", \"*\"),\n        (\"HTTP_PROXY\", \"http://127.0.0.1:8080\"),\n", 1)'
_expect "T2 runner unexpected-pair mutation" 2 "FAIL  \[T2\] executor-runner: unexpected literal pair HTTP_PROXY"

# T3: inherit allowlist widened.
a="$(_mktree)"
_mutate "$a/src/agentic/executor/runner.rs" '
text = text.replace("    \"PYTHONIOENCODING\",\n]", "    \"PYTHONIOENCODING\",\n    \"HTTPS_PROXY\",\n]", 1)'
_expect "T3 allowlist-widened mutation" 2 "FAIL  \[T3\] runner ALLOWED_ENV_VARS: unexpected"

# T3: secret strip list shrunk.
a="$(_mktree)"
_mutate "$a/src/common/mcp.rs" '
text = text.replace("    \"GH_TOKEN\",\n    \"GITHUB_TOKEN\",\n", "    \"GITHUB_TOKEN\",\n", 1)'
_expect "T3 strip-list-shrunk mutation" 2 "FAIL  \[T3\] mcp SECRET_ENV: missing"

# T4: staleness via a future --as-of: WARN by default, exit 2 under --strict.
a="$(_mktree)"
out="$(python3 "$checker" --repo-root "$a" --as-of 2027-12-31 2>&1)"; rc=$?
if [ "$rc" -eq 0 ] && echo "$out" | grep -q "WARN  \[T4\]"; then
  echo "T4 future --as-of (default): PASS (WARN, exit 0)"
else
  echo "T4 future --as-of (default): FAIL — expected WARN/exit 0, got rc=$rc" >&2; echo "$out" >&2
  fails=$((fails + 1))
fi
out="$(python3 "$checker" --repo-root "$a" --as-of 2027-12-31 --strict 2>&1)"; rc=$?
if [ "$rc" -eq 2 ]; then
  echo "T4 future --as-of (--strict): PASS (escalated to exit 2)"
else
  echo "T4 future --as-of (--strict): FAIL — expected exit 2, got rc=$rc" >&2
  fails=$((fails + 1))
fi
rm -rf "$a"

# T5: a telemetry SDK crate enters the lock graph.
a="$(_mktree)"
_mutate "$a/Cargo.lock" '
text = text + "\n[[package]]\nname = \"opentelemetry\"\nversion = \"0.30.0\"\n"'
_expect "T5 telemetry-crate-in-lock mutation" 2 "FAIL  \[T5\] Cargo.lock.*opentelemetry"

# T6: a verified network crate drifts (pattern-matched bump, never a literal).
a="$(_mktree)"
_mutate "$a/Cargo.lock" '
import re
text, n = re.subn(r"name = \"reqwest\"\nversion = \"[0-9.]+\"", "name = \"reqwest\"\nversion = \"0.12.999\"", text)
assert n == 1'
_expect "T6 crate-pin-drift mutation" 0 "WARN  \[T6\] Cargo.lock: reqwest"

# T7: a spawn appears in an unlisted file (prepended: the test tail is stripped).
a="$(_mktree)"
_mutate "$a/src/common/home.rs" '
text = "fn zz_spawn() { let _ = std::process::Command::new(\"curl\"); }\n" + text'
_expect "T7 new-spawn-site (default)" 0 "WARN  \[T7\] src/common/home.rs: new spawn site"
a="$(_mktree)"
_mutate "$a/src/common/home.rs" '
text = "fn zz_spawn() { let _ = std::process::Command::new(\"curl\"); }\n" + text'
_expect "T7 new-spawn-site (--strict)" 2 "FAIL  \[T7\] src/common/home.rs: new spawn site" "--strict"

# T7: the server grows a spawn (I6 process map says it never does).
a="$(_mktree)"
_mutate "$a/src/server/state.rs" '
text = "fn zz_spawn() { let _ = std::process::Command::new(\"gh\"); }\n" + text'
_expect "T7 server-spawn mutation" 2 "FAIL  \[T7\] src/server/state.rs"

# T8: a kill name is removed from the parent env programmatically.
a="$(_mktree)"
_mutate "$a/src/common/home.rs" '
text = "fn zz_bypass() { std::env::remove_var(\"DO_NOT_TRACK\"); }\n" + text'
_expect "T8 remove_var bypass mutation" 2 "FAIL  \[T8\] src/common/home.rs: removes DO_NOT_TRACK"

# T9: a gh spawn stops using gh_env() (inherits the parent verbatim). T2 stays
# green -- gh_env() itself is untouched -- so only the wiring check sees it.
a="$(_mktree)"
_mutate "$a/src/agentic/gh_client.rs" '
text = text.replace("    let env = gh_env();\n", "    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();\n", 1)'
_expect "T9 gh spawn bypasses builder mutation" 2 "FAIL  \[T9\] src/agentic/gh_client.rs.*bypasses"

# T9: a RunSpec inherits (`env: None`) on the writer path.
a="$(_mktree)"
_mutate "$a/src/agentic/writer.rs" '
text = text.replace("        env: Some(&env),\n", "        env: None,\n", 1)'
_expect "T9 writer env-None mutation" 2 "FAIL  \[T9\] src/agentic/writer.rs.*inherits"

# T10: an unclassified direct dependency lands.
a="$(_mktree)"
_mutate "$a/Cargo.toml" '
text = text.replace("[dependencies]\n", "[dependencies]\nzz-unknown-crate = \"1\"\n", 1)'
_expect "T10 unclassified dependency (default)" 0 "WARN  \[T10\] unclassified component Cargo.toml: zz-unknown-crate"
a="$(_mktree)"
_mutate "$a/Cargo.toml" '
text = text.replace("[dependencies]\n", "[dependencies]\nzz-unknown-crate = \"1\"\n", 1)'
_expect "T10 unclassified dependency (--strict)" 2 "FAIL  \[T10\] unclassified component" "--strict"

# T10: a dynamic launcher site loses its marker (row must be retired, not left
# describing a surface that is gone). The replacement must not contain the
# original marker as a substring, or the check still sees it.
a="$(_mktree)"
_mutate "$a/src/shim/mod.rs" '
text = text.replace("run_argv", "zz_argv_runner")
assert "run_argv" not in text'
_expect "T10 dynamic-launcher-gone mutation" 0 "WARN  \[T10\] dynamic launcher .shim-agentic-child.*no longer carries"

# T11: the OS opener stops clearing its environment.
a="$(_mktree)"
_mutate "$a/desktop/src/main.rs" '
text = text.replace("                    .env_clear()\n                    .env(\"PATH\", \"/usr/bin:/bin\")", "                    .env(\"PATH\", \"/usr/bin:/bin\")", 1)'
_expect "T11 opener env_clear-dropped mutation" 2 "FAIL  \[T11\] desktop/src/main.rs"

# T12: the bwrap wrapper stops unsharing the network namespace.
a="$(_mktree)"
_mutate "$a/src/common/sandbox_wrap.rs" '
text = text.replace("\"--unshare-net\"", "\"--share-net\"", 1)'
_expect "T12 sandbox network-denial-dropped mutation" 2 "FAIL  \[T12\] bwrap"

echo
if [ "$fails" -eq 0 ]; then
  echo "cgagentharness-otel-hardening verify: ALL PASS"
  exit 0
else
  echo "cgagentharness-otel-hardening verify: $fails mutation test(s) FAILED" >&2
  exit 1
fi
