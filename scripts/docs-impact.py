#!/usr/bin/env python3
"""Check whether a PR updates or reviews the guides that own its changes."""
import argparse
import fnmatch
import hashlib
import json
import re
import subprocess
import sys

CONFIG = "assets/config.default.yaml"
# Order has no effect: a shared implementation can affect more than one guide.
OWNERS = {
    "API_ROUTES": ("src/server/routes/mod.rs", "src/server/schemas.rs"),
    "Browser-Use": ("browser/*", "src/common/browser*", "src/common/computer*", "src/server/browser*",
                    "src/server/routes/browser.rs"),
    "CODING_PIPELINE": ("src/agentic/*", "src/shim/*", "src/server/agent_*", "src/server/routes/agent.rs",
                        "src/server/schedule_time.rs", "scripts/prepare-cargo.py"),
    "CONSOLE": ("assets/static/*", "assets/soul*", "data/styles/*", "src/common/pdf_text*",
                "src/server/attach*", "src/server/compaction.rs", "src/server/console.rs", "src/server/docx.rs",
                "src/server/live_state.rs", "src/server/loop_runner.rs", "src/server/prompts.rs",
                "src/server/retrieval*.rs", "src/server/routes/core.rs", "src/server/routes/goals.rs",
                "src/server/routes/loop_runner.rs", "src/server/routes/persona.rs", "src/server/routes/skills.rs",
                "src/server/routes/style.rs", "src/server/routes/session_io.rs", "src/server/sessions.rs",
                "src/server/session_*.rs", "src/server/self_manual.rs", "src/server/slash.rs",
                "src/server/style.rs", "src/server/tool_inventory.rs", "src/server/views.rs",
                "src/common/tool_broker.rs"),
    "DEPENDENCIES": ("Cargo.*", "desktop/Cargo.*", "deny.toml", "desktop/deny.toml", "*rust-toolchain.toml"),
    "DESKTOP": ("desktop/*", "src/server/desktop.rs", "scripts/package-desktop.sh", "scripts/start-web.sh"),
    "GITHOOKS": (".githooks/*", "scripts/*githooks.sh"),
    "INSTALL": ("src/common/config.rs", "src/common/credential*", "src/common/home*", "src/main.rs",
                "src/server/config_reload.rs", "src/server/env_keys.rs", "scripts/*-macos.sh", "scripts/init.sh"),
    "MCP": ("src/common/mcp*", "src/common/tool_broker.rs", "src/server/mcp*"),
    "MEMORY_GUIDE": ("src/server/memory_notes.rs", "src/server/notes_corpus.rs", "src/server/structured_memory_suggest.rs",
                     "src/server/routes/notes_corpus.rs", "src/server/routes/structured_memory.rs"),
    "MODELS": ("src/llm/*", "src/server/generation_gate.rs", "src/server/model_limits.rs",
               "src/server/routes/ollama.rs", "scripts/*.Modelfile"),
    "PROCESS_LIFECYCLE": ("src/common/child_env.rs", "src/common/process*", "src/common/sandbox_wrap.rs",
                          "src/common/windows_job.rs", "src/common/mcp_worker.rs"),
    "RELEASING": (".github/workflows/bundle.yml", ".github/workflows/desktop.yml",
                  ".github/workflows/release.yml", "scripts/package-release.sh", "scripts/release-plan.py"),
    "SECURE_RESEARCH": ("src/common/auth*", "src/common/local_tls.rs", "src/common/ratelimit.rs",
                        "src/server/guards.rs", "src/server/headers.rs", "src/server/routes/auth.rs",
                        "src/server/transport.rs", "src/server/web_command.rs", "src/server/web_policy.rs",
                        "src/server/web_research.rs", "src/server/routes/panels.rs"),
    "SPEND_AND_NOTIFICATIONS": ("src/llm/spend.rs", "src/server/notification*", "src/server/routes/notifications.rs"),
    "STRUCTURED_MEMORY": ("src/server/structured_memory*", "src/server/routes/structured_memory.rs",
                          "src/common/private_sqlite.rs"),
    "TROUBLESHOOTING": ("src/server/diag_reader.rs", "src/server/chat_diag.rs", "src/server/routes/diag.rs"),
    "WEB_DECISION_ROUTER": ("src/server/chat_web*", "src/server/fetch_confirm.rs", "src/server/web_*",
                            "src/server/passage_index.rs"),
    "netconnect": ("src/netconnect/*",),
}
CONFIG_OWNERS = {
    "models": ("MODELS",), "chat": ("CONSOLE",), "compaction": ("CONSOLE",),
    "attachments": ("CONSOLE",), "notes_corpus": ("MEMORY_GUIDE",),
    "api": ("API_ROUTES",), "loop": ("CONSOLE",), "web": ("WEB_DECISION_ROUTER", "SECURE_RESEARCH"),
    "browser": ("Browser-Use",), "computer": ("Browser-Use",), "notifications": ("SPEND_AND_NOTIFICATIONS",),
    "mcp": ("MCP",), "security": ("SECURE_RESEARCH",), "tls": ("SECURE_RESEARCH",),
    "auth": ("SECURE_RESEARCH",), "logging": ("TROUBLESHOOTING",), "policy": ("SECURE_RESEARCH",),
    "personality": ("CONSOLE",), "agentic": ("CODING_PIPELINE",), "unslop": ("CODING_PIPELINE",),
    "diagnostics": ("TROUBLESHOOTING",), "memory": ("MEMORY_GUIDE",),
    "structured_memory": ("STRUCTURED_MEMORY", "MEMORY_GUIDE"), "netconnect": ("netconnect",),
    "scheduling": ("CODING_PIPELINE",),
}


