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

    # Bind a no-change decision to the exact production diff, including deletions.
    changed = set(
        git("diff", "-w", "--no-renames", "--diff-filter=M", "--name-only", "-z", base, head, "--", "docs/")
        .decode().split("\0")
    )
    trailers = git("log", "--format=%(trailers:key=Docs-Impact)", f"{base}..{head}").decode().splitlines()
    pending, fingerprints = {}, {}
    for guide, paths in sorted(affected.items()):
        tree_entry = git("ls-tree", head, "--", guide)
        if not tree_entry.startswith(b"100644 blob "):
            raise ValueError(f"{guide} must remain a regular guide in this change.")
        diff = git("diff", "--raw", "--no-abbrev", "--no-renames", "-z", base, head, "--", *sorted(paths))
        fingerprint = hashlib.sha256(diff).hexdigest()
        fingerprints[guide] = fingerprint
        receipt = f"Docs-Impact: {guide} {fingerprint} "
        reviewed = any(
            line.startswith(receipt) and (reason := line[len(receipt):].strip()) and not reason.startswith("<")
            for line in trailers
        )
        if guide not in changed and not reviewed:
            pending[guide] = sorted(paths)
    return pending, fingerprints


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="origin/main")
    parser.add_argument("--head", default="HEAD")
    args = parser.parse_args()
    base = git("merge-base", args.base, args.head).decode().strip()
    pending, fingerprints = review_plan(base, args.head)
    if not pending:
        print("Docs impact check passed.")
        return 0
    for guide, paths in pending.items():
        print(f"{guide} needs review for {json.dumps(paths)}.")
        print(f"Update its owning section or add this trailer to the final commit:")
        print(f"Docs-Impact: {guide} {fingerprints[guide]} <specific reason the guide remains accurate>")
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
