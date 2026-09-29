#!/usr/bin/env bash
# Prove default `netconnect status` and `devices`, and a rejected scope, send
# no packets.
#
# The traced commands run under `unshare -rn` (private network namespace,
# loopback only) and `strace -f -e trace=socket,connect,sendto,setsockopt`.
# AF_NETLINK and AF_UNIX are allowed. glibc getifaddrs opens AF_NETLINK.
# Any other socket family, any connect/sendto that is not one of those
# families, and any multicast-membership setsockopt fails the proof.
#
# Missing strace, a failed `unshare -rn`, or a refused ptrace does not skip:
# the script exits non-zero and says which prerequisite failed. On a CI runner
# where AppArmor sets kernel.apparmor_restrict_unprivileged_userns=1, the
# script logs that value and turns the bit off for this job, then retries.
# If the retry still fails, the script exits non-zero.
#
# `--self-check` feeds fixture traces through the same judge and does not
# need strace, unshare, or a binary.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
  printf '%s\n' \
    "usage: scripts/netconnect-syscall-proof.sh --self-check" \
    "       scripts/netconnect-syscall-proof.sh --binary PATH" >&2
  exit 2
}

judge() {
  python3 - "$1" <<'PY'
import re
import sys

ALLOWED = {"UNIX", "LOCAL", "NETLINK"}
NUM_ALLOWED = {"1": "UNIX", "16": "NETLINK"}
FAMILY = re.compile(r"\b(?:AF|PF)_([A-Z0-9]+)\b")
SA_NUM = re.compile(r"sa_family=(\d+)")
MEMBERSHIP = re.compile(
    r"\b(IP_ADD_MEMBERSHIP|IP_ADD_SOURCE_MEMBERSHIP|IPV6_JOIN_GROUP|"
    r"IPV6_ADD_MEMBERSHIP|IPV6_JOIN_ANYCAST|MCAST_JOIN_GROUP|"
    r"MCAST_JOIN_SOURCE_GROUP|PACKET_ADD_MEMBERSHIP)\b"
)
SOCKET = re.compile(r"\bsocket\(([^,\s)]+)")
RET = re.compile(r"=\s*(-?\d+)\b")
FD = re.compile(r"\b(?:connect|sendto)\((\d+)")


def family_name(token):
    if token in NUM_ALLOWED:
        return NUM_ALLOWED[token]
    match = re.fullmatch(r"(?:AF|PF)_(.+)", token)
    if match:
        return match.group(1)
    if token.isdigit():
        return "NUM:" + token
    return token


def line_families(line):
    found = [match.group(1) for match in FAMILY.finditer(line)]
    numeric = SA_NUM.search(line)
    if numeric:
        found.append(NUM_ALLOWED.get(numeric.group(1), "NUM:" + numeric.group(1)))
    return found


violations = []
fds = {}
with open(sys.argv[1], errors="replace") as handle:
    for raw in handle:
        line = raw.rstrip("\n")
        if MEMBERSHIP.search(line):
            violations.append(line)
            continue
        socket = SOCKET.search(line)
        if socket:
            fam = family_name(socket.group(1))
            returned = RET.search(line)
            if returned and int(returned.group(1)) >= 0:
                fds[returned.group(1)] = fam
            if fam not in ALLOWED:
                violations.append(line)
            continue
        families = line_families(line)
        if families:
            if any(name not in ALLOWED for name in families):
                violations.append(line)
            continue
        if re.search(r"\b(connect|sendto)\(", line):
            if "NULL" in line:
                fd = FD.search(line)
                fam = fds.get(fd.group(1)) if fd else None
                if fam not in ALLOWED:
                    violations.append(line)
            else:
                violations.append(line)

if violations:
    for line in violations:
        print(line, file=sys.stderr)
    print("netconnect syscall proof: %d disallowed call(s)" % len(violations), file=sys.stderr)
    sys.exit(1)
PY
}

expect_judge_fails() {
  local trace="$1"
  local label="$2"
  local err
  err="$(mktemp)"
  if judge "$trace" 2>"$err"; then
    printf 'self-check accepted a trace that must fail: %s\n' "$label" >&2
    cat "$err" >&2
    rm -f "$err"
    exit 1
  fi
  if [[ ! -s "$err" ]]; then
    printf 'self-check failure for %s produced no explanation\n' "$label" >&2
    rm -f "$err"
    exit 1
  fi
  rm -f "$err"
}