def git(*args):
    return subprocess.check_output(["git", "--no-pager", "--literal-pathspecs", *args])


def blob(ref, path):
    return git("show", f"{ref}:{path}").decode("utf-8")


def config_sections(text):
    parts = re.split(r"(?m)^([a-z][a-z0-9_]*):", text)[1:]
    return dict(zip(parts[::2], parts[1::2], strict=True))


def guides_for(path):
    guides = {
        guide for guide, patterns in OWNERS.items()
        if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)
    }
    if not guides and path.endswith((".rs", ".js", ".mjs", ".ts", ".html", ".css")):
        if path.startswith(("src/", "assets/")):
            guides.add("CONSOLE")
    return guides


def content_changed(numstat):
    """Paths with lines left to count in `git diff -w --numstat -z` output."""
    changed = set()
    for record in filter(None, numstat.split("\0")):
        added, deleted, path = record.split("\t", 2)
        if (added, deleted) != ("0", "0"):
            changed.add(path)
    return changed


def change_fingerprints(base, head, paths):
    """Two fingerprints of what `base..head` does to `paths`: (moved, exact).

    `exact` is the raw diff: each path's mode and its blob id on both sides. It
    is the same only while the base's copy of every path is the same, so main
    changing one of those paths anywhere, or a merge-queue candidate built on
    another pull request that did, gives a reviewed change a new one.

    `moved` survives that. It is the patch without positions: per path, in
    order, its header (mode, creation, deletion) and every removed and added
    line. Blob ids and hunk line numbers are left out. It changes when a line
    the change removes or adds changes; it does not change when the same
    lines land at another place in the file, which `exact` would catch.
    """
    paths = sorted(paths)
    exact = git("diff", "--raw", "--no-abbrev", "--no-renames", "-z", base, head, "--", *paths)
    # Pinned so a contributor's git config cannot change the patch text.
    patch = git("-c", "diff.algorithm=myers", "-c", "core.quotepath=true", "diff", "--no-renames", "--no-color",
                "--no-ext-diff", "--no-textconv", "--text", "--indent-heuristic", "--src-prefix=a/",
                "--dst-prefix=b/", "-U0", base, head, "--", *paths)
    kept = []
    for line in patch.split(b"\n"):
        # Header lines carry no +, - or space prefix, so content cannot match.
        if line.startswith(b"index "):
            continue
        kept.append(b"@@" if line.startswith(b"@@ ") else line)
    return hashlib.sha256(b"\n".join(kept)).hexdigest(), hashlib.sha256(exact).hexdigest()


