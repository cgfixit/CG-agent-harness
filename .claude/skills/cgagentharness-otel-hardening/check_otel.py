#!/usr/bin/env python3
"""check_otel.py - static validation of CG-agent-harness's telemetry-kill contract.

Usage:
    python3 .claude/skills/cgagentharness-otel-hardening/check_otel.py
        [--repo-root PATH] [--strict] [--json] [--as-of YYYY-MM-DD]

Port of CyClaw's .claude/skills/otel-hardening/check_otel.py, re-derived for a
Rust codebase. The contract is the same sentence -- unsolicited secondary
telemetry and analytics are disabled before every process the harness owns or
launches can emit any, intentional policy-gated feature traffic is documented
separately and never mislabeled as telemetry, and no environment variable is
sold as a general network kill switch -- but the mechanism is different:

  * Rust crates do not phone home the way Python packages do. There is no
    "env before import" race; the in-process guarantee is that the LOCK GRAPH
    carries no telemetry/analytics SDK at all (T5), for both crates.
  * The telemetry-capable things the harness touches are EXTERNAL BINARIES it
    spawns (gh, git-which-spawns-gh, caller-declared verification checks,
    operator-declared MCP servers, the bundled python helper) and the
    policy-gated network features it implements itself with reqwest.
  * So the kill switch here is the per-spawn-site child environment: every
    function that BUILDS a child env must still deliver exactly the pairs this
    file pins (T2) -- its own literals plus the arm of the shared
    `common::child_env` builder it names, whose per-kind table is pinned the
    same way -- every env allowlist/denylist must still be the expected set
    (T3), every gh spawn must still route through gh_env() (T9), and a new
    spawn site, dependency, or binary cannot land unclassified (T7, T10).

Everything below is an INDEPENDENT oracle: never derived from the Rust source,
so deleting a pair, flipping "false" to "true", or widening an allowlist
disagrees with THIS copy and fails. Update both sides in one commit.

Zero third-party imports (tomllib is stdlib on 3.11+), reads files only, never
builds or runs cargo, so it works in a fresh clone before any toolchain exists.
It cannot prove a vendor did not change its contract in a newer release: pin
drift and stale review dates surface as WARN, the prompt for SKILL.md step 3.

Severity:
    FAIL  a kill-switch invariant actually broke (exit 2).
    WARN  re-verification due (pin drift, stale review date, unclassified
          component or spawn site) -- exit 0; --strict escalates to failure.
    INFO  advisory; never affects the exit code.

Exit codes (repo convention: 0 ok, 2 failed, 3 env/config):
    0  contract holds (warnings may be present without --strict)
    2  a FAIL check tripped (or a WARN under --strict)
    3  a required file is missing or unparseable
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from datetime import date
from pathlib import Path

# ---------------------------------------------------------------------------
# ORACLE 1 -- child-environment builders. One entry per function that
# constructs the environment of a spawned process. `pairs` is the EXACT set of
# name -> value pairs the function delivers (missing, extra, or mismatched all
# FAIL): its own literals plus, for a site carrying `child_env`, the arm of the
# shared builder it names. The builder's own entry pins each arm under `arms`
# and their union under `pairs`. A name an arm owns may not also be pinned
# literally by a site that routes through it (one owner per name). Values
# assigned from a variable (the git `null` device, the home path) cannot be
# pinned as literals and are listed under `mentions` instead, which only
# proves the name is still handled.
# ---------------------------------------------------------------------------

SITES: dict[str, dict[str, object]] = {
    # The shared builder (parity row O6): the one table every routed site
    # reaches through `child_env::apply(&mut env, Child::<Kind>)`, called LAST
    # at each site so nothing inherited, caller-supplied or operator-declared
    # can switch telemetry back on. Every arm carries DO_NOT_TRACK=1; the gh
    # arm adds gh's telemetry switch and both of its update notifiers; the
    # verifier arm adds what a verified Python repository's checks read.
    "child-env": {
        "file": "src/common/child_env.rs",
        "signature": r"pub fn telemetry_opt_outs\(",
        "launches": ("every child of a site that names one of its arms",),
        "pairs": {
            "DO_NOT_TRACK": "1",
            "GH_TELEMETRY": "false",
            "GH_NO_UPDATE_NOTIFIER": "1",
            "GH_NO_EXTENSION_UPDATE_NOTIFIER": "1",
            "HF_HUB_DISABLE_TELEMETRY": "1",
            "ANONYMIZED_TELEMETRY": "false",
        },
        "mentions": ("match child",),
        "arms": {
            "McpServer": {"DO_NOT_TRACK": "1"},
            "Gh": {
                "DO_NOT_TRACK": "1",
                "GH_TELEMETRY": "false",
                "GH_NO_UPDATE_NOTIFIER": "1",
                "GH_NO_EXTENSION_UPDATE_NOTIFIER": "1",
            },
            "VerificationCheck": {
                "DO_NOT_TRACK": "1",
                "GH_TELEMETRY": "false",
                "HF_HUB_DISABLE_TELEMETRY": "1",
                "ANONYMIZED_TELEMETRY": "false",
            },
        },
    },
    # git is spawned with `credential.helper=!gh auth git-credential`, so git's
    # child env is ALSO gh's env: the gh arm rides here too, after `extra`.
    "agentic-git": {
        "file": "src/agentic/git.rs",
        "signature": r"fn environment\(",
        "launches": ("git", "gh"),
        "child_env": "Gh",
        "pairs": {
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_ATTR_NOSYSTEM": "1",
            "GIT_OPTIONAL_LOCKS": "0",
            "GIT_NO_REPLACE_OBJECTS": "1",
            "GIT_TERMINAL_PROMPT": "0",
            "GH_TELEMETRY": "false",
            "GH_NO_UPDATE_NOTIFIER": "1",
            "GH_NO_EXTENSION_UPDATE_NOTIFIER": "1",
            "DO_NOT_TRACK": "1",
        },
        "mentions": ("GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"),
    },
    # Full inherited env (std::env::vars()) with the gh arm forced on top, so
    # an ambient GH_TELEMETRY=true can never reach a gh child.
    "agentic-gh": {
        "file": "src/agentic/gh_client.rs",
        "signature": r"pub fn gh_env\(",
        "launches": ("gh",),
        "child_env": "Gh",
        "pairs": {
            "GH_TELEMETRY": "false",
            "GH_NO_UPDATE_NOTIFIER": "1",
            "GH_NO_EXTENSION_UPDATE_NOTIFIER": "1",
            "GIT_TERMINAL_PROMPT": "0",
            "DO_NOT_TRACK": "1",
        },
        "mentions": ("std::env::vars()",),
    },
    # Caller-declared verification checks (pytest, ruff, cargo, ...) run inside
    # the hard sandbox with network denied; these pairs are the second layer
    # for whatever the verified repository's own toolchain reads. The proxy,
    # pip and cargo pairs stay literal here (they are not telemetry switches);
    # the verifier arm supplies the rest.
    "executor-runner": {
        "file": "src/agentic/executor/runner.rs",
        "signature": r"pub fn scrubbed_env\(",
        "launches": ("caller-declared checks",),
        "child_env": "VerificationCheck",
        "pairs": {
            "NO_PROXY": "*",
            "no_proxy": "*",
            "PIP_NO_INDEX": "1",
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "CARGO_NET_OFFLINE": "true",
            "DO_NOT_TRACK": "1",
            "GH_TELEMETRY": "false",
            "HF_HUB_DISABLE_TELEMETRY": "1",
            "ANONYMIZED_TELEMETRY": "false",
        },
        "mentions": ("ALLOWED_ENV_VARS",),
    },
    # Operator-declared MCP stdio servers: env_clear(), operator `env` through
    # filter_env() (secret + linker-hijack names dropped), then the MCP arm
    # (DO_NOT_TRACK only -- an arbitrary server reads nothing else) applied
    # AFTER the filter so a declared `env` cannot override it, which matters
    # for a server granted `network: unrestricted`; fixed PATH/locale, every
    # home-ish variable pointed at a scratch dir.
    "mcp-stdio": {
        "file": "src/common/mcp.rs",
        "signature": r"pub async fn spawn\(",
        "launches": ("operator-declared MCP servers",),
        "child_env": "McpServer",
        "pairs": {"PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C", "DO_NOT_TRACK": "1"},
        "mentions": ("env_clear()", "filter_env(", "HOME", "TMPDIR"),
    },
    # Linux strict containment, two spawns: `prepare()` launches the transient
    # systemd service with a cleared env + fixed PATH/locale; inside it, the
    # worker's `run()` launches the MCP server with exactly what mcp.rs built
    # (env_clear + envs(&spec.env)) -- no literal pairs of its own.
    "mcp-worker-service": {
        "file": "src/common/mcp_worker.rs",
        "signature": r"pub fn prepare\(",
        "launches": ("systemd-run",),
        "pairs": {"PATH": "/usr/bin:/bin", "LANG": "C"},
        "mentions": ("env_clear()",),
    },
    "mcp-worker": {
        "file": "src/common/mcp_worker.rs",
        "signature": r"async fn run\(",
        "launches": ("operator-declared MCP servers",),
        "pairs": {},
        "mentions": (".env_clear()", ".envs(&spec.env)"),
    },
    # Desktop shell -> hidden `cgagentharness desktop` sidecar. Inherits the
    # shell's env plus home/PATH; RUSTUP_AUTO_INSTALL=0 stops rustup from
    # downloading a toolchain on the operator's behalf.
    "desktop-sidecar": {
        "file": "desktop/src/backend.rs",
        "signature": r"pub fn start\(",
        "launches": ("cgagentharness desktop",),
        "pairs": {"RUSTUP_AUTO_INSTALL": "0"},
        "mentions": ("CGAGENTHARNESS_HOME", "PATH"),
    },
    # Bundled python3 prepare-cargo.py helper (cargo vendor --locked; the
    # helper itself downloads nothing).
    "desktop-prepare-helper": {
        "file": "desktop/src/main.rs",
        "signature": r"async fn prepare_cargo\(",
        "launches": ("python3", "cargo", "rustc", "xcrun", "otool"),
        "pairs": {"RUSTUP_AUTO_INSTALL": "0"},
        "mentions": ("PATH",),
    },
    # Native "open this URL in your browser?" confirmation: fixed OS opener,
    # cleared env, scheme-checked argv. The browser's egress is the OS's.
    "desktop-opener": {
        "file": "desktop/src/main.rs",
        "signature": r"fn external_link\(",
        "launches": ("/usr/bin/open",),
        "pairs": {"PATH": "/usr/bin:/bin"},
        "mentions": ("env_clear()",),
    },
}

# Sites that launch an EXTERNAL binary which has a documented telemetry or
# update-check switch. Each must carry every CANONICAL_BASE pair (T9). The
# shared builder delivers them today; the check stays so a site that drops its
# `child_env::apply` call, or routes through an arm that lacks them, still
# fails on the effective pairs rather than on wording.
EXTERNAL_BINARY_SITES = ("agentic-git", "agentic-gh", "executor-runner")
CANONICAL_BASE: dict[str, str] = {"DO_NOT_TRACK": "1", "GH_TELEMETRY": "false"}

# ---------------------------------------------------------------------------
# ORACLE 2 -- allowlists and denylists. Widening an inherit allowlist (adding
# HTTPS_PROXY, a LANGSMITH_* name, a token) or shrinking a strip list is a
# regression T2 cannot see, because those arrays carry names, not pairs.
# ---------------------------------------------------------------------------

EXPECTED_ARRAYS: dict[str, dict[str, object]] = {
    "runner ALLOWED_ENV_VARS": {
        "file": "src/agentic/executor/runner.rs",
        "anchor": r"const ALLOWED_ENV_VARS\b",
        "names": {"PATH", "LANG", "LC_ALL", "PYTHONPATH", "VIRTUAL_ENV", "PYTHONIOENCODING"},
    },
    "git inherit allowlist": {
        "file": "src/agentic/git.rs",
        "anchor": r"fn environment\(",
        "names": {"PATH", "HOME", "LANG", "LC_ALL", "GH_TOKEN", "GITHUB_TOKEN"},
    },
    "mcp SECRET_ENV": {
        "file": "src/common/mcp.rs",
        "anchor": r"pub const SECRET_ENV\b",
        "names": {
            "ANTHROPIC_API_KEY", "CGAGENTHARNESS_API_KEY", "CGAGENTHARNESS_WEBHOOK_TOKEN",
            "CLAUDE_API_KEY", "DEEPAGENT_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "GROK_API_KEY",
            "OPENAI_API_KEY", "SERPAPI_API_KEY", "XAI_API_KEY",
        },
    },
    "mcp HIJACK_ENV": {
        "file": "src/common/mcp.rs",
        "anchor": r"const HIJACK_ENV\b",
        "names": {
            "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "LD_DEBUG", "LD_DYNAMIC_WEAK",
            "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH", "DYLD_FALLBACK_FRAMEWORK_PATH", "PYTHONHOME",
        },
    },
}

# ---------------------------------------------------------------------------
# ORACLE 3 -- the lock graph. A crate whose name matches any of these is a
# push-telemetry/analytics SDK; none belongs in either lock. Pull-based local
# metrics (prometheus) and the `tracing` facade are NOT on this list on
# purpose: they have no network sink unless an exporter crate (which IS
# listed) joins the graph.
# ---------------------------------------------------------------------------

DENIED_TELEMETRY_CRATE_PATTERNS: tuple[str, ...] = (
    r"^opentelemetry", r"^tracing-opentelemetry$", r"^sentry", r"^posthog",
    r"^segment", r"^mixpanel", r"^amplitude", r"^datadog", r"^honeycomb",
    r"^bugsnag", r"^rudder", r"^statsig", r"^launchdarkly", r"^minitrace",
    r"telemetry", r"analytics",
)

# Crates whose network behavior the inventory describes, and the version last
# read when that description was written. Drift WARNs: not a leak, the prompt
# to re-read the changelog (SKILL.md step 3).
LAST_VERIFIED_CRATE_PINS: dict[str, str] = {
    "reqwest": "0.12.28",
    "rmcp": "3.4.0",
    "axum": "0.8.9",
    "rustls": "0.23.45",
    "hyper": "1.11.1",
    "keyring": "3.6.3",
    "tantivy": "0.26.2",
    "tracing-subscriber": "0.3.23",
}
DESKTOP_VERIFIED_CRATE_PINS: dict[str, str] = {"tauri": "2.11.5", "reqwest": "0.12.28"}

# gh floor the agentic layer enforces (src/agentic/gh_client.rs DEFAULT_MIN_GH).
EXPECTED_MIN_GH = (2, 40, 0)

STALE_AFTER_DAYS = 120

# Every file allowed to contain `Command::new(` outside test code. A spawn in
# any other file is an unclassified launcher (WARN; FAIL under --strict).
# `src/server` must stay at zero: INVARIANTS.md states it contains none.
KNOWN_SPAWN_FILES: frozenset[str] = frozenset({
    "src/agentic/executor/sandbox.rs",
    "src/common/mcp.rs",
    "src/common/mcp_worker.rs",
    "src/common/process.rs",
    "src/shim/mod.rs",
    "desktop/src/backend.rs",
    "desktop/src/main.rs",
})

# External executables any harness code path can launch, each resolving to an
# INVENTORY row (directly or via alias). A new spawn site must add its binary.
KNOWN_EXTERNAL_BINARIES: tuple[str, ...] = (
    "git", "gh", "bwrap", "unshare", "sandbox-exec", "systemd-run", "python3",
    "/usr/bin/open", "cargo", "rustc", "rustdoc", "xcrun", "otool",
)

# Spawn surfaces whose executable is chosen at run time. The marker proves the
# site still spawns; the row describes the surface and its gate, never a binary.
DYNAMIC_LAUNCHER_SITES: tuple[tuple[str, str, tuple[str, ...]], ...] = (
    ("mcp-stdio-servers", "src/common/mcp.rs", ("cmd.env_clear()", "filter_env(")),
    ("executor-caller-checks", "src/agentic/executor/runner.rs", ("production_sandbox()", "scrubbed_env")),
    ("shim-agentic-child", "src/shim/mod.rs", ("run_argv",)),
)

# ---------------------------------------------------------------------------
# Egress classification inventory. Every component that can touch a network
# -- crate, provider, executable, connector, launcher -- carries exactly one
# category:
#   1  unsolicited telemetry/analytics WITH an official control (every control
#      pair must exist in a SITES oracle above -- never invent one);
#   2  ancillary update/version/toolchain-fetch egress (NOT telemetry);
#   3  intentional, policy-gated functional egress (controls stay EMPTY: the
#      gate is harness policy, not an env var);
#   4  local-only (loopback, IPC, disk; or network denied by the sandbox);
#   5  absent / no mechanism found (controls EMPTY; evidence + date).
# `reviewed` is the date the evidence was last checked against the code.
# ---------------------------------------------------------------------------

INVENTORY: tuple[dict[str, object], ...] = (
    {
        "name": "github cli (gh)", "category": 1,
        "controls": {"GH_TELEMETRY": "false"},
        "url": "https://cli.github.com/manual/gh_help_environment",
        "versions": "external binary >= 2.40.0 (DEFAULT_MIN_GH in src/agentic/gh_client.rs)",
        "enforcement": "forced at every spawn through the shared builder's gh arm "
                       "(src/common/child_env.rs, Child::Gh): gh_env() for direct gh (gh_client.rs x2, "
                       "writer.rs) and git.rs environment() because git's credential.helper is "
                       "`!gh auth git-credential`; executor-runner carries it in the verifier arm for "
                       "checks that shell out to gh",
        "scope": "agentic read/write ops", "reviewed": "2026-10-03",
        "evidence": "code read 2026-10-03: one builder arm delivers it to both gh sites and the verifier "
                    "(parity O6 closed); the control name and true/false/log semantics are inherited "
                    "from CyClaw's 2026-08-27 vendor-doc read, not re-read here -- step 3 re-verifies "
                    "against gh help environment",
    },
    {
        "name": "huggingface-hub telemetry ping (inside verified repositories)", "category": 1,
        "controls": {"HF_HUB_DISABLE_TELEMETRY": "1", "DO_NOT_TRACK": "1"},
        "url": "https://huggingface.co/docs/huggingface_hub/package_reference/environment_variables",
        "versions": "not a harness dependency; whatever the verified repo's checks import (CyClaw pins "
                    "huggingface-hub itself). finetune/ mlx-lm is operator-shell only (see its row)",
        "enforcement": "executor-runner scrubbed_env() via the builder's verifier arm "
                       "(child_env Child::VerificationCheck); the hard sandbox already denies network, so "
                       "this is the second layer for a repo whose pytest imports huggingface_hub",
        "scope": "caller-declared verification checks", "reviewed": "2026-10-03",
        "evidence": "child_env.rs VerificationCheck arm read 2026-10-03; three-name OR "
                    "(HF_HUB_DISABLE_TELEMETRY / DISABLE_TELEMETRY / DO_NOT_TRACK) per CyClaw's "
                    "2026-09-28 read of v1.32.0 constants.py",
    },
    {
        "name": "chromadb posthog telemetry (inside verified repositories)", "category": 1,
        "controls": {"ANONYMIZED_TELEMETRY": "false"},
        "url": "https://docs.trychroma.com/docs/overview/telemetry",
        "versions": "not a harness dependency; a verified repo's own pin",
        "enforcement": "executor-runner scrubbed_env() via the builder's verifier arm (lower-case "
                       "`false`, which chromadb's pydantic settings parse the same as CyClaw's `False`); "
                       "network denied by the sandbox first",
        "scope": "caller-declared verification checks", "reviewed": "2026-10-03",
        "evidence": "child_env.rs VerificationCheck arm read 2026-10-03",
    },
    {
        "name": "gh update notifier", "category": 2,
        "controls": {"GH_NO_UPDATE_NOTIFIER": "1", "GH_NO_EXTENSION_UPDATE_NOTIFIER": "1"},
        "url": "https://cli.github.com/manual/gh_help_environment",
        "versions": "external binary",
        "enforcement": "the builder's gh arm (child_env Child::Gh), reached by agentic-git and agentic-gh. "
                       "GH_NO_EXTENSION_UPDATE_NOTIFIER landed with the shared builder (parity O6), "
                       "closing the step 4 candidate: extension update checks are a second release-"
                       "endpoint lookup gh runs on its own, independent of the CLI's own notifier",
        "scope": "agentic gh children", "reviewed": "2026-10-03",
        "evidence": "version-check egress to the release endpoint (CLI and installed extensions); "
                    "reports nothing about usage; both names mirror CyClaw's utils/telemetry_kill.py "
                    "UPDATE_CHECK_OPT_OUT",
    },
    {
        "name": "pip index / version check", "category": 2,
        "controls": {"PIP_NO_INDEX": "1", "PIP_DISABLE_PIP_VERSION_CHECK": "1"},
        "url": "https://pip.pypa.io/en/stable/cli/pip/#cmdoption-disable-pip-version-check",
        "versions": "any", "enforcement": "executor-runner scrubbed_env()",
        "scope": "caller-declared verification checks", "reviewed": "2026-10-03",
        "evidence": "documented pip options; PIP_NO_INDEX also refuses an accidental install step",
    },
    {
        "name": "cargo registry fetch", "category": 2,
        "controls": {"CARGO_NET_OFFLINE": "true"},
        "url": "https://doc.rust-lang.org/cargo/reference/config.html#netoffline",
        "versions": "any", "enforcement": "executor-runner scrubbed_env(); prepared CargoInputs supply "
                                          "the vendored sources (executor/prepared.rs)",
        "scope": "cargo checks inside the sandbox", "reviewed": "2026-10-03",
        "evidence": "cargo has no telemetry; the only egress is index/crate download, refused offline",
    },
    {
        "name": "rustup toolchain auto-install", "category": 2,
        "controls": {"RUSTUP_AUTO_INSTALL": "0"},
        "url": "https://rust-lang.github.io/rustup/environment-variables.html",
        "versions": "any", "enforcement": "desktop-sidecar and desktop-prepare-helper sites",
        "scope": "desktop shell children", "reviewed": "2026-10-03",
        "evidence": "rustup removed its own telemetry in 1.18 (2019); the remaining egress is a "
                    "toolchain download, which this pins off for the sidecar and helper",
    },
    {
        "name": "cloud chat providers (xAI Grok, Anthropic Claude, OpenAI-compatible)", "category": 3,
        "controls": {},
        "url": "docs/MODELS.md",
        "versions": "reqwest 0.12 (rustls-tls); src/llm/cloud_chat.rs, src/llm/openai_chat.rs",
        "enforcement": "provider key present (OS credential store or inherited env) + model selection; "
                       "spend ledger records tokens; no vendor-SDK telemetry switch exists -- the only "
                       "egress IS the model call",
        "scope": "console chat and coding runs", "reviewed": "2026-10-03",
        "evidence": "feature traffic, not telemetry; never block with an env pair",
    },
    {
        "name": "agentic cloud proposer", "category": 3, "controls": {},
        "url": "docs/CODING_PIPELINE.md",
        "versions": "reqwest blocking; src/agentic/proposer.rs, src/agentic/cloud_proposer.rs",
        "enforcement": "--provider / --confirm-online on the agentic child; agentic.enabled ships false",
        "scope": "agentic planner only", "reviewed": "2026-10-03",
        "evidence": "policy-gated feature traffic",
    },
    {
        "name": "ollama (loopback daemon)", "category": 3, "controls": {},
        "url": "docs/MODELS.md",
        "versions": "HTTP client only (src/llm/ollama.rs, src/llm/inventory.rs); never spawned",
        "enforcement": "loopback port 11434 by default; POST /api/ollama/pull asks the DAEMON to fetch a "
                       "model, admin/operator roles only. The daemon's own registry egress is outside "
                       "this process and has no documented telemetry switch -- do not invent one",
        "scope": "local inference + explicit pulls", "reviewed": "2026-10-03",
        "evidence": "loopback generate/tags are category 4; the pull is the one intentional egress",
    },
    {
        "name": "web search / fetch (SerpAPI, public Google, permitted URLs)", "category": 3,
        "controls": {},
        "url": "docs/SECURE_RESEARCH.md",
        "versions": "src/server/web_search.rs, web_policy.rs, web_research.rs",
        "enforcement": "web.enabled + exact/wildcard URL allowlist + DNS-pinned SSRF checks; proxies, "
                       "redirects, compression, retries and connection reuse disabled on the client",
        "scope": "chat web_search/web_fetch tools", "reviewed": "2026-10-03",
        "evidence": "policy-gated feature traffic",
    },
    {
        "name": "completion webhooks", "category": 3, "controls": {},
        "url": "docs/SPEND_AND_NOTIFICATIONS.md",
        "versions": "src/server/notifications.rs",
        "enforcement": "default-off, restart-only, exact owned destination grants, DNS pinning, no "
                       "redirects, metadata-only payloads",
        "scope": "job completion notices", "reviewed": "2026-10-03",
        "evidence": "policy-gated feature traffic",
    },
    {
        "name": "mcp clients (SSE / streamable HTTP)", "category": 3, "controls": {},
        "url": "docs/MCP_CLIENT.md",
        "versions": "rmcp =3.4.0 + reqwest; src/server/mcp.rs",
        "enforcement": "mcp.enabled literal true, declared mcp.servers only, DNS-pinned SSRF, "
                       "sse_allow_loopback ships false, confirm: true per call",
        "scope": "declared MCP servers", "reviewed": "2026-10-03",
        "evidence": "policy-gated feature traffic",
    },
    {
        "name": "github via gh/git (agentic reads and gated writes)", "category": 3, "controls": {},
        "url": "docs/CODING_PIPELINE.md",
        "versions": "external git + gh; src/agentic/git.rs, gh_client.rs, writer.rs",
        "enforcement": "agentic.enabled / deepagent_github.enabled / allow_git_write_tools all ship "
                       "false; confirm never defaulted; reason required; exit 4 = write refused",
        "scope": "agentic pipeline", "reviewed": "2026-10-03",
        "evidence": "feature traffic; the gh telemetry/update rows above are its secondary egress",
    },
    {
        "name": "mlx-lm fine-tune toolchain", "category": 3, "controls": {},
        "url": "docs/FINETUNE.md",
        "versions": "finetune/requirements.txt: mlx-lm>=0.22.0 (operator venv on Apple Silicon)",
        "enforcement": "operator-run shell, never spawned by the harness; model download from the HF "
                       "hub is the intentional egress. Shell-only HF_HUB_DISABLE_TELEMETRY=1 is the "
                       "operator's opt-out for the hub ping (a harness-side key would sit unread)",
        "scope": "finetune/ only", "reviewed": "2026-10-03",
        "evidence": "finetune/smoke_test.sh runs a stdlib mock server only; no HF import there",
    },
    {
        "name": "loopback console + sidecar transport", "category": 4, "controls": {},
        "url": "INVARIANTS.md",
        "versions": "axum 0.8, axum-server 0.8, rustls 0.23, rcgen, x509-parser; desktop reqwest probe",
        "enforcement": "non-loopback bind refused at startup; the desktop probe pins the sidecar's "
                       "exact certificate (desktop/src/backend.rs)",
        "scope": "every API request", "reviewed": "2026-10-03",
        "evidence": "loopback only",
    },
    {
        "name": "local stores and logs", "category": 4, "controls": {},
        "url": "docs/ANALYTICS.md",
        "versions": "rusqlite (bundled), tantivy 0.26, keyring 3.6.3 (OS store; Linux Secret Service "
                    "over D-Bus is local IPC), tracing-subscriber 0.3 (stderr + env-filter), audit and "
                    "spend JSONL",
        "enforcement": "no exporter crate in either lock graph (T5); `docs/ANALYTICS.md` is the local "
                       "spend/usage rollup, not vendor analytics",
        "scope": "process-local", "reviewed": "2026-10-03",
        "evidence": "T5 lock scan clean on 2026-10-03",
    },
    {
        "name": "hard-sandboxed verification children", "category": 4, "controls": {},
        "url": "docs/PROCESS_LIFECYCLE.md",
        "versions": "bwrap (--unshare-net) > unshare --net on Linux; sandbox-exec (deny network*) on "
                    "Darwin; Windows Job Object (process-tree kill only -- sockets keep working there)",
        "enforcement": "src/agentic/executor/sandbox.rs fails closed when no backend probes; the "
                       "executor-runner env pairs are the second layer, and the only layer on Windows",
        "scope": "caller-declared checks", "reviewed": "2026-10-03",
        "evidence": "network-denial literals confirmed in sandbox_wrap.rs / sandbox.rs (T12)",
    },
    {
        "name": "netconnect passive LAN inventory", "category": 4, "controls": {},
        "url": "docs/netconnect.md",
        "versions": "src/netconnect/ (first-party, stdlib + libc)",
        "enforcement": "read-only neighbor-cache/interface reads; no probes; gates ship false",
        "scope": "/net panel", "reviewed": "2026-10-03",
        "evidence": "no socket writes by design (INVARIANTS.md netconnect section)",
    },
    {
        "name": "os opener (/usr/bin/open)", "category": 4, "controls": {},
        "url": "docs/DESKTOP.md",
        "versions": "fixed path, env_clear(), PATH=/usr/bin:/bin",
        "enforcement": "native confirmation dialog first; scheme-checked URL; the browser that opens "
                       "is the OS's process, outside this contract",
        "scope": "desktop external links", "reviewed": "2026-10-03",
        "evidence": "desktop/src/main.rs external_link()",
    },
    {
        "name": "tauri desktop shell", "category": 5, "controls": {},
        "url": "https://v2.tauri.app/",
        "versions": "tauri =2.11.5, tauri-plugin-dialog 2, tauri-build 2; objc2/WebKit bindings",
        "enforcement": "no analytics or crash-reporting crate in desktop/Cargo.lock (T5); the webview's "
                       "own OS-level network behavior is Apple's, not this process's",
        "scope": "desktop/", "reviewed": "2026-10-03",
        "evidence": "T5 lock scan clean on 2026-10-03; tauri's updater plugin is NOT a dependency",
    },
    {
        "name": "pure library crates", "category": 5, "controls": {},
        "url": "docs/DEPENDENCIES.md",
        "versions": "see INVENTORY_ALIASES; versions authoritative in the lockfiles",
        "enforcement": "serialization, hashing, parsing, time, process and FFI crates with no network "
                       "code of their own; the HTTP stack (reqwest/hyper/rustls/url) is a transport "
                       "whose callers are classified in the category 3/4 rows",
        "scope": "both crates", "reviewed": "2026-10-03",
        "evidence": "T5 lock scan + manual read of each manifest on 2026-10-03",
    },
    {
        "name": "sandbox and toolchain binaries", "category": 5, "controls": {},
        "url": "docs/PROCESS_LIFECYCLE.md",
        "versions": "bwrap, unshare, sandbox-exec, systemd-run, git, cargo, rustc, rustdoc, xcrun, "
                    "otool, python3 (stdlib helper scripts only)",
        "enforcement": "none of these carries a documented telemetry switch; git has no telemetry; "
                       "cargo/rustc have none; the python helper imports only the stdlib",
        "scope": "spawned by the sites above", "reviewed": "2026-10-03",
        "evidence": "negative finding recorded 2026-10-03; re-check when a binary gains a switch",
    },
)

# Every direct dependency (both manifests), every external binary, and every
# finetune requirement resolves to a row name here or above.
INVENTORY_ALIASES: dict[str, str] = {
    # backend crates
    "rusqlite": "local stores and logs", "tantivy": "local stores and logs",
    "keyring": "local stores and logs", "tracing": "local stores and logs",
    "tracing-subscriber": "local stores and logs",
    "axum": "loopback console + sidecar transport", "axum-server": "loopback console + sidecar transport",
    "rustls": "loopback console + sidecar transport", "rcgen": "loopback console + sidecar transport",
    "x509-parser": "loopback console + sidecar transport",
    "rmcp": "mcp clients (SSE / streamable HTTP)",
    "reqwest": "pure library crates", "url": "pure library crates",
    "tokio": "pure library crates", "tokio-util": "pure library crates",
    "futures-util": "pure library crates", "serde": "pure library crates",
    "serde_json": "pure library crates", "serde_yaml_ng": "pure library crates",
    "clap": "pure library crates", "regex": "pure library crates",
    "unicode-normalization": "pure library crates", "sha2": "pure library crates",
    "hex": "pure library crates", "base64": "pure library crates", "subtle": "pure library crates",
    "scrypt": "pure library crates", "rand": "pure library crates", "tempfile": "pure library crates",
    "uuid": "pure library crates", "time": "pure library crates", "chrono": "pure library crates",
    "chrono-tz": "pure library crates", "cron": "pure library crates",
    "thiserror": "pure library crates", "anyhow": "pure library crates",
    "walkdir": "pure library crates", "zip": "pure library crates", "quick-xml": "pure library crates",
    "dunce": "pure library crates", "cap-std": "pure library crates",
    "scraper": "web search / fetch (SerpAPI, public Google, permitted URLs)",
    "robotstxt": "web search / fetch (SerpAPI, public Google, permitted URLs)",
    "libc": "pure library crates", "windows-sys": "pure library crates",
    # desktop crates
    "tauri": "tauri desktop shell", "tauri-plugin-dialog": "tauri desktop shell",
    "tauri-build": "tauri desktop shell", "objc2": "tauri desktop shell",
    "objc2-foundation": "tauri desktop shell", "objc2-web-kit": "tauri desktop shell",
    "objc2-core-foundation": "tauri desktop shell", "objc2-security": "tauri desktop shell",
    "block2": "tauri desktop shell",
    # external binaries
    "gh": "github cli (gh)", "git": "sandbox and toolchain binaries",
    "bwrap": "sandbox and toolchain binaries", "unshare": "sandbox and toolchain binaries",
    "sandbox-exec": "sandbox and toolchain binaries", "systemd-run": "sandbox and toolchain binaries",
    "python3": "sandbox and toolchain binaries", "cargo": "sandbox and toolchain binaries",
    "rustc": "sandbox and toolchain binaries", "rustdoc": "sandbox and toolchain binaries",
    "xcrun": "sandbox and toolchain binaries", "otool": "sandbox and toolchain binaries",
    "/usr/bin/open": "os opener (/usr/bin/open)",
    # finetune
    "mlx-lm": "mlx-lm fine-tune toolchain",
}

_REQUIRED_ROW_KEYS = ("name", "category", "controls", "url", "versions", "enforcement",
                      "scope", "reviewed", "evidence")

# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

_fails: list[dict[str, str]] = []
_warns: list[dict[str, str]] = []


def fail(check: str, detail: str) -> None:
    _fails.append({"check": check, "detail": detail})
    print(f"  FAIL  [{check}] {detail}")


def warn(check: str, detail: str) -> None:
    _warns.append({"check": check, "detail": detail})
    print(f"  WARN  [{check}] {detail}")


def ok(check: str, detail: str) -> None:
    print(f"  ok    [{check}] {detail}")


def info(check: str, detail: str) -> None:
    print(f"  info  [{check}] {detail}")


# ---------------------------------------------------------------------------
# Rust source helpers. No parser: a brace-matched function body with string
# literals and line comments skipped is enough for the literal shapes the
# oracles pin, and keeps the checker stdlib-only and toolchain-free.
# ---------------------------------------------------------------------------

def _read(root: Path, rel: str) -> str | None:
    path = root / rel
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return None


def _strip_test_tail(text: str) -> str:
    """Drop everything from the first `#[cfg(test)]` on (repo convention: the
    tests module is the last item in a file)."""
    idx = text.find("#[cfg(test)]")
    return text if idx < 0 else text[:idx]


def _strip_line_comments(text: str) -> str:
    """Drop `//` comments but leave string literals intact: a URL value whose
    scheme separator is `//` must survive, or a proxy pointed at a child would
    vanish from the sweep (verify.sh's T2 unexpected-pair scenario)."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(text[i:j + 1])
            i = j + 1
        elif text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _function_body(text: str, signature: str) -> str | None:
    """Body (between the outermost braces) of the first function whose
    signature matches, or None. Strings and `//` comments do not count."""
    m = re.search(signature, text)
    if not m:
        return None
    i = text.find("{", m.end())
    if i < 0:
        return None
    depth, j, n = 0, i, len(text)
    while j < n:
        c = text[j]
        if c == '"':
            j += 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
        elif text.startswith("//", j):
            j = text.find("\n", j)
            if j < 0:
                break
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1:j]
        j += 1
    return None


