# Netconnect

Index: [README.md](../README.md). Gate contract: [INVARIANTS.md](../INVARIANTS.md#netconnect-is-fail-closed-and-lan-scoped). Console row: [CONSOLE.md](CONSOLE.md). Route: [API_ROUTES.md](API_ROUTES.md).

Netconnect is fail-closed LAN observation. It ships passive. Every gate in `assets/config.default.yaml` is the boolean `false`, and `allowed_cidrs` is an empty list. `devices` and the enabled panel read local tables and set `packets_sent` to 0. `status` does not load a collector and does not print `packets_sent`. The only active tier in this build is `diagnostics`: a budgeted TCP connect probe of configured targets (see [Diagnostics](#diagnostics)). It does not scan ports, measure throughput, or control a device.

A tier can run only when `netconnect.enabled` and that tier's flag are both the unquoted boolean `true`, and `allowed_cidrs` is a non-empty list the validator accepted. Scope comes from that list. It is never copied from local interface addresses. `passive_listen` is a flag only: this mode does not join a multicast group.

These keys are outside the 23-key reload allowlist. Edit `config.yaml` under `~/.CGagentHarness` (`CGAGENTHARNESS_HOME`), then restart `cgagentharness serve`. The CLI reads the file on each invocation. `POST /api/config/reload` and SIGHUP leave this section unchanged.

## Gates

`flag_is_true` arms a gate only for the YAML boolean `true`. The quoted string `"true"` stays off. A missing `netconnect` section stays closed. If `netconnect` is present and is not a mapping, load fails with `CONFIG_ERROR`.

`tier_enabled` is the master flag and that tier's flag. Scope is not part of it. `tier_may_run` is `tier_enabled` plus a non-empty scope. An empty scope leaves every armed tier refused. `status` can still exit 0 and set `active_tiers_refused` to true.

| Key | Shipped value | What this build does with it |
|---|---|---|
| `enabled` | `false` | Master switch. `false` refuses the CLI (exit 4) and registers no tools. The panel still answers and does not read tables. |
| `passive_listen` | `false` | Flag only. `/net watch` maps here and does not join multicast. |
| `discovery` | `false` | Flag only. No discovery tool is registered. |
| `port_scan` | `false` | Flag only. `/net ports` maps here and does not scan. |
| `diagnostics` | `false` | Arms `netconnect_diag`; see [Diagnostics](#diagnostics). |
| `throughput` | `false` | Flag only. A literal `true`, or any `throughput_endpoint`, warns at load. Passive commands do not use the endpoint. |
| `anomaly_detection` | `false` | Flag only. No anomaly tool is registered. |
| `home_automation` | `false` | Flag only. `/net device` always refuses, including when this flag and the scope are set. |

`runnable: true` in `status` means `tier_may_run` is true. It does not mean a tier tool exists. Only `diagnostics` has a tool in this build. Another armed tier still returns `NETCONNECT_REFUSED` with `netconnect tier <name> has no tool in this build`.

## `allowed_cidrs`

The shipped list is empty. An empty scope contains no address. `Scope::check_target` then returns `NETCONNECT_REFUSED` for every address, including `127.0.0.1`.

The validator accepts an entry only when all of the following hold. One rejected entry rejects the whole list.

- The list has at most 64 strings. A non-list value is `CONFIG_ERROR`.
- The text is one IPv4 CIDR, `address/prefix`. Surrounding spaces are trimmed. IPv6, a colon, a bare address, and a prefix above 32 are rejected (`IPv6 is not permitted` or `not a valid IPv4 CIDR`).
- The prefix is the canonical decimal form (`16`, not `016`) and is from 16 through 32. A wider prefix, including `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/15`, and `127.0.0.0/8`, is `prefix wider than /16`.
- Host bits are zero (`192.168.1.5/24` is rejected).
- The whole block sits inside one of these containers: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, or `127.0.0.0/8`. Anything else is `outside RFC1918 and loopback`.
- The same canonical CIDR is not repeated.

Accepted examples include `10.0.0.0/16`, `10.1.2.0/24`,
`172.31.255.0/24`, `192.168.1.0/24`, `127.0.0.0/16`, and `127.0.0.1/32`.
Public, link-local, CGNAT, multicast, reserved, adjacent non-private, IPv6, and
overly broad private blocks are rejected. Control characters are removed from
error text.

A route is kept only when some allowed CIDR covers that route's entire prefix. A neighbor or interface address is kept only when some allowed CIDR contains that address. A default gateway is kept only when the gateway address itself is inside the scope.

## Other keys

These values are checked at load and printed by `status`. `diag` enforces `host_cap`, `rate_limit_per_min`, `connect_timeout_ms`, and `max_dials`; nothing reads `read_timeout_ms` yet.

| Key | Shipped value | Load rule |
|---|---|---|
| `port_allowlist` | `[]` | Up to 128 integers, each 1–65535, no duplicates. |
| `host_cap` | `32` | Integer 1–4096. |
| `rate_limit_per_min` | `60` | Integer 1–10000. |
| `connect_timeout_ms` | `1000` | Integer 100–60000. |
| `read_timeout_ms` | `2000` | Integer 100–60000. |
| `max_dials` | `256` | Integer 1–65536. Per-run dial ceiling; a run also stops at `rate_limit_per_min`. |
| `diagnostics_targets` | `[]` | Up to 64 IPv4 addresses, no duplicates, each inside `allowed_cidrs`. Host names and IPv6 are rejected. A target with an empty scope fails load. |
| `throughput_endpoint` | `null` | String or null. See endpoints below. |
| `ha_endpoint` | `null` | Stored only. Home automation does not connect. |
| `mqtt_endpoint` | `null` | Stored only. Home automation does not connect. |
| `untrusted_string_max_chars` | `64` | Integer 8–256. Caps device-supplied names. |

An endpoint must be a string or null. Empty text, control characters, more than 256 characters, `@`, `?`, or `#` fail load. The error says the endpoint must not carry credentials, a query, or a fragment, and it does not echo the rejected secret.

If `throughput` is literal `true`, or `throughput_endpoint` is set, load adds this warning and the CLI prints it on stderr: `netconnect throughput is configured; it is internet egress outside LAN scope and passive commands do not use it`. The warning text does not include the endpoint. Quoted `"true"` does not warn. A successful `status` still prints `throughput_endpoint` in the JSON when the master gate is on. A refused `status` (`enabled: false`) omits the endpoint from the JSON body.

## CLI

The binary exposes three subcommands:

```text
cgagentharness netconnect status
cgagentharness netconnect devices
cgagentharness netconnect diag
cgagentharness netconnect --config /path/to/config.yaml status
```

`--config` defaults to `config.yaml` in the harness home (`CGAGENTHARNESS_HOME`, otherwise `~/.CGagentHarness`). A slash line is not a CLI argument.

JSON goes to stdout. Warnings, and the `message` on a non-zero exit, go to stderr as `netconnect: ...`.

| Exit | When |
|---|---|
| 0 | `enabled` is literal `true` and the action finished. An empty scope can still exit 0. |
| 2 | Collector I/O, an operating system without collectors, a panic, or any other code. Stderr includes `netconnect: internal error` after a panic. |
| 3 | Config load failed: missing file, invalid YAML, `netconnect` not a mapping, a rejected CIDR, a bad limit, or a bad endpoint. Code `CONFIG_ERROR`. |
| 4 | `enabled` is false. Code `NETCONNECT_DISABLED`, message `netconnect.enabled is false`. Every action refuses before any collector or dial runs. `NETCONNECT_REFUSED` and a budget-truncated `diag` (`NETCONNECT_BUDGET`) use this exit too. |

`status` prints gates, scope, limits, endpoints, warnings, `scope_empty`,
`active_tiers_refused`, and one row per tier. It does not load a collector. An
enabled tier with no CIDRs is `tier_enabled: true`, `runnable: false`; status
still exits 0.

`devices` reads the passive tables, then drops every address outside the scope. The body includes `packets_sent` (always 0), `interfaces`, `routes`, `default_gateways`, and `neighbors`. An empty scope still reads the tables and then emits no addresses. On Linux the readers are `/proc/net/arp`, `/proc/net/route`, and `getifaddrs`. On macOS they are the fixed argv `/usr/sbin/arp -an` and `/usr/sbin/netstat -rn -f inet` (no shell, two-second timeout) plus `getifaddrs`. Other platforms return `NETCONNECT_UNSUPPORTED` and exit 2. `status` does not need those readers.

Merge this block into the seeded home `config.yaml`. Do not replace that file with only these lines. A file that omits `auth.enabled` and `tls.enabled` leaves both off, and `serve` then listens on HTTP with no account gate. Tiers stay off:

```yaml
netconnect:
  enabled: true
  allowed_cidrs:
    - 192.168.1.0/24
```

Restart `serve` after saving that file. The CLI picks the file up on the next invocation.

Names that survive filtering are JSON objects `{"value":"...","untrusted":true}`. Interface and route labels must start with an ASCII letter or digit and use only ASCII letters, digits, `_`, `.`, `:`, and `-`. The label is dropped when it is longer than the smaller of 32 and `untrusted_string_max_chars`. A label that fails that rule is omitted. Host names are capped, control characters are removed, and the result is marked untrusted. They are display data. They are not argv, a command, or a path.

## `/net` and its aliases

The slash roots are `/net`, `/netconnect`, `/lan`, `/scan`, `/ports`, and `/speed`. Each alias is kept only when it does not collide with a slash root that already existed. The current registry keeps all six. A colliding alias is dropped. `/netconnect` is an exact alias of `/net`. It does not enable a scan. `/scan`, `/ports`, and `/speed` are the same read-only family.

The parser lowercases the command and one subcommand. Exact subcommands are
`status`, `devices`, `ports`, `diag`, `watch`, and `device`. Other tokens only
suggest a command. Edit distance 1 or a prefix of at least three characters can
offer nearby names; distant input offers `/net status`. Fuzzy matching omits
`device`. Extra words, missing subcommands, control characters, and root
near-misses only suggest and never read tables.

Suggestions are not execution. The console shows `did you mean ...?` and inserts a button you can edit and send. The library evaluator sets `executed` false and exits 2 with code `NETCONNECT_SUGGEST`.

Exact lines do parse as dispatch:

| Line | Library evaluator | Console (`runSlash`) |
|---|---|---|
| `/net status` and the same subcommand on any kept alias | `netconnect_status`. No collector. Exit 0 when `enabled` is true. | Opens the LAN panel with `GET /api/netconnect`. It does not call the status tool. |
| `/net devices` and aliases | Passive tables through `netconnect_devices`. Exit 0 when `enabled` is true. | Same panel GET. It does not call the devices tool by name. |
| `/net diag` | `invoke_tier` for `diagnostics`. Never dials: refuses, and when the tier may run names the route and CLI that do. Exit 4. | Calls `GET /api/netconnect/diag` and prints one row per target and port. A refusal shows the server message. |
| `/net ports`, `/net watch` | `invoke_tier` for `port_scan` or `passive_listen`. Refuses; no tool exists. Exit 4. | Prints `netconnect refusal: this console does not run active tiers or device control` without loading the panel. |
| `/net device` | Always `NETCONNECT_REFUSED`, exit 4. No collector. | Same refusal, no panel load. |

So a near-miss never executes. An exact `status` or `devices` in the console refreshes the read-only panel. It does not start a tier. When `enabled` is true, that panel read does load the three passive sources, including for `/net status`. The CLI `status` command and the `netconnect_status` tool do not.

The library evaluator's `ports`, `diag`, and `watch` check the matching tier and do not connect. Supplying an address to `invoke_tier` rechecks it with `Scope::check_target` first. An address outside the scope is `NETCONNECT_REFUSED` (`address <ip> is outside netconnect scope`). A host name is not a target.

## Panel

The console side tab **LAN** (`pane-netconnect`) calls `GET /api/netconnect`. The pane has no button, input, or form. The page says network names are untrusted data, not commands, then lists `enabled`, the scope or `empty`, each tier's flag and runnable bit, and each neighbor as address, host, vendor, and `(untrusted)`. An empty neighbor list prints `passive devices: none`.

The route is GET only. POST, PUT, PATCH, and DELETE return 405 and do not collect. The query string is ignored: `cmd` and `target` are not a command and are not copied into the JSON. The request uses the same Host and CSRF guards as other guarded reads (`X-CyClaw-CSRF`). A bad Host is 400 and does not collect. A missing CSRF token is 403 and does not collect.

The JSON has `ok`, `enabled`, `scope_empty`, `allowed_cidrs`, `tiers`, `packets_sent` (0), `neighbors`, `interfaces`, `routes`, and `notice`. It does not include `default_gateways`; the CLI `devices` body does. Neighbor objects add `hostname`, `mdns_name`, `ssdp`, and `vendor`. The last three are null in this build. A config error is HTTP 400. A collector error is HTTP 502.

When `enabled` is false the handler returns HTTP 200 with empty neighbor, interface, and route arrays and does not call a collector. When `enabled` is true it loads each of the three sources once, then applies the scope filter. Sources are fixed when the server is built. The request cannot swap them.

## Tools

`GET /api/tools` adds netconnect rows from `registered_tools`. Chat does not. The chat tool list stays `web_search` and `web_fetch` when web is on, and empty when web is off. The prompt does not offer `netconnect_status` or `netconnect_devices`.

| Mode | Rows added to `/api/tools` |
|---|---|
| `enabled` false, missing, or quoted `"true"` | None. |
| `enabled` literal `true`, empty or non-empty scope | Two passive rows: `netconnect_status` and `netconnect_devices`. |
| `diagnostics` with `tier_may_run` true | One additional row: `netconnect_diag` (`path: /api/netconnect/diag`). Other tiers add nothing. |

Each passive row is `method: GET`, `path: /api/netconnect`, `kind: netconnect`, `wired: true`, `registered: true`, `invoked: false`, `ready: null`, and `unavailable_reason: read-only; network names are untrusted data`. `netconnect_status` describes gates and scope and sends no packets. `netconnect_devices` describes in-scope neighbors and says network names are untrusted data, not arguments. Calling either tool while `enabled` is false returns `NETCONNECT_DISABLED`, not an empty success. An unknown netconnect tool name returns `NETCONNECT_REFUSED`.

## Security and trust boundaries

The [gate contract](../INVARIANTS.md#netconnect-is-fail-closed-and-lan-scoped) owns the shipped gates and scope rules. In addition:

- `devices`, `netconnect_devices`, and the enabled panel load each passive source
  once, then remove IPv6 and out-of-scope rows. Failed guards do not load them.
- `packets_sent` is 0 for everything except `diag`, where it counts connect
  attempts. Linux `getifaddrs` can open a netlink socket inside libc; outside
  `connect.rs` this module opens no packet, raw, UDP, or TCP socket. macOS uses two fixed
  commands plus `getifaddrs`, without caller-supplied argv or privilege changes.
- Device text is capped, stripped of controls, marked untrusted, and never used
  as argv, a path, or a fetched URL. The browser cannot supply a command.
- `diag` rechecks each final IPv4 address with `Scope::check_target` before its
  dial and never resolves a name. Targets and ports come from config only.
- Throughput is internet egress outside LAN scope. Passive commands ignore it.
  Endpoint errors omit the rejected value, but successful status prints it.
- These restart-only keys must not contain secrets.

## Troubleshooting

| What you see | What it means |
|---|---|
| Exit 4, `NETCONNECT_DISABLED`, `netconnect.enabled is false` | The master boolean is not `true`. Quoted `"true"` looks enabled and is off. `status` and `devices` both stop here. |
| Exit 3, `outside RFC1918 and loopback` or `prefix wider than /16` | That CIDR failed the validator. The rest of the list is not applied. `/8` and `/12` private aggregates are too wide; use `/16` or longer inside those blocks. |
| Exit 3, `host bits must be zero` or `not a valid IPv4 CIDR` | Use the network address (`192.168.1.0/24`) and a prefix from 16 to 32. IPv6 is rejected. |
| Exit 3, `diagnostics_targets[...] is outside allowed_cidrs` | Every target must already sit in `allowed_cidrs`. A host name is not a target. |
| Exit 3, cannot read config file | `--config` or the home `config.yaml` is missing or unreadable. |
| Exit 0, `scope_empty: true`, `active_tiers_refused: true` | The master gate and a tier flag are on, and the scope is empty. The tier is not runnable. `status` still succeeds. |
| Exit 0, `runnable: true`, and a later call says `has no tool in this build` | The tier is armed inside a valid scope. Only `diagnostics` has a tool. |
| `diag` exit 4, `NETCONNECT_BUDGET`, `truncated: true` | A limit stopped the run; `truncated_reason` names it. Raise that key or narrow the targets. |
| `diag` refused, `needs non-empty diagnostics_targets and port_allowlist` | Configure both lists. |
| Exit 2, `NETCONNECT_UNSUPPORTED` | `devices` on an OS other than Linux or macOS. `status` can still run. |
| Exit 2, `NETCONNECT_IO`, `cannot read a local network table` | `/proc/net/arp`, `/proc/net/route`, or the macOS `arp`/`netstat` helper failed or timed out. |
| stderr `netconnect throughput is configured...` | The throughput flag or endpoint is set. Passive commands do not call it. The warning is not a scan. |
| `did you mean /net status or /net devices?` | The line was a near-miss. Nothing ran. Send an exact line. |
| Console text `this console does not run active tiers or device control` | The subcommand was not `status`, `devices`, or `diag`. The panel did not refresh. No tier ran. |
| Panel HTTP 400 | Host guard, or the `netconnect` section failed to load (a bad CIDR, limit, or endpoint). Collectors did not run. |
| Panel HTTP 403 | CSRF guard. Collectors did not run. |
| Panel HTTP 405 | The method was not GET. |
| Panel HTTP 502 | A collector failed after the guards passed. |
| Empty device list while `enabled` is true | The scope matches nothing on this machine, or the scope is empty (tables are read, then every row is dropped). Public, link-local, CGNAT, and IPv6 rows are omitted. |
| Edits to `config.yaml` seem ignored by the console | This section is restart-only. Restart `serve`. The CLI reads the file each time. |
| `/api/tools` has no netconnect rows | `enabled` is not the boolean `true`. Arming a tier without the master flag adds no row. |

## Tiers in this build

Only `diagnostics` is implemented. Other tiers keep the flag, scope check, and refusal path until a later change fills their `tools_for_tier` arm. Plan: [#339](https://github.com/cgfixit/CG-agent-harness/issues/339).

| Tier | Config flag | Slash | Not implemented yet |
|---|---|---|---|
| Passive listen | `passive_listen` | `/net watch` | Joining a multicast group. The flag does not listen. |
| Discovery | `discovery` | none | A discovery tool. `devices` remains the passive table read. |
| Port scan | `port_scan` | `/net ports` | Any dial. `port_allowlist` feeds only `diag`. |
| Diagnostics | `diagnostics` | `/net diag` | Implemented: TCP connect only, no ICMP, no reads. |
| Throughput | `throughput` | none | Any measurement. The endpoint is internet egress and triggers a warning. |
| Anomaly detection | `anomaly_detection` | none | A detector. |
| Home automation | `home_automation` | `/net device` always refuses | Device control. `ha_endpoint` and `mqtt_endpoint` are stored and are not dialed. |

## Diagnostics

`cgagentharness netconnect diag`, guarded `GET /api/netconnect/diag`, and the console `/net diag` run `netconnect_diag`. It needs `enabled` and `diagnostics` as literal `true`, a non-empty scope, and non-empty `diagnostics_targets` and `port_allowlist`. The caller supplies nothing; the route ignores its query string and refuses with HTTP 409.

Each target and port gets one TCP connect, in order, one at a time: `open`, `closed`, `timeout`, `unreachable`, or `refused_scope` (no packet sent). An open connection is closed at once. A run stops at `host_cap` distinct hosts, at `min(max_dials, rate_limit_per_min)` dials, when the per-minute bucket (shared by every server request) cannot refill before the deadline, or at the deadline: `connect_timeout_ms` per allowed dial, at most 60 seconds. A stopped run returns its partial `results` with `ok: false`, `code: NETCONNECT_BUDGET`, and `truncated_reason`. The route reports this as HTTP 200. On Windows a closed port can take about two seconds to refuse because the stack retries the SYN, so with the shipped `connect_timeout_ms` it reads as `timeout`; raise it to tell the two apart.

The probe runs inside `serve`, not a child process; it stops when the server drops the request, and the deadline bounds it otherwise.
