# Security policy

CGagentHarness is early software (`0.1.x` / `main`). This repo is a
loopback-only Rust coding harness. It is **not** CyClaw: there is no RAG,
corpus, soul, or Telegram surface here.

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

What the code guarantees lives in [INVARIANTS.md](INVARIANTS.md). In short:

- Bind is loopback-only; a non-loopback host is refused at startup.
- An unset `CGAGENTHARNESS_API_KEY` fails closed (guarded routes 401).
- I6: the HTTP process never calls the agentic pipeline in-process; it
  only spawns `cgagentharness agentic …` as a child.
- Every write gate ships closed.

## Out of scope

- Social engineering or phishing against operators
- Denial of service against a local loopback listener
- Issues that reproduce only after write or agentic gates are deliberately armed