_NAME = r'"([A-Za-z_][A-Za-z0-9_]*)"'
_VALUE = r'"([^"\\]*)"'
# A bare tuple literal `("K", "V")`: not preceded by an identifier char or
# `!`, so `mcp_err("CODE", "msg")` and `format!(...)` never match.
_TUPLE_RE = re.compile(r'(?<![A-Za-z0-9_!])\(\s*' + _NAME + r'\s*,\s*' + _VALUE + r'\s*\)')
_INSERT_RE = re.compile(r'\.insert\(\s*' + _NAME + r'\.into\(\)\s*,\s*' + _VALUE + r'\.into\(\)\s*\)')
_ENV_CALL_RE = re.compile(r'\.env\(\s*' + _NAME + r'\s*,\s*' + _VALUE + r'\s*\)')


def _literal_pairs(body: str) -> dict[str, str]:
    pairs: dict[str, str] = {}
    for rx in (_TUPLE_RE, _INSERT_RE, _ENV_CALL_RE):
        for k, v in rx.findall(body):
            pairs[k] = v
    return pairs


def _string_array_after(text: str, anchor: str) -> set[str] | None:
    """Names in the first `= [..]` / `= &[..]` VALUE after the anchor (the
    `[&str; 6]` type annotation that precedes it is skipped on purpose)."""
    m = re.search(anchor, text)
    if not m:
        return None
    v = re.compile(r"=\s*&?\[").search(text, m.end())
    if not v:
        return None
    start = v.end()
    end = text.find("]", start)
    if end < 0:
        return None
    return set(re.findall(r'"([^"]+)"', text[start:end]))


