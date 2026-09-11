#!/usr/bin/env node
// Agent driver for the cgagentharness console. Launches a loopback fake
// OpenAI-compatible model, a disposable home pointed at it, and the real
// `cgagentharness serve` binary; then drives it over HTTP (smoke), in a
// headless Chromium (shot), or leaves it running (serve).
//
//   node driver.mjs smoke                    API smoke through the guard chain; JSON summary
//   node driver.mjs shot out.png [cmd ...]   run console commands in the real UI, screenshot
//   node driver.mjs serve                    bring up and stay up; Ctrl-C stops everything
//
// Env: CGAH_BIN (target/debug/cgagentharness), CGAH_PORT (8790),
//      CGAH_MODEL_PORT (18434), CGAH_HOME (mkdtemp), CGAH_BASE (attach to an
//      already-running server; skips launch; CGAGENTHARNESS_API_KEY is sent as
//      a bearer token if set), CHROME_BIN, PLAYWRIGHT_MODULE.
import {spawn} from 'node:child_process';
import {createServer} from 'node:http';
import {mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';

const REPO = resolve(new URL('../../..', import.meta.url).pathname);
const BIN = resolve(REPO, process.env.CGAH_BIN || 'target/debug/cgagentharness');
const PORT = Number(process.env.CGAH_PORT || 8790);
const MODEL_PORT = Number(process.env.CGAH_MODEL_PORT || 18434);
const [mode = 'smoke', ...rest] = process.argv.slice(2);
const log = (...a) => console.error('[driver]', ...a);
const sleep = ms => new Promise(r => setTimeout(r, ms));

// ---- fake model -----------------------------------------------------------
function startModel() {
  const server = createServer(async (req, res) => {
    let data = ''; for await (const c of req) data += c;
    const json = v => { const b = JSON.stringify(v); res.writeHead(200, {'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(b)}); res.end(b); };
    if (req.method === 'GET') return json({data: [{id: 'fixture-model'}]});
    const body = JSON.parse(data || '{}');
    const last = (body.messages || []).at(-1)?.content || '';
    json({model: body.model, choices: [{finish_reason: 'stop', message: {role: 'assistant', content: 'fixture reply to: ' + String(last).slice(0, 60)}}],
          usage: {prompt_tokens: 10, completion_tokens: 5}});
  });
  return new Promise(r => server.listen(MODEL_PORT, '127.0.0.1', () => r(server)));
}

// ---- home + server --------------------------------------------------------
const MARKER = '# written by .claude/skills/run-cg-agent-harness/driver.mjs (fixture home; safe to overwrite)\n';
function makeHome() {
  const home = process.env.CGAH_HOME || mkdtempSync(join(tmpdir(), 'cgah-home-'));
  const existing = join(home, 'config.yaml');
  if (existsSync(existing) && !readFileSync(existing, 'utf8').startsWith(MARKER))
    throw new Error(`${existing} exists and was not written by this driver; refusing to overwrite an operator's config. Use a different CGAH_HOME or attach with CGAH_BASE.`);
  const cfg = MARKER + readFileSync(join(REPO, 'assets/config.default.yaml'), 'utf8')
    .replaceAll('base_url: "http://127.0.0.1:11434/v1"', `base_url: "http://127.0.0.1:${MODEL_PORT}/v1"`)
    .replaceAll('model: "qwen3.8:27b-mlx"', 'model: "fixture-model"');
  if (!cfg.includes(`127.0.0.1:${MODEL_PORT}`)) throw new Error('config.default.yaml no longer matches the sed pattern; update driver.mjs');
  writeFileSync(join(home, 'config.yaml'), cfg);
  return home;
}

async function startServer(home) {
  if (!existsSync(BIN)) throw new Error(`binary missing: ${BIN}  (run: cargo build --locked)`);
  const env = {...process.env, CGAGENTHARNESS_HOME: home};
  for (const k of ['GROK_API_KEY', 'ANTHROPIC_API_KEY', 'DEEPAGENT_API_KEY', 'CGAGENTHARNESS_API_KEY']) delete env[k];
  const base = `http://127.0.0.1:${PORT}`;
  if (await fetch(base + '/').then(() => true, () => false))
    throw new Error(`something already answers on ${base}; refusing to launch. Set CGAH_PORT, or attach to it with CGAH_BASE=${base}.`);
  const child = spawn(BIN, ['serve', '--port', String(PORT)], {env, stdio: ['ignore', 'pipe', 'pipe']});
  let out = ''; child.stdout.on('data', d => out += d); child.stderr.on('data', d => out += d);
  for (let i = 0; i < 150; i++) {
    if (child.exitCode !== null) throw new Error(`serve exited ${child.exitCode}:\n${out}`);
    try {
      if ((await fetch(base + '/')).ok) {
        const {api} = await client(base);
        const st = await api('/api/status');
        if (st.status !== 200 || st.json.home !== home) { child.kill('SIGKILL'); throw new Error(`server on ${base} reports home ${st.json?.home}, not ${home}; not the process we spawned`); }
        return {child, base, logs: () => out};
      }
    } catch (e) { if (/not the process we spawned/.test(String(e))) throw e; }
    await sleep(100);
  }
  child.kill('SIGKILL'); throw new Error('serve never answered GET / within 15s:\n' + out);
}

// ---- API client (guard chain: same-origin + CSRF) --------------------------
async function client(base) {
  const html = await (await fetch(base + '/')).text();
  const csrf = html.match(/name="csrf-token" content="([^"]+)"/)?.[1];
  if (!csrf) throw new Error('no csrf-token meta in GET /');
  const auth = process.env.CGAH_BASE && process.env.CGAGENTHARNESS_API_KEY ? {Authorization: 'Bearer ' + process.env.CGAGENTHARNESS_API_KEY} : {};
  const headers = {...auth, 'X-CyClaw-CSRF': csrf, 'Origin': base, 'Content-Type': 'application/json'};
  const api = async (path, body, extra = {}) => {
    const r = await fetch(base + path, {method: body === undefined ? 'GET' : 'POST', headers: {...headers, ...extra}, body: body === undefined ? undefined : JSON.stringify(body)});
    const text = await r.text(); let json; try { json = JSON.parse(text); } catch { json = text; }
    return {status: r.status, json};
  };
  return {csrf, api, headers};
}

// ---- modes ----------------------------------------------------------------
async function up() {
  if (process.env.CGAH_BASE) { log('attaching to', process.env.CGAH_BASE); return {base: process.env.CGAH_BASE, stop: async () => {}}; }
  const model = await startModel();
  const home = makeHome();
  const srv = await startServer(home);
  log('serve up', srv.base, 'home', home);
  const stop = async () => {
    srv.child.kill('SIGTERM'); await Promise.race([new Promise(r => srv.child.once('exit', r)), sleep(3000)]);
    if (srv.child.exitCode === null) srv.child.kill('SIGKILL');
    model.close(); if (!process.env.CGAH_HOME) rmSync(home, {recursive: true, force: true});
  };
  return {base: srv.base, home, stop, srv};
}

// attached: driving a server we did not launch. Its model and config are
// unknown, so fixture-specific assertions relax and the /api/agent/run probe
// is skipped (it would start a real run if the operator armed the gates).
async function smoke(base, attached) {
  const {api, headers} = await client(base);
  const checks = [];
  const expect = (name, cond, detail) => { checks.push({name, ok: !!cond, detail}); if (!cond) log('FAIL', name, JSON.stringify(detail)); };
  const {'X-CyClaw-CSRF': _drop, ...noCsrf} = headers;
  let r = await fetch(base + '/api/sessions', {method: 'POST', headers: noCsrf, body: '{}'});
  expect('missing CSRF is 403', r.status === 403, r.status);
  r = await api('/api/sessions', {}, {Origin: 'https://evil.invalid'});
  expect('foreign Origin is 403', r.status === 403, r.status);
  r = await api('/api/status');
  if (attached) expect('status 200 + a model configured', r.status === 200 && typeof r.json.model === 'string' && r.json.model, r.json);
  else expect('status 200 + fixture model', r.status === 200 && r.json.model === 'fixture-model', r.json);
  r = await api('/api/sessions', {}); expect('session create 201', r.status === 201 && r.json.session_id, r.status);
  const sid = r.json.session_id;
  r = await api('/api/chat', {session_id: sid, message: 'hello harness'});
  if (attached) expect('chat round-trips through the configured model', r.status === 200 && typeof r.json.reply === 'string' && r.json.reply.length > 0, r.json);
  else expect('chat round-trips through model', r.status === 200 && /fixture reply to: hello harness/.test(r.json.reply), r.json);
  r = await api('/api/tools'); expect('tools wired', r.status === 200 && /registered/.test(r.json.diagram || ''), r.status);
  r = await api('/api/agent/checks'); expect('agent check profiles listed', r.status === 200 && Array.isArray(r.json.profiles), r.status);
  if (attached) log('skipping /api/agent/run probe in attach mode (could start a real run on an armed server)');
  else {
    r = await api('/api/agent/run', {branch: 'claude/probe', instruction: 'probe', commit_message: 'probe', confirm: true, reason: 'probe'});
    expect('agent run refused: gates ship closed (409 AGENTIC_DISABLED)', r.status === 409 && r.json.detail?.code === 'AGENTIC_DISABLED', r.json);
  }
  r = await api('/api/github/status'); expect('shim spawns agentic child (status action exit 0)', r.status === 200 && r.json.ok === true && r.json.exit_code === 0, r.json);
  const failed = checks.filter(c => !c.ok).length;
  console.log(JSON.stringify({base, passed: checks.length - failed, failed, checks: checks.map(c => (c.ok ? 'ok   ' : 'FAIL ') + c.name)}, null, 1));
  return failed === 0;
}

async function shot(base, out, cmds) {
  const pwPath = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.mjs';
  const {chromium} = await import(pwPath);
  const browser = await chromium.launch({executablePath: process.env.CHROME_BIN || '/opt/pw-browsers/chromium', args: ['--no-sandbox', '--disable-gpu']});
  try {
    const page = await browser.newPage({viewport: {width: 1280, height: 800}});
    page.on('pageerror', e => log('pageerror', e.message));
    await page.goto(base, {waitUntil: 'networkidle'});
    await page.waitForFunction(() => typeof onSend === 'function');
    for (const cmd of cmds.length ? cmds : ['/status', 'hello from the driver']) {
      const before = await page.evaluate(() => document.getElementById('stream').innerText.length);
      await page.fill('#input', cmd); await page.evaluate(() => onSend());
      await page.waitForFunction(b => document.getElementById('stream').innerText.length > b, before, {timeout: 15000});
      await page.waitForFunction(() => !window.inflightChat, null, {timeout: 15000}).catch(() => {});
      await page.waitForTimeout(300);
    }
    console.log(await page.evaluate(() => document.getElementById('stream').innerText.slice(-1200)));
    await page.screenshot({path: out, fullPage: true}); log('screenshot', out);
  } finally { await browser.close(); }
}

const ctx = await up();
let ok = true;
try {
  if (mode === 'smoke') ok = await smoke(ctx.base, !!process.env.CGAH_BASE);
  else if (mode === 'shot') { if (!rest[0]) throw new Error('shot needs an output path'); await shot(ctx.base, rest[0], rest.slice(1)); }
  else if (mode === 'serve') {
    const {csrf} = await client(ctx.base);
    console.log(JSON.stringify({base: ctx.base, home: ctx.home, csrf, pid: ctx.srv?.child.pid, hint: `curl --noproxy '*' -H 'X-CyClaw-CSRF: ${csrf}' -H 'Origin: ${ctx.base}' ${ctx.base}/api/status`}, null, 1));
    await new Promise(r => { process.once('SIGINT', r); process.once('SIGTERM', r); });
  } else throw new Error(`unknown mode ${mode}; use smoke | shot <out.png> [cmd ...] | serve`);
} catch (e) { ok = false; log(e.stack || e); if (ctx.srv) log('server log tail:\n' + ctx.srv.logs().slice(-2000)); }
finally { await ctx.stop(); }
process.exit(ok ? 0 : 1);
