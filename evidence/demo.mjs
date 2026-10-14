// Screen recording of the operator's reported session, replayed with the fixed commands.
// Usage: node demo.mjs <base_url> <home> <video_dir> <log.json>
import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
const require = createRequire('/opt/node-tools/node_modules/');
const { chromium } = require('playwright');

const [base, home, videoDir, logPath] = process.argv.slice(2);
const sleep = ms => new Promise(r => setTimeout(r, ms));
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, recordVideo: { dir: videoDir, size: { width: 1280, height: 800 } } });
const page = await context.newPage();
const log = [];

await page.goto(base + '/');
await page.waitForSelector('#hAuthUser', { state: 'visible', timeout: 60000 });
const banner = () => page.evaluate(() => {
  if (document.getElementById('demoBanner')) return;
  const b = document.createElement('div'); b.id = 'demoBanner';
  b.textContent = 'DEMO · local build of this PR · fresh home, empty URL allowlist · model replies come from a deterministic loopback stand-in served as qwen3.8:27b-mlx-cg (no real LLM here) · this sandbox has no internet egress, so live Google and page reads end in a network error';
  Object.assign(b.style, { position: 'fixed', left: 0, right: 0, bottom: 0, zIndex: 99999, background: '#2b2140', color: '#fff', font: '12px/1.4 system-ui, sans-serif', padding: '6px 10px', opacity: 0.92 });
  document.body.appendChild(b);
});
await banner();
const setup = (await readFile(join(home, 'setup-code'), 'utf8')).trim();
await page.fill('#hAuthUser', 'admin'); await page.fill('#hAuthPass', setup); await page.click('#hAuthLogin');
await page.waitForFunction(() => document.getElementById('passwordDialog').open, null, { timeout: 30000 });
await page.fill('#passwordCurrent', setup); await page.fill('#passwordNew', 'demo-password-1234'); await page.fill('#passwordConfirm', 'demo-password-1234');
await page.evaluate(() => document.getElementById('passwordForm').requestSubmit());
await page.waitForFunction(() => !document.getElementById('passwordDialog').open, null, { timeout: 30000 });
await page.waitForSelector('#input', { state: 'visible' });
await banner();
await sleep(1200);

async function settle(maxMs = 30000) {
  const start = Date.now(); let last = -1, stable = Date.now();
  while (Date.now() - start < maxMs) {
    const [n, busy] = await page.evaluate(() => [document.querySelectorAll('#stream .msg').length, document.getElementById('send').disabled]);
    if (n !== last || busy) { last = n; stable = Date.now(); }
    if (Date.now() - stable > 1100) return;
    await sleep(150);
  }
}
const rows = () => page.evaluate(() => [...document.querySelectorAll('#stream .msg')].map(d => d.className.replace('msg ', '') + ': ' + d.innerText.replace(/\s+/g, ' ').slice(0, 400)));
async function type(line, { enter = true, pause = 1800, maxMs } = {}) {
  const before = (await rows()).length;
  await page.click('#input'); await page.fill('#input', '');
  await page.type('#input', line, { delay: 28 });
  await sleep(450);
  if (enter) { await page.press('#input', 'Enter'); await settle(maxMs); }
  await sleep(pause);
  const after = await rows();
  log.push({ input: line, rows: after.slice(Math.min(before, after.length)) });
}

// 1. The reported session, with the corrected commands.
await type('/session new hvac research');
await type('/model use qwen3.827b-mlx-cg');            // the typo from the report: now warned
await type('/model use qwen3.8:27b-mlx-cg');
await type('yoo');
await type('/goal find the best hvac repair service thats not too expensive (mid-tier) near cumming, georgia or zip code 30041');
await type('/goal --web all');                            // switch this goal to search without retyping it
await type('/loop auto');                                 // says plainly that it starts nothing yet
await type('use /web search and /web research to research goal and reply with detailed and unique, customized and citeable findings of the top 2-3 hvac repair vendors that have offices within 50 miles from 30041', { pause: 2500 });
await type('/web status', { pause: 2500 });
// 2. The allowlist: the fuzzy form from the report now works, then the broad form.
await type('/web allow *.google.com google.com/* --group google', { pause: 2600 });
await type('/web allow https://www.google.com/* https://*.google.com/* --group google', { pause: 2600 });
// 3. Paste a command copied from a reply's code block (it carries a line break).
await page.click('#input'); await page.fill('#input', '');
await page.evaluate(t => {
  const dt = new DataTransfer(); dt.setData('text/plain', t);
  const el = document.getElementById('input');
  if (el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }))) el.setRangeText(t.replace(/\n/g, ''), el.selectionStart, el.selectionEnd, 'end');
}, '/web check https://www.google.com/search?q=hvac\n');
await sleep(1500); await page.press('#input', 'Enter'); await settle(); await sleep(2000);
log.push({ input: 'paste "/web check https://www.google.com/search?q=hvac\\n"', rows: (await rows()).slice(-2) });
// 4. A command typed without its slash gets a button; clicking it inserts, Enter runs.
await type('web check https://www.google.com', { pause: 1500 });
await page.locator('#stream button.cmd-item', { hasText: '/web check https://www.google.com' }).last().click();
await sleep(1200); await page.press('#input', 'Enter'); await settle(); await sleep(1800);
log.push({ input: 'clicked the suggestion, then Enter', rows: (await rows()).slice(-2) });
// 5. The research commands themselves (they reach the network boundary here).
await type('/web search --count 5 mid-tier hvac repair near cumming ga 30041', { pause: 2500, maxMs: 40000 });
await type('/web research --url https://www.google.com/search?q=hvac+repair+cumming+ga+30041 top 2-3 mid-tier hvac repair vendors with offices within 50 miles of 30041; cite sources', { pause: 2500, maxMs: 90000 });
// 6. Goal + loop: /loop auto then /loop 2 runs bounded turns toward the goal.
await type('/loop 2', { pause: 3000, maxMs: 90000 });
await type('/loop stop', { pause: 1500 });
// 7. ArrowUp now recalls every line, including the refused and the slashless ones.
await page.click('#input'); await page.fill('#input', '');
const recalled = [];
for (let i = 0; i < 6; i++) { await page.press('#input', 'ArrowUp'); await sleep(700); recalled.push(await page.inputValue('#input')); }
log.push({ input: 'ArrowUp x6', recalled });
await sleep(1500);
await page.fill('#input', '');
await sleep(800);
await page.screenshot({ path: join(videoDir, 'final.png') });
await context.close();
await browser.close();
await writeFile(logPath, JSON.stringify(log, null, 1));
console.log('demo done');
