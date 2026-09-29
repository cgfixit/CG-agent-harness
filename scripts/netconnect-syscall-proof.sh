#!/usr/bin/env bash
# Prove default `netconnect status` and `devices`, and a rejected scope, send
# no packets. Enabled passive status exits 0 and opens no socket of any
# family. Enabled passive devices exits 0 and opens exactly one AF_NETLINK
# socket and no other family. A send is allowed only when its own line
# labels that descriptor NETLINK or UNIX.
#
# The traced commands run under `unshare -rn` (private network namespace,
# loopback only). strace runs inside that namespace:
# `unshare -rn strace -f -yy -e trace=socket,connect,sendto,sendmsg,sendmmsg,setsockopt`.
# `-yy` resolves each descriptor through sock_diag in strace's own namespace,
# so the label is only meaningful when strace is the namespaced command.
# A sendto, sendmsg, or sendmmsg is allowed only when that same line labels
# the descriptor NETLINK or UNIX, for example `3<NETLINK:[ROUTE:...]>` or
# `3<UNIX:[...]>`. TCP, UDP, UDPv6, RAW, a bare `socket:[inode]`, and an
# unlabeled descriptor fail. A bare inode is unknown. The judge keeps no
# descriptor map: a prior socket() line does not make a send legal, and a
# decoded sockaddr or payload family does not either. Payload fields such
# as ifi_family and ifa_family are not the descriptor type.
# socket() lines are still counted by their decoded family. strace -f prints
# a pid column, and it may split a call into "<unfinished ...>" and
# "resumed" lines; the counts reassemble those before counting.
# The judge allows AF_NETLINK and AF_UNIX socket() and connect calls.
# glibc getifaddrs opens AF_NETLINK. Any other socket family, any connect
# that is not one of those families, and any multicast-membership setsockopt
# fails the proof. The enabled-devices count is stricter: any family other
# than AF_NETLINK fails it, including AF_UNIX.
#
# Missing strace, a failed `unshare -rn`, or a refused ptrace does not skip:
# the script exits non-zero and says which prerequisite failed. On a CI runner
# where AppArmor sets kernel.apparmor_restrict_unprivileged_userns=1, the
# script logs that value and turns the bit off for this job, then retries.
# If unshare or strace still cannot run after that retry, the script exits
# non-zero. It does not exit 0 and it does not print a skip.
#
# `--self-check` feeds fixture traces through the same judge and does not
# need strace, unshare, or a binary. It also rejects a proof plan that drops
# the enabled passive `status` and `devices` traces.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
  printf '%s\n' \
    "usage: scripts/netconnect-syscall-proof.sh --self-check" \
    "       scripts/netconnect-syscall-proof.sh --binary PATH" >&2
  exit 2
}

