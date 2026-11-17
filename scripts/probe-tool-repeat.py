#!/usr/bin/env python3
"""Loopback relay that replays chat turns with and without tool definitions (#941).

Point a disposable test home's local model `base_url` at the address the relay
prints (loopback, `/v1`) and chat as usual. Every request is relayed unchanged
to the upstream local server. After a chat completion that offered
tools, the relay replays that exact payload `--runs` times with its tools and
`--runs` times with the tool fields removed, unstreamed and capped at `--cap`
output tokens, then prints one row per replay. Rows hold HTTP status, finish
reason, completion tokens, tool-call count and the largest count of one
repeated line or sentence. Prompt and reply text are never printed or stored.

Both ends are loopback only. The relay sends nothing beyond the upstream you
name, and it runs only while you run it.
"""

import argparse
import collections
import copy
import http.client
import http.server
import ipaddress
import json
import re
import sys
import threading
import urllib.parse

LOOPBACK_NAMES = {"localhost"}
HOP_HEADERS = {"connection", "keep-alive", "transfer-encoding", "content-length", "host"}
CHAT_PATH = "/v1/chat/completions"


def loopback(host):
    if host in LOOPBACK_NAMES:
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def max_repeat(text):
    """Largest count of one identical non-empty line or sentence."""
    pieces = [p.strip().lower() for p in re.split(r"[\n.!?]+", text or "")]
    counts = collections.Counter(p for p in pieces if p)
    return max(counts.values(), default=0)


def strip_tools(payload):
    out = copy.deepcopy(payload)
    for key in ("tools", "tool_choice", "parallel_tool_calls"):
        out.pop(key, None)
    return out


def summarize(status, body):
    row = {"status": status, "finish": "-", "completion": "-", "calls": 0, "repeat": 0}
    try:
        data = json.loads(body)
        choice = (data.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        finish = choice.get("finish_reason")
        known = {"stop", "length", "tool_calls", "content_filter"}
        row["finish"] = finish if finish in known else ("missing" if finish is None else "other")
        usage = data.get("usage") or {}
        if isinstance(usage.get("completion_tokens"), int):
            row["completion"] = usage["completion_tokens"]
        row["calls"] = len(message.get("tool_calls") or [])
        row["repeat"] = max_repeat(message.get("content") if isinstance(message.get("content"), str) else "")
    except (ValueError, AttributeError, IndexError, TypeError):
        row["finish"] = "unparsed"
    return row


class Relay:
    def __init__(self, upstream, runs, cap, once):
        self.upstream = upstream
        self.runs = runs
        self.cap = cap
        self.once = once
        self.probed = 0
        self.lock = threading.Lock()
        self.done = threading.Event()

    def connection(self, timeout=600):
        cls = http.client.HTTPSConnection if self.upstream.scheme == "https" else http.client.HTTPConnection
        return cls(self.upstream.hostname, self.upstream.port, timeout=timeout)

    def path(self, path):
        base = self.upstream.path.rstrip("/")
        return base + path

    def post(self, path, payload):
        conn = self.connection()
        try:
            conn.request("POST", self.path(path), json.dumps(payload).encode(), {"content-type": "application/json"})
            resp = conn.getresponse()
            return resp.status, resp.read()
        except OSError as err:
            return f"error:{type(err).__name__}", b""
        finally:
            conn.close()

    def probe(self, payload):
        with self.lock:
            self.probed += 1
            number = self.probed
        base = copy.deepcopy(payload)
        base["stream"] = False
        base.pop("stream_options", None)
        base["max_tokens"] = self.cap
        tools = len(base.get("tools") or [])
        rows = []
        for label, body in (("tools", base), ("no-tools", strip_tools(base))):
            for run in range(1, self.runs + 1):
                status, raw = self.post(CHAT_PATH, body)
                rows.append((label, run, summarize(status, raw)))
        out = [f"\nprobe {number}: model={payload.get('model')} tools_offered={tools} cap={self.cap}",
               f"{'variant':<9} {'run':>3} {'status':>6} {'finish':<14} {'compl':>5} {'calls':>5} {'repeat':>6}"]
        for label, run, r in rows:
            out.append(f"{label:<9} {run:>3} {str(r['status']):>6} {r['finish']:<14} {str(r['completion']):>5} "
                       f"{r['calls']:>5} {r['repeat']:>6}")
        for label in ("tools", "no-tools"):
            group = [r for name, _, r in rows if name == label]
            stops = sum(1 for r in group if r["finish"] == "stop" and r["calls"] == 0)
            out.append(f"{label}: {stops}/{len(group)} plain stops, "
                       f"{sum(1 for r in group if r['finish'] == 'length')} hit the cap, "
                       f"{sum(1 for r in group if r['calls'])} returned tool calls")
        print("\n".join(out), flush=True)
        if self.once:
            self.done.set()


def handler(relay):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def relay(self, method):
            length = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(length) if length else None
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP_HEADERS}
            conn = relay.connection()
            try:
                conn.request(method, relay.path(self.path), body, headers)
                resp = conn.getresponse()
            except OSError:
                conn.close()
                self.send_error(502)
                return
            self.send_response(resp.status)
            for key, value in resp.getheaders():
                if key.lower() not in HOP_HEADERS:
                    self.send_header(key, value)
            self.send_header("connection", "close")
            self.end_headers()
            try:
                while True:
                    chunk = resp.read1(65536)
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    self.wfile.flush()
            except OSError:
                pass
            finally:
                conn.close()
                self.close_connection = True
            if method == "POST" and self.path.split("?")[0] == CHAT_PATH and body:
                try:
                    payload = json.loads(body)
                except ValueError:
                    return
                if payload.get("tools"):
                    threading.Thread(target=relay.probe, args=(payload,), daemon=True).start()

        def do_GET(self):
            self.relay("GET")

        def do_POST(self):
            self.relay("POST")

        def do_HEAD(self):
            self.relay("HEAD")

    return Handler


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--listen", type=int, default=18799, help="loopback port for the relay (default 18799)")
    parser.add_argument("--upstream", default="http://127.0.0.1:11434", help="loopback local model server root")  # DevSkim: ignore DS137138,DS162092 because the relay and its upstream are loopback only.
    parser.add_argument("--runs", type=int, default=5, help="replays per variant (default 5)")
    parser.add_argument("--cap", type=int, default=96, help="max_tokens for replays (default 96)")
    parser.add_argument("--once", action="store_true", help="exit after the first probe")
    args = parser.parse_args(argv)
    upstream = urllib.parse.urlsplit(args.upstream)
    if upstream.scheme not in ("http", "https") or not upstream.hostname or not loopback(upstream.hostname):
        parser.error("--upstream must be an http(s) loopback URL")
    if upstream.path.rstrip("/").endswith("/v1"):
        parser.error("--upstream is the server root, without /v1")
    if not (1 <= args.runs <= 50 and 1 <= args.cap <= 8192 and 1024 <= args.listen <= 65535):
        parser.error("--runs 1-50, --cap 1-8192, --listen 1024-65535")
    relay = Relay(upstream, args.runs, args.cap, args.once)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.listen), handler(relay))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    print(f"relay on http://127.0.0.1:{args.listen}/v1 -> {args.upstream}; Ctrl-C to stop", flush=True)  # DevSkim: ignore DS137138,DS162092 because the relay and its upstream are loopback only.
    try:
        relay.done.wait() if args.once else threading.Event().wait()
    except KeyboardInterrupt:
        pass
    server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
