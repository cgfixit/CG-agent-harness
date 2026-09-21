# Private read-only MCP memory gateway

The gateway implements the local, read-only selection in issues #102 and #185.
It runs in the harness process, uses the existing owner-filtered memory store,
and listens separately from the console. It does not open another structured
memory database or expose the portal. The outbound client remains controlled by
`mcp.enabled`; the inbound gateway uses `mcp.server.enabled`.

A new or upgraded home has no inbound listener unless that second switch is
literal `true`. Listener enablement grants no tools: `mcp.server.tools` defaults
to an empty list. The supported tools are `memory_list_facts`, `memory_get_fact`,
and `memory_search`. Each requires a dedicated `memory:read` key. No proposal,
fact write, session, notes, spend, audit, agent, chat or loop tool is published.

## Deliberate local activation

1. Stop the backend before changing listener configuration. Keep the home private;
   on Unix, its owner must own the directory and its mode must be `0700`. If an
   older home uses `0755`, explicitly change its permissions to `0700` first.
2. Enable structured memory and select exactly the tools to publish:

   ```yaml
   structured_memory:
     enabled: true
   mcp:
     server:
       enabled: true
       host: 127.0.0.1
       port: 8791
       tools: [memory_list_facts, memory_get_fact, memory_search]
   ```

   Merge these fields into the existing configuration; do not replace other
   sections. All gateway fields are restart-only and outside config reload's
   22-key allowlist. Port collision or invalid configuration refuses startup.
3. Mint a key using the local CLI. Use an existing account's stable `user_id`
   (visible in its authenticated account identity), `local` for an intentionally
   auth-disabled home's namespace, or an explicit `user_*` operator namespace:

   ```sh
   cgagentharness mcp-key create --owner YOUR_OWNER_ID --label workstation \
     --confirm --reason 'Allow this workstation to read the selected memory namespace'
   ```

   The CLI runs under trusted local filesystem authority; it does not impersonate
   a console administrator. `issued_by_user_id` is therefore null. Creation does
   not create facts or copy another owner's memory. Store the returned bearer
   token in the client's protected credential storage. It is printed once; it
   cannot be retrieved later. Do not put it in config, a URL, logs or screenshots.
4. Start `cgagentharness serve` or the native sidecar. Configure the MCP client
   to POST Streamable HTTP to `http://127.0.0.1:8791/mcp` with
   `Authorization: Bearer <token>`. Use the literal configured IP and port;
   `localhost` is not an alias for a different Host authority. The console
   retains its own account and TLS configuration.

The pinned official Rust SDK is `rmcp =3.4.0`, with only server and Streamable
HTTP transport features. The transport is stateless, including older supported
protocol versions; it allocates no durable MCP sessions. SDK trace output is
suppressed by the CLI's logging filter because protocol traces can contain
memory. Harness call/refusal audits contain public key IDs, recognized tool names
and coarse outcomes only, never arguments, fact text, tokens or hashes.

## Keys, isolation and revocation

Machine keys are independent of console users and cookies, even when console
account authentication is disabled. A key has a public 128-bit ID and a random
256-bit secret. Only the SHA-256 secret hash is stored, compared in constant time;
unknown/disabled keys follow a dummy-hash comparison path. No password KDF runs
per tool call. The dedicated `mcp_keys.sqlite3` uses a STRICT schema with its own
`user_version=1` and `mcp_keys.initialized` marker. Auth and memory schemas are
unchanged. On Unix, database and marker must be owned regular mode-0600 files,
not symlinks or hard links, and the home directory mode is `0700`. Missing
initialized, corrupt, oversized or unknown schema storage refuses instead of
recreating credentials. Windows does not get those Unix mode checks and this
build does not install a Windows ACL. Windows relies on the operator home's
private ACL plus SQLite's no-follow opening. Unix `0700`/`0600` is not claimed
on Windows.

```sh
cgagentharness mcp-key list
cgagentharness mcp-key revoke KEY_ID --confirm --reason 'Retire workstation access'
```

Listing returns metadata only. `mcp-key create` refuses an owner that does not
exist or is disabled. When account authentication is off, the only owner it
accepts is `local`. Disabling or deleting an account sets `disabled=1` on that
owner's machine keys. Revoking one key by its public id still works and does
not disable a different owner's keys. Revocation is observed by every later
request and rechecked before memory reads. A read already executing may finish.
That is not an instantaneous kill of the in-flight read. Owner binding is
immutable; rotation requires minting another
key. The table defaults to eight rows, with a hard ceiling of 32. At capacity,
revoke an old key before minting; oldest disabled rows are removed during rotation.
Content-free creation/revocation events remain in the bounded audit log.

The key owner comes only from its stored row. Arguments cannot override it.
Every fact query filters by that owner; inactive facts are hidden, and a foreign
fact ID has the same not-found result as an unknown ID. Search reuses the bounded
literal-substring path, not a query language or FTS expression. Returned memory
is untrusted data, never tool authority or system instructions.

## Bounds and failures

| Restart-only setting | Default | Accepted range |
|---|---:|---:|
| `max_keys` | 8 | 1–32 |
| `max_request_bytes` | 16384 | 1024–65536 |
| `max_result_bytes` | 65536 | 1024–262144 |
| `max_results` | 32 | 1–64 |
| `max_query_chars` | 256 | 1–512 |
| `request_timeout_sec` | 5 | 1–30 |
| `concurrency` | 2 | 1–8 |
| `requests_per_minute` | 60 | 1–120 per key |
| `global_requests_per_minute` | 180 | 1–600 total, including invalid keys |

Limits are under `mcp.server`. Rates use fixed one-minute windows. Authentication,
body read and response collection share the request deadline. Synchronous store
work holds its concurrency permit until completion even if the caller times out.
Request body limits apply to streamed bodies, independent of Content-Length.
The complete serialized response is capped; oversize results fail without a
partial fact. List/search inspect the store's existing latest-256-active-facts
window and return at most `max_results`; they are not an unbounded archive export.

Only direct loopback peers and the exact configured Host authority are accepted.
Any Origin or forwarding header is refused. This endpoint is for machine clients,
not browser JavaScript. Cookies never authorize it; a machine key never authorizes
console routes. Plain HTTP is confined to local loopback. No proxy, tunnel, port
mapping or remote listener is installed. Drop/shutdown closes the owned listener.

## Future remote exposure requires a separate reviewed change

There is no remotely enabled mode in this release. A future LAN/private-overlay
mode and a future public-internet mode require separate deployment acceptance:
explicit interface and mode selection, TLS for remote HTTP, authenticated machine
identities, revocable per-tool/owner scope, exact Host/Origin rules, bounded load,
console isolation, and tested disable/rollback. Changing one tool grant must never
change deployment mode. Remote proposal writes, session handoff, spend, audit and
eval tools each require their own capability decision and tests. Memory proposals
must still pass the existing scanner and human application flow; no remote tool
may write canonical facts directly or inherit coding authority.