# Modes: judge, netlink-count, socket-count. One parser, so a pid column or
# an unfinished/resumed split cannot drift between the judge and the counts.
read_trace() {
  python3 - "$@" <<'PY'
import re
import sys

mode = sys.argv[1]
path = sys.argv[2]
ALLOWED = {"UNIX", "LOCAL", "NETLINK"}
NUM_ALLOWED = {"1": "UNIX", "16": "NETLINK"}
MEMBERSHIP = re.compile(
    r"\b(IP_ADD_MEMBERSHIP|IP_ADD_SOURCE_MEMBERSHIP|IPV6_JOIN_GROUP|"
    r"IPV6_ADD_MEMBERSHIP|IPV6_JOIN_ANYCAST|MCAST_JOIN_GROUP|"
    r"MCAST_JOIN_SOURCE_GROUP|PACKET_ADD_MEMBERSHIP)\b"
)
PID = re.compile(r"^(?:\[pid\s+(\d+)\]|(\d+))\s+(.*)$")
UNFINISHED = re.compile(r"^(.*)\s*<unfinished \.\.\.>\s*$")
RESUMED = re.compile(r"^<\.\.\.\s+\w+\s+resumed>\s*(.*)$")
SOCKET = re.compile(r"^socket\(([^,\s)]+)")
# End-anchored so nlmsg_len=20 is not a return code. -yy appends <TYPE:[...]>
# with no space: `= 5<NETLINK:[161353]>`.
RET = re.compile(r"=\s*(-?\d+)(?:<[^>]*>)?(?:\s+\S.*)?$")
SEND = re.compile(r"^(sendto|sendmsg|sendmmsg)\((\d+)(?:<([^>]*)>)?")
CONNECT = re.compile(r"^connect\(")
# The sockaddr family only. ifi_family and ifa_family sit inside the payload.
SA = re.compile(r"\bsa_family=(?:(?:AF|PF)_([A-Z0-9]+)|(\d+))")
CALL = re.compile(r"\b(socket|connect|sendto|sendmsg|sendmmsg|setsockopt)\(")


def family_name(token):
    if token in NUM_ALLOWED:
        return NUM_ALLOWED[token]
    match = re.fullmatch(r"(?:AF|PF)_(.+)", token)
    if match:
        return match.group(1)
    if token.isdigit():
        return "NUM:" + token
    return token


def send_label_allowed(label):
    # sock_diag type at the start of the -yy annotation. NETLINK:[...] and
    # UNIX:[...] pass. socket:[inode], TCP, UDP, UDPv6, and RAW do not.
    if not label:
        return False
    return label.startswith("NETLINK") or label.startswith("UNIX")


def sockaddr_families(line):
    found = []
    for match in SA.finditer(line):
        if match.group(1):
            found.append(match.group(1))
        else:
            found.append(NUM_ALLOWED.get(match.group(2), "NUM:" + match.group(2)))
    return found


def strip_pid(line):
    match = PID.match(line)
    if not match:
        return "", line
    return match.group(1) or match.group(2), match.group(3)


def reassemble(path):
    pending = {}
    calls = []
    with open(path, errors="replace") as handle:
        for raw in handle:
            pid, body = strip_pid(raw.rstrip("\n"))
            unfinished = UNFINISHED.match(body)
            if unfinished:
                pending[pid] = unfinished.group(1)
                continue
            resumed = RESUMED.match(body)
            if resumed and pid in pending:
                body = pending.pop(pid) + resumed.group(1)
            calls.append((pid, body))
    for pid, body in pending.items():
        calls.append((pid, body))
    return calls


def returned_fd(line):
    match = RET.search(line)
    if not match:
        return None
    value = int(match.group(1))
    if value < 0:
        return None
    return str(value)


calls = reassemble(path)

if mode == "socket-count":
    print(sum(1 for _pid, body in calls if body.startswith("socket(")))
    sys.exit(0)

if mode == "netlink-count":
    netlink = 0
    other = []
    for _pid, body in calls:
        socket = SOCKET.match(body)
        if not socket:
            continue
        fam = family_name(socket.group(1))
        opened = returned_fd(body) is not None
        if fam != "NETLINK":
            other.append(fam)
            continue
        if opened:
            netlink += 1
    if other:
        print("disallowed socket families: %s" % ",".join(other), file=sys.stderr)
        print(netlink)
        sys.exit(2)
    print(netlink)
    sys.exit(0)

if mode != "judge":
    print("unknown trace mode: %s" % mode, file=sys.stderr)
    sys.exit(2)

violations = []
kinds = []
for _pid, body in calls:
    kind = CALL.search(body)
    kind_name = kind.group(1) if kind else None

    def reject():
        violations.append(body)
        if kind_name and kind_name not in kinds:
            kinds.append(kind_name)

    if MEMBERSHIP.search(body):
        if "setsockopt" not in kinds:
            kinds.append("setsockopt")
        violations.append(body)
        continue
    socket = SOCKET.match(body)
    if socket:
        fam = family_name(socket.group(1))
        if fam not in ALLOWED:
            reject()
        continue
    send = SEND.match(body)
    if send:
        # The -yy label on this line is the only authority. A sockaddr, a
        # payload family, or an earlier socket() does not make a send legal.
        if not send_label_allowed(send.group(3)):
            reject()
        continue
    families = sockaddr_families(body)
    if families:
        if any(name not in ALLOWED for name in families):
            reject()
        continue
    connect = CONNECT.match(body)
    if connect:
        reject()

if violations:
    for line in violations:
        print(line, file=sys.stderr)
    named = ", ".join(kinds) if kinds else "call"
    print(
        "netconnect syscall proof: %d disallowed %s call(s)"
        % (len(violations), named),
        file=sys.stderr,
    )
    sys.exit(1)
PY
}

