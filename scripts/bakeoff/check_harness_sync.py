#!/usr/bin/env python3
"""Fail when the harness source and the bench's mirrors of it disagree.

bakeoff_harness.py drives the real binary, but qwen_needle_test.py emulates compaction in Python and the harness
lane parses login, config and route shapes. This reads the checked-out harness (default: the repo this folder is
in) and checks each mirrored fact against the code. Run it after every `git pull` before trusting a bench:

  python3 scripts/bakeoff/check_harness_sync.py                 # this checkout
  python3 scripts/bakeoff/check_harness_sync.py --fetch         # also fetch origin/main and say how far behind HEAD is

It detects what the checkout contains and checks the matching mirrors:
  legacy compaction  the layout on main at LEGACY_REF (49463b7, the 2026-10-14 bench). Checked against the tree
                     when the tree still has it, otherwise against `git show LEGACY_REF:...`.
  main (#513-#516)   prompt_messages framing, tagged summarizer input, head+tail clip, repair (whole-token grounding,
                     short-quote rule, carried corrections), token-sized tail, trigger vs hard limit, no_gain and
                     summary_unusable skips, the compaction report. PR #517 (window-scaled trigger) is read from the
                     profile at run time and needs no mirror at the 32768 contract window.
  fact ledger        with_fact_ledger and its reserve, when the stacked patch is present.
Exit 0: in sync. Exit 1: a mirror drifted (the line names the file to update). Stdlib only.
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
LEGACY_REF = "49463b7"
MAIN_REFS = {"#513": "0d144c7", "#514": "ba16fe5", "#515": "90ebbb3", "#516": "8c07a04"}


def rust_literal(src, i):
    """Parse the Rust string literal starting at src[i] (", r", r#"). Returns (value, index after it)."""
    if src.startswith('r#"', i):
        j = src.index('"#', i + 3)
        return src[i + 3:j], j + 2
    if src.startswith('r"', i):
        j = src.index('"', i + 2)
        return src[i + 2:j], j + 1
    assert src[i] == '"', src[i:i + 20]
    out, j = [], i + 1
    simple = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}
    while src[j] != '"':
        c = src[j]
        if c != "\\":
            out.append(c)
            j += 1
            continue
        nxt = src[j + 1]
        if nxt == "\n":  # line continuation: skip the newline and the next line's leading whitespace
            j += 2
            while src[j] in " \t\n\r":
                j += 1
        elif nxt == "u":
            k = src.index("}", j)
            out.append(chr(int(src[j + 3:k], 16)))
            j = k + 1
        else:
            out.append(simple[nxt])
            j += 2
    return "".join(out), j + 1


def rust_str(src, name):
    m = re.search(rf"const {name}: &str = ", src)
    return rust_literal(src, m.end())[0] if m else None


def rust_num(src, name):
    m = re.search(rf"const {name}: (?:u64|usize) = ([0-9_]+);", src)
    return m and int(m.group(1).replace("_", ""))


def rust_str_array(src, name):
    m = re.search(rf"const {name}: \[&str; \d+\] = \[", src)
    if not m:
        return None
    out, j = [], m.end()
    while True:
        while src[j] in " \t\n\r,":
            j += 1
        if src[j] == "]":
            return out
        val, j = rust_literal(src, j)
        out.append(val)


def rust_fn_format(src, fn):
    """The first format! literal inside `fn <fn>(`."""
    m = re.search(rf"fn {fn}\(", src)
    if not m:
        return None
    k = src.index("format!(", m.end())
    j = k + len("format!(")
    while src[j] in " \t\n\r":
        j += 1
    return rust_literal(src, j)[0]


def rust_regex(src, fn):
    """The pattern passed to Regex::new inside `fn <fn>(`."""
    m = re.search(rf"fn {fn}\(", src)
    if not m:
        return None
    k = src.index("Regex::new(", m.end()) + len("Regex::new(")
    while src[k] in " \t\n\r":
        k += 1
    return rust_literal(src, k)[0]


