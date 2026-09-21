# External MCP client capabilities

MCP stays disabled until an operator declares servers and enables `mcp.enabled`.
Calls require an authenticated operator/admin, CSRF, the declared namespaced
tool in the broker allowlist, and literal `confirm: true`. There is no discovery
or model/repository permission to expand the declaration. `/loop` has no MCP
tools. The client is separate from the proposed read-only memory gateway (#185).

## Migrate a stdio declaration

Existing stdio declarations without `capabilities` are refused at startup.
Review the paths and choose a lifecycle policy explicitly; no upgrade silently
grants old ambient access. Disabled MCP with no declarations is unchanged.
All declarations and grants require restart and are outside the 22-key config
reload allowlist.

```yaml
mcp:
  enabled: true
  sse_allow_loopback: false
  timeout_sec: 15
  max_result_bytes: 65536
  servers:
    - name: documents
      transport: stdio
      command: [/usr/bin/python3, /srv/mcp/server.py]
      tools: [lookup]
      capabilities:
        version: 1
        read_roots: [/srv/mcp/server.py, /srv/documents]
        write_roots: []
        network: deny
        containment: strict
        limits:
          processes: 32
          memory_mb: 256
```

Replace these example paths with existing, deliberately granted paths. At most
16 read/write roots are accepted. Roots must be absolute, cannot contain `..`
or name the filesystem root, and are canonicalized again for every invocation.
The harness home and any ancestor/descendant overlap are refused. The optional
`cwd` is an additional read-only grant; omit it for an empty owned directory.
The executable and fixed OS runtime files are readable. Script arguments do
not grant their directories: declare scripts, modules and non-system runtimes
explicitly. Host writes are limited to owned scratch and declared write roots.
Linux also has private namespace storage (including `/tmp`); a write there does
not modify a same-named host path. Tests check the host filesystem as well as
the tool's result, including an unmounted host canary that must remain unchanged.

`network: deny` is mandatory unless the operator explicitly selects
`unrestricted`. Missing fields, unknown versions/fields/enums, invalid limits
and unavailable protections fail closed. The unrestricted grant includes all
destinations the OS allows; subprocess networking does not use the web URL
allowlist or HTTP DNS pinning. Linux never silently drops network isolation
when its network namespace probe fails.

## Lifecycle choices

| Policy / host | Behavior |
|---|---|
| `strict`, Linux | systemd service + cgroup v2 + bubblewrap; requires usable process and memory controllers |
| `strict`, macOS / Windows | refuses before the declared program runs |
| `process_group`, macOS | Seatbelt filesystem/network policy; explicit weaker lifecycle exception |
| `process_group`, Linux | bubblewrap filesystem policy and selected network policy; explicit weaker lifecycle exception |
| `process_group`, Windows | refuses; required filesystem confinement is unavailable |

Strict limits are required: `processes` 8–128 and `memory_mb` 64–4096. The
process count includes supervisor/runtime processes and the memory cap covers
the cgroup. A working systemd version supporting `--expand-environment=no`
(254+) and cgroup v2 is required. Normal Linux operators use their existing
user service manager; root uses the system manager. The harness does not invoke
sudo, install a service or change host namespace settings. Unavailable user
controllers or namespace permissions cause refusal.

To deliberately retain weaker cleanup on macOS/Linux, replace `containment`
with `process_group` and omit `limits`. This exception promises neither
aggregate resource limits nor cleanup after runner death or detached descendant
escape. `/tools mcp` displays the exception. Do not describe it as strict
containment. See [lifecycle details and acceptance](PROCESS_LIFECYCLE.md).

## Inspect and troubleshoot

`/tools mcp` and authenticated `GET /api/mcp` show declarations and grants,
without spawning tools or claiming readiness. Successful calls audit the actual
backend, policy and probe result; refusals audit the policy and error code.
Arguments, tool results and environment secrets are not audited.

| Result | Meaning |
|---|---|
| `CONFIG_ERROR` at startup | Missing/invalid capabilities; repair the declaration before restarting |
| `MCP_CAPABILITY_REFUSED` | A granted path is unavailable or exposes a strict control boundary |
| `MCP_HOME_REFUSED` | Command path, working directory or grant overlaps the harness home |
| `HARD_SANDBOX_UNAVAILABLE` | Required filesystem/network protection cannot execute |
| `MCP_CONTAINMENT_UNAVAILABLE` | Strict OS ownership/limits cannot be established |
| `MCP_TIMEOUT` | The overall invocation deadline expired |

SSE declarations continue using DNS-pinned URL checks and the separate,
default-off loopback grant. They do not receive local stdio filesystem grants.