def _diff_mapping(check: str, label: str, actual: dict, expected: dict) -> bool:
    good = True
    for k in sorted(set(expected) - set(actual)):
        fail(check, f"{label}: missing {k}={expected[k]!r}")
        good = False
    for k in sorted(set(actual) - set(expected)):
        fail(check, f"{label}: unexpected literal pair {k}={actual[k]!r} (classify it in the oracle)")
        good = False
    for k in sorted(set(actual) & set(expected)):
        if actual[k] != expected[k]:
            fail(check, f"{label}: {k} is {actual[k]!r}, expected {expected[k]!r}")
            good = False
    return good


# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------

_ARM_RE = re.compile(r"Child::([A-Za-z_][A-Za-z0-9_]*)\s*=>")


def _builder_arms(body: str) -> dict[str, dict[str, str]]:
    """Literal pairs per `Child::<Kind> =>` arm of the shared builder's match."""
    parts = _ARM_RE.split(body)
    return {parts[i]: _literal_pairs(parts[i + 1]) for i in range(1, len(parts), 2)}


# `Child::K` may be path-qualified (`child_env::Child::K`): mcp.rs already
# binds tokio's process `Child`, so it names the builder's enum by path.
_APPLY_RE = re.compile(r"child_env::apply\(\s*&mut\s+[A-Za-z_][A-Za-z0-9_]*\s*,\s*"
                       r"(?:[A-Za-z_][A-Za-z0-9_]*::)*Child::([A-Za-z_][A-Za-z0-9_]*)\s*\)")


