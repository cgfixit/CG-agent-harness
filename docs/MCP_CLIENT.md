# External MCP client capabilities

MCP stays disabled until an operator declares servers and enables `mcp.enabled`.
Calls require an authenticated operator/admin, CSRF, the declared namespaced
tool in the broker allowlist, and literal `confirm: true`. There is no discovery
or model/repository permission to expand the declaration. `/loop` has no MCP
tools. The client is separate from the [read-only memory gateway](MCP_SERVER.md), whose listener, keys and tool grants are independent.

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
        filesystem: confined
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
For confined policies (the default), the harness home and any ancestor/descendant overlap are refused. The optional
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
| `job_object`, Windows | explicit trusted-server exception; unrestricted filesystem/network, atomic process-tree ownership and resource limits |
| `job_object`, macOS / Linux | refuses before execution |

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

## Explicit Windows trusted-server exception

Windows cannot enforce this client's filesystem/network sandbox policy. Only
for a trusted server, an operator can deliberately declare:

```yaml
capabilities:
  version: 1
  filesystem: unrestricted
  network: unrestricted
  containment: job_object
  limits: {processes: 8, memory_mb: 256}
```

Use an absolute native `.exe` command; batch files are refused. Read/write root
lists must be empty because they cannot restrict this mode. Missing
`filesystem` defaults to `confined` and therefore refuses this exception.
Process limits use the same finite ranges as strict Linux; memory covers the
whole job. The backend is reported as `windows-job-object-unrestricted`.
The console explicitly warns that account-accessible secrets are readable.
Scrubbing environment variables and checking declared command/cwd paths do not
prevent this trusted program from opening other paths or contacting services.

Membership exists before the first child instruction. Normal detached children
cannot break away, and the OS terminates job members when the runner exits even
without cleanup code. Windows services outside the job (including WMI/COM) are
outside this guarantee. A responsive harness enforces the request deadline;
this is not an OS wall-clock timer or a hostile-code sandbox. No fallback,
model output or hot reload can choose this exception. Future AppContainer/VM
isolation requires a separate implementation and acceptance.

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
