#!/usr/bin/env python3
"""Validate the fixed parity identity set and render its readable status view."""
import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    directory = Path(__file__).resolve().parent.parent / "docs" / "parity"
    ledger = json.loads((directory / "actions.json").read_text())
    expected = {f"{prefix}{i}" for prefix, count in
                [("M", 5), ("C", 10), ("I", 8), ("A", 9), ("L", 7), ("O", 9)]
                for i in range(1, count + 1)}
    actions = ledger["actions"]
    if len(actions) != 48 or {a["id"] for a in actions} != expected:
        raise SystemExit("Parity ledger must contain each of the original 48 IDs exactly once")
    states = {"existing-equivalent", "needs-extension", "missing", "implemented", "adapted", "blocked"}
    for action in actions:
        if action["implementation"]["status"] not in states:
            raise SystemExit(f"Invalid implementation status: {action['id']}")
        if not 1 <= action["phase"] <= 11:
            raise SystemExit(f"Invalid phase: {action['id']}")
        if not set(action["dependencies"]) <= expected or action["id"] in action["dependencies"]:
            raise SystemExit(f"Invalid dependencies: {action['id']}")
    lines = ["# Parity status", "", "Generated from [actions.json](actions.json) with "
             "`python3 scripts/parity-status.py`. Do not edit this view directly.", "",
             "Implementation and validation are independent. This is a fixed action inventory, "
             "not a percentage of CyClaw ported. Missing actions remain requested work.", "",
             f"Rust baseline: [pinned commit]({ledger['source_pins']['rust']}). "
             f"CyClaw reference: [pinned commit]({ledger['source_pins']['python']}).", "",
             "See [execution state](WORK.md) for baseline evidence and native acceptance blockers, "
             "and [non-RAG contracts](CONTRACTS.md) for deliberate adaptations.", "",
             "| ID | Phase | Implementation | Fixtures | Native | Capability |",
             "|---|---:|---|---|---|---|"]
    for action in actions:
        scope = action["scope"].replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {action['id']} | {action['phase']} | {action['implementation']['status']} | "
                     f"{action['validation']['fixtures']} | {action['validation']['native_platform']} | {scope} |")
    rendered = "\n".join(lines) + "\n"
    target = directory / "STATUS.md"
    if args.check:
        if not target.exists() or target.read_text() != rendered:
            raise SystemExit("Parity status view is stale; run python3 scripts/parity-status.py")
        print("48 parity IDs and readable view verified")
    else:
        target.write_text(rendered)


if __name__ == "__main__":
    main()