def check_sites(root: Path) -> dict[str, dict[str, str]]:
    """T1 shapes + T2 exact pairs. Returns the EFFECTIVE pairs per site for T9:
    a site's own literals plus the shared-builder arm it names."""
    actual: dict[str, dict[str, str]] = {}
    arms: dict[str, dict[str, str]] = {}
    # The builder first, whatever the dict order, so every routed site can
    # resolve its arm; a missing or renamed builder then fails T1 once and
    # each routed site on its missing pairs (never a KeyError).
    for site in sorted(SITES, key=lambda s: "arms" not in SITES[s]):
        spec = SITES[site]
        text = _read(root, str(spec["file"]))
        if text is None:
            fail("T1", f"{site}: {spec['file']} missing")
            continue
        body = _function_body(_strip_test_tail(text), str(spec["signature"]))
        if body is None:
            fail("T1", f"{site}: no function matching /{spec['signature']}/ in {spec['file']}")
            continue
        ok("T1", f"{site}: builder present in {spec['file']}")
        clean = _strip_line_comments(body)
        pairs = _literal_pairs(clean)
        expected_arms = spec.get("arms")
        if expected_arms is not None:
            arms = _builder_arms(clean)
            for kind in sorted(set(expected_arms) - set(arms)):  # type: ignore[arg-type]
                fail("T2", f"{site}: builder has no `Child::{kind} =>` arm")
            for kind in sorted(set(arms) - set(expected_arms)):  # type: ignore[arg-type]
                fail("T2", f"{site}: unclassified arm Child::{kind} (pin it under `arms` in the oracle)")
            for kind in sorted(set(arms) & set(expected_arms)):  # type: ignore[arg-type]
                if _diff_mapping("T2", f"{site}/{kind}", arms[kind], dict(expected_arms[kind])):  # type: ignore[index]
                    ok("T2", f"{site}/{kind}: {len(arms[kind])} pair(s) match the oracle")
        # Credit a site only with the arms it ACTUALLY calls: a dropped or
        # misrouted apply() must lose those pairs, not inherit the oracle's
        # expectation (verify.sh's drop-apply and wrong-arm scenarios).
        called = _APPLY_RE.findall(clean)
        kind = spec.get("child_env")
        if kind is not None and str(kind) not in called:
            fail("T2", f"{site}: body no longer routes through `child_env::apply(&mut env, ...Child::{kind})` "
                       "(the site's own literals are not the whole contract)")
        if kind is None and called:
            fail("T2", f"{site}: calls child_env::apply but the oracle names no `child_env` arm for it")
        for called_kind in called:
            arm = arms.get(called_kind, {})
            for k in sorted(set(pairs) & set(arm)):
                fail("T2", f"{site}: pins {k} itself; the child_env `{called_kind}` arm owns it "
                           "(one owner per name)")
            pairs = {**pairs, **arm}
        actual[site] = pairs
        if _diff_mapping("T2", site, pairs, dict(spec["pairs"])):  # type: ignore[arg-type]
            ok("T2", f"{site}: {len(pairs)} effective pair(s) match the oracle")
        for needle in spec["mentions"]:  # type: ignore[union-attr]
            if needle not in body:
                fail("T2", f"{site}: body no longer mentions {needle!r}")
    return actual


