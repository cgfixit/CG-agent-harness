// Drives the real console in headless Chromium against a running `cgagentharness serve`.
// Usage: node driver.mjs <base_url> <home> <plan.mjs> <out.json>
import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const require = createRequire('/opt/node-tools/node_modules/');
let playwright;
try { playwright = require('playwright'); } catch { playwright = (await import('/opt/node-tools/node_modules/playwright/index.mjs')).default; }

const [base, home, planPath, outPath] = process.argv.slice(2);
const plan = (await import(pathToFileURL(planPath).href)).default;
const browser = await playwright.chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1400, height: 1000 } });
const page = await context.newPage();
const consoleErrors = [];
page.on('pageerror', e => consoleErrors.push(String(e)));
page.on('console', m => { if (m.type() === 'error') consoleErrors.push(m.text()); });

const sleep = ms => new Promise(r => setTimeout(r, ms));
await page.goto(base + '/');
await page.waitForSelector('#hAuthUser', { state: 'visible', timeout: 60000 });

async function login() {
  const setup = (await readFile(join(home, 'setup-code'), 'utf8')).trim();
  await page.fill('#hAuthUser', 'admin');
  await page.fill('#hAuthPass', setup);
  await page.click('#hAuthLogin');
  await page.waitForFunction(() => document.getElementById('passwordDialog').open, null, { timeout: 30000 });
  await page.fill('#passwordCurrent', setup);
  await page.fill('#passwordNew', 'fixture-password-123');
  await page.fill('#passwordConfirm', 'fixture-password-123');
  await page.evaluate(() => document.getElementById('passwordForm').requestSubmit());
  await page.waitForFunction(() => !document.getElementById('passwordDialog').open, null, { timeout: 30000 });
  await page.waitForSelector('#input', { state: 'visible', timeout: 30000 });
  await sleep(1500);
}

const messages = (onlyNew = false) => page.evaluate(onlyNew => [...document.querySelectorAll('#stream .msg')]
  .filter(d => !onlyNew || !d.dataset.seen).map(d => ({
  kind: d.className.replace('msg ', ''),
  who: d.querySelector('.who')?.textContent || '',
  body: (d.querySelector('.body')?.innerText || '') + (d.querySelector('.meta') ? ' {meta: ' + d.querySelector('.meta').innerText + '}' : ''),
  buttons: [...d.querySelectorAll('button.cmd-item')].map(b => b.textContent),
})), onlyNew);
// Rows already on screen are marked, so a command that clears the transcript
// (/session new, /clear) still reports exactly the rows it added.
const markSeen = () => page.evaluate(() => document.querySelectorAll('#stream .msg').forEach(d => { d.dataset.seen = '1'; }));

async function settle(quietMs = 900, maxMs = 30000) {
  const start = Date.now();
  let last = -1, stableSince = Date.now();
  while (Date.now() - start < maxMs) {
    const state = await page.evaluate(() => [document.querySelectorAll('#stream .msg').length, document.getElementById('send').disabled]);
    if (state[0] !== last || state[1]) { last = state[0]; stableSince = Date.now(); }
    if (Date.now() - stableSince >= quietMs) return;
    await sleep(150);
  }
}

const api = {
  page, sleep, settle, messages,
  async send(line, opts = {}) {
    await markSeen();
    await page.fill('#input', line);
    await page.press('#input', 'Enter');
    await settle(opts.quietMs, opts.maxMs);
    return messages(true);
  },
  async paste(text) {
    await markSeen();
    await page.fill('#input', '');
    await page.focus('#input');
    await page.evaluate(t => {
      const dt = new DataTransfer(); dt.setData('text/plain', t);
      const ev = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true });
      const el = document.getElementById('input');
      if (el.dispatchEvent(ev)) {
        // Default action: the browser would insert the text; single-line inputs drop newlines.
        const v = el.tagName === 'INPUT' ? t.replace(/\r?\n/g, '') : t;
        el.setRangeText(v, el.selectionStart, el.selectionEnd, 'end');
        el.dispatchEvent(new Event('input', { bubbles: true }));
      }
    }, text);
    await settle(400);
    return { value: await page.inputValue('#input'), added: await messages(true) };
  },
  async arrow(key, times = 1) {
    for (let i = 0; i < times; i++) { await page.press('#input', key); await sleep(250); }
    return page.inputValue('#input');
  },
  async fetchJson(path, method = 'GET', body) {
    return page.evaluate(async ([path, method, body]) => {
      const csrf = document.querySelector('meta[name="csrf-token"]').content;
      const r = await fetch(path, { method, headers: { 'Content-Type': 'application/json', 'X-CyClaw-CSRF': csrf }, credentials: 'same-origin', body: body ? JSON.stringify(body) : undefined });
      let json = null; try { json = await r.json(); } catch {}
      return { status: r.status, json };
    }, [path, method, body]);
  },
  async reload() { await page.reload(); await page.waitForSelector('#input', { state: 'visible', timeout: 30000 }); await sleep(1500); },
};

const results = [];
try {
  await login();
  for (const step of plan) {
    const started = Date.now();
    let outcome;
    try {
      const evidence = await step.run(api);
      outcome = { id: step.id, area: step.area, input: step.input, pass: true, evidence };
    } catch (e) {
      outcome = { id: step.id, area: step.area, input: step.input, pass: false, error: String(e && e.message || e) };
    }
    outcome.ms = Date.now() - started;
    results.push(outcome);
    console.log((outcome.pass ? 'PASS ' : 'FAIL ') + step.id + (outcome.pass ? '' : ' :: ' + outcome.error));
  }
  await page.screenshot({ path: outPath.replace(/\.json$/, '.png'), fullPage: false });
} finally {
  await writeFile(outPath, JSON.stringify({ base, consoleErrors, results }, null, 1));
  await browser.close();
}
