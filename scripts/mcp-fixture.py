#!/usr/bin/env python3
"""Disposable MCP fixture for stdio and SSE contract tests. Not a product server."""

from __future__ import annotations

import argparse
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys
import threading


TOOLS = (
    {"name": "echo", "description": "Return arguments", "inputSchema": {"type": "object"}},
    {"name": "env_probe", "description": "Return environment variable names", "inputSchema": {"type": "object"}},
    {"name": "crash", "description": "Exit the fixture process", "inputSchema": {"type": "object"}},
    {"name": "read_path", "description": "Read a host path", "inputSchema": {"type": "object"}},
)


def _read_stdio():
    headers = b""
    while b"\r\n\r\n" not in headers:
        chunk = sys.stdin.buffer.read(1)
        if not chunk:
            return None
        headers += chunk
        if len(headers) > 4096:
            raise SystemExit("stdio headers exceeded 4 KiB")
    length = 0
    for line in headers.decode("ascii", "replace").split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":", 1)[1].strip())
    if length < 1 or length > 65536:
        raise SystemExit("stdio frame length refused")
    body = sys.stdin.buffer.read(length)
    if len(body) != length:
        return None
    return json.loads(body)


def _write_stdio(message):
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


def _handle(message):
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "cgah-mcp-fixture", "version": "0"},
            },
        }
    if method == "notifications/initialized":
        return None
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": request_id, "result": {"tools": list(TOOLS)}}
    if method == "tools/call":
        name = (message.get("params") or {}).get("name")
        args = (message.get("params") or {}).get("arguments") or {}
        if name == "crash":
            os._exit(1)
        if name == "env_probe":
            payload = {
                "keys": sorted(os.environ),
                "HOME": os.environ.get("HOME", ""),
                "PATH": os.environ.get("PATH", ""),
            }
        elif name == "echo":
            payload = {"echo": args}
        elif name == "read_path":
            path = str(args.get("path") or "")
            try:
                payload = {"text": open(path, encoding="utf-8").read()}
            except OSError as exc:
                payload = {"error": str(exc)}
        else:
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "unknown tool"},
            }
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"content": [{"type": "text", "text": json.dumps(payload, separators=(",", ":"))}]},
        }
    if request_id is None:
        return None
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {"code": -32601, "message": "unknown method"},
    }


def run_stdio():
    while True:
        message = _read_stdio()
        if message is None:
            return
        reply = _handle(message)
        if reply is not None:
            _write_stdio(reply)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path.split("?", 1)[0] != "/sse":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(b"event: endpoint\ndata: /message\n\n")
        self.wfile.flush()
        self.close_connection = False
        threading.Event().wait()

    def do_POST(self):
        if self.path.split("?", 1)[0] != "/message":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        if length < 1 or length > 65536:
            self.send_error(400)
            return
        message = json.loads(self.rfile.read(length))
        reply = _handle(message)
        body = b"" if reply is None else json.dumps(reply, separators=(",", ":")).encode()
        self.send_response(200 if reply is not None else 204)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)


def run_sse(host, port):
    server = ThreadingHTTPServer((host, port), Handler)
    bound = server.server_address[1]
    print(json.dumps({"port": bound}), flush=True)
    server.serve_forever()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--stdio", action="store_true")
    parser.add_argument("--sse", action="store_true")
    parser.add_argument("--host", default="127.0.0.1")  # DevSkim: ignore DS162092 because this fixture must bind only to loopback.
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    if args.stdio == args.sse:
        raise SystemExit("choose --stdio or --sse")
    if args.stdio:
        run_stdio()
        return
    run_sse(args.host, args.port)


if __name__ == "__main__":
    main()
