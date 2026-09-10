#!/usr/bin/env python3
"""Public desktop sidecar tests. Disposable homes, no cloud/model requests."""
import contextlib
import json
import os
from pathlib import Path
import re
import secrets
import select
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request

BIN = Path(os.environ.get("CGAH_TEST_BINARY", "target/release/cgagentharness")).resolve()
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))

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
        child = self.start(key=key)
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
            self.assertEqual(child.request('/api/agent/runs')[0], 401)
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