def check_arrays(root: Path) -> None:
    for label, spec in EXPECTED_ARRAYS.items():
        text = _read(root, str(spec["file"]))
        if text is None:
            fail("T3", f"{label}: {spec['file']} missing")
            continue
        names = _string_array_after(_strip_test_tail(text), str(spec["anchor"]))
        if names is None:
            fail("T3", f"{label}: array after /{spec['anchor']}/ not found in {spec['file']}")
            continue
        expected: set[str] = spec["names"]  # type: ignore[assignment]
        extra, missing = sorted(names - expected), sorted(expected - names)
        if extra:
            fail("T3", f"{label}: unexpected name(s) {extra} (an inherit allowlist only ever narrows "
                       "without a matching oracle change)")
        if missing:
            fail("T3", f"{label}: missing name(s) {missing}")
        if not extra and not missing:
            ok("T3", f"{label}: {len(names)} names match")


def check_staleness(today: date) -> None:
    stale = 0
    for row in INVENTORY:
        try:
            reviewed = date.fromisoformat(str(row["reviewed"]))
        except ValueError:
            fail("T4", f"inventory row {row['name']!r}: unparseable reviewed date {row['reviewed']!r}")
            continue
        age = (today - reviewed).days
        if age > STALE_AFTER_DAYS:
            stale += 1
            warn("T4", f"inventory row {row['name']!r} reviewed {reviewed} ({age}d ago, > {STALE_AFTER_DAYS}d): "
                       "re-run the live vendor sweep and refresh the row")
    if not stale:
        ok("T4", f"all {len(INVENTORY)} inventory rows reviewed within {STALE_AFTER_DAYS}d of {today}")


