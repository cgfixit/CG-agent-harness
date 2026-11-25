#!/usr/bin/env python3
"""Exercise POST /api/browser/run with accounts, Chrome and stub Ollama."""

import json
import os
import re
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[1]
LOOPBACK = '127.0.0.1'  # DevSkim: ignore DS162092 because every fixture listener binds only to loopback.
MODEL = 'browser-fixture:latest'
MARKER = 'Deadline fixture page'
HTTP = build_opener(ProxyHandler({}))


class OllamaAndPage(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, body, content_type='application/json'):
        data = json.dumps(body).encode() if content_type == 'application/json' else body.encode()
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == '/api/ps':
            self.server.windows_read.append(self.server.window)
            self.reply({'models': [{'name': MODEL, 'context_length': self.server.window}]})
        elif self.path == '/api/tags':
            self.reply({'models': [{'name': MODEL}]})
        elif self.path == '/v1/models':
            self.reply({'data': [{'id': MODEL}]})
        elif self.path == '/':
            self.reply(f'<!doctype html><h1>{MARKER}</h1>', 'text/html')
        else:
            self.send_error(404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/api/show':
            self.reply({'capabilities': ['completion', 'tools']})
        elif self.path == '/api/generate':
            self.reply({'done': True})
        elif self.path == '/api/chat':
            self.server.chats.append(body)
            if len(self.server.chats) == 1:
                self.reply({'message': {'role': 'assistant', 'tool_calls': [{'function': {
                    'name': 'navigate', 'arguments': {'url': self.server.origin + '/'},
                }}]}})
            else:
                # No HTTP response until teardown, so only the Harness deadline can end the call.
                self.server.stop.wait()
        else:
            self.send_error(404)


def request(base, path, headers=None, body=None, timeout=5):
    req = Request(base + path, headers=headers or {}, data=None if body is None else json.dumps(body).encode())
    try:
        response = HTTP.open(req, timeout=timeout)
    except HTTPError as error:
        response = error
    with response:
        return response.status, response.read(), response.headers


def wait_for_console(child, base):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and child.poll() is None:
        try:
            status, page, _ = request(base, '/', timeout=1)
            if status == 200:
                return re.search(rb'<meta name="csrf-token" content="([^"]+)"', page)[1].decode()
        except (URLError, TimeoutError):
            time.sleep(0.05)
    raise AssertionError('fixture console did not become ready')


def check_routes(child, base, home, fixture):
    headers = {'Origin': base, 'X-CyClaw-CSRF': wait_for_console(child, base), 'Content-Type': 'application/json'}
    setup = (home / 'setup-code').read_text().strip()
    status, _, reply = request(base, '/api/auth/login', headers, {'username': 'admin', 'password': setup})
    assert status == 200, f'fixture login returned {status}'
    headers['Cookie'] = reply['Set-Cookie'].split(';', 1)[0]
    status, _, reply = request(base, '/api/auth/password', headers, {
        'current_password': setup, 'password': secrets.token_urlsafe(24),
    }, timeout=15)
    assert status == 200, f'fixture password change returned {status}'
    headers['Cookie'] = reply['Set-Cookie'].split(';', 1)[0]

    deadline = time.monotonic() + 15
    while True:
        status, raw, _ = request(base, '/api/ollama/profile', headers)
        assert status == 200, (status, raw)
        if (json.loads(raw).get('tuning') or {}).get('window') == fixture.window:
            break
        assert time.monotonic() < deadline, f'fixture never finished tuning: {raw!r}'
        time.sleep(0.2)

    task = {'task': f'Read the heading at {fixture.origin}/', 'origins': [fixture.origin]}
    fixture.window = 4096
    before = len(fixture.windows_read)
    status, raw, _ = request(base, '/api/browser/run', headers, task, timeout=50)
    assert (status, json.loads(raw).get('detail', {}).get('code')) == (409, 'BROWSER_WINDOW'), (status, raw)
    assert 4096 in fixture.windows_read[before:] and not fixture.chats, 'expected a fresh window check and no chat'
    print('409 BROWSER_WINDOW: /api/ps read 4096, no /api/chat', flush=True)

    fixture.window = 32768
    status, raw, _ = request(base, '/api/browser/run', headers, task, timeout=50)
    assert status == 200, (status, raw)
    body = json.loads(raw)
    assert body['finished'] is False and body['exit_code'] == 2, body
    report = body['report']
    assert report['stopped'] == "the task's time budget ran out", body
    assert MARKER in report['page_text'] and report['transcript'], body
    assert len(fixture.chats) == 2 and all(call['model'] == MODEL for call in fixture.chats), 'expected two model calls'
    assert fixture.chats[1]['messages'][-1]['role'] == 'tool', 'second call must follow a browser tool result'
    print('200 unfinished: second /api/chat stalled, deadline report retained the page and transcript')


def main(binary):
    with tempfile.TemporaryDirectory(prefix='cgah-browser-route-') as directory, \
            ThreadingHTTPServer((LOOPBACK, 0), OllamaAndPage) as fixture:
        home = Path(directory)
        fixture.origin = f'http://{LOOPBACK}:{fixture.server_port}'  # DevSkim: ignore DS137138 because the synthetic model and page both serve only on loopback.
        fixture.window, fixture.windows_read, fixture.chats = 32768, [], []
        fixture.stop = threading.Event()
        worker = threading.Thread(target=fixture.serve_forever, daemon=True)
        worker.start()
        try:
            config = {
                'auth': {'enabled': True}, 'tls': {'enabled': False},
                'browser': {'enabled': True, 'sidecar': str(ROOT / 'browser/sidecar.mjs'), 'timeout_sec': 30},
                'models': {'local_llm': {'provider': 'ollama', 'base_url': fixture.origin + '/v1',
                                         'model': MODEL, 'auto_tune': True}},
            }
            path = home / 'config.yaml'
            path.touch(mode=0o600)
            path.write_text(json.dumps(config))  # JSON is valid YAML.
            scratch = home / 'tmp'
            scratch.mkdir()
            env = {key: os.environ[key] for key in ('PATH', 'HOME') if key in os.environ}
            env.update(CGAGENTHARNESS_HOME=str(home), CGAH_TEST_PRIVATE_BROWSER_ORIGINS='1', TMPDIR=str(scratch))
            with socket.socket() as reservation:
                reservation.bind((LOOPBACK, 0))
                port = reservation.getsockname()[1]
            base = f'http://{LOOPBACK}:{port}'  # DevSkim: ignore DS137138 because the console fixture uses a disposable account on loopback.
            with tempfile.TemporaryFile() as output, subprocess.Popen(
                    [binary, 'serve', '--host', LOOPBACK, '--port', str(port)],
                    cwd=ROOT, env=env, stdout=output, stderr=output) as child:
                try:
                    check_routes(child, base, home, fixture)
                except Exception:
                    output.seek(0)
                    sys.stderr.write(output.read().decode(errors='replace'))
                    raise
                finally:
                    child.terminate()
                    try:
                        child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
        finally:
            fixture.stop.set()
            fixture.shutdown()
            worker.join()


if __name__ == '__main__':
    main(Path(sys.argv[1] if len(sys.argv) == 2 else ROOT / 'target/debug/cgagentharness').resolve())
