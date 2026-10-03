# External MCP client capabilities

MCP requires declared servers and enabled `mcp.enabled`. Calls need an
authenticated operator/admin, CSRF, a declared namespaced tool in the broker
allowlist, and literal `confirm: true`. Discovery, models and repositories
cannot expand declarations. `/loop` has no MCP tools. The client is separate from the [read-only memory gateway](MCP_SERVER.md), whose listener, keys and tool grants are independent.

## Migrate a stdio declaration

Stdio declarations need `capabilities` or startup refuses. Review paths and
lifecycle policy; upgrades grant no ambient access. Disabled, undeclared MCP
stays off. Declarations require restart, outside the 22-key reload allowlist.

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

Replace the example paths with deliberate grants. At most 16 roots are accepted.
Roots must be absolute, exclude `..` and the filesystem root, and are
re-canonicalized for each call. Confined policies refuse the harness home and
all overlaps. Optional `cwd` is a read-only grant; omit it for an empty owned
directory. Declare scripts, modules, and non-system runtimes explicitly because
arguments grant no paths. Host writes stay in owned scratch and write roots.
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

Strict mode requires `processes` 8–128, `memory_mb` 64–4096, systemd 254 or
newer with `--expand-environment=no`, and cgroup v2. Counts include supervisor
and runtime processes; memory covers the cgroup. Linux uses the existing user
service manager, or the system manager for root. The harness does not invoke
sudo, install services, or change host namespace settings. Missing controllers
or namespace permissions cause refusal.

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

Membership exists before the first child instruction. Detached children cannot
break away, and runner exit terminates job members without cleanup code. Windows
services such as WMI or COM remain outside the job. The request deadline depends
on a responsive harness; it is not an OS timer or hostile-code sandbox. Only
configuration can select this exception.

## Inspect and troubleshoot

`/tools mcp` and authenticated `GET /api/mcp` show declarations/grants without
spawning or claiming readiness. Success audits backend, policy and probe result;
refusals audit policy/error code. Arguments, results and environment secrets
stay out of audit.

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