def _lock_packages(root: Path, rel: str) -> dict[str, list[str]] | None:
    text = _read(root, rel)
    if text is None:
        return None
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        fail("T5", f"{rel}: unparseable Cargo.lock: {exc}")
        return None
    out: dict[str, list[str]] = {}
    for pkg in data.get("package", []):
        out.setdefault(str(pkg.get("name")), []).append(str(pkg.get("version")))
    return out


def check_lock_graph(root: Path) -> dict[str, dict[str, list[str]]]:
    locks: dict[str, dict[str, list[str]]] = {}
    for rel in ("Cargo.lock", "desktop/Cargo.lock"):
        pkgs = _lock_packages(root, rel)
        if pkgs is None:
            fail("T5", f"{rel} missing")
            continue
        locks[rel] = pkgs
        hits = sorted(n for n in pkgs if any(re.search(p, n) for p in DENIED_TELEMETRY_CRATE_PATTERNS))
        if hits:
            fail("T5", f"{rel}: telemetry/analytics SDK crate(s) in the lock graph: {hits}")
        else:
            ok("T5", f"{rel}: {len(pkgs)} packages, no telemetry/analytics SDK crate")
    return locks


def check_pin_drift(root: Path, locks: dict[str, dict[str, list[str]]]) -> None:
    for rel, expected in (("Cargo.lock", LAST_VERIFIED_CRATE_PINS),
                          ("desktop/Cargo.lock", DESKTOP_VERIFIED_CRATE_PINS)):
        pkgs = locks.get(rel)
        if pkgs is None:
            continue
        drift = 0
        for name, want in expected.items():
            have = pkgs.get(name)
            if have is None:
                warn("T6", f"{rel}: {name} is no longer in the lock graph; retire its pin and row")
                drift += 1
            elif want not in have:
                warn("T6", f"{rel}: {name} is {'/'.join(have)}, last verified {want}: re-read its changelog "
                           "for new network behavior, then update LAST_VERIFIED_*")
                drift += 1
            elif len(have) > 1:
                info("T6", f"{rel}: {name} resolves to several versions {have} (duplicate-version policy "
                           "is warn-only in deny.toml)")
        if not drift:
            ok("T6", f"{rel}: {len(expected)} verified pins unchanged")
    text = _read(root, "src/agentic/gh_client.rs")
    m = re.search(r"DEFAULT_MIN_GH[^=]*=\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", text or "")
    if not m:
        warn("T6", "DEFAULT_MIN_GH tuple not found in src/agentic/gh_client.rs")
    elif tuple(int(x) for x in m.groups()) != EXPECTED_MIN_GH:
        warn("T6", f"gh floor is {m.groups()}, inventory says {EXPECTED_MIN_GH}: update the gh row")
    else:
        ok("T6", f"gh floor {'.'.join(map(str, EXPECTED_MIN_GH))} unchanged")


