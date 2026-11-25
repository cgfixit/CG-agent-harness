# Netconnect

Fail-closed LAN observation. `status` does no collection. `devices` and the
LAN panel read local tables without sending packets. Only `diag` dials.
See [invariants](../INVARIANTS.md#netconnect-is-fail-closed-and-lan-scoped).

## Gates

All `netconnect.*` flags ship false. An active tier needs literal
`enabled: true`, its own literal true flag and nonempty accepted scope.
An armed tier may still have no implementation. These settings require a
server restart. The CLI loads config on each invocation.

## `allowed_cidrs`

Up to 64 distinct canonical IPv4 CIDRs with zero host bits and prefixes
/16 through /32. The whole block must be within RFC1918 or IPv4 loopback.
Any invalid entry refuses the list. IPv6, public and link-local addresses
are outside scope. Empty scope refuses every target. Interfaces,
neighbors and gateways must be in scope. A route's entire prefix must fit.

## Other keys

The active probe uses only configured IPv4 `diagnostics_targets` and
`port_allowlist`. Targets must be inside scope. No names or caller-provided
ports. `host_cap`, `rate_limit_per_min`, `connect_timeout_ms` and
`max_dials` bound diagnostics. See
[shipped config](../assets/config.default.yaml) for values and
[src/netconnect](../src/netconnect/README.md) for validation.
Endpoint fields are stored configuration. Passive and diagnostic commands
never use the throughput, automation or MQTT endpoint.

| Bound | Accepted values |
|---|---|
| `diagnostics_targets` | Up to 64 distinct in-scope IPv4 literals |
| `port_allowlist` | Up to 128 distinct integers, 1–65535 |
| `host_cap` | 1–4096 |
| `rate_limit_per_min` | 1–10000 |
| `connect_timeout_ms` | 100–60000 |
| `max_dials` | 1–65536 |

To enable passive reporting, merge the following into the existing YAML.
Keep other first-run settings and tier flags intact.

```yaml
netconnect:
  enabled: true
  allowed_cidrs: [192.168.1.0/24]
```

## CLI

```text
cgagentharness netconnect [--config /path/to/config.yaml] status|devices|diag
```

Outputs JSON on stdout. Warnings and failure messages use stderr.
Exit 0 means completed, 2 collector/OS failure, 3 invalid config and 4
disabled, refused or budget-truncated. `status` may succeed with empty
scope and `active_tiers_refused: true`. `devices` reports `packets_sent: 0`.
Linux reads proc tables and interfaces. macOS uses fixed argv for arp
and netstat plus interface reads. Other platforms report unsupported
collectors. Scope filters all exposed rows.
Labels that survive validation are emitted as `{"value":"…","untrusted":true}`.
Interface and route labels must start with an ASCII alphanumeric and use
only alphanumerics, underscore, period, colon or hyphen. Invalid labels
are omitted. Hostnames are capped and stripped of controls.

## `/net` and its aliases

`/net`, `/netconnect`, `/lan`, `/scan`, `/ports` and `/speed` hit the same
command family. `status` and `devices` open the read-only LAN panel.
`diag` calls the budgeted probe. `ports`, `watch` and `device` refuse.
Misspellings suggest commands without executing.

## Panel

`GET /api/netconnect` shows gates, scope, tier flags and passive rows.
It loads passive data only when enabled. The panel supplies no scan or
control input. Query parameters cannot choose commands or addresses.
`/api/netconnect` and `/api/netconnect/diag` are guarded GETs.
Tool inventory lists diagnostic enablement separately from invocation.
See [routes](API_ROUTES.md#console-and-status).

## Tiers in this build

Only diagnostics is active-capable. `passive_listen` joins no multicast.
Discovery, port scanning, throughput, anomaly detection and device
control have flags without implementations. A configured throughput
endpoint can produce a load warning but is never contacted.

## Diagnostics

`netconnect diag`, `/net diag` and guarded `GET /api/netconnect/diag`
probe the configured target/port pairs. Require enabled diagnostics,
scope, targets and ports. A configuration refusal returns HTTP 409.

One sequential TCP connect per pair, without payload. The dialer rechecks
scope. Results are `open`, `closed`, `timeout`, `unreachable` or
`refused_scope`. Close successful sockets immediately. Host, dial, time
(up to 60 s) and shared server rate limits may stop the run early.
A run stops at the smaller of the per-run dial cap and configured rate
allowance. The rate bucket is shared across server requests.
Partial results use `ok: false`, `NETCONNECT_BUDGET` and
`truncated_reason` in HTTP 200. With a short connect deadline, Windows
can report a closed port as a timeout while the stack retries.
The probe runs in the server and stops on request drop or deadline.
