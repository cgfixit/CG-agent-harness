# Private memory gateway acceptance

Verified on 2026-09-21 before committing or drafting this PR, against base
`cd55c3f73e7b5877a2ce763f8d5342ca70a6b36b`.

[Gateway diagnostics](private-memory-gateway-safari.png) was captured through
Safari Computer Use on this gateway-only binary, SHA-256:
`086ff8e5044930e59c7803d9bb351a35f04cc45df73ddc00786901f6ae2f2dca`.
The console's `/tools mcp` displays the distinct default-off outbound client and
explicitly configured local gateway, three selected read tools, dedicated machine
key requirement and owner-bound `memory:read` scope. This is configuration
information, not a live listener-health probe.

[Final combined diagnostics](combined-final-gateway-safari.jpg) was captured on
the locally combined automation and gateway binary, SHA-256:
`3f8eb280b5934ba297d99b0c48b42a163d843d54925f1c53753a9ccc4298b318`.
The automation feature is a separate main-based PR and is not included here.

All homes/accounts/facts/model output were disposable synthetic fixtures on
loopback. No bearer was displayed in screenshots or saved in the browser.
An actual current-protocol HTTP tool call read only the fixture key's fact;
revoking that key through the public CLI made the next request return 401 without
restarting the service. That combined runtime call used the preceding integration
binary `e866e262cc5ed3ec202267b08f045caad4b728428dcf0712c67f7c5bc3124811`;
its gateway code is identical to the final combined binary (only automation form
reset changed afterward).

Deterministic final verification: 661 tests in 48 suites on this branch; 682 tests
in 49 suites on the combined candidate; strict Clippy, formatting, cargo-deny and
shipped-config invariant checks passed. Eleven focused gateway tests exercise
actual current/legacy Streamable HTTP, feature-off/empty-tools behavior, owner
isolation, cookie/key separation, revocation, malformed private key storage,
request/result/rate/concurrency bounds and slow-body timeout recovery. The final
combined native sidecar also passed all 11 lifecycle/auth/home-lock tests.

No remote listener, remote writes, live paid provider, packaged/notarized GUI or
third-party MCP client interoperability claim is made. The SDK protocol is tested
directly; future remote deployment requires its own acceptance.
