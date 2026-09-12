#!/usr/bin/env python3
"""Prepare and serve a new, disposable arithmetic fixture; no real GitHub writes."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
from urllib.parse import urlparse

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('directory', type=Path, help='new directory; must not exist')
parser.add_argument('--model', required=True, help='exact installed Ollama tag')
parser.add_argument('--endpoint', default='http://127.0.0.1:11434/v1')
parser.add_argument('--port', type=int, default=8792)
parser.add_argument('--http', action='store_true', help='explicit HTTP browser-fixture lane; account authentication stays enabled')
parser.add_argument('--prepare-only', action='store_true', help='prepare the fixture without launching a server (desktop acceptance)')
args = parser.parse_args()
assert urlparse(args.endpoint).hostname == '127.0.0.1', 'local inference only'
repo = Path(__file__).resolve().parent.parent
binary = repo / 'target/release/cgagentharness'
assert binary.is_file(), 'build the release binary first'
root = args.directory.resolve()
root.mkdir(mode=0o700)
seed = root / 'seed'
(seed / 'src').mkdir(parents=True)
(seed / 'Cargo.toml').write_text("[package]\nname='browser-fixture'\nversion='0.1.0'\nedition='2021'\n")
(seed / 'src/lib.rs').write_text('// retained padding\n' * 600 + 'pub fn add(a: i32, b: i32) -> i32 { a - b }\n#[test] fn adds_positive() { assert_eq!(add(2,3),5); }\n')
(seed / 'README.md').write_text('# Disposable arithmetic fixture\n')
env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
           GROK_API_KEY='', ANTHROPIC_API_KEY='', DEEPAGENT_API_KEY='', CARGO_NET_OFFLINE='true')
def run(argv, cwd=seed):
    subprocess.run(argv, cwd=cwd, env=env, check=True, capture_output=True)
run(['cargo', 'generate-lockfile', '--offline'])
run(['git', 'init', '-q', '-b', 'main'])
run(['git', 'add', '.'])
run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Seed controlled arithmetic defect'])
bare = root / 'origin.git'
run(['git', 'clone', '--bare', str(seed), str(bare)])
home = root / 'home'
home.mkdir()
config = (repo / 'assets/config.default.yaml').read_text()
def override(text, dotted, value):
    parts = dotted.split('.')
    parents = []
    output = []
    found = False
    for line in text.splitlines():
        trimmed = line.lstrip()
        indent = len(line) - len(trimmed)
        if trimmed and not trimmed.startswith('#') and ':' in trimmed:
            while parents and indent <= parents[-1][0]:
                parents.pop()
            key = trimmed.split(':', 1)[0].strip()
            if [p[1] for p in parents] + [key] == parts:
                line = ' ' * indent + key + ': ' + json.dumps(value)
                found = True
            if not trimmed.split(':', 1)[1].strip():
                parents.append((indent, key))
        output.append(line)
    assert found, dotted
    return '\n'.join(output) + '\n'
for key, value in {
    'agentic.repo': 'fixture/repository', 'agentic.enabled': True, 'agentic.deepagent_github.enabled': True,
    'agentic.deepagent_github.allow_git_write_tools': True,
    'agentic.deepagent_github.base_url': args.endpoint,
    'agentic.deepagent_github.model': args.model,
    'agentic.deepagent_github.planner_timeout_sec': 180,
    'agentic.deepagent_github.planner_max_tokens': 1024,
    'models.local_llm.base_url': args.endpoint,
    'models.local_llm.model': args.model,
    'models.local_llm.max_tokens': 256, 'tls.enabled': not args.http,
}.items():
    config = override(config, key, value)
(home / 'config.yaml').write_text(config)
run(['python3', str(repo / 'scripts/prepare-cargo.py'), str(seed), str(home)])
fakebin = root / 'fakebin'
fakebin.mkdir()
(fakebin / 'gh').write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
command = sys.argv[1:3]
if sys.argv[1:] == ['--version']: print('gh version 2.60.0')
elif command == ['repo', 'view']: print(json.dumps({'name': 'fixture', 'description': 'Disposable arithmetic fixture', 'defaultBranchRef': {'name': 'main'}, 'url': 'https://example.invalid/fixture'}))
elif command == ['repo', 'clone']: os.execvp('git', ['git', 'clone', '-q', ''' + repr(str(bare)) + ''', sys.argv[4]])
elif command in [['pr', 'list'], ['issue', 'list']]: print('[]')
elif command == ['pr', 'create']:
    body = Path(sys.argv[sys.argv.index('--body-file') + 1]).read_text()
    Path(''' + repr(str(root / 'published-body')) + ''').write_text(body)
    print('https://example.invalid/pull/1')
else: sys.exit('unsupported fixture operation')
''')
(fakebin / 'gh').chmod(0o700)
key = secrets.token_hex(20)
keyfile = root / 'api-key'
keyfile.touch(mode=0o600)
keyfile.write_text(key)
env.update(CGAGENTHARNESS_HOME=str(home), CGAGENTHARNESS_API_KEY=key,
           PATH=str(fakebin) + os.pathsep + env['PATH'])
print(json.dumps({'base_url': f'{"http" if args.http else "https"}://127.0.0.1:{args.port}', 'home': str(home), 'key_file': str(keyfile), 'fixture': 'disposable-arithmetic'}), flush=True)
if args.prepare_only:
    (home / '.env').touch(mode=0o600)
    (home / '.env').write_text("export CGAGENTHARNESS_API_KEY='" + key + "'\n")
    (home / 'desktop-tools.json').touch(mode=0o600)
    (home / 'desktop-tools.json').write_text(json.dumps({'directories': [str(fakebin)]}) + '\n')
else:
    try:
        subprocess.run([str(binary), 'serve', '--port', str(args.port)], env=env, check=True)
    except KeyboardInterrupt:
        pass
