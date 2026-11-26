# MCP: external tools and the private memory gateway

Outbound `mcp.enabled` and inbound `mcp.server.enabled` are independent and
ship off. `/loop` receives no MCP tools.

## Outbound client capabilities

Calls require an authenticated operator or administrator, declared namespaced
tool, broker grant, CSRF and `confirm: true`. Stdio with empty write roots,
denied networking and containment other than `job_object` is read-only.
Other servers, including all SSE, are privileged and admin-only unless that
server has `operator_allowed: true`. `/tools mcp` and `GET /api/mcp` inspect
authority without launching a server.

## Migrate a stdio declaration

Specify all capabilities in private `config.yaml` and restart. Missing
or malformed declarations refuse startup.

```yaml
mcp:
  enabled: true
  servers:
    - name: documents
      transport: stdio
      command: [/usr/bin/python3, /srv/mcp/server.py]
      tools: [lookup]
      capabilities:
        version: 1
        filesystem: confined
        read_roots: [/srv/mcp/server.py, /srv/documents]
        write_roots: []
        network: deny
        containment: strict
        limits: {processes: 32, memory_mb: 256}
```

Grant only required absolute roots (up to 16). Overlap with the home fails.
An optional `cwd` is a read grant. Command arguments confer no path grants.
`network: unrestricted` permits OS network access without web URL pinning.
Whole-value `env:NAME` for `env` or the full SSE `url` resolves from
inherited environment with reserved secret and runtime names refused.

## Lifecycle choices

| Mode | Platform |
|---|---|
| `strict` | Linux, systemd 254+, cgroup v2 process/memory controllers and bubblewrap |
| `process_group` | Linux bubblewrap or macOS Seatbelt, with weaker cleanup and no `limits` |
| `job_object` | Windows only, trusted-server exception |

Strict and Job Object limits are 8–128 processes and 64–4096 MiB.
Missing protections fail closed. See [lifecycle](PROCESS_LIFECYCLE.md#explicit-mcp-lifecycle-policy).

### Read-only host files on macOS

Use `filesystem: confined`, `containment: process_group`, empty
`write_roots`, `network: deny` and only the declared tool and needed read
roots. Omit `limits`. Each call still needs explicit confirmation.

## Explicit Windows trusted-server exception

Use an absolute native `.exe`, `filesystem: unrestricted`,
`network: unrestricted`, `containment: job_object` and explicit limits.
Root lists must be empty. The Job Object owns the tree and aggregate
resources. Host files and network remain accessible to this trusted server.

## Inspect and troubleshoot

`MCP_HOME_REFUSED` means a grant overlaps the home.
`HARD_SANDBOX_UNAVAILABLE` and `MCP_CONTAINMENT_UNAVAILABLE` mean required
OS protections are unavailable. Invocation deadlines report `MCP_TIMEOUT`.
`mcp.timeout_sec` and `mcp.max_result_bytes` bound overall call time and
newline-delimited response size. `MCP_ROLE_REFUSED` identifies the denied
server without including arguments.
SSE uses the 2024-11-05 HTTP+SSE transport, pinned DNS, bundled roots and distinct
`sse_allow_loopback` opt-in. Audit records authority and coarse outcomes.

## Private read-only memory gateway

The inbound listener shares the structured store and serves only selected
`memory_list_facts`, `memory_get_fact` and `memory_search` tools under
`memory:read` scope. Search is bounded literal substring over active facts.

## Deliberate local activation

1. Stop the backend. On Unix require an owner-private 0700 home.
2. Enable `structured_memory.enabled` and `mcp.server.enabled`. Set a
   literal loopback host, port (default 8791) and exact `mcp.server.tools` list.
3. Use `cgagentharness mcp-key create --owner <owner-id> --label <label>
   --confirm --reason <reason>`. Use a stable account ID, or `local` when
   auth is off. The owner must be enabled, past bootstrap and allowed memory.
4. Restart and configure the machine client for Streamable HTTP POST to
   `http://127.0.0.1:8791/mcp` (adjust to match configuration) with the
   one-time bearer in protected storage. Match the exact configured Host.
   Cookies, Origin and forwarding headers grant no authority.

## Keys, isolation and revocation

Keys store only secret hashes in `mcp_keys.sqlite3` alongside an
initialization marker. `mcp-key list` returns metadata. Revoke using
`mcp-key revoke <id> --confirm --reason <reason>`. Disablement, deletion or
demotion to auditor disables owner keys. Owner binding cannot be reassigned.
Each query rechecks scope and owner. A read already executing may finish.
Unknown and foreign IDs give the same result. Revocation leaves the owner
binding immutable. Promotion does not restore disabled machine keys.
On Unix the key database and marker must be owner-owned regular private
files with no links. Missing initialized storage or invalid schema refuses.

## Bounds and failures

Request, response, rate, concurrency and deadline bounds are under
`mcp.server` in [shipped configuration](../assets/config.default.yaml).
Body size applies during streaming. Oversized results fail whole. Audit
contains key IDs, tool names and coarse outcomes. The separate listener is
plain HTTP on loopback with no remote mode.
