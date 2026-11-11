# Netconnect

Index: [README.md](../README.md). Gate contract: [INVARIANTS.md](../INVARIANTS.md#netconnect-is-fail-closed-and-lan-scoped). Console row: [CONSOLE.md](CONSOLE.md). Route: [API_ROUTES.md](API_ROUTES.md).

Netconnect is fail-closed LAN observation. It ships passive: every gate in `assets/config.default.yaml` is the boolean `false`, and `allowed_cidrs` is empty. `devices` and the enabled panel read local tables and send no packets; `status` loads no collector. The only active tier in this build is [`diagnostics`](#diagnostics), a budgeted TCP connect probe of configured targets. Nothing scans ports, measures throughput, or controls a device.

A tier can run only when `netconnect.enabled` and that tier's flag are both the unquoted boolean `true`, and `allowed_cidrs` is a non-empty list the validator accepted. Scope comes from that list, never from local interface addresses. `passive_listen` is a flag only: this build joins no multicast group.

These keys are outside `RELOADABLE`: edit `config.yaml` in the home, then restart `cgagentharness serve` (reload and SIGHUP leave this section unchanged). The CLI reads the file on each invocation.

## Gates

`flag_is_true` arms a gate only for the YAML boolean `true`; quoted `"true"` stays off. A missing `netconnect` section stays closed, and one that is not a mapping fails load with `CONFIG_ERROR`.

`tier_enabled` is the master flag and that tier's flag. `tier_may_run` adds a non-empty scope, so an empty scope leaves every armed tier refused while `status` still exits 0 with `active_tiers_refused: true`.

| Key | Shipped value | What this build does with it |
|---|---|---|
| `enabled` | `false` | Master switch. `false` refuses the CLI (exit 4) and registers no tools. The panel still answers and does not read tables. |
| `passive_listen`, `discovery`, `port_scan`, `anomaly_detection`, `home_automation` | `false` | Flags only; no tool exists and `/net watch`, `/net ports` and `/net device` refuse ([tiers](#tiers-in-this-build)). |
| `diagnostics` | `false` | Arms `netconnect_diag`; see [Diagnostics](#diagnostics). |
| `throughput` | `false` | Flag only. A literal `true`, or any `throughput_endpoint`, warns at load. Passive commands do not use the endpoint. |

`runnable: true` in `status` means `tier_may_run`, not that a tool exists. Only `diagnostics` has one; another armed tier returns `NETCONNECT_REFUSED` with `netconnect tier <name> has no tool in this build`.

## `allowed_cidrs`

The shipped list is empty, so `Scope::check_target` returns `NETCONNECT_REFUSED` for every address, including `127.0.0.1`.

The validator accepts an entry only when all of the following hold. One rejected entry rejects the whole list.

- The list has at most 64 strings. A non-list value is `CONFIG_ERROR`.
- The text is one IPv4 CIDR, `address/prefix`, with surrounding spaces trimmed. IPv6, a colon, a bare address, and a prefix above 32 are rejected (`IPv6 is not permitted` or `not a valid IPv4 CIDR`).
- The prefix is the canonical decimal form (`16`, not `016`) and is from 16 through 32. A wider prefix, including `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/15`, and `127.0.0.0/8`, is `prefix wider than /16`.
- Host bits are zero (`192.168.1.5/24` is rejected).
- The whole block sits inside `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, or `127.0.0.0/8`. Anything else is `outside RFC1918 and loopback`.
- The same canonical CIDR is not repeated.

Accepted examples: `10.1.2.0/24`, `172.31.255.0/24`, `192.168.1.0/24`,
`127.0.0.1/32`. Public, link-local, CGNAT, multicast, IPv6 and overly broad
private blocks are rejected; error text has control characters removed.

A route is kept only when some allowed CIDR covers its entire prefix. A neighbor, interface address or default gateway is kept only when some allowed CIDR contains that address.

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
| `ha_endpoint`, `mqtt_endpoint` | `null` | Stored only. Home automation does not connect. |
| `untrusted_string_max_chars` | `64` | Integer 8–256. Caps device-supplied names. |

An endpoint must be a string or null. Empty text, control characters, more than 256 characters, `@`, `?`, or `#` fail load (no credentials, query or fragment), without echoing the rejected value.

If `throughput` is literal `true`, or `throughput_endpoint` is set, load adds this warning and the CLI prints it on stderr: `netconnect throughput is configured; it is internet egress outside LAN scope and passive commands do not use it`. The warning omits the endpoint, but a successful `status` prints `throughput_endpoint` in its JSON; a refused `status` (`enabled: false`) omits it.

## CLI

```text
cgagentharness netconnect status
cgagentharness netconnect devices
cgagentharness netconnect diag
cgagentharness netconnect --config /path/to/config.yaml status
```

`--config` defaults to `config.yaml` in the harness home (`CGAGENTHARNESS_HOME`, otherwise `~/.CGagentHarness`). JSON goes to stdout. Warnings, and the `message` on a non-zero exit, go to stderr as `netconnect: ...`.

| Exit | When |
|---|---|
| 0 | `enabled` is literal `true` and the action finished. An empty scope can still exit 0. |
| 2 | Collector I/O, an operating system without collectors, a panic (stderr adds `netconnect: internal error`), or any other code. |
| 3 | Config load failed: missing file, invalid YAML, `netconnect` not a mapping, a rejected CIDR, a bad limit, or a bad endpoint. Code `CONFIG_ERROR`. |
| 4 | `enabled` is false: code `NETCONNECT_DISABLED`, message `netconnect.enabled is false`, before any collector or dial runs. `NETCONNECT_REFUSED` and a budget-truncated `diag` (`NETCONNECT_BUDGET`) use this exit too. |

`status` prints gates, scope, limits, endpoints, warnings, `scope_empty`,
`active_tiers_refused`, and one row per tier.

`devices` reads the passive tables, then drops every address outside the scope (all of them when the scope is empty). The body includes `packets_sent` (always 0), `interfaces`, `routes`, `default_gateways`, and `neighbors`. On Linux the readers are `/proc/net/arp`, `/proc/net/route`, and `getifaddrs`. On macOS they are the fixed argv `/usr/sbin/arp -an` and `/usr/sbin/netstat -rn -f inet` (no shell, two-second timeout) plus `getifaddrs`. Other platforms return `NETCONNECT_UNSUPPORTED` and exit 2; `status` needs no reader.

Merge this block into the seeded home `config.yaml`; never replace the file with only these lines, because a file without `auth.enabled` and `tls.enabled` leaves both off and `serve` then listens on HTTP with no account gate. Tiers stay off:

```yaml
netconnect:
  enabled: true
  allowed_cidrs:
    - 192.168.1.0/24
```

Names that survive filtering are JSON objects `{"value":"...","untrusted":true}`. Interface and route labels must start with an ASCII letter or digit and use only ASCII letters, digits, `_`, `.`, `:`, and `-`; a label that fails that rule, or is longer than the smaller of 32 and `untrusted_string_max_chars`, is omitted. Host names are capped and stripped of control characters. All of it is display data.

## `/net` and its aliases

The slash roots are `/net`, `/netconnect`, `/lan`, `/scan`, `/ports`, and `/speed`: one read-only family. An alias is kept only when it does not collide with an existing slash root; the current registry keeps all six. No alias enables a scan.

The parser lowercases the command and one subcommand. Exact subcommands are
`status`, `devices`, `ports`, `diag`, `watch`, and `device`. Anything else
(extra words, a missing subcommand, control characters, a root near-miss) only
suggests and never reads tables: edit distance 1 or a prefix of at least three
characters offers nearby names, distant input offers `/net status`, and fuzzy
matching omits `device`. The console shows `did you mean ...?` with a button
you can edit and send.

| Line (any kept alias) | Console behavior |
|---|---|
| `/net status`, `/net devices` | Opens the LAN panel with `GET /api/netconnect`; it calls neither tool. When `enabled` is true that read loads the three passive sources, which the CLI `status` command does not. |
| `/net diag` | Calls `GET /api/netconnect/diag` and prints one row per target and port. A refusal shows the server message. |
| `/net ports`, `/net watch`, `/net device` | Prints `netconnect refusal: this console does not run active tiers or device control` without loading the panel. |

The library evaluator behind these lines is a test-only executable
specification (`test-support`): it never dials, answers a near-miss with
`NETCONNECT_SUGGEST` (exit 2) and an unbuilt tier or `device` with
`NETCONNECT_REFUSED` (exit 4), and rechecks any supplied address with
`Scope::check_target` first.

## Panel

The console side tab **LAN** (`pane-netconnect`) calls `GET /api/netconnect`. The pane has no button, input, or form. It says network names are untrusted data, not commands, then lists `enabled`, the scope or `empty`, each tier's flag and runnable bit, and each neighbor as address, host, vendor, and `(untrusted)`, or `passive devices: none`.

The route is GET only: other methods return 405. The query string is ignored: `cmd` and `target` are not a command and are not copied into the JSON. The usual Host and CSRF guards apply (`X-CyClaw-CSRF`): a bad Host is 400, a missing CSRF token is 403, and neither collects.

The JSON has `ok`, `enabled`, `scope_empty`, `allowed_cidrs`, `tiers`, `packets_sent` (0), `neighbors`, `interfaces`, `routes`, and `notice`, but not the `default_gateways` the CLI `devices` body carries. Neighbor objects add `hostname`, `mdns_name`, `ssdp`, and `vendor`; the last three are null in this build. A config error is HTTP 400. A collector error is HTTP 502.

When `enabled` is false the handler returns HTTP 200 with empty neighbor, interface, and route arrays and calls no collector. When `enabled` is true it loads each of the three sources once, then applies the scope filter. Sources are fixed at build time; a request cannot swap them.

## Tools

`GET /api/tools` adds netconnect rows from `registered_tools`. Chat does not: its tool list stays `web_search` and `web_fetch`, or empty when web is off.

| Mode | Rows added to `/api/tools` |
|---|---|
| `enabled` false, missing, or quoted `"true"` | None. |
| `enabled` literal `true`, empty or non-empty scope | Two passive rows: `netconnect_status` and `netconnect_devices`. |
| `diagnostics` with `tier_may_run` true | One additional row: `netconnect_diag` (`path: /api/netconnect/diag`). Other tiers add nothing. |

Each passive row is `method: GET`, `path: /api/netconnect`, `kind: netconnect`, `wired: true`, `registered: true`, `invoked: false`, `ready: null`, and `unavailable_reason: read-only; network names are untrusted data`. In the test-only evaluator, either tool returns `NETCONNECT_DISABLED` while `enabled` is false, and an unknown netconnect tool name returns `NETCONNECT_REFUSED`.

## Security and trust boundaries

The [gate contract](../INVARIANTS.md#netconnect-is-fail-closed-and-lan-scoped) owns the shipped gates and scope rules. In addition:

- `devices`, `netconnect_devices`, and the enabled panel load each passive source
  once, then remove IPv6 and out-of-scope rows. Failed guards do not load them.
- `packets_sent` is 0 for everything except `diag`, where it counts connect
  attempts. Linux `getifaddrs` can open a netlink socket inside libc; outside
  `connect.rs` this module opens no packet, raw, UDP, or TCP socket. The macOS
  commands take no caller-supplied argv and change no privilege.
- Device text is never used as argv, a path, or a fetched URL. The browser
  cannot supply a command.
- `diag` rechecks each final IPv4 address with `Scope::check_target` before its
  dial and never resolves a name. Targets and ports come from config only.
- Throughput is internet egress outside LAN scope. Passive commands ignore it.
- These restart-only keys must not contain secrets: a successful `status`
  prints endpoints.

## Troubleshooting

| What you see | What it means |
|---|---|
| Exit 4, `NETCONNECT_DISABLED` | The master boolean is not `true`. Quoted `"true"` looks enabled and is off. |
| Exit 3 naming a CIDR (`outside RFC1918 and loopback`, `prefix wider than /16`, `host bits must be zero`) | That entry failed the validator, so the whole list is rejected. Use a network address with a `/16` or longer prefix inside the private blocks. |
| Exit 3, `diagnostics_targets[...] is outside allowed_cidrs` | Every target must already sit in `allowed_cidrs`. A host name is not a target. |
| Exit 0, `scope_empty: true`, `active_tiers_refused: true` | A tier is armed but the scope is empty, so it is not runnable. |
| Exit 0, `runnable: true`, and a later call says `has no tool in this build` | The tier is armed inside a valid scope. Only `diagnostics` has a tool. |
| `diag` exit 4, `NETCONNECT_BUDGET`, `truncated: true` | A limit stopped the run; `truncated_reason` names it. Raise that key or narrow the targets. |
| `diag` refused, `needs non-empty diagnostics_targets and port_allowlist` | Configure both lists. |
| Exit 2, `NETCONNECT_UNSUPPORTED` | `devices` on an OS other than Linux or macOS. `status` can still run. |
| Exit 2, `NETCONNECT_IO`, `cannot read a local network table` | `/proc/net/arp`, `/proc/net/route`, or the macOS `arp`/`netstat` helper failed or timed out. |
| `did you mean ...?`, or `this console does not run active tiers or device control` | A near-miss, or a subcommand other than `status`, `devices` or `diag`. Nothing ran. |
| Panel HTTP 400, 403, 405 or 502 | Host guard or a `netconnect` load error; CSRF guard; a method other than GET; a collector failure. Only 502 ran a collector. |
| Empty device list while `enabled` is true | The scope is empty or matches nothing on this machine. Public, link-local, CGNAT and IPv6 rows are always omitted. |

## Tiers in this build

Only `diagnostics` is implemented; other tiers keep the flag, scope check and refusal path. Plan: [#339](https://github.com/cgfixit/CG-agent-harness/issues/339).

| Tier | Config flag | Slash | Not implemented yet |
|---|---|---|---|
| Passive listen | `passive_listen` | `/net watch` | Joining a multicast group. |
| Discovery | `discovery` | none | A discovery tool. `devices` remains the passive table read. |
| Port scan | `port_scan` | `/net ports` | Any dial. `port_allowlist` feeds only `diag`. |
| Diagnostics | `diagnostics` | `/net diag` | Implemented: TCP connect only, no ICMP, no reads. |
| Throughput | `throughput` | none | Any measurement. |
| Anomaly detection | `anomaly_detection` | none | A detector. |
| Home automation | `home_automation` | `/net device` always refuses | Device control. `ha_endpoint` and `mqtt_endpoint` are never dialed. |

## Diagnostics

`cgagentharness netconnect diag`, guarded `GET /api/netconnect/diag`, and the console `/net diag` run `netconnect_diag`. It needs `enabled` and `diagnostics` as literal `true`, a non-empty scope, and non-empty `diagnostics_targets` and `port_allowlist`. The caller supplies nothing; the route ignores its query string and refuses with HTTP 409.

Each target and port gets one TCP connect, in order, one at a time: `open`, `closed`, `timeout`, `unreachable`, or `refused_scope` (no packet sent). An open connection is closed at once. A run stops at `host_cap` distinct hosts, at `min(max_dials, rate_limit_per_min)` dials, when the per-minute bucket (shared by every server request) cannot refill before the deadline, or at the deadline: `connect_timeout_ms` per allowed dial, at most 60 seconds. A stopped run returns its partial `results` with `ok: false`, `code: NETCONNECT_BUDGET`, and `truncated_reason`. The route reports this as HTTP 200. On Windows a closed port can take about two seconds to refuse because the stack retries the SYN, so with the shipped `connect_timeout_ms` it reads as `timeout`; raise it to tell the two apart.

The probe runs inside `serve`, not a child process; it stops when the server drops the request, and the deadline bounds it otherwise.
