# Private read-only MCP memory gateway

The gateway runs in the harness process, uses the owner-filtered memory store,
and listens separately from the console. It does not open another memory
database or expose the portal. `mcp.enabled` controls the outbound client;
`mcp.server.enabled` controls this inbound gateway.

New and upgraded homes have no inbound listener unless the switch is literal
`true`. `mcp.server.tools` defaults empty. Supported tools are
`memory_list_facts`, `memory_get_fact`, and `memory_search`; each requires a
`memory:read` key. The gateway publishes no write, session, notes, spend, audit,
agent, chat, or loop tool.

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

   The CLI uses trusted local filesystem authority, so `issued_by_user_id` is
   null. Creation neither adds facts nor copies memory. Store the one-time bearer
   token in protected client storage, never config, URLs, logs, or screenshots.
4. Start `cgagentharness serve` or the native sidecar. Configure the MCP client
   to POST Streamable HTTP to `http://127.0.0.1:8791/mcp` with
   `Authorization: Bearer <token>`. Use the literal configured IP and port;
   `localhost` is not an alias for a different Host authority. The console
   retains its own account and TLS configuration.

The pinned Rust SDK is `rmcp =3.4.0`, with only server and Streamable HTTP
transport features. The transport is stateless, including for older supported
protocol versions, and creates no durable MCP sessions. The CLI suppresses SDK trace output because protocol traces can
contain memory. Call and refusal audits contain only public key IDs, recognized
tool names, and coarse outcomes. They omit arguments, fact text, tokens, and hashes.

## Keys, isolation and revocation

Machine keys are independent of console users and cookies, even when console
account authentication is disabled. A key has a public 128-bit ID and a random
256-bit secret. Only the SHA-256 secret hash is stored, compared in constant time;
unknown/disabled keys follow a dummy-hash comparison path. No password KDF runs
per tool call. The dedicated `mcp_keys.sqlite3` uses a STRICT schema with its own
`user_version=1` and `mcp_keys.initialized` marker. Auth and memory schemas are
unchanged. On Unix, the database and marker must be owner-owned, regular
mode-0600 files with no symlinks or hard links; the home must be mode `0700`.
Windows does not apply those Unix mode checks or install a database ACL. It
relies on the operator home's private ACL and SQLite's no-follow open. Missing,
corrupt, oversized, or unknown-schema storage refuses instead of recreating
credentials. Unix `0700` and `0600` are not claimed on Windows.

```sh
cgagentharness mcp-key list
cgagentharness mcp-key revoke KEY_ID --confirm --reason 'Retire workstation access'
```

Listing returns metadata only. `mcp-key create` refuses a missing or disabled
owner. With account authentication off, it accepts only `local`. Disabling or
deleting an account disables that owner's machine keys. Revoking one key does
not affect another owner's keys. Each later request observes revocation and
rechecks it before a memory read, but an executing read may finish. Owner binding
is immutable; mint a new key to rotate it. The table defaults to eight rows and
has a hard ceiling of 32. At capacity, revoke an old key; rotation removes the
oldest disabled rows.
Content-free creation/revocation events remain in the bounded audit log.

The stored row alone supplies the owner. Every query filters on it, hides
inactive facts, and gives foreign and unknown IDs the same result. Search is a
bounded literal substring, not FTS or a query language. Returned memory is
untrusted data, not tool authority or instructions.

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

This release has no remote mode. LAN, private-overlay, and public modes require
separate acceptance for interface selection, TLS, machine identity, revocable
tool and owner scope, Host and Origin rules, bounded load, console isolation,
and rollback. Tool grants must not change deployment mode. Each new write,
session, spend, audit, or evaluation tool needs its own capability and tests.
Remote tools must not write canonical facts directly or inherit coding authority.
Memory proposals must still pass the scanner and human application flow.
