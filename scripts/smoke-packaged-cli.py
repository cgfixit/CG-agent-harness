"""Check the extracted CLI before Bundle or release uploads it."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main(binary):
    binary = Path(binary).resolve(strict=True)
    # Only the test-support feature compiles this fixture hook (src/llm/cloud_chat.rs);
    # tests/invariant_guard.rs keeps the two in step.
    if b"CGAH_TEST_CLOUD_BASE_URL" in binary.read_bytes():
        raise SystemExit("packaged CLI: built with test-support fixture hooks")
    with tempfile.TemporaryDirectory(prefix="cgah-package-smoke-") as scratch:
        env = dict(os.environ, CGAGENTHARNESS_HOME=scratch)

        def require(args, code, marker):
            try:
                result = subprocess.run(
                    [binary, *args], cwd=scratch, env=env,
                    capture_output=True, timeout=20, check=False,
                )
            except subprocess.TimeoutExpired:
                raise SystemExit(f"packaged CLI: {args[0]} timed out") from None
            if result.returncode != code or marker not in result.stdout + result.stderr:
                raise SystemExit(f"packaged CLI: {args[0]} expected exit {code} and {marker!r}; got {result.returncode}")

        require(["--help"], 0, b"serve")
        # TEST-NET-1 is reserved for documentation; the guard should refuse it
        # before trying to bind. A failed bind alone cannot pass this check.
        require(["serve", "--host", "192.0.2.1"], 1, b"binds loopback only")
        require(["agentic", "--config", "missing.yaml", "status"], 3, b"cannot read config file: file is missing")

    print("packaged CLI: no test-support hooks; help, loopback refusal and missing config verified")


if __name__ == "__main__":
    main(sys.argv[1])