self_check() {
  if ! command -v python3 >/dev/null 2>&1; then
    printf '%s\n' "python3 is required for the syscall judge; refusing to skip" >&2
    exit 1
  fi
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' \
    'sendto(3, "x", 1, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 1' \
    >"$tmp/netlink.txt"
  judge "$tmp/netlink.txt"

  printf '%s\n' \
    'socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 4' \
    'connect(4, {sa_family=AF_UNIX, sun_path="/tmp/x"}, 10) = 0' \
    'sendto(4, "x", 1, 0, NULL, 0) = 1' \
    >"$tmp/unix.txt"
  judge "$tmp/unix.txt"

  printf '%s\n' 'socket(16, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' >"$tmp/numeric-netlink.txt"
  judge "$tmp/numeric-netlink.txt"

  : >"$tmp/empty.txt"
  judge "$tmp/empty.txt"

  printf '%s\n' '[pid 99] socket(AF_INET, SOCK_DGRAM|SOCK_CLOEXEC, IPPROTO_UDP) = 5' >"$tmp/inet.txt"
  expect_judge_fails "$tmp/inet.txt" "AF_INET"

  printf '%s\n' 'socket(AF_INET6, SOCK_DGRAM, IPPROTO_UDP) = 5' >"$tmp/inet6.txt"
  expect_judge_fails "$tmp/inet6.txt" "AF_INET6"

  printf '%s\n' '999 socket(AF_PACKET, SOCK_RAW, 768) = 4' >"$tmp/packet.txt"
  expect_judge_fails "$tmp/packet.txt" "AF_PACKET"

  printf '%s\n' 'socket(2, SOCK_DGRAM, 0) = 3' >"$tmp/numeric-inet.txt"
  expect_judge_fails "$tmp/numeric-inet.txt" "numeric AF_INET"

  printf '%s\n' 'connect(3, {sa_family=AF_INET, sin_port=htons(80), sin_addr=inet_addr("192.0.2.1")}, 16) = 0' >"$tmp/connect.txt"
  expect_judge_fails "$tmp/connect.txt" "connect AF_INET"

  printf '%s\n' 'sendto(3, "x", 1, 0, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("8.8.8.8")}, 16) = 1' >"$tmp/sendto.txt"
  expect_judge_fails "$tmp/sendto.txt" "sendto AF_INET"

  printf '%s\n' 'setsockopt(3, SOL_IP, IP_ADD_MEMBERSHIP, {imr_multiaddr=inet_addr("224.0.0.1")}, 8) = 0' >"$tmp/membership.txt"
  expect_judge_fails "$tmp/membership.txt" "IP_ADD_MEMBERSHIP"

  printf '%s\n' 'setsockopt(3, SOL_IPV6, IPV6_JOIN_GROUP, {ipv6mr_multiaddr=in6addr_any}, 20) = 0' >"$tmp/join.txt"
  expect_judge_fails "$tmp/join.txt" "IPV6_JOIN_GROUP"

  printf '%s\n' 'setsockopt(3, SOL_IPV6, MCAST_JOIN_GROUP, {group=224.0.0.1}, 16) = 0' >"$tmp/mcast.txt"
  expect_judge_fails "$tmp/mcast.txt" "MCAST_JOIN_GROUP"

  printf '%s\n' \
    'socket(AF_INET, SOCK_DGRAM, 0) = 4' \
    'sendto(4, "x", 1, 0, NULL, 0) = 1' \
    >"$tmp/null-inet.txt"
  expect_judge_fails "$tmp/null-inet.txt" "NULL sendto on AF_INET"

  printf '%s\n' 'sendto(9, "x", 1, 0, NULL, 0) = 1' >"$tmp/null-unknown.txt"
  expect_judge_fails "$tmp/null-unknown.txt" "NULL sendto with unknown fd"

  printf '%s\n' 'connect(3, 0x7ffee, 16) = 0' >"$tmp/opaque-connect.txt"
  expect_judge_fails "$tmp/opaque-connect.txt" "connect without a decoded family"

  trap - RETURN
  rm -rf "$tmp"
  printf '%s\n' "netconnect syscall proof self-check: ok"
}

require_tool() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    printf '%s is required; refusing to skip the syscall proof\n' "$name" >&2
    exit 1
  fi
}