def git(repo, *argv):
    r = subprocess.run(["git", "-C", str(repo), *argv], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default=str(HERE.parent.parent))
    ap.add_argument("--fetch", action="store_true", help="git fetch origin main and report how far HEAD is behind it")
    args = ap.parse_args(argv)
    repo = Path(args.repo).expanduser()
    comp = (repo / "src/server/compaction.rs").read_text()
    core_rs = (repo / "src/server/routes/core.rs").read_text()
    inj = (repo / "src/common/injection.rs").read_text()
    routes = (repo / "src/server/routes/mod.rs").read_text()
    cfg = (repo / "assets/config.default.yaml").read_text()
    authn = (repo / "src/common/authn.rs").read_text()
    guards = (repo / "src/server/guards.rs").read_text()

    import qwen_needle_test as nt
    import bakeoff_harness as hb

    checks, notes = [], []

    def check(name, ok, fix):
        checks.append((name, bool(ok), fix))

    head = git(repo, "rev-parse", "--short", "HEAD")
    if args.fetch:
        git(repo, "fetch", "-q", "origin", "main")
    origin = git(repo, "rev-parse", "--short", "origin/main")
    behind = git(repo, "rev-list", "--count", "HEAD..origin/main") if origin else ""
    check(f"checkout {head or '?'} is not behind origin/main {origin or '?'}", origin and behind == "0",
          f"git pull --rebase (HEAD is {behind or '?'} commit(s) behind origin/main), rebuild, rerun this check")

    has_513 = "fn prompt_messages" in comp
    has_514 = "fn correction_sources" in comp
    has_515 = "fn worth_summarizing" in comp and "compaction_skipped" in core_rs
    has_ledger = "fn with_fact_ledger" in comp
    merged = [k for k, sha in MAIN_REFS.items() if subprocess.run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", sha, "HEAD"], capture_output=True).returncode == 0]
    notes.append(f"compaction in this checkout: {'main' if has_513 else 'legacy (main ' + LEGACY_REF + ')'}"
                 f"{' (' + ', '.join(merged) + ')' if merged else ''}{' + fact ledger' if has_ledger else ''}")

    # constants every layout shares
    check("COMPACT_PREFIX", rust_str(comp, "COMPACT_PREFIX") == nt.COMPACT_PREFIX, "qwen_needle_test.py COMPACT_PREFIX")
    for name in ("SUMMARY_INPUT_CHARS", "SUMMARY_TURN_CHARS", "MAX_PROMPT_TOKENS", "MIN_PROMPT_HEADROOM"):
        check(name, rust_num(comp, name) == getattr(nt, name), f"qwen_needle_test.py {name}")
    check("estimate is bytes/4", "(text.len() as u64).div_ceil(4)" in comp, "qwen_needle_test.py est_tokens")
    check("ratio bounds 1.0..3.0", "ratio.clamp(1.0, 3.0)" in comp, "qwen_needle_test.py clamp_ratio")
    check("calibration rises at once, decays by half", "sample.max((old + sample) / 2.0)" in comp,
          "qwen_needle_test.py calibrate")
    check("default compact threshold 24000", "compact_prompt_tokens: 24000" in cfg, "qwen_needle_test.py --compact-threshold")
    check("default keep 8", "compact_keep_messages: 8" in cfg, "qwen_needle_test.py --keep")
    check("default summary tokens 768", "summary_max_tokens: 768" in cfg, "qwen_needle_test.py --summary-tokens")
    check("default reply 4096 at temperature 0.2", "max_tokens: 4096" in cfg and "temperature: 0.2" in cfg,
          "qwen_needle_test.py --max-tokens/--temperature")

    # the legacy layout (main at LEGACY_REF)
    legacy_src = comp if not has_513 else git(repo, "show", f"{LEGACY_REF}:src/server/compaction.rs")
    if legacy_src:
        check(f"legacy summary prompt ({'tree' if not has_513 else LEGACY_REF})",
              rust_str(legacy_src, "SUMMARY_SYSTEM") == nt.LEGACY_SUMMARY_SYSTEM, "qwen_needle_test.py LEGACY_SUMMARY_SYSTEM")
        check("legacy summary is one assistant message", 'role: "assistant".into(),\n            text: summary.to_string()'
              in legacy_src, "qwen_needle_test.py legacy_compact")
        check("legacy clip is the first 800 chars", "msg.text.chars().take(SUMMARY_TURN_CHARS).collect()" in legacy_src,
              "qwen_needle_test.py legacy_summary_input")
    else:
        notes.append(f"legacy layout not checked: {LEGACY_REF} is not in this clone (git fetch --depth=50 origin main)")

    # main (#513-#516)
    if has_513:
        check("main: #514 repair (whole tokens, short quotes, carried corrections) is present", has_514,
              "qwen_needle_test.py repair_summary (this checkout predates #514)")
        check("main: #515 trigger/hard-limit split and skips are present", has_515,
              "qwen_needle_test.py Branch.turn (this checkout predates #515)")
        for name in ("BRIDGE_TEXT", "NOTE_TEXT", "ACK_TEXT", "UNVERIFIED"):
            check(f"513 {name}", rust_str(comp, name) == getattr(nt, name), f"qwen_needle_test.py {name}")
        check("513 summary prompt template", rust_fn_format(comp, "summary_system") == nt.SUMMARY_SYSTEM_TEMPLATE,
              "qwen_needle_test.py SUMMARY_SYSTEM_TEMPLATE")
        check("513 summary word target 9/20 of summary_max_tokens", "summary_max_tokens * 9 / 20" in comp,
              "qwen_needle_test.py summary_system")
        check("513 pinned and clip bounds", all(x in comp for x in (
            "const GOAL_CHARS: usize = SUMMARY_TURN_CHARS;", "const PINNED_CHARS: usize = 2 * SUMMARY_TURN_CHARS;",
            "const CLIP_TAIL_CHARS: usize = SUMMARY_TURN_CHARS / 4;", "for first_limit in [PINNED_CHARS, PINNED_CHARS / 4, 0]")),
              "qwen_needle_test.py GOAL_CHARS/PINNED_CHARS/CLIP_TAIL_CHARS/summary_input")
        check("513 reserved tags", rust_str_array(comp, "RESERVED_TAGS") == nt.RESERVED_TAGS, "qwen_needle_test.py RESERVED_TAGS")
        check("513 identifier regex", rust_regex(comp, "identifier_re") == nt.IDENTIFIER_RX, "qwen_needle_test.py IDENTIFIER_RX")
        check("513 absence regex", rust_regex(comp, "absence_re") == nt.ABSENCE_RX, "qwen_needle_test.py ABSENCE_RX")
        check("513 quote regex", rust_regex(comp, "quote_re") == nt.QUOTE_RX, "qwen_needle_test.py QUOTE_RX")
        check("513 default tail percent 25, clamped 10..50", rust_num(comp, "DEFAULT_TAIL_PERCENT") == nt.DEFAULT_TAIL_PERCENT
              and ".clamp(10, 50)" in comp and "compact_tail_percent: 25" in cfg, "qwen_needle_test.py --tail-percent")
        check("513 summary call sends the reservation as max_tokens",
              re.search(r"Some\(model\),\s*reservation,\s*0\.0,", comp), "qwen_needle_test.py Branch.summarize")
        check("513 tail budget: min(percent of trigger, room after fixed + summary + scaffold)",
              "let room = threshold.saturating_sub(fixed + summary_send + scaffold" in core_rs
              and "tail_percent(&state.cfg) / 100).min(room)" in core_rs, "qwen_needle_test.py Branch.turn")
        check("513 retained check adds the scaffold", "if middle.is_empty() { 0 } else { scaffold }" in core_rs,
              "qwen_needle_test.py Branch.turn")
        check("513 prompt history is prompt_messages", "crate::server::compaction::prompt_messages(messages)" in core_rs,
              "qwen_needle_test.py render")
        check("513 response carries the compaction report", '"compaction": compaction_report' in core_rs,
              "bakeoff_harness.py long_run")
        if has_514:
            check("514 omission marker reserved in the frame", "earlier turns omitted]\\n\", ordinary.len())" in comp
                  or 'format!("[{} earlier turns omitted]\\n", ordinary.len())' in comp, "qwen_needle_test.py summary_input")
            check("514 prior summaries are neutralized in the prompt", "neutralize(&m.text)," in comp, "qwen_needle_test.py summary_input")
            check("514 quotes of any length reach quotes_user", rust_regex(comp, "quote_re") == nt.QUOTE_RX
                  and "fn quotes_user" in comp and ".count() >= 8" in comp, "qwen_needle_test.py QUOTE_RX / quotes_user")
            check("514 whole-token grounding", "contains_token(&haystack, &token.to_lowercase())" in comp,
                  "qwen_needle_test.py contains_token")
            check("514 corrections sourced from prior summaries", "let corrections = correction_sources(middle);" in comp,
                  "qwen_needle_test.py correction_sources")
            check("514 stored_reply guards the prefix", "fn stored_reply" in comp
                  and "compaction::stored_reply(" in (repo / "src/server/sessions.rs").read_text(), "qwen_needle_test.py stored_reply")
        if has_515:
            check("515 trigger counts one reply, hard limit the reservation",
                  "if project(&history, max_tokens) > threshold || projected > hard_limit" in core_rs,
                  "qwen_needle_test.py Branch.turn")
            check("515 hard limit is the prompt cap via turn_prompt_limit(prompt_cap, prompt_cap", 
                  re.search(r"turn_prompt_limit\(\s*prompt_cap,\s*prompt_cap,", core_rs), "qwen_needle_test.py --hard-limit")
            check("515 worth_summarizing is twice the allowance", "allowance.saturating_mul(2)" in comp
                  and "worth_summarizing(middle, summary_send + scaffold, ratio)" in core_rs, "qwen_needle_test.py worth_summarizing")
            check("515 unusable summary falls back when the turn fits", "SUMMARY_UNUSABLE && fits_uncompacted" in core_rs,
                  "qwen_needle_test.py Branch.turn")
            check("515 response carries compaction_skipped", '"compaction_skipped": compaction_skipped' in core_rs,
                  "bakeoff_harness.py long_run")
            check("516 fixed prompt projected with max_tokens", "let fixed = project(&prompt_history(&pinned_only), max_tokens);" in core_rs,
                  "qwen_needle_test.py Branch.turn (fixed)")
    else:
        notes.append("main compaction (#513+) not in this checkout: the main and ledger layouts measure unmerged code")

    # the fact ledger (stacked on #513)
    if has_ledger:
        check("ledger is hooked into chat compaction", "compaction::with_fact_ledger(" in core_rs,
              "src/server/routes/core.rs (the patch hook)")
        check("ledger budget is reserved before the tail is sized", "fact_ledger_reserve_tokens(" in core_rs,
              "src/server/routes/core.rs (room)")
        check("LEDGER_HEADER", rust_str(comp, "LEDGER_HEADER") == nt.LEDGER_HEADER and hb.LEDGER_HEADER == nt.LEDGER_HEADER,
              "qwen_needle_test.py / bakeoff_harness.py LEDGER_HEADER")
        check("LEDGER_CUES", rust_str_array(comp, "LEDGER_CUES") == list(nt.LEDGER_CUES), "qwen_needle_test.py LEDGER_CUES")
        check("LEDGER_SENTENCE_CHARS", rust_num(comp, "LEDGER_SENTENCE_CHARS") == nt.LEDGER_SENTENCE_CHARS,
              "qwen_needle_test.py LEDGER_SENTENCE_CHARS")
        check("ledger default bytes", rust_num(comp, "DEFAULT_FACT_LEDGER_MAX_BYTES") == nt.DEFAULT_FACT_LEDGER_MAX_BYTES
              and f"fact_ledger_max_bytes: {nt.DEFAULT_FACT_LEDGER_MAX_BYTES}" in cfg, "qwen_needle_test.py --ledger-bytes")
        check("golden lines match the Rust test", all(f'"{line}"' in comp for line in nt.GOLDEN_OUT),
              "GOLDEN_OUT in qwen_needle_test.py and fact_ledger_matches_the_python_golden")
    else:
        notes.append("fact ledger not in this checkout: the ledger layout measures the stacked proposal")
    check("core injection patterns", rust_str_array(inj, "CORE_INJECTION_PATTERNS") == nt.CORE_INJECTION_PATTERNS,
          "qwen_needle_test.py CORE_INJECTION_PATTERNS")

    # login and routes (bakeoff_harness.py)
    check("setup code file name", rust_str(authn, "SETUP_CODE_FILE") == "setup-code", "bakeoff_harness.py Disposable.start")
    check("no public bootstrap route (#512)", "/api/auth/bootstrap-password" not in routes, "bakeoff_harness.py login path")
    for path in ("/api/auth/login", "/api/auth/password", "/api/chat", "/api/model", "/api/ollama/profile",
                 "/api/sessions/{session_id}", "/api/prompt/preview", "/api/web", "/api/web/research"):
        check(f"route {path}", f'"{path}"' in routes, f"bakeoff_harness.py ({path})")
    check("CSRF header name (case-insensitive)", (rust_str(guards, "CSRF_HEADER") or "").lower() == "x-cyclaw-csrf",
          "bakeoff_harness.py Harness._req")
    check("bench targets the same window", hb.TARGET == rust_num(comp, "BASE_WINDOW"), "bakeoff_harness.py TARGET")
    check("profile reports the trigger in force", '"prompt_limit": prompt_limit,' in (repo / "src/server/routes/ollama.rs").read_text(),
          "bakeoff_harness.py trigger_in_force")

    bad = [c for c in checks if not c[1]]
    for note in notes:
        print(f"  note  {note}")
    for name, ok, fix in checks:
        print(f"  {'ok  ' if ok else 'DRIFT'} {name}" + ("" if ok else f"  -> update {fix}"))
    print(f"harness sync: {'OK' if not bad else f'{len(bad)} drifted'} (checkout {head}, origin/main {origin or 'unknown'})")
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