def review_plan(base, head):
    paths = git("diff", "--no-renames", "--name-only", "-z", base, head).decode().split("\0")
    affected = {}
    for path in filter(None, paths):
        if path.endswith((".md", ".markdown", ".mdx")):
            continue
        guides = guides_for(path)
        if path == CONFIG:
            before, after = (config_sections(blob(revision, path)) for revision in (base, head))
            for key in before.keys() | after.keys():
                if before.get(key) != after.get(key):
                    guides.update(CONFIG_OWNERS.get(key, ("INSTALL",)))
        for guide in guides:
            affected.setdefault(f"docs/{guide}.md", set()).add(path)

    # A guide counts as updated only when lines differ after -w. --numstat
    # counts those lines on every git; --name-only lists a whitespace-only
    # edit on git 2.43 and omits it on 2.56.
    changed = content_changed(
        git("diff", "-w", "--no-renames", "--diff-filter=M", "--numstat", "-z", base, head, "--", "docs/").decode()
    )
    # Bind a no-change decision to the exact production diff, including deletions.
    # Whole message lines, not git's trailer block: a merge-queue squash commit
    # lists every PR commit and ends with GitHub's Co-authored-by paragraph,
    # where git parses no Docs-Impact trailer.
    trailers = [
        line for line in git("log", "--format=%B", f"{base}..{head}").decode().splitlines()
        if line.startswith("Docs-Impact: ")
    ]
    pending, fingerprints = {}, {}
    for guide, paths in sorted(affected.items()):
        tree_entry = git("ls-tree", head, "--", guide)
        if not tree_entry.startswith(b"100644 blob "):
            raise ValueError(f"{guide} must remain a regular guide in this change.")
        moved, exact = change_fingerprints(base, head, paths)
        # New receipts carry `moved`. One written with `exact` still counts
        # wherever it matched before.
        fingerprints[guide] = moved
        reviewed = any(
            line.startswith(receipt) and (reason := line[len(receipt):].strip()) and not reason.startswith("<")
            for receipt in (f"Docs-Impact: {guide} {moved} ", f"Docs-Impact: {guide} {exact} ")
            for line in trailers
        )
        if guide not in changed and not reviewed:
            pending[guide] = sorted(paths)
    return pending, fingerprints


def review_steps(base, head, each_commit):
    """The (base, head) pairs to judge. One for a pull request. For a
    merge-queue candidate, one per first-parent commit: each is one queued
    pull request's squash, whose message holds only that request's receipts
    and whose guide edits are its own. Judged as one range, two requests that
    change different files of one guide would need a receipt neither wrote."""
    if not each_commit:
        return [(base, head)]
    commits = git("rev-list", "--first-parent", "--reverse", f"{base}..{head}").decode().split()
    return list(zip([base, *commits], commits))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="origin/main")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--each-commit", action="store_true",
                        help="judge each first-parent commit of base..head on its own: a merge-queue "
                             "candidate stacks one squash commit per queued pull request")
    args = parser.parse_args()
    base = git("merge-base", args.base, args.head).decode().strip()
    failed = False
    for step_base, step_head in review_steps(base, args.head, args.each_commit):
        pending, fingerprints = review_plan(step_base, step_head)
        for guide, paths in pending.items():
            failed = True
            where = f" in {step_head[:12]}" if args.each_commit else ""
            print(f"{guide} needs review for {json.dumps(paths)}{where}.")
            print(f"Update its owning section or add this trailer to the final commit:")
            print(f"Docs-Impact: {guide} {fingerprints[guide]} <specific reason the guide remains accurate>")
    if failed:
        return 1
    print("Docs impact check passed.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
