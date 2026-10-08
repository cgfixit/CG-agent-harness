# Security policy

CGagentHarness is early software (`0.1.x` / `main`): a loopback-only Rust chat
and coding harness. Sessions, memory notes, the chat persona and the bounded
public-web cache and research controller are local surfaces. No Telegram integration.

## Supported versions

| Version | Supported |
|---|---|
| `0.1.x` and `main` | Yes; expect breaking changes. |
| Anything else | No published support window |

There is no LTS release yet. Fixes land on `main`.

## Reporting a vulnerability

Do **not** file a public issue, discussion, or pull request.

Report privately through GitHub Security Advisories on this repository:

https://github.com/cgfixit/CG-agent-harness/security/advisories

Use **Report a vulnerability**. If that button is missing, open a **draft
security advisory** on the same page, or ask a maintainer to start one and omit
exploit details until it exists.

Include the affected version or commit, impact on a local operator and
reproduction steps.

## Scope

The enforcement contracts live in [INVARIANTS.md](INVARIANTS.md):

- Bind is loopback-only; a non-loopback host is refused at startup.
- Fresh homes require HTTPS and account authentication. Accounts and hashed
  sessions persist transactionally in SQLite. Bootstrap admin/admin is restricted
  to password replacement; roles protect operational reads as well as writes.
- Harness API keys are optional metadata and never bypass accounts or coding
  gates. Forwarded/proxy requests are refused.
- Content URLs require current whole-policy permission and DNS-pinned public
  destinations. Discovery, cached passages and injected context use the same
  policy. This is not an OS firewall or permission for model/GitHub connections.
- Native TLS trust is bound to the owned sidecar's exact certificate; no system
  root install or global validation bypass.
- I6: the HTTP process never calls the agentic pipeline in-process; it
  only spawns `cgagentharness agentic …` as a child.
- Master, deepagent, and clone-write gates ship closed (`agentic.enabled`,
  `deepagent_github.enabled`, `allow_git_write_tools`). Adjacent fields such as
  `mode: write` and `writes_enabled: true` already ship permissive and cannot
  arm writes alone.
- Provider keys live in the OS credential store; environment variables win and
  the plaintext key file is an opt-in that ships off.
- Netconnect is passive, read-only and LAN-scoped: every gate ships false and an
  empty `allowed_cidrs` refuses armed tiers.
- No push telemetry: `deny.toml` bans such crates; CI runs `cargo deny` and zizmor
  and SHA-pins actions.

## Out of scope

- Social engineering or phishing against operators
- Denial of service against a local loopback listener
- Issues that reproduce only after write or agentic gates are deliberately armed

## Local authority and shared resources

An administrator/operator can access shared portal chat sessions, jobs, persona
and notes; these are not tenants. Research requests, web selections, and
structured-memory facts/episodes/FTS hits are account scoped. Recalled facts
are untrusted context and cannot grant tool, coding, or network authority. Auditors see designated redacted status/audit information. Filesystem
owners can edit configuration, databases and binaries, and are outside the portal
account boundary; no guarantee against a malicious same-user filesystem race is
claimed. Native sandbox limits remain in INVARIANTS.md. See
[secure research](docs/SECURE_RESEARCH.md) for migration, revocation, TLS recovery
and evidence limitations.
