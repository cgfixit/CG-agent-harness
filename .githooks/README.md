# .githooks

Agent-neutral Git hooks. Run `bash scripts/ensure-githooks.sh` once per clone;
it preserves an existing hook manager, so inspect its output. Local checks
are bypassable; CI scans and separately configured branch protection remain
necessary. Full behavior and overrides: [GITHOOKS.md](../docs/GITHOOKS.md).

- [`pre-commit`](pre-commit): staged secrets, filenames, protected controls, identity, new-file size/metadata and conditional Rust formatting.
- [`pre-push`](pre-push): protected-branch/ref-rewrite guards; scans newly published commits, tag annotations and blob/tree targets.
- [`_security.sh`](_security.sh): shared scanning, policy loading and refusal/reminder logic; Bash 3.2 compatible.
- [`security.conf`](security.conf): harness state-file blocks, fixture exceptions, video limits, core-path reminders and formatting callback.