ensure_user_namespace() {
  if unshare -rn true; then
    return 0
  fi
  local bit="unknown"
  local sysctl_path="/proc/sys/kernel/apparmor_restrict_unprivileged_userns"
  if [[ -r "$sysctl_path" ]]; then
    bit="$(cat "$sysctl_path")"
  fi
  printf 'unshare -rn failed (apparmor_restrict_unprivileged_userns=%s)\n' "$bit" >&2
  if [[ "${CI:-}" == "true" && "$bit" == "1" ]] && command -v sudo >/dev/null 2>&1; then
    printf '%s\n' \
      "CI runner blocks unprivileged user namespaces; setting kernel.apparmor_restrict_unprivileged_userns=0 for this job" >&2
    sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
    if unshare -rn true; then
      return 0
    fi
  fi
  printf '%s\n' "unshare -rn is unavailable; refusing to skip the syscall proof" >&2
  exit 1
}

trace_command() {
  local binary="$1"
  local cfg="$2"
  local action="$3"
  local expect="$4"
  local trace="$5"
  local out="$6"
  local err="$7"
  local unshare_bin strace_bin
  unshare_bin="$(command -v unshare)"
  strace_bin="$(command -v strace)"
  set +e
  env -i PATH="$PATH" LANG=C LC_ALL=C HOME="$(dirname "$cfg")" TMPDIR="$(dirname "$cfg")" \
    "$unshare_bin" -rn "$strace_bin" -f -e trace=socket,connect,sendto,setsockopt -o "$trace" -- \
    "$binary" netconnect --config "$cfg" "$action" >"$out" 2>"$err"
  local code=$?
  set -e
  if [[ ! -f "$trace" ]]; then
    printf 'strace wrote no trace for netconnect %s; refusing to skip\n' "$action" >&2
    cat "$err" >&2
    exit 1
  fi
  if grep -E -q 'strace:.*([Oo]peration not permitted|PTRACE_|attach)' "$err"; then
    printf 'strace could not trace netconnect %s; refusing to skip\n' "$action" >&2
    cat "$err" >&2
    exit 1
  fi
  if [[ "$code" -ne "$expect" ]]; then
    printf 'netconnect %s exited %s; expected %s\n' "$action" "$code" "$expect" >&2
    cat "$err" >&2
    exit 1
  fi
  if ! judge "$trace"; then
    printf 'disallowed socket call while tracing netconnect %s\n' "$action" >&2
    exit 1
  fi
}

prove() {
  local binary="$1"
  if [[ ! -x "$binary" ]]; then
    printf 'binary is not executable: %s\n' "$binary" >&2
    exit 1
  fi
  require_tool python3
  require_tool unshare
  require_tool strace
  ensure_user_namespace

  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  cp "$root/assets/config.default.yaml" "$tmp/default.yaml"
  printf '%s\n' "netconnect:" "  enabled: true" "  allowed_cidrs: ['8.8.8.8/32']" >"$tmp/rejected.yaml"

  trace_command "$binary" "$tmp/default.yaml" status 4 "$tmp/default-status.txt" "$tmp/default-status.out" "$tmp/default-status.err"
  trace_command "$binary" "$tmp/default.yaml" devices 4 "$tmp/default-devices.txt" "$tmp/default-devices.out" "$tmp/default-devices.err"
  trace_command "$binary" "$tmp/rejected.yaml" status 3 "$tmp/rejected-status.txt" "$tmp/rejected-status.out" "$tmp/rejected-status.err"
  trace_command "$binary" "$tmp/rejected.yaml" devices 3 "$tmp/rejected-devices.txt" "$tmp/rejected-devices.out" "$tmp/rejected-devices.err"

  printf '%s\n' "netconnect syscall proof: default status and devices, and a rejected scope, opened no disallowed socket"
}

if [[ "${1:-}" == "--self-check" ]]; then
  if [[ "$#" -ne 1 ]]; then
    usage
  fi
  self_check
elif [[ "${1:-}" == "--binary" ]]; then
  if [[ "$#" -ne 2 || -z "${2:-}" ]]; then
    usage
  fi
  prove "$2"
else
  usage
fi
