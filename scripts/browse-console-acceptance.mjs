// #1123 phases 3 and 4: /browse takeover through the real console, the real
// server and real Chrome, with a scripted stand-in for the model so CI needs
// none; then finding a waiting task again after a reload, a session switch,
// logout and a restart.
//
// One loopback server is both the stand-in Ollama and the shop the task
// browses; a second counts any request a page could leak to it. `serve`
// (a test-support build) runs the task; this script signs in, loads the
// shipped console in Chrome and drives each case through the waiting card.
// Chrome keeps its sandbox, so this refuses to run as root.
//
// Usage: node scripts/browse-console-acceptance.mjs target/debug/cgagentharness
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { access, constants as fsConstants, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const binary = resolve(process.argv[2] || 'target/debug/cgagentharness');
const MODEL = 'stand-in:latest';
const WAIT_SEC = 12;
if (process.getuid && process.getuid() === 0) {
  console.error("refusing to run as root: Chrome's sandbox must stay on");
  process.exit(1);
}
const pause = ms => new Promise(r => setTimeout(r, ms));

// --- The page a leaked request would reach ---------------------------------
const leaks = [];
const collector = createServer((req, res) => { leaks.push(req.method + ' ' + req.url); res.end(); });
await new Promise(r => collector.listen(0, '127.0.0.1', r));
const collectorOrigin = 'http://127.0.0.1:' + collector.address().port;

// --- Stand-in Ollama and the shop -------------------------------------------
// The body the shop POSTs: page-written markup the console must show as text.
const BODY = `qty=2 <img src="${collectorOrigin}/leak" onerror="window.BROWSE_INJECTED=1">`;
const BODY_SHA = createHash('sha256').update(BODY).digest('hex');
const SHOP = `<!doctype html><h1>SHOP</h1>
<button onclick="save()">Save order</button><p id="state">idle</p>
<script>
function save() {
  fetch('/api/save', {method: 'POST', headers: {'content-type': 'text/plain'}, body: ${JSON.stringify(BODY)}})
    .then(r => { document.getElementById('state').textContent = 'saved ' + r.status; })
    .catch(() => { document.getElementById('state').textContent = 'not saved'; });
}
</script>`;
const saves = [];
const json = (res, body) => { const data = JSON.stringify(body); res.writeHead(200, {'content-type': 'application/json', 'content-length': Buffer.byteLength(data)}); res.end(data); };
// The model's next step depends only on what it has seen so far: open the
// shop (the granted root: provenance refuses a composed URL), click "Save
// order", then finish. Every job runs the same script.
const nextStep = messages => {
  const results = messages.filter(m => m.role === 'tool');
  if (results.length === 0) return {name: 'navigate', arguments: {url: shopOrigin + '/'}};
  if (results.length === 1) {
    const seen = JSON.parse(results[0].content || '{}');
    const button = (seen.elements || []).find(el => el.name === 'Save order');
    if (button) return {name: 'click', arguments: {ref_id: button.ref_id}};
  }
  return {name: 'finish', arguments: {answer: 'ORDER-TASK-DONE'}};
};
const shop = createServer((req, res) => {
  let raw = '';
  req.on('data', chunk => { raw += chunk; });
  req.on('end', () => {
    if (req.method === 'GET' && req.url === '/api/ps') return json(res, {models: [{name: MODEL, context_length: 32768}]});
    if (req.method === 'GET' && req.url === '/api/tags') return json(res, {models: [{name: MODEL}]});
    if (req.method === 'GET' && req.url === '/v1/models') return json(res, {data: [{id: MODEL}]});
    if (req.method === 'POST' && req.url === '/api/show') return json(res, {capabilities: ['completion', 'tools']});
    if (req.method === 'POST' && req.url === '/api/generate') return json(res, {done: true});
    if (req.method === 'POST' && req.url === '/api/chat') {
      const call = nextStep(JSON.parse(raw).messages || []);
      return json(res, {message: {role: 'assistant', tool_calls: [{function: call}]}, done: true});
    }
    if (req.method === 'GET' && req.url === '/') { res.writeHead(200, {'content-type': 'text/html'}); return res.end(SHOP); }
    if (req.method === 'POST' && req.url === '/api/save') { saves.push(raw); res.writeHead(200, {'content-type': 'text/plain'}); return res.end('SAVED'); }
    res.writeHead(404); res.end();
  });
});
await new Promise(r => shop.listen(0, '127.0.0.1', r));
const shopOrigin = 'http://127.0.0.1:' + shop.address().port;

// --- The real server --------------------------------------------------------
const home = await mkdtemp(join(tmpdir(), 'cgah-browse-console-'));
await writeFile(join(home, 'config.yaml'), JSON.stringify({
  auth: {enabled: true},
  tls: {enabled: false},
  api: {rate_limit: {max_requests: 1000}},
  browser: {enabled: true, sidecar: join(ROOT, 'browser/sidecar.mjs'), timeout_sec: 120,
    takeover: {enabled: true, wait_sec: WAIT_SEC}},
  models: {local_llm: {provider: 'ollama', base_url: shopOrigin + '/v1', model: MODEL, auto_tune: true}},
}), {mode: 0o600});
const port = await new Promise(r => { const probe = createServer(); probe.listen(0, '127.0.0.1', () => { const p = probe.address().port; probe.close(() => r(p)); }); });
const base = 'http://127.0.0.1:' + port;
const serverLog = [];
const startServer = () => {
  const child = spawn(binary, ['serve', '--host', '127.0.0.1', '--port', String(port)], {
    cwd: ROOT,
    env: {PATH: process.env.PATH, HOME: home, CGAGENTHARNESS_HOME: home, CGAH_TEST_PRIVATE_BROWSER_ORIGINS: '1', TMPDIR: home},
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { serverLog.push(String(chunk)); if (serverLog.length > 200) serverLog.shift(); });
  return child;
};
const stopServer = async child => {
  if (child.exitCode === null && child.signalCode === null) { child.kill('SIGTERM'); await new Promise(r => { child.once('exit', r); setTimeout(r, 5000); }); }
};
let server = startServer();

let chrome, profile, ws;
const cleanup = async () => {
  try { ws && ws.close(); } catch {}
  if (chrome && chrome.exitCode === null) { chrome.kill('SIGTERM'); await new Promise(r => { chrome.once('exit', r); setTimeout(r, 2000); }); }
  await stopServer(server);
  collector.close(); shop.close();
  for (const dir of [profile, home]) if (dir) await rm(dir, {recursive: true, force: true, maxRetries: 10, retryDelay: 200});
};

try {
  // Sign in over HTTP as the bootstrap admin and replace its setup code, as
  // a first run does; the console then uses that session's cookie.
  const answering = async () => {
    let token;
    for (let i = 0; i < 200 && !token; i++) {
      if (server.exitCode !== null) throw new Error('serve exited: ' + serverLog.join(''));
      try { token = (await (await fetch(base + '/')).text()).match(/<meta name="csrf-token" content="([^"]+)"/)?.[1]; } catch { await pause(100); }
    }
    assert.ok(token, 'the console never answered');
    return token;
  };
  const csrf = await answering();
  const headers = {'origin': base, 'x-cyclaw-csrf': csrf, 'content-type': 'application/json'};
  const post = (path, body, cookie) => fetch(base + path, {method: 'POST', headers: {...headers, ...(cookie ? {cookie} : {})}, body: JSON.stringify(body)});
  const setup = (await readFile(join(home, 'setup-code'), 'utf8')).trim();
  let reply = await post('/api/auth/login', {username: 'admin', password: setup});
  assert.equal(reply.status, 200, 'bootstrap sign-in');
  let cookie = reply.headers.get('set-cookie').split(';')[0];
  reply = await post('/api/auth/password', {current_password: setup, password: randomBytes(18).toString('hex')}, cookie);
  assert.equal(reply.status, 200, 'setup code replaced');
  cookie = reply.headers.get('set-cookie').split(';')[0];
  const tuned = async () => {
    for (let i = 0; ; i++) {
      const profile = await (await fetch(base + '/api/ollama/profile', {headers: {...headers, cookie}})).json();
      if (profile.tuning && profile.tuning.window === 32768) return;
      assert.ok(i < 150, 'the stand-in model was never tuned: ' + JSON.stringify(profile));
      await pause(100);
    }
  };
  await tuned();

  // Chrome for the console, sandbox on.
  const chromeBinary = process.env.CHROME_BIN || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome');
  await access(chromeBinary, fsConstants.X_OK);
  profile = await mkdtemp(join(tmpdir(), 'cgah-browse-console-chrome-'));
  chrome = spawn(chromeBinary, ['--headless=new', '--no-first-run', '--disable-background-networking', '--disable-extensions',
    '--disable-gpu', '--disable-dev-shm-usage', '--remote-debugging-port=0', '--user-data-dir=' + profile, 'about:blank'], {stdio: 'ignore'});
  let devtools;
  for (let i = 0; i < 300 && !devtools; i++) {
    try { devtools = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0].trim(); } catch { await pause(100); }
  }
  assert.ok(devtools, 'Chrome must start: ' + chromeBinary);
  const targets = await (await fetch('http://127.0.0.1:' + devtools + '/json')).json();
  ws = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let seq = 0; const waiting = new Map();
  ws.onmessage = e => { const v = JSON.parse(e.data); const p = v.id && waiting.get(v.id); if (!p) return; waiting.delete(v.id); v.error ? p.reject(Error(JSON.stringify(v.error))) : p.resolve(v.result); };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++seq; waiting.set(id, {resolve, reject}); ws.send(JSON.stringify({id, method, params})); });
  const evaluate = async expression => {
    const v = await call('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
    if (v.exceptionDetails) throw Error(JSON.stringify(v.exceptionDetails));
    return v.result.value;
  };
  const until = async (expression, seconds = 30) => {
    for (let i = 0; i < seconds * 10; i++) { const v = await evaluate(expression); if (v) return v; await pause(100); }
    throw Error('Timed out: ' + expression + '\nstream: ' + await evaluate('document.getElementById("stream").innerText.slice(-1500)'));
  };
  await call('Network.enable');
  await call('Network.setCookie', {name: 'cgagentharness_session', value: cookie.split('=').slice(1).join('='), url: base, httpOnly: true, sameSite: 'Strict'});
  await call('Page.enable'); await call('Runtime.enable');
  await call('Page.navigate', {url: base});
  const ready = 'typeof onSend === "function" && document.getElementById("hAuthWho").textContent.includes("admin")';
  await until(ready);

  const card = 'Array.from(document.querySelectorAll(".fetch-confirm")).at(-1)';
  const streamText = () => evaluate('document.getElementById("stream").innerText');
  const cards = () => evaluate('document.querySelectorAll(".fetch-confirm").length');
  // Start one /browse task and wait for its waiting card.
  const browse = async () => {
    const before = await cards();
    await evaluate(`document.getElementById("input").value = ${JSON.stringify('/browse ' + shopOrigin + '/ save the order')}; onSend()`);
    await until(`document.querySelectorAll(".fetch-confirm").length > ${before} && ${card}.textContent.includes("It is paused, not sent")`);
  };
  // Each task that ends prints one result line carrying this meta, whether
  // the model finished or a held request stopped it.
  const ENDED = 'page text is untrusted';
  const done = async () => (await streamText()).split(ENDED).length - 1;
  // Waits for one more ended task than `marker` counted.
  const finished = async marker => until(`document.getElementById("stream").innerText.split(${JSON.stringify(ENDED)}).length - 1 > ${marker}`, 60);

  // 1. The card shows the request as sent, and its page-written parts as text.
  await browse();
  const shown = await evaluate(`${card}.innerText`);
  for (const part of ['POST ' + shopOrigin + '/api/save', 'text/plain', Buffer.byteLength(BODY) + ' bytes', 'SHA-256 ' + BODY_SHA, BODY, 'Waiting about']) {
    assert.ok(shown.includes(part), `the card must show ${JSON.stringify(part)}:\n${shown}`);
  }
  assert.equal(await evaluate(`${card}.querySelectorAll("img").length`), 0, 'page-written markup is text, not elements');
  assert.equal(await evaluate('window.BROWSE_INJECTED'), undefined, 'page-written handlers never run');
  console.log('waiting card -> method, URL, content type, size, SHA-256, preview and wait shown; markup inert');

  // 2. An invalid approval is refused and leaves the card answerable; a valid
  // one sends the request once and the task reports it.
  let marker = await done();
  await evaluate(`(() => { const c = ${card}; const r = c.querySelector("input"); r.value = "\\u200b"; r.dispatchEvent(new Event("input")); c.querySelectorAll("button")[0].click(); })()`);
  // The note leaves "Waiting about" only once the server has answered.
  await until(`!${card}.querySelector("input").disabled && !${card}.lastElementChild.textContent.startsWith("Waiting about")`);
  assert.equal(saves.length, 0, 'a refused approval sends nothing');
  const refusal = await evaluate(`${card}.lastElementChild.textContent`);
  assert.ok(!refusal.startsWith('Approved'), 'a blank reason must be refused by the server: ' + refusal);
  await evaluate(`(() => { const c = ${card}; const r = c.querySelector("input"); r.value = "the order I asked for"; r.dispatchEvent(new Event("input")); c.querySelectorAll("button")[0].click(); })()`);
  await finished(marker);
  assert.deepEqual(saves, [BODY], 'approved: the shop received the request once, as shown');
  const report = await streamText();
  assert.ok(report.includes('Sent with your approval: POST ' + shopOrigin + '/api/save'), 'the report lists the approved request');
  assert.ok(report.includes('ORDER-TASK-DONE'), 'the model finished after the approved request was answered');
  console.log(`invalid approval -> refused (${refusal}), card still answerable; valid approval -> sent once, reported`);

  // 3. Hold: not sent, and the report says so.
  marker = await done();
  await browse();
  await evaluate(`${card}.querySelectorAll("button")[1].click()`);
  await finished(marker);
  assert.equal(saves.length, 1, 'a held request is not sent');
  assert.ok((await streamText()).split('Held, not sent: POST ' + shopOrigin + '/api/save').length > 1, 'the report lists the held request');
  console.log('hold -> not sent, reported held');

  // 4. No answer before the wait runs out: held, and a late yes sends nothing.
  marker = await done();
  await browse();
  await finished(marker);
  await evaluate(`(() => { const c = ${card}; const r = c.querySelector("input"); r.value = "too late"; r.dispatchEvent(new Event("input")); c.querySelectorAll("button")[0].click(); })()`);
  await until(`${card}.lastElementChild.textContent.includes("BROWSER_") || ${card}.lastElementChild.textContent.includes("waiting")`, 10).catch(() => {});
  await pause(1000);
  assert.equal(saves.length, 1, 'an expired wait, and a yes after it, send nothing');
  console.log(`expired after ${WAIT_SEC} s -> held; a late yes sent nothing`);

  // 5. Cancel while waiting: the task stops, and the stale card sends nothing.
  await browse();
  await evaluate(`document.getElementById("input").value = "/browse cancel"; onSend()`);
  await until('document.getElementById("stream").innerText.includes("The browser task was cancelled.")', 60);
  await evaluate(`(() => { const c = ${card}; const r = c.querySelector("input"); r.value = "after cancel"; r.dispatchEvent(new Event("input")); c.querySelectorAll("button")[0].click(); })()`);
  await pause(1500);
  assert.equal(saves.length, 1, 'a cancelled task, and a yes after it, send nothing');
  console.log('cancel while waiting -> cancelled; the stale card sent nothing');

  // Phase 4: the tab keeps only {session_id, job_id} (sessionStorage), and
  // finds the task again from the server's record.
  const KEY = JSON.stringify('cgah.browseJob');
  const stored = () => evaluate(`sessionStorage.getItem(${KEY})`);
  const send = line => evaluate(`document.getElementById("input").value = ${JSON.stringify(line)}; onSend()`);
  const reload = async () => {
    await call('Page.reload');
    await pause(300);
    await until(ready);
  };
  const FOLLOWING = 'Following the browser task this tab started.';
  const approve = reason => evaluate(`(() => { const c = ${card}; const r = c.querySelector("input"); r.value = ${JSON.stringify(reason)}; r.dispatchEvent(new Event("input")); c.querySelectorAll("button")[0].click(); })()`);

  // 6. Reload while waiting: the card comes back from the server, the yes
  // sends the request once, and the ended task does not come back.
  await browse();
  const pair = JSON.parse(await stored());
  assert.deepEqual(Object.keys(pair).sort(), ['job_id', 'session_id'], 'only the pair is kept: ' + JSON.stringify(pair));
  assert.equal(pair.session_id, null);
  assert.match(pair.job_id, /^[0-9a-f]{32}$/);
  const pending = await evaluate(`${card}.innerText`);
  await reload();
  await until(`document.querySelectorAll(".fetch-confirm").length === 1 && ${card}.textContent.includes("It is paused, not sent")`);
  assert.ok((await streamText()).includes(FOLLOWING), 'the reloaded console says it is following the task');
  assert.ok((await evaluate(`${card}.innerText`)).includes('SHA-256 ' + BODY_SHA), 'the card shows the same request:\n' + pending);
  marker = await done();
  await approve('the order I asked for, after a reload');
  await finished(marker);
  assert.equal(saves.length, 2, 'approved after a reload: sent once');
  assert.equal(await stored(), null, 'an ended task is forgotten');
  await reload();
  await pause(2 * 1500 + 500);
  assert.equal(await cards(), 0, 'an ended task brings back no card');
  assert.ok(!(await streamText()).includes(FOLLOWING), 'an ended task is not followed again');
  console.log('reload while waiting -> the card came back; one yes sent it once; the ended task did not come back');

  // 7. Switch sessions while waiting: the card leaves with the session and
  // comes back with it; a hold there reaches the same request.
  await send('/session new browse-here');
  await until('document.getElementById("stream").innerText.includes("session created: ")');
  const sessionA = await evaluate('currentSession');
  await browse();
  assert.equal(JSON.parse(await stored()).session_id, sessionA, 'the pair names the session the task started in');
  await send('/session new elsewhere');
  await until(`currentSession && currentSession !== ${JSON.stringify(sessionA)} && document.getElementById("stream").innerText.includes("session created: ")`);
  await pause(2 * 1500 + 500);
  assert.equal(await cards(), 0, 'another session shows no card');
  await send('/session use ' + sessionA);
  await until(`currentSession === ${JSON.stringify(sessionA)} && document.querySelectorAll(".fetch-confirm").length === 1`);
  marker = await done();
  await evaluate(`${card}.querySelectorAll("button")[1].click()`);
  await finished(marker);
  assert.equal(saves.length, 2, 'held after a session switch: not sent');
  assert.ok((await streamText()).includes('Held, not sent: POST ' + shopOrigin + '/api/save'), 'the report lists the held request');
  assert.equal(await stored(), null, 'an ended task is forgotten');
  console.log('session switch while waiting -> no card elsewhere; back in its session the card returned; hold reached it');

  // 8. A task the server does not have (never existed, another account's,
  // or ended and cleared) is forgotten, and nothing is shown or sent.
  await evaluate(`sessionStorage.setItem(${KEY}, JSON.stringify({session_id: null, job_id: ${JSON.stringify(randomBytes(16).toString('hex'))}}))`);
  await reload();
  await until('document.getElementById("stream").innerText.includes("unavailable, nothing was resumed")');
  assert.equal(await stored(), null, 'an unknown task is forgotten');
  assert.equal(await cards(), 0);
  console.log('unknown task id -> "unavailable, nothing was resumed"; forgotten');

  // 9. Restart while waiting: the record is gone, the console says so, and the
  // held request is never sent.
  await browse();
  await stopServer(server);
  server = startServer();
  // A new process issues a new CSRF token; the sign-in itself survives.
  headers['x-cyclaw-csrf'] = await answering();
  await reload();
  await until('document.getElementById("stream").innerText.includes("unavailable, nothing was resumed")', 60);
  assert.equal(await stored(), null, 'a task lost to a restart is forgotten');
  assert.equal(await cards(), 0, 'no card after a restart');
  await pause((WAIT_SEC + 2) * 1000);
  assert.equal(saves.length, 2, 'a restart sends nothing');
  console.log('restart while waiting -> "unavailable, nothing was resumed"; nothing sent');

  // 10. Logout forgets the pair.
  await tuned();
  await browse();
  assert.ok(await stored(), 'a waiting task is kept');
  await evaluate('document.getElementById("hAuthLogout").click()');
  await until('document.body.classList.contains("signed-out")');
  assert.equal(await stored(), null, 'logout forgets the pair');
  console.log('logout -> the pair is forgotten');

  assert.deepEqual(leaks, [], 'no page-written request reached the collector');
  console.log('collector received nothing; the shop received exactly the two approved POSTs');
} catch (error) {
  console.error(error && error.stack || error);
  console.error('--- serve output (tail) ---\n' + serverLog.join('').slice(-4000));
  process.exitCode = 1;
} finally {
  await cleanup();
}
