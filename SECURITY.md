# Security policy

CGagentHarness is early software (`0.1.x` / `main`). This repo is a
loopback-only Rust chat and coding harness. Persistent sessions, operator memory
notes and an explicitly edited chat persona are local application surfaces.
The permitted public-web subsystem has a bounded passage cache/index and local
research controller. There is no Telegram integration. New chat sessions retain
shared persona/notes and the initiating account's selected web context.

## Supported versions

| Version | Supported |
|---|---|
| `0.1.x` and `main` | Yes — current development line. Expect breaking changes. |
| Anything else | No published support window |

There is no LTS release yet. Fixes land on `main`.

## Reporting a vulnerability

Do **not** file a public issue, discussion, or pull request.

Report privately through GitHub Security Advisories on this repository:

https://github.com/cgfixit/CG-agent-harness/security/advisories

Use **Report a vulnerability** when private vulnerability reporting is
enabled. If that button is missing, open a **draft security advisory** on
the same page (ask a maintainer to start one if you cannot, and omit
exploit details until the advisory exists).

Include the affected version or commit, impact on a local operator, and
reproduction steps. Coordinate disclosure through the advisory.

## Scope

The enforcement contracts live in [INVARIANTS.md](INVARIANTS.md):

- Bind is loopback-only; a non-loopback host is refused at startup.
- Fresh homes require HTTPS and account authentication. Accounts and hashed
  sessions persist transactionally in SQLite. Bootstrap admin/admin is restricted
  to password replacement; roles protect operational reads as well as writes.
- Harness API keys are optional metadata. They never bypass accounts or coding
  gates. Forwarded/proxy requests are refused; existing configuration is preserved.
- Content URLs require current whole-policy permission and DNS-pinned public
  destinations. Discovery, cached passages and injected context use the same
  policy. This is not an OS firewall or permission for model/GitHub connections.
- Native TLS trust is bound to the owned sidecar's exact certificate; no system
  root installation or global certificate-validation bypass occurs.
- I6: the HTTP process never calls the agentic pipeline in-process; it
  only spawns `cgagentharness agentic …` as a child.
- Master, deepagent, and clone-write gates ship closed (`agentic.enabled`,
  `deepagent_github.enabled`, `allow_git_write_tools`). Adjacent fields such as
  `mode: write` and `writes_enabled: true` already ship permissive and cannot
  arm writes alone.

## Out of scope

- Social engineering or phishing against operators
- Denial of service against a local loopback listener
- Issues that reproduce only after write or agentic gates are deliberately armed

## Local authority and shared resources

An administrator/operator can access shared portal chat sessions, jobs, persona
and notes; these are not tenants. Research requests and web selections are account
scoped. Auditors see designated redacted status/audit information. Filesystem
owners can edit configuration, databases and binaries, and are outside the portal
account boundary. No atomic guarantee against a malicious same-user filesystem
race is claimed. Existing native execution-sandbox/platform limits remain in
INVARIANTS.md. See [secure research](docs/SECURE_RESEARCH.md) for migration,
revocation, TLS recovery and evidence limitations.