def _rust_sources(root: Path) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for base in ("src", "desktop/src"):
        for path in sorted((root / base).rglob("*.rs")):
            rel = path.relative_to(root).as_posix()
            try:
                out.append((rel, _strip_test_tail(path.read_text(encoding="utf-8"))))
            except OSError:
                continue
    return out


def check_spawn_sites(sources: list[tuple[str, str]], strict: bool) -> None:
    seen: set[str] = set()
    for rel, text in sources:
        if "Command::new(" not in _strip_line_comments(text):
            continue
        seen.add(rel)
        if rel.startswith("src/server/"):
            fail("T7", f"{rel}: `Command::new(` inside src/server (INVARIANTS.md: the server spawns nothing; "
                       "only the shim does)")
        elif rel not in KNOWN_SPAWN_FILES:
            (fail if strict else warn)("T7", f"{rel}: new spawn site -- add it to KNOWN_SPAWN_FILES with a "
                                             "SITES oracle entry (or an INVENTORY row) for what it launches")
    for rel in sorted(KNOWN_SPAWN_FILES - seen):
        warn("T7", f"{rel}: listed in KNOWN_SPAWN_FILES but spawns nothing now; retire it")
    if seen <= KNOWN_SPAWN_FILES and not (KNOWN_SPAWN_FILES - seen):
        ok("T7", f"{len(seen)} spawn file(s), all classified")


_KILL_NAMES: dict[str, str] = {}
for _spec in SITES.values():
    _KILL_NAMES.update(_spec["pairs"])  # type: ignore[arg-type]
# Only the opt-out names matter for the bypass sweep; fixed PATH/locale values
# legitimately differ per site.
_SWEEP_NAMES = {k: v for k, v in _KILL_NAMES.items() if k not in ("PATH", "LANG", "LC_ALL")}
_REMOVE_RE = re.compile(r'(?:remove_var|env_remove)\(\s*"([A-Za-z_][A-Za-z0-9_]*)"')
_SET_VAR_RE = re.compile(r'set_var\(\s*"([A-Za-z_][A-Za-z0-9_]*)"')


def check_bypasses(sources: list[tuple[str, str]]) -> None:
    hits = 0
    for rel, text in sources:
        clean = _strip_line_comments(text)
        for k, v in _literal_pairs(clean).items():
            want = _SWEEP_NAMES.get(k)
            if want is not None and v != want:
                hits += 1
                fail("T8", f"{rel}: literal {k}={v!r} re-enables what every site pins to {want!r}")
        for rx, verb in ((_REMOVE_RE, "removes"), (_SET_VAR_RE, "sets process-wide")):
            for name in rx.findall(clean):
                if name in _SWEEP_NAMES:
                    hits += 1
                    fail("T8", f"{rel}: {verb} {name} (kill names are delivered per child, never mutated "
                               "in the parent)")
    if not hits:
        ok("T8", f"no programmatic re-enable of {len(_SWEEP_NAMES)} kill names in non-test source")


def check_wiring(root: Path, actual: dict[str, dict[str, str]]) -> None:
    # Every gh spawn builds its env from gh_env(); every git spawn from
    # environment(). A RunSpec with `env: None` inherits the parent verbatim
    # and silently loses the opt-outs -- T2 cannot see that.
    for rel, builder in (("src/agentic/gh_client.rs", "gh_env()"),
                         ("src/agentic/writer.rs", "gh_env()"),
                         ("src/agentic/git.rs", "environment(")):
        text = _read(root, rel)
        if text is None:
            fail("T9", f"{rel} missing")
            continue
        body = _strip_line_comments(_strip_test_tail(text))
        if "env: None" in body:
            fail("T9", f"{rel}: a RunSpec inherits the parent env (`env: None`); route it through {builder}")
        specs = body.count("RunSpec {")
        # Every textual occurrence minus the definition itself (`fn gh_env()`).
        calls = body.count(builder) - body.count(f"fn {builder}")
        if specs and calls < specs:
            fail("T9", f"{rel}: {specs} RunSpec(s) but only {calls} {builder} call(s); a spawn bypasses "
                       "the canonical builder")
        else:
            ok("T9", f"{rel}: {specs} spawn(s) all built by {builder}")
    for site in EXTERNAL_BINARY_SITES:
        pairs = actual.get(site)
        if pairs is None:
            continue
        for k, v in CANONICAL_BASE.items():
            if pairs.get(k) != v:
                fail("T9", f"{site}: external-binary site lacks canonical {k}={v!r}")
    routed = [s for s, spec in SITES.items() if spec.get("child_env")]
    ok("T9", f"shared builder src/common/child_env.rs (parity O6) feeds {len(routed)} site(s): "
             f"{', '.join(routed)}; the external-binary sites hold CANONICAL_BASE through it")


