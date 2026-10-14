#!/usr/bin/env python3
"""Create a disposable harness home pointed at the fake loopback model."""
import json
import sys
from pathlib import Path

repo = Path(sys.argv[1])
home = Path(sys.argv[2])
model_port = int(sys.argv[3])
home.mkdir(mode=0o700, parents=True, exist_ok=False)
config = (repo / "assets/config.default.yaml").read_text()


def override(text, dotted, value):
    parts = dotted.split(".")
    parents, output, found = [], [], False
    for line in text.splitlines():
        trimmed = line.lstrip()
        indent = len(line) - len(trimmed)
        if trimmed and not trimmed.startswith("#") and ":" in trimmed:
            while parents and indent <= parents[-1][0]:
                parents.pop()
            key = trimmed.split(":", 1)[0].strip()
            if [p[1] for p in parents] + [key] == parts:
                line = " " * indent + key + ": " + json.dumps(value)
                found = True
            if not trimmed.split(":", 1)[1].strip():
                parents.append((indent, key))
        output.append(line)
    assert found, dotted
    return "\n".join(output) + "\n"


for key, value in {
    "models.local_llm.base_url": f"http://127.0.0.1:{model_port}/v1",
    "models.local_llm.model": "qwen3.8:27b-mlx-cg",
    "models.local_llm.max_tokens": 256,
    "tls.enabled": False,
    "security.allow_plaintext_key_file": True,
    # The scripted browser types far faster than a person; keep 429s out of evidence.
    "api.rate_limit.max_requests": 100000,
    "api.harness_loop_rate_limit.max_requests": 1000,
}.items():
    config = override(config, key, value)
(home / "config.yaml").touch(mode=0o600)
(home / "config.yaml").write_text(config)
print(home)
