#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock server for the CG-Agent MLX smoke test.

Purpose: empirically confirm CG-Agent can use a fused-MLX-style OpenAI-compatible
loopback endpoint with NO Rust changes — before you spend time training.

It speaks just enough of the OpenAI API that CG-Agent's resolver, ChatClient, and
LocalProposerClient all succeed against it:
  GET  /v1/models            -> {"data":[{"id":"<model>"}]}
  POST /v1/chat/completions  -> {"choices":[{"finish_reason":"stop","message":{"content":"..."}}]}

Run:  python3 finetune/mock_server.py --port 1235 --model cgagent-fused
Then point BOTH configs at http://127.0.0.1:1235/v1 (see smoke_test.sh).
Standard library only — no install needed.
"""
from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    model = "cgagent-fused"

    def _json(self, code: int, body: dict) -> None:
        payload = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:  # noqa: N802
        path = self.path.split("?")[0].rstrip("/") or "/"
        if path in ("/v1/models", "/models"):
            self._json(200, {"data": [{"id": self.model, "object": "model"}]})
            return
        self._json(404, {"error": {"message": f"unknown path {path}"}})

    def do_POST(self) -> None:  # noqa: N802
        path = self.path.split("?")[0].rstrip("/") or "/"
        if path not in ("/v1/chat/completions", "/chat/completions"):
            self._json(404, {"error": {"message": f"unknown path {path}"}})
            return
        length = int(self.headers.get("Content-Length", "0") or "0")
        try:
            req = json.loads(self.rfile.read(length) or b"{}")
        except Exception:
            req = {}
        model = req.get("model", self.model)
        prompt = " ".join(
            m.get("content", "") for m in req.get("messages", []) if isinstance(m, dict)
        )
        reply = f"[mock:{model}] ok — received {len(prompt)} chars"
        self._json(200, {
            "id": "mock-1",
            "model": model,
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": reply},
            }],
        })

    def log_message(self, fmt: str, *args) -> None:  # quieter logs
        print(f"[mock] {self.address_string()} {fmt % args}", flush=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=1235)
    ap.add_argument("--model", default="cgagent-fused")
    args = ap.parse_args()
    Handler.model = args.model
    srv = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"[mock] OpenAI-compatible mock on http://{args.host}:{args.port}/v1 "
          f"(model={args.model})", flush=True)
    print("[mock] endpoints: GET /v1/models, POST /v1/chat/completions", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        srv.shutdown()


if __name__ == "__main__":
    main()