judge() {
  read_trace judge "$1"
}

expect_judge_fails() {
  local trace="$1"
  local label="$2"
  local need="${3:-}"
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
  if [[ -n "$need" ]] && ! grep -q "$need" "$err"; then
    printf 'self-check failure for %s did not name %s\n' "$label" "$need" >&2
    cat "$err" >&2
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
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3<NETLINK:[ROUTE]>' \
    'sendto(3<NETLINK:[ROUTE]>, "x", 1, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 1' \
    >"$tmp/netlink.txt"
  judge "$tmp/netlink.txt"

  printf '%s\n' \
    'socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 4<UNIX:[123]>' \
    'connect(4<UNIX:[123]>, {sa_family=AF_UNIX, sun_path="/tmp/x"}, 10) = 0' \
    'sendto(4<UNIX:[123]>, "x", 1, 0, NULL, 0) = 1' \
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

  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3<NETLINK:[ROUTE]>' \
    'sendmsg(3<NETLINK:[ROUTE]>, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12, msg_iov=[{iov_base="x", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 1' \
    'sendmmsg(3<UNIX:[123]>, [{msg_hdr={msg_name={sa_family=AF_UNIX, sun_path="/tmp/x"}, msg_namelen=10}}], 1, 0) = 1' \
    >"$tmp/sendmsg-netlink.txt"
  judge "$tmp/sendmsg-netlink.txt"

  printf '%s\n' \
    'sendmsg(7, {msg_name=NULL, msg_namelen=0, msg_iov=[{iov_base="x", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 1' \
    >"$tmp/sendmsg-unknown.txt"
  expect_judge_fails "$tmp/sendmsg-unknown.txt" "sendmsg on an unknown fd"

  printf '%s\n' \
    'sendmsg(8, {msg_name={sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, msg_namelen=12}, 0) = 1' \
    >"$tmp/sendmsg-inherited.txt"
  expect_judge_fails "$tmp/sendmsg-inherited.txt" "sendmsg claiming netlink on an fd this trace did not open"

  printf '%s\n' \
    'socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) = 5' \
    'sendmsg(5, {msg_name={sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("8.8.8.8")}, msg_namelen=16, msg_iov=[{iov_base="x", iov_len=1}], msg_iovlen=1, msg_controllen=0, msg_flags=0}, 0) = 1' \
    >"$tmp/sendmsg-inet.txt"
  expect_judge_fails "$tmp/sendmsg-inet.txt" "sendmsg on AF_INET"

  printf '%s\n' \
    'sendmmsg(9, [{msg_hdr={msg_name=NULL}}], 1, 0) = 1' \
    >"$tmp/sendmmsg-unknown.txt"
  expect_judge_fails "$tmp/sendmmsg-unknown.txt" "sendmmsg on an unknown fd"

  printf '%s\n' \
    'socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) = 6' \
    'sendmmsg(6, [{msg_hdr={msg_name={sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("8.8.8.8")}, msg_namelen=16}}], 1, 0) = 1' \
    >"$tmp/sendmmsg-inet.txt"
  expect_judge_fails "$tmp/sendmmsg-inet.txt" "sendmmsg on AF_INET"

  printf '%s\n' 'connect(3, 0x7ffee, 16) = 0' >"$tmp/opaque-connect.txt"
  expect_judge_fails "$tmp/opaque-connect.txt" "connect without a decoded family"

  # (a) The runner's strace -f column and a netlink-labeled sendto. The two
  # spaces after a 4-digit pid are strace's width-5 pid field. ifi_family and
  # ifa_family are payload fields; the <NETLINK:...> label is what allows the send.
  printf '%s\n' \
    '2733  socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3<NETLINK:[ROUTE]>' \
    '2733  sendto(3<NETLINK:[ROUTE:...]>, [{nlmsg_len=20, nlmsg_type=RTM_GETLINK, nlmsg_flags=NLM_F_REQUEST|NLM_F_DUMP, nlmsg_seq=1790695426, nlmsg_pid=0}, {ifi_family=AF_UNSPEC, ...}], 20, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 20' \
    '2733  sendto(3<NETLINK:[ROUTE:...]>, [{nlmsg_len=20, nlmsg_type=RTM_GETADDR, nlmsg_flags=NLM_F_REQUEST|NLM_F_DUMP, nlmsg_seq=1790695427, nlmsg_pid=0}, {ifa_family=AF_UNSPEC, ...}], 20, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 20' \
    >"$tmp/ci-devices.txt"
  judge "$tmp/ci-devices.txt"
  exactly_one_netlink "$tmp/ci-devices.txt" "ci netlink devices"

  # A decoded sockaddr does not rescue a send with no type label.
  printf '%s\n' \
    '2733  sendto(3, [{nlmsg_len=20, nlmsg_type=RTM_GETLINK, nlmsg_flags=NLM_F_REQUEST|NLM_F_DUMP, nlmsg_seq=1790695426, nlmsg_pid=0}, {ifi_family=AF_UNSPEC, ...}], 20, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 20' \
    >"$tmp/ci-unlabeled.txt"
  expect_judge_fails "$tmp/ci-unlabeled.txt" "pid-prefixed sendto with no type label" "disallowed sendto"

  # (b) TCP and UDP labels are packet paths.
  printf '%s\n' \
    '2733  sendto(3<TCP:[127.0.0.1:1->127.0.0.1:80]>, "x", 1, 0, {sa_family=AF_INET, sin_port=htons(80), sin_addr=inet_addr("127.0.0.1")}, 16) = 1' \
    >"$tmp/tcp-sendto.txt"
  expect_judge_fails "$tmp/tcp-sendto.txt" "sendto on a TCP descriptor" "disallowed sendto"
  printf '%s\n' \
    '2733  sendto(3<UDP:[127.0.0.1:53]>, "x", 1, 0, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("127.0.0.1")}, 16) = 1' \
    >"$tmp/udp-sendto.txt"
  expect_judge_fails "$tmp/udp-sendto.txt" "sendto on a UDP descriptor" "disallowed sendto"
  printf '%s\n' \
    '2733  sendto(3<UDPv6:[::1:53]>, "x", 1, 0, {sa_family=AF_INET6}, 28) = 1' \
    >"$tmp/udpv6-sendto.txt"
  expect_judge_fails "$tmp/udpv6-sendto.txt" "sendto on a UDPv6 descriptor" "disallowed sendto"
  printf '%s\n' \
    '2733  sendto(3<RAW:[255.255.255.255]>, "x", 1, 0, {sa_family=AF_INET}, 16) = 1' \
    >"$tmp/raw-sendto.txt"
  expect_judge_fails "$tmp/raw-sendto.txt" "sendto on a RAW descriptor" "disallowed sendto"

  # (c) A bare inode has no protocol type. sa_family does not make it a pass.
  printf '%s\n' \
    '2733  sendto(3<socket:[12345]>, "x", 1, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 1' \
    >"$tmp/bare-inode.txt"
  expect_judge_fails "$tmp/bare-inode.txt" "sendto on a bare socket inode" "disallowed sendto"

  printf '%s\n' \
    '2733  socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE <unfinished ...>' \
    '2733  <... socket resumed>) = 3<NETLINK:[161353]>' \
    '2733  sendto(3<NETLINK:[161353]>, "x", 1, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 1' \
    >"$tmp/resumed.txt"
  judge "$tmp/resumed.txt"
  exactly_one_netlink "$tmp/resumed.txt" "resumed netlink socket"

  sandbox_setup_failure_exits_nonzero "$tmp"
  enabled_passive_traces_are_required "$tmp"

  trap - RETURN
  rm -rf "$tmp"
  printf '%s\n' "netconnect syscall proof self-check: pid-prefixed netlink-labeled sendto passes"
  printf '%s\n' "netconnect syscall proof self-check: tcp and udp labeled sendto fails"
  printf '%s\n' "netconnect syscall proof self-check: bare socket inode sendto fails"
  printf '%s\n' "netconnect syscall proof self-check: ok"
}

# A failing unshare, including after the AppArmor sysctl retry, and a strace
# that cannot trace, must exit non-zero. The success line is not printed.
sandbox_setup_failure_exits_nonzero() {
  local tmp="$1"
  local script="$root/scripts/netconnect-syscall-proof.sh"
  local bin="$tmp/cgagentharness"
  printf '%s\n' '#!/bin/sh' 'exit 4' >"$bin"
  chmod 755 "$bin"

  local unshare_dir="$tmp/fail-unshare"
  mkdir -p "$unshare_dir"
  printf '%s\n' '#!/bin/sh' 'exit 1' >"$unshare_dir/unshare"
  printf '%s\n' '#!/bin/sh' 'exit 0' >"$unshare_dir/strace"
  local marker="$tmp/sudo-marker"
  cat >"$unshare_dir/sudo" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> $(printf '%q' "$marker")
exit 0
EOF
  chmod 755 "$unshare_dir/unshare" "$unshare_dir/strace" "$unshare_dir/sudo"
  printf '1\n' >"$unshare_dir/bit"
  run_setup_failure \
    "unshare still fails after the AppArmor retry" \
    "$tmp/unshare.out" \
    "$tmp/unshare.err" \
    env CI=true PATH="$unshare_dir:$PATH" NETCONNECT_SYSCALL_APPARMOR_SYSCTL="$unshare_dir/bit" \
    bash "$script" --binary "$bin"
  if ! grep -q 'kernel.apparmor_restrict_unprivileged_userns=0' "$marker"; then
    printf '%s\n' "self-check: AppArmor sysctl retry did not run" >&2
    cat "$marker" >&2
    exit 1
  fi

  local strace_dir="$tmp/fail-strace"
  mkdir -p "$strace_dir"
  cat >"$strace_dir/unshare" <<'EOF'
#!/bin/sh
if [ "$1" = "-rn" ] && [ "$2" = "true" ]; then
  exit 0
fi
if [ "$1" = "-rn" ]; then
  shift
fi
exec "$@"
EOF
  local strace_argv="$tmp/strace-argv"
  cat >"$strace_dir/strace" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> $(printf '%q' "$strace_argv")
printf '%s\n' "strace: attach: Operation not permitted" >&2
exit 1
EOF
  chmod 755 "$strace_dir/unshare" "$strace_dir/strace"
  run_setup_failure \
    "strace cannot trace" \
    "$tmp/strace.out" \
    "$tmp/strace.err" \
    env PATH="$strace_dir:$PATH" \
    bash "$script" --binary "$bin"
  if ! grep -q -- '-yy' "$strace_argv"; then
    printf '%s\n' "self-check: strace inside unshare was not passed -yy" >&2
    cat "$strace_argv" >&2
    exit 1
  fi
  if grep -q 'unshare' "$strace_argv"; then
    printf '%s\n' "self-check: strace wrapped unshare; -yy must run inside the namespace" >&2
    cat "$strace_argv" >&2
    exit 1
  fi
  printf '%s\n' "netconnect syscall proof self-check: strace -yy runs inside unshare"

  printf '%s\n' "netconnect syscall proof self-check: sandbox setup failure exits non-zero"
}

# prove() must trace enabled status and devices. A plan that only has the
# closed-master traces is not a passive proof.
prove_source() {
  awk '/^prove\(\) \{/{found=1} found{print} found && /^}$/{exit}' "$root/scripts/netconnect-syscall-proof.sh"
}

declares_enabled_passive_traces() {
  local body="$1"
  local key
  # shellcheck disable=SC2016 # needles are source text, not expansions
  for key in \
    "enabled: true" \
    "passive_listen: false" \
    "discovery: false" \
    "port_scan: false" \
    "diagnostics: false" \
    "throughput: false" \
    "anomaly_detection: false" \
    "home_automation: false" \
    "127.0.0.0/16" \
    'enabled.yaml" status 0' \
    'enabled.yaml" devices 0' \
    'assert_enabled_status "$tmp/enabled-status.txt" 0' \
    'exactly_one_netlink "$tmp/enabled-devices.txt" "devices"'
  do
    if ! grep -F -q "$key" <<<"$body"; then
      return 1
    fi
  done
  return 0
}

enabled_passive_traces_are_required() {
  local tmp="$1"
  # shellcheck disable=SC2016 # synthetic prove() body, not an expansion
  if declares_enabled_passive_traces 'trace_command "$binary" "$tmp/default.yaml" status 4'; then
    printf '%s\n' "self-check accepted a proof that does not run the enabled passive traces" >&2
    exit 1
  fi
  if ! declares_enabled_passive_traces "$(prove_source)"; then
    printf '%s\n' "self-check: prove() does not run the enabled passive traces; refusing to skip" >&2
    exit 1
  fi

  # Exit 0 and an empty trace is the enabled status path.
  assert_enabled_status "$tmp/empty.txt" 0

  # (a) Passive status opens any socket. A netlink socket is the case the
  # previous exactly-one rule would have accepted.
  printf '%s\n' 'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' >"$tmp/status-netlink.txt"
  expect_status_socket_fails "$tmp/status-netlink.txt" "AF_NETLINK"
  printf '%s\n' 'socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 4' >"$tmp/status-unix.txt"
  expect_status_socket_fails "$tmp/status-unix.txt" "AF_UNIX"

  # (d) Exit 4 and an empty trace is the closed master gate. It must not pass.
  local gated_err
  gated_err="$(mktemp)"
  if assert_enabled_status "$tmp/empty.txt" 4 2>"$gated_err"; then
    printf '%s\n' "self-check accepted a gated status with an empty trace" >&2
    rm -f "$gated_err"
    exit 1
  fi
  if ! grep -q 'exited 4' "$gated_err"; then
    printf '%s\n' "self-check: gated status failure did not report exit 4" >&2
    cat "$gated_err" >&2
    rm -f "$gated_err"
    exit 1
  fi
  rm -f "$gated_err"

  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' \
    'sendto(3, "x", 1, 0, {sa_family=AF_NETLINK, nl_pid=0, nl_groups=00000000}, 12) = 1' \
    >"$tmp/one-netlink.txt"
  exactly_one_netlink "$tmp/one-netlink.txt" "one netlink fixture"

  # (b) Devices opens zero netlink sockets.
  if exactly_one_netlink "$tmp/empty.txt" "devices"; then
    printf '%s\n' "self-check accepted devices with zero AF_NETLINK sockets" >&2
    exit 1
  fi
  # (c) Devices opens two netlink sockets.
  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 4' \
    >"$tmp/two-netlink.txt"
  if exactly_one_netlink "$tmp/two-netlink.txt" "two netlink sockets"; then
    printf '%s\n' "self-check accepted two AF_NETLINK sockets" >&2
    exit 1
  fi
  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' \
    'socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 4' \
    >"$tmp/netlink-unix.txt"
  if exactly_one_netlink "$tmp/netlink-unix.txt" "netlink plus unix"; then
    printf '%s\n' "self-check accepted a non-netlink socket beside AF_NETLINK" >&2
    exit 1
  fi
  printf '%s\n' \
    'socket(AF_NETLINK, SOCK_RAW|SOCK_CLOEXEC, NETLINK_ROUTE) = 3' \
    'socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) = 5' \
    >"$tmp/netlink-inet.txt"
  if exactly_one_netlink "$tmp/netlink-inet.txt" "netlink plus inet"; then
    printf '%s\n' "self-check accepted a non-netlink socket beside AF_NETLINK" >&2
    exit 1
  fi
  printf '%s\n' "netconnect syscall proof self-check: enabled passive traces are required"
  printf '%s\n' "netconnect syscall proof self-check: enabled status opens no sockets and a gated empty trace does not pass"
  printf '%s\n' "netconnect syscall proof self-check: devices opens exactly one AF_NETLINK socket"
}

# Passive status must exit 0. Any socket() call, of any family, fails.
expect_status_socket_fails() {
  local trace="$1"
  local label="$2"
  local err
  err="$(mktemp)"
  if assert_enabled_status "$trace" 0 2>"$err"; then
    printf 'self-check accepted a passive status that opened a socket: %s\n' "$label" >&2
    rm -f "$err"
    exit 1
  fi
  if ! grep -q 'socket' "$err"; then
    printf 'self-check: status socket failure had no socket explanation: %s\n' "$label" >&2
    cat "$err" >&2
    rm -f "$err"
    exit 1
  fi
  rm -f "$err"
}

# Print how many socket() calls the trace contains, successful or not.
# A split unfinished/resumed socket is one call.
socket_call_count() {
  read_trace socket-count "$1"
}

# Enabled status exits 0 and opens nothing. A non-zero exit, including the
# closed master gate's exit 4, fails even when the trace has no socket() call.
assert_enabled_status() {
  local trace="$1"
  local code="$2"
  local calls
  if [[ "$code" -ne 0 ]]; then
    printf 'enabled status exited %s; expected 0\n' "$code" >&2
    return 1
  fi
  calls="$(socket_call_count "$trace")"
  if [[ "$calls" != "0" ]]; then
    printf 'enabled status opened %s socket call(s); expected 0\n' "$calls" >&2
    return 1
  fi
  return 0
}

# Print the number of successful AF_NETLINK socket calls. Exit 2 when any
# other family was opened, including AF_UNIX. A failed socket() of another
# family still counts as that family.
netlink_socket_count() {
  read_trace netlink-count "$1"
}

exactly_one_netlink() {
  local trace="$1"
  local label="$2"
  local err count code
  err="$(mktemp)"
  set +e
  count="$(netlink_socket_count "$trace" 2>"$err")"
  code=$?
  set -e
  if [[ "$code" -ne 0 || "$count" != "1" ]]; then
    printf 'enabled %s opened %s AF_NETLINK sockets; expected exactly 1\n' "$label" "${count:-unknown}" >&2
    cat "$err" >&2
    rm -f "$err"
    return 1
  fi
  rm -f "$err"
  return 0
}

run_setup_failure() {
  local label="$1"
  local out="$2"
  local err="$3"
  shift 3
  local code=0
  set +e
  "$@" >"$out" 2>"$err"
  code=$?
  set -e
  if [[ "$code" -eq 0 ]]; then
    printf 'self-check: sandbox setup failure was a pass: %s\n' "$label" >&2
    cat "$err" >&2
    cat "$out" >&2
    exit 1
  fi
  if ! grep -q 'refusing to skip' "$err"; then
    printf 'self-check: sandbox setup failure had no refusal: %s\n' "$label" >&2
    cat "$err" >&2
    exit 1
  fi
  if grep -E -q 'skipping|proof skipped' "$err" "$out"; then
    printf 'self-check: sandbox setup failure printed a skip: %s\n' "$label" >&2
    exit 1
  fi
  if grep -q 'opened no disallowed socket' "$out" "$err"; then
    printf 'self-check: missing sandbox counted as a proof: %s\n' "$label" >&2
    exit 1
  fi
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
  # The default file is the kernel bit. A test may point this at a fixture.
  # Pointing it elsewhere cannot turn a failed unshare into a pass.
  local sysctl_path="${NETCONNECT_SYSCALL_APPARMOR_SYSCTL:-/proc/sys/kernel/apparmor_restrict_unprivileged_userns}"
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
    "$unshare_bin" -rn "$strace_bin" -f -yy -e trace=socket,connect,sendto,sendmsg,sendmmsg,setsockopt -o "$trace" -- \
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
    printf 'filtered trace for netconnect %s:\n' "$action" >&2
    cat "$trace" >&2
    printf 'netconnect %s exited %s; expected %s\n' "$action" "$code" "$expect" >&2
    cat "$err" >&2
    exit 1
  fi
  local judged kinds
  judged="$(mktemp)"
  if ! judge "$trace" 2>"$judged"; then
    cat "$judged" >&2
    printf 'filtered trace for netconnect %s:\n' "$action" >&2
    cat "$trace" >&2
    kinds="$(sed -n 's/^netconnect syscall proof: [0-9]* disallowed \(.*\) call(s)$/\1/p' "$judged" | tail -1)"
    printf 'disallowed %s while tracing netconnect %s\n' "${kinds:-call}" "$action" >&2
    rm -f "$judged"
    exit 1
  fi
  rm -f "$judged"
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

  cat >"$tmp/enabled.yaml" <<'EOF'
netconnect:
  enabled: true
  passive_listen: false
  discovery: false
  port_scan: false
  diagnostics: false
  throughput: false
  anomaly_detection: false
  home_automation: false
  allowed_cidrs: ['127.0.0.0/16']
EOF
  trace_command "$binary" "$tmp/enabled.yaml" status 0 "$tmp/enabled-status.txt" "$tmp/enabled-status.out" "$tmp/enabled-status.err"
  trace_command "$binary" "$tmp/enabled.yaml" devices 0 "$tmp/enabled-devices.txt" "$tmp/enabled-devices.out" "$tmp/enabled-devices.err"
  local failed=0
  local status_sockets devices_netlink devices_err devices_code
  status_sockets="$(socket_call_count "$tmp/enabled-status.txt")"
  printf 'enabled status socket() calls: %s\n' "$status_sockets"
  devices_err="$(mktemp)"
  set +e
  devices_netlink="$(netlink_socket_count "$tmp/enabled-devices.txt" 2>"$devices_err")"
  devices_code=$?
  set -e
  printf 'enabled devices AF_NETLINK sockets: %s\n' "${devices_netlink:-unknown}"
  if [[ -s "$devices_err" ]]; then
    cat "$devices_err" >&2
  fi
  rm -f "$devices_err"
  if ! assert_enabled_status "$tmp/enabled-status.txt" 0; then
    failed=1
  fi
  if ! exactly_one_netlink "$tmp/enabled-devices.txt" "devices"; then
    failed=1
  fi
  if [[ "$devices_code" -ne 0 || "$devices_netlink" != "1" ]]; then
    failed=1
  fi
  if [[ "$failed" -ne 0 ]]; then
    exit 1
  fi

  printf '%s\n' "netconnect syscall proof: default status and devices and a rejected scope sent no packets; enabled status opened no sockets; enabled devices opened exactly one AF_NETLINK socket"
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
