#!/usr/bin/env python3
"""Deterministic loopback stand-in for Ollama (native + /v1 OpenAI routes).

Every request is appended to requests.jsonl so a test can inspect the system
prompt, offered tools and messages the harness actually sent.
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 11434
LOG = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("requests.jsonl")
MODELS = ["qwen3.8:27b-mlx-cg", "fake-alt:7b"]


def tag(name):
    return {"name": name, "model": name, "size": 1_000_000, "digest": "0" * 64,
            "details": {"family": "qwen3", "parameter_size": "27B", "quantization_level": "Q8_0"}}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, value, status=200):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            return {}

    def record(self, body):
        with LOG.open("a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path, "body": body}) + "\n")

    def do_GET(self):
        self.record(None)
        if self.path == "/api/tags":
            return self.reply({"models": [tag(m) for m in MODELS]})
        if self.path == "/api/ps":
            return self.reply({"models": [dict(tag(MODELS[0]), context_length=32768, size_vram=1, expires_at="2099-01-01T00:00:00Z")]})
        if self.path == "/v1/models":
            return self.reply({"object": "list", "data": [{"id": m, "object": "model"} for m in MODELS]})
        if self.path == "/api/version":
            return self.reply({"version": "0.12.0"})
        return self.reply({"error": "not found"}, 404)

    def do_POST(self):
        body = self.body()
        self.record(body)
        if self.path == "/api/show":
            return self.reply({"details": {"family": "qwen3"}, "model_info": {"qwen3.context_length": 262144}, "capabilities": ["completion", "tools"]})
        if self.path == "/api/generate":
            return self.reply({"model": body.get("model"), "done": True, "response": ""})
        if self.path == "/api/pull":
            return self.reply({"status": "success"})
        if self.path.endswith("/chat/completions"):
            return self.chat(body)
        return self.reply({"error": "not found"}, 404)

    def chat(self, body):
        messages = body.get("messages") or []
        last = messages[-1] if messages else {}
        tools = [t.get("function", {}).get("name") for t in body.get("tools") or []]
        text = last.get("content") if isinstance(last.get("content"), str) else ""
        usage = {"prompt_tokens": 100, "completion_tokens": 10}
        # A tool result: summarise it so the turn completes.
        if last.get("role") == "tool":
            return self.reply({"choices": [{"finish_reason": "stop", "message": {"role": "assistant",
                               "content": "tool result seen: " + (text or "")[:160]}}], "usage": usage})
        # Demo: a request to use /web search (or a loop turn) makes one web_search call,
        # as a real local model would; only when that tool was offered this turn.
        asked = "/web search" in (text or "") or (text or "").startswith("Loop turn")
        if asked and "web_search" in tools and not any(m.get("role") == "tool" for m in messages):
            args = json.dumps({"query": "mid-tier HVAC repair near Cumming GA 30041", "count": 5})
            return self.reply({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
                               "tool_calls": [{"id": "call_demo", "type": "function", "function": {"name": "web_search", "arguments": args}}]}}],
                               "usage": usage})
        # "CALL <tool> <json-args>" asks for one tool call when that tool was offered.
        if text.startswith("CALL ") and len(text.split(" ", 2)) == 3:
            _, name, args = text.split(" ", 2)
            if name in tools:
                return self.reply({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
                                   "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": name, "arguments": args}}]}}],
                                   "usage": usage})
        if "SLOWREPLY" in (text or ""):
            time.sleep(4)
        reply = "fake reply to: " + (text or "")[:200] + " | tools=" + ",".join(t for t in tools if t)
        if "GOAL_DONE" in json.dumps(messages[:1]) and "finish" in (text or "").lower():
            reply += "\nGOAL_DONE"
        return self.reply({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": reply}}], "usage": usage})


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