def _manifest_direct_deps(root: Path, rel: str) -> set[str] | None:
    text = _read(root, rel)
    if text is None:
        return None
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        fail("T10", f"{rel}: unparseable: {exc}")
        return None
    names: set[str] = set(data.get("dependencies", {}))
    names |= set(data.get("build-dependencies", {}))
    for target in data.get("target", {}).values():
        names |= set(target.get("dependencies", {}))
    return names


def _finetune_requirements(root: Path) -> set[str]:
    text = _read(root, "finetune/requirements.txt") or ""
    out: set[str] = set()
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            out.add(re.split(r"[<>=!~\[; ]", line, 1)[0].lower())
    return out


def check_inventory(root: Path, strict: bool) -> None:
    names = {str(r["name"]) for r in INVENTORY}
    site_pairs: dict[str, set[str]] = {}
    for spec in SITES.values():
        for k, v in spec["pairs"].items():  # type: ignore[union-attr]
            site_pairs.setdefault(k, set()).add(v)
    bad = 0
    for row in INVENTORY:
        missing = [k for k in _REQUIRED_ROW_KEYS if k not in row]
        if missing:
            fail("T10", f"row {row.get('name')!r} lacks {missing}")
            bad += 1
        cat, controls = row.get("category"), row.get("controls", {})
        if cat not in (1, 2, 3, 4, 5):
            fail("T10", f"row {row['name']!r}: category {cat!r} not in 1-5")
            bad += 1
        if cat in (3, 4, 5) and controls:
            fail("T10", f"row {row['name']!r}: category {cat} rows never carry controls")
            bad += 1
        if cat in (1, 2) and not controls:
            fail("T10", f"row {row['name']!r}: category {cat} needs an official control pair")
            bad += 1
        for k, v in dict(controls).items():  # type: ignore[arg-type]
            if v not in site_pairs.get(k, set()):
                fail("T10", f"row {row['name']!r}: control {k}={v!r} is delivered by no SITES builder "
                           "(a control nothing sets advertises protection the process cannot deliver)")
                bad += 1
    for alias, target in INVENTORY_ALIASES.items():
        if target not in names:
            fail("T10", f"alias {alias!r} -> {target!r}: no such inventory row")
            bad += 1
    if not bad:
        ok("T10", f"{len(INVENTORY)} rows + {len(INVENTORY_ALIASES)} aliases well-formed")

    unclassified: list[str] = []
    for rel in ("Cargo.toml", "desktop/Cargo.toml"):
        deps = _manifest_direct_deps(root, rel)
        if deps is None:
            fail("T10", f"{rel} missing")
            continue
        for dep in sorted(deps):
            if dep not in INVENTORY_ALIASES and dep not in names:
                unclassified.append(f"{rel}: {dep}")
    for binary in KNOWN_EXTERNAL_BINARIES:
        if binary not in INVENTORY_ALIASES and binary not in names:
            unclassified.append(f"binary: {binary}")
    for req in sorted(_finetune_requirements(root)):
        if req not in INVENTORY_ALIASES and req not in names:
            unclassified.append(f"finetune/requirements.txt: {req}")
    for item in unclassified:
        (fail if strict else warn)("T10", f"unclassified component {item}: add an INVENTORY row or alias "
                                          "with exactly one category")
    if not unclassified:
        ok("T10", "every direct dependency, external binary and finetune requirement is classified")

    for site, rel, markers in DYNAMIC_LAUNCHER_SITES:
        text = _strip_test_tail(_read(root, rel) or "")
        gone = [m for m in markers if m not in text]
        if gone:
            warn("T10", f"dynamic launcher {site!r}: {rel} no longer carries {gone}; retire or re-anchor its row")
        else:
            ok("T10", f"dynamic launcher {site!r} still anchored in {rel}")


def check_launcher_posture(root: Path) -> None:
    # The spawn sites that must CLEAR rather than inherit, and the worker that
    # must forward exactly what mcp.rs built. These are T2 `mentions` too, but
    # a dedicated verdict keeps the posture readable in the report.
    rules = (
        ("src/common/mcp.rs", r"pub async fn spawn\(", ("cmd.env_clear()", "filter_env(")),
        ("src/common/mcp_worker.rs", r"async fn run\(", (".env_clear()", ".envs(&spec.env)")),
        ("desktop/src/main.rs", r"fn external_link\(", (".env_clear()",)),
        ("desktop/src/backend.rs", r"pub fn start\(", ('.env("CGAGENTHARNESS_HOME"',)),
    )
    bad = 0
    for rel, sig, needles in rules:
        body = _function_body(_strip_test_tail(_read(root, rel) or ""), sig)
        if body is None:
            fail("T11", f"{rel}: function /{sig}/ not found")
            bad += 1
            continue
        for needle in needles:
            if needle not in body:
                fail("T11", f"{rel}: /{sig}/ no longer contains {needle!r}")
                bad += 1
    if not bad:
        ok("T11", "MCP spawn and worker clear the env; opener clears; sidecar pins its home")


def check_sandbox_network(root: Path) -> None:
    # The executor-runner pairs are the SECOND layer: the first is the sandbox
    # denying network. If those literals disappear the pairs become the only
    # layer on every platform, which Windows already is.
    wrap = _strip_test_tail(_read(root, "src/common/sandbox_wrap.rs") or "")
    sandbox = _strip_test_tail(_read(root, "src/agentic/executor/sandbox.rs") or "")
    bad = 0
    for label, text, needle in (("bwrap", wrap, '"--unshare-net"'),
                                ("seatbelt", wrap, "deny network*"),
                                ("netns", sandbox, '"--net"')):
        if needle not in text:
            fail("T12", f"{label}: network-denial literal {needle} missing from the sandbox wrapper")
            bad += 1
    if not bad:
        ok("T12", "sandbox network denial literals present (bwrap --unshare-net, seatbelt deny network*, "
                  "unshare --net); Windows Job Object remains env-only")


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--repo-root", type=Path, default=None)
    p.add_argument("--strict", action="store_true", help="treat WARN as failure (exit 2 on any warning)")
    p.add_argument("--json", action="store_true")
    p.add_argument("--as-of", type=date.fromisoformat, default=None, metavar="YYYY-MM-DD",
                   help="staleness anchor (default: today); lets tests pin a deterministic date")
    args = p.parse_args(argv)
    today = args.as_of or date.today()
    root = args.repo_root or Path(__file__).resolve().parents[3]
    if not (root / "Cargo.toml").exists():
        print(f"env error: not a harness checkout (no Cargo.toml): {root}", file=sys.stderr)
        return 3

    print(f"== cgagentharness-otel-hardening: telemetry-kill contract (as of {today.isoformat()}) ==")
    actual = check_sites(root)
    check_arrays(root)
    check_staleness(today)
    locks = check_lock_graph(root)
    check_pin_drift(root, locks)
    sources = _rust_sources(root)
    check_spawn_sites(sources, strict=args.strict)
    check_bypasses(sources)
    check_wiring(root, actual)
    check_inventory(root, strict=args.strict)
    check_launcher_posture(root)
    check_sandbox_network(root)

    strict_fail = args.strict and _warns
    print(f"\n{len(_fails)} failure(s), {len(_warns)} warning(s)"
          + (" (--strict: warnings count as failures)" if args.strict else ""))
    if args.json:
        print(json.dumps({"fails": _fails, "warns": _warns, "strict": args.strict,
                          "as_of": today.isoformat()}, indent=2))
    return 2 if (_fails or strict_fail) else 0


if __name__ == "__main__":
    sys.exit(main())
