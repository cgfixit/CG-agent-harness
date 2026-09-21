// Exercise the console's real fetch helpers against headers-first HTTP replies.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createServer} from 'node:http';
import vm from 'node:vm';

const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('async function fetchHold('), html.indexOf('\nconst STATUS_BASE_INTERVAL'));
const context = vm.createContext({fetch, AbortController, Response, window: {setTimeout, clearTimeout}});
vm.runInContext(source, context);
const server = createServer((req, res) => {
  if (req.url === '/empty') { res.writeHead(204); res.end(); return; }
  res.writeHead(req.url === '/refused' ? 403 : 200, {'Content-Type': 'application/json'});
  res.flushHeaders();
  if (req.url === '/stall') { res.write('{'); return; }
  res.end(req.url === '/refused' ? 'not JSON' : '{"role":"admin"}');
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = 'http://127.0.0.1:' + server.address().port;
async function read(path, options = {}, timeout = 1000) {
  const response = await context.fetchWithTimeout(base + path, options, timeout);
  return response.json();
}
async function bounded(operation) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('body outlived the request deadline')), 2000);
    })]);
  } finally { clearTimeout(timer); }
}
try {
  assert.deepEqual(await read('/ok'), {role: 'admin'});
  const refused = await context.fetchWithTimeout(base + '/refused');
  assert.equal(refused.status, 403, 'non-JSON errors must retain their HTTP status');
  await assert.rejects(refused.json(), SyntaxError);
  const empty = await context.fetchWithTimeout(base + '/empty');
  assert.equal(empty.status, 204);
  assert.equal(await empty.text(), '');
  await assert.rejects(bounded(read('/stall', {}, 50)), {name: 'AbortError'});
  const controller = new AbortController();
  const cancelled = read('/stall', {signal: controller.signal});
  setTimeout(() => controller.abort(), 50);
  await assert.rejects(bounded(cancelled), {name: 'AbortError'});
  console.log('auth deadlines: complete JSON, refusal, empty body, stalled body and caller cancellation passed');
} finally {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
