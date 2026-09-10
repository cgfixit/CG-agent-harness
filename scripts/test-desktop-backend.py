#!/usr/bin/env python3
"""Public desktop sidecar tests. Disposable homes and a loopback model fixture; no external inference."""
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import select
import socket
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request

BIN = Path(os.environ.get("CGAH_TEST_BINARY", "target/release/cgagentharness")).resolve()
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))

class ModelFixture:
    """Exercise the real HTTP client without model downloads or cloud credentials."""
    def __init__(self):
        self.requests = []
        requests = self.requests

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                if self.path != '/v1/chat/completions':
                    self.send_error(404)
                    return
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                requests.append(request)
                body = json.dumps({
                    "model": request["model"],
                    "choices": [{"finish_reason": "stop", "message": {
                        "role": "assistant", "content": "fixture reply"}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 2}
                }).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.base = f'http://127.0.0.1:{self.server.server_port}/v1'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

class Sidecar:
    def __init__(self, home, key=None, protocol=1):
        env = {k: v for k, v in os.environ.items() if k not in (
            "CGAGENTHARNESS_API_KEY", "GROK_API_KEY", "ANTHROPIC_API_KEY", "DEEPAGENT_API_KEY")}
        env["CGAGENTHARNESS_HOME"] = str(home)
        self.process = subprocess.Popen([str(BIN), "desktop"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd="/", env=env)
        self.challenge = secrets.token_hex(32)
        self.buffer = b""
        try:
            self.send({"protocol": protocol, "challenge": self.challenge, "initialize_key": key})
            self.hello = self.read()
        except BaseException:
            self.close()
            raise
        self.base = f'http://127.0.0.1:{self.hello.get("port", 0)}'

    def send(self, frame):
        self.process.stdin.write(json.dumps(frame).encode() + b"\n")
        self.process.stdin.flush()

    def read(self):
        deadline = time.monotonic() + 25
        while b"\n" not in self.buffer:
            if time.monotonic() > deadline:
                raise AssertionError("private control deadline exceeded")
            ready, _, _ = select.select([self.process.stdout], [], [], .1)
            if ready:
                data = os.read(self.process.stdout.fileno(), 16384)
                if not data:
                    raise AssertionError("control channel closed before response")
                self.buffer += data
                assert len(self.buffer) <= 32768
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def request(self, path, headers=None, body=None):
        request = urllib.request.Request(self.base + path, headers=headers or {},
            data=None if body is None else json.dumps(body).encode())
        if body is not None:
            request.add_header("Content-Type", "application/json")
        try:
            response = HTTP.open(request, timeout=4)
        except urllib.error.HTTPError as error:
            response = error
        return response.status, response.read(), response.headers

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()  # exercise parent EOF, not a global kill
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
                raise AssertionError("sidecar did not exit on parent EOF")
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()

class DesktopBoundary(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="cgah desktop fixture ")
        self.home = Path(self.directory.name) / "home"
        self.children = []

    def tearDown(self):
        for child in reversed(self.children):
            child.close()
        self.directory.cleanup()

    def start(self, **kwargs):
        child = Sidecar(self.home, **kwargs)
        self.children.append(child)
        return child

    def test_owned_readiness_is_not_operator_authorization(self):
        key = secrets.token_hex(32)
        seed = self.start(key=key)
        seed.close()
        config = self.home / 'config.yaml'
        config.write_text(config.read_text().replace('api_key_optional: true', 'api_key_optional: false'))
        child = self.start()
        self.assertFalse(child.hello["api_key_optional"])
        self.assertEqual(child.hello["challenge"], child.challenge)
        self.assertEqual(child.hello["pid"], child.process.pid)
        self.assertEqual(child.request('/_desktop/ready')[0], 401)
        self.assertEqual(child.request('/_desktop/ready', {"x-cgah-readiness": "wrong"})[0], 401)
        self.assertEqual(child.request('/_desktop/ready', {"x-cgah-readiness": child.challenge})[0], 200)
        status, html, headers = child.request('/')
        self.assertEqual(status, 200)
        self.assertIn("default-src 'none'", headers["Content-Security-Policy"])
        self.assertNotIn(key.encode(), html)
        csrf = re.search(rb'<meta name="csrf-token" content="([^"]+)"', html).group(1).decode()
        self.assertEqual(child.request('/api/keys', {"Authorization": "Bearer " + child.challenge, "X-CyClaw-CSRF": csrf})[0], 401)
        auth = {"Authorization": "Bearer " + key, "X-CyClaw-CSRF": csrf, "Origin": child.base}
        self.assertEqual(child.request('/api/keys', auth)[0], 200)
        self.assertEqual(child.request('/api/keys', {**auth, "Origin": "https://unapproved.invalid"})[0], 403)
        self.assertEqual(child.request('/api/keys', {**auth, "X-CyClaw-CSRF": "wrong"})[0], 403)
        child.send({"command": "approve"})
        self.assertEqual(child.read()["error"], "unknown_control_command")
        child.send({"command": "status"})
        self.assertEqual(child.read()["active"], 0)
        self.assertEqual((self.home / '.env').stat().st_mode & 0o777, 0o600)

    def test_fresh_home_needs_no_key_or_login(self):
        child = self.start()
        self.assertTrue(child.hello["api_key_optional"])
        self.assertFalse(child.hello["key_configured"])
        html = child.request('/')[1]
        csrf = re.search(rb'<meta name="csrf-token" content="([^"]+)"', html).group(1).decode()
        headers = {"X-CyClaw-CSRF": csrf, "Origin": child.base}
        self.assertEqual(child.request('/api/keys', headers)[0], 200)
        self.assertEqual(child.request('/api/memory/add', headers, {"text":"no credentials needed"})[0], 200)
        self.assertEqual(child.request('/api/keys')[0], 403)
        self.assertEqual(child.request('/api/keys', {**headers, "Origin":"https://unapproved.invalid"})[0], 403)
        self.assertEqual(child.request('/api/keys', {**headers, "X-Forwarded-For":"127.0.0.1"})[0], 401)
        self.assertEqual(child.request('/api/agent/run', headers, {"instruction":"x", "branch":"codex/test", "commit_message":"test", "reason":"fixture"})[0], 409)

    def test_chat_goal_memory_and_model_survive_backend_restart_without_credentials(self):
        model = ModelFixture()
        self.addCleanup(model.close)
        seed = self.start()
        seed.close()
        config = self.home / 'config.yaml'
        original = config.read_text()
        endpoint = 'base_url: "http://127.0.0.1:11434/v1"'
        self.assertIn(endpoint, original)
        config.write_text(original.replace(endpoint, f'base_url: "{model.base}"'))

        def headers_for(child):
            self.assertTrue(child.hello['api_key_optional'])
            self.assertFalse(child.hello['key_configured'])
            status, html, _ = child.request('/')
            self.assertEqual(status, 200)
            csrf = re.search(rb'<meta name="csrf-token" content="([^"]+)"', html).group(1).decode()
            return {"X-CyClaw-CSRF": csrf, "Origin": child.base}

        def call(child, headers, path, body=None, expected=200):
            status, payload, _ = child.request(path, headers, body)
            self.assertEqual(status, expected, payload)
            return json.loads(payload)

        child = self.start()
        headers = headers_for(child)
        session = call(child, headers, '/api/sessions', {"title": "restart fixture"}, expected=201)
        sid = session['session_id']
        path = '/api/sessions/' + sid
        goal = 'Explain the repository checks'
        note = 'Prefer small patches'
        call(child, headers, path + '/goal', {"goal": goal})
        call(child, headers, '/api/memory/add', {"text": note})
        call(child, headers, '/api/memory', {"enabled": True})
        call(child, headers, '/api/model', {"model": "fixture-model"})
        reply = call(child, headers, '/api/chat', {"session_id": sid, "message": "first turn"})
        self.assertEqual(reply['reply'], 'fixture reply')
        self.assertEqual(reply['tally']['total'], 12)
        self.assertEqual(len(model.requests), 1)
        self.assertEqual(model.requests[0]['model'], 'fixture-model')
        self.assertIn(goal, model.requests[0]['messages'][0]['content'])
        self.assertIn(note, model.requests[0]['messages'][0]['content'])
        child.close()  # real parent EOF, then a fresh process against the same home

        restarted = self.start()
        fresh = headers_for(restarted)
        self.assertNotEqual(fresh['X-CyClaw-CSRF'], headers['X-CyClaw-CSRF'])
        stale = {**fresh, 'X-CyClaw-CSRF': headers['X-CyClaw-CSRF']}
        self.assertEqual(restarted.request(path, stale)[0], 403)
        restored = call(restarted, fresh, path)
        self.assertEqual(restored['title'], 'restart fixture')
        self.assertEqual(restored['goal'], goal)
        self.assertEqual(restored['tokens']['exchanges'], 1)
        self.assertEqual([m['content'] for m in restored['messages']], ['first turn', 'fixture reply'])
        self.assertTrue(call(restarted, fresh, '/api/memory')['enabled'])
        self.assertEqual(call(restarted, fresh, '/api/status')['model'], 'fixture-model')
        self.assertEqual(len(model.requests), 1, 'startup must not replay inference')
        reply = call(restarted, fresh, '/api/chat', {
            "session_id": sid, "message": "continue the goal", "loop": True})
        self.assertEqual(reply['tally']['exchanges'], 2)
        self.assertEqual(reply['tally']['total'], 24)
        self.assertEqual(len(model.requests), 2)
        request = model.requests[1]
        self.assertEqual(request['model'], 'fixture-model')
        self.assertIn(goal, request['messages'][0]['content'])
        self.assertIn(note, request['messages'][0]['content'])
        self.assertEqual([m['content'] for m in request['messages'][1:]],
            ['first turn', 'fixture reply', 'continue the goal'])
        self.assertEqual(call(restarted, fresh, path)['message_count'], 4)

    def test_home_lock_prevents_second_writer_and_releases_after_eof(self):
        first = self.start()
        second = self.start()
        self.assertEqual(second.hello["error"], "home_unavailable")
        first.close()
        third = self.start()
        self.assertIn("port", third.hello)

    def test_unrelated_listener_is_never_adopted(self):
        with contextlib.closing(socket.socket()) as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            port = listener.getsockname()[1]
            self.home.mkdir()
            (self.home / 'harness.json').write_text(json.dumps({"port": port}))
            child = self.start()
            self.assertNotEqual(port, child.hello["port"])
            child.close()
            self.assertEqual(listener.getsockname()[1], port)

    def test_private_dotenv_is_data_and_existing_values_are_preserved(self):
        self.home.mkdir()
        marker = Path(self.directory.name) / 'must-not-exist'
        key = f'$(touch {marker})'
        env_file = self.home / '.env'
        original = f"# unrelated comment\nUNRELATED=value\nexport CGAGENTHARNESS_API_KEY='{key}'\n"
        env_file.write_text(original)
        env_file.chmod(0o600)
        child = self.start()
        self.assertTrue(child.hello['key_configured'])
        self.assertFalse(marker.exists())
        child.close()
        refused = self.start(key='replacement')
        self.assertEqual(refused.hello['error'], 'key_exists')
        self.assertEqual(env_file.read_text(), original)

    def test_unsafe_credentials_and_protocol_fail_closed(self):
        self.home.mkdir()
        env_file = self.home / '.env'
        env_file.write_text('CGAGENTHARNESS_API_KEY=private-fixture\n')
        env_file.chmod(0o644)
        child = self.start()
        self.assertEqual(child.hello['error'], 'credentials_unreadable')
        child.close()
        wrong = self.start(protocol=999)
        self.assertEqual(wrong.hello['error'], 'startup_failed')

    def test_retained_runs_reconcile_only_after_worker_lease_releases(self):
        key = secrets.token_hex(24)
        child = self.start(key=key)
        child.close()
        config = self.home / 'config.yaml'
        config.write_text(config.read_text().replace('agentic:\n  enabled: false', 'agentic:\n  enabled: true'))
        runs = self.home / 'data/agentic/workspaces/runs'
        runs.mkdir(parents=True, exist_ok=True)
        run_id = 'a' * 32
        (runs / (run_id + '.json')).write_text(json.dumps({'run_id':run_id,'repo':'fixture','dest':str(self.home / 'missing-clone'),'status':'running'}))
        lease = subprocess.Popen(['python3','-c', 'import fcntl,sys; f=open(sys.argv[1],"w"); fcntl.flock(f,fcntl.LOCK_EX); print("ready",flush=True); sys.stdin.read()', str(runs / (run_id + '.lease'))], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        try:
            self.assertEqual(lease.stdout.readline(), b'ready\n')
            child = self.start()
            html = child.request('/')[1]
            csrf = re.search(rb'<meta name="csrf-token" content="([^"]+)"', html).group(1).decode()
            auth = {"Authorization":"Bearer " + key,"X-CyClaw-CSRF":csrf,"Origin":child.base}
            self.assertEqual(child.request('/api/agent/runs')[0], 403)  # CSRF still required
            def listing():
                status, body, _ = child.request('/api/agent/runs', auth)
                self.assertEqual(status, 200)
                return json.loads(body)['parsed']['runs'][0]['status']
            self.assertEqual(listing(), 'running')
            # Closing the test worker's parent channel ends real OS ownership.
            lease.stdin.close(); lease.wait(timeout=3)
            self.assertEqual(listing(), 'interrupted')
            self.assertEqual(json.loads((runs / (run_id + '.json')).read_text())['status'], 'interrupted')
        finally:
            if lease.poll() is None: lease.kill(); lease.wait()
            lease.stdout.close()
            if not lease.stdin.closed: lease.stdin.close()

if __name__ == '__main__':
    unittest.main(verbosity=2)
