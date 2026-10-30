#!/usr/bin/env python3
"""bakeoff9_research — skeleton for -cg research/agent tags on one Mac Ollama.

Scratch tool. Pulls nothing, edits no config, picks no winner, invents no timings.
Target tags (defaults after create-cg):
  - gemma4-26b-a4b:ud-q4_k_xl-cg
  - qwen3.8:27b-mlx-cg
  - qwen3.6:35b-a3b-cg   (requires parent qwen3.6:35b-a3b + ollama create)

Wire matches CG-agent-harness /v1: temperature from tag (or 0.2 override),
reasoning_effort "none", no num_ctx in request, max_tokens 4096.

STATUS: skeleton only. Fill probes from bakeoff_7 harness lane once tags exist.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from datetime import datetime
from pathlib import Path

VERSION = "9.0-skeleton"
HOST = os.environ.get("BAKEOFF_OLLAMA", "http://127.0.0.1:11434")
LOCAL = urllib.request.build_opener(urllib.request.ProxyHandler({}))

# Locked research defaults (must match Modelfiles on Desktop/modelfile/)
RESEARCH_TAGS = [
    "gemma4-26b-a4b:ud-q4_k_xl-cg",
    "qwen3.8:27b-mlx-cg",
    "qwen3.6:35b-a3b-cg",
]


def api(path: str, body: dict | None = None, timeout: float = 120.0):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        f"{HOST}{path}",
        data=data,
        headers={"Content-Type": "application/json"} if data else {},
        method="GET" if data is None else "POST",
    )
    with LOCAL.open(req, timeout=timeout) as r:
        return json.load(r)


def list_installed() -> set[str]:
    tags = set()
    for m in api("/api/tags").get("models") or []:
        for k in ("name", "model"):
            if m.get(k):
                tags.add(m[k])
    return tags


def show_parameters(name: str) -> dict:
    # /api/show returns modelfile + details; parse PARAMETER lines when present
    info = api("/api/show", {"name": name})
    params = {}
    mf = info.get("modelfile") or ""
    for line in mf.splitlines():
        if line.startswith("PARAMETER "):
            _, rest = line.split(" ", 1)
            key, _, val = rest.partition(" ")
            params[key] = val.strip().strip('"')
    details = info.get("details") or {}
    return {"parameters": params, "details": details, "model_info_keys": sorted((info.get("model_info") or {}).keys())[:20]}


def preflight(models: list[str]) -> dict:
    installed = list_installed()
    rows = []
    for name in models:
        present = any(name == t or t.startswith(name + ":") or name in t for t in installed)
        # exact match preferred
        exact = name in installed
        row = {"tag": name, "installed": exact, "near": sorted(t for t in installed if name.split(":")[0] in t)[:8]}
        if exact:
            try:
                row["show"] = show_parameters(name)
            except Exception as e:
                row["show_error"] = str(e)
        rows.append(row)
    return {
        "version": VERSION,
        "host": HOST,
        "when": datetime.now().isoformat(timespec="seconds"),
        "rows": rows,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--preflight", action="store_true", help="list research tags + parameters only")
    ap.add_argument("--models", nargs="*", default=RESEARCH_TAGS)
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()

    if not args.preflight:
        print(
            "bakeoff9_research skeleton: only --preflight is implemented.\n"
            "Fill harness quality/tools/web probes after qwen3.6:35b-a3b-cg exists.\n"
            "Reuse bakeoff_7.py --lanes harness --models <tag> until then.",
            file=sys.stderr,
        )
        report = preflight(args.models)
        print(json.dumps(report, indent=2))
        return 2

    report = preflight(args.models)
    text = json.dumps(report, indent=2)
    print(text)
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        (args.out / "preflight.json").write_text(text)
        print(f"wrote {args.out / 'preflight.json'}", file=sys.stderr)
    missing = [r["tag"] for r in report["rows"] if not r["installed"]]
    return 0 if not missing else 1


if __name__ == "__main__":
    raise SystemExit(main())
