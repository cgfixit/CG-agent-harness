// Smoother re-take of the operator's reported session: headed Chromium (kiosk)
// on an X display that ffmpeg records at a steady 30 fps. The console is the
// real `cgagentharness serve`; the only page additions are presentation
// overlays (caption pill, cursor, disclosure banner, smooth scrolling).
// Usage: node demo2.mjs <base_url> <home> <log.json> <ready_file>
import { createRequire } from 'node:module';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
const require = createRequire('/opt/node-tools/node_modules/');
const { chromium } = require('playwright');

const [base, home, logPath, readyFile] = process.argv.slice(2);
const W = 1440, H = 900;
const sleep = ms => new Promise(r => setTimeout(r, ms));
// A persistent context so --kiosk applies to the first (only) window: no tabs or URL bar.
// The throwaway profile has the password manager off, so no "Save password?" bubble.
const profile = join(process.argv[3], '..', 'chrome-profile');
await mkdir(join(profile, 'Default'), { recursive: true });
await writeFile(join(profile, 'Default', 'Preferences'), JSON.stringify({ credentials_enable_service: false, profile: { password_manager_enabled: false } }));
const context = await chromium.launchPersistentContext(profile, {
  headless: false, viewport: null,
  executablePath: process.env.CHROME_BIN,
  ignoreDefaultArgs: ['--enable-automation'],
  args: ['--kiosk', `--window-size=${W},${H}`, '--window-position=0,0', '--disable-infobars',
    '--hide-scrollbars', '--force-device-scale-factor=1', '--disable-features=Translate'],
});
const page = context.pages()[0] || await context.newPage();
const log = [];

const overlay = () => page.evaluate(() => {
  if (document.getElementById('demoOverlay')) return;
  // The console's CSP admits styles only with its nonce; borrow the page's own.
  const css = document.createElement('style'); css.id = 'demoOverlay';
  css.nonce = document.querySelector('style[nonce]').nonce;
  css.textContent = `
    #stream { scroll-behavior: smooth; }
    #demoCaption { position: fixed; top: 14px; left: 50%; transform: translate(-50%, -8px); z-index: 100000;
      background: rgba(20,16,36,.88); color: #fff; font: 600 15px/1.35 Inter, system-ui, sans-serif;
      padding: 9px 18px; border-radius: 999px; box-shadow: 0 6px 24px rgba(0,0,0,.25);
      opacity: 0; transition: opacity .35s ease, transform .35s ease; pointer-events: none; max-width: 80vw; text-align: center; }
    #demoCaption.on { opacity: 1; transform: translate(-50%, 0); }
    #demoCaption b { color: #b9a4ff; font-weight: 700; margin-right: 8px; }
    #demoCursor { position: fixed; left: 0; top: 0; z-index: 100001; width: 22px; height: 22px; pointer-events: none;
      transform: translate(720px, 450px); transition: transform .55s cubic-bezier(.33,.0,.2,1); }
    #demoCursor.click::after { content: ''; position: absolute; left: -9px; top: -9px; width: 22px; height: 22px;
      border-radius: 50%; border: 2px solid #8b6cff; animation: demoPing .45s ease-out forwards; }
    @keyframes demoPing { from { opacity: .9; transform: scale(.4); } to { opacity: 0; transform: scale(1.6); } }
    #demoBanner { position: fixed; left: 0; right: 0; bottom: 0; z-index: 100003; background: #2b2140; color: #fff;
      font: 12px/1.4 Inter, system-ui, sans-serif; padding: 6px 12px; opacity: .93; }
    #demoCard { position: fixed; inset: 0; z-index: 100002; display: flex; align-items: center; justify-content: center;
      background: #15111f; color: #fff; font: 15px/1.6 Inter, system-ui, sans-serif; transition: opacity .6s ease; }
    #demoCard h1 { font-size: 30px; margin: 0 0 10px; font-weight: 700; }
    #demoCard p { margin: 4px 0; color: #cfc6e6; }
    #demoCard > div { max-width: 820px; text-align: center; }
    #demoCard.off { opacity: 0; pointer-events: none; }`;
  document.head.appendChild(css);
  const cap = document.createElement('div'); cap.id = 'demoCaption'; document.body.appendChild(cap);
  const cur = document.createElement('div'); cur.id = 'demoCursor';
  cur.innerHTML = '<svg width="22" height="22" viewBox="0 0 22 22"><path d="M3 2l15 8.2-6.6 1.6L8.2 19z" fill="#fff" stroke="#111" stroke-width="1.4" stroke-linejoin="round"/></svg>';
  document.body.appendChild(cur);
  const b = document.createElement('div'); b.id = 'demoBanner';
  b.textContent = 'DEMO · local build of PR #508 (84940db) in Chromium on Linux, not the macOS app · fresh home, empty URL allowlist · model replies come from a deterministic loopback stand-in served as qwen3.8:27b-mlx-cg (no real LLM) · this sandbox has no internet, so live Google and page reads end in a network error';
  document.body.appendChild(b);
});
const card = (title, lines) => page.evaluate(([t, ls]) => {
  let c = document.getElementById('demoCard');
  if (!c) { c = document.createElement('div'); c.id = 'demoCard'; document.body.appendChild(c); }
  c.innerHTML = '<div><h1></h1></div>';
  c.querySelector('h1').textContent = t;
  for (const l of ls) { const p = document.createElement('p'); p.textContent = l; c.firstChild.appendChild(p); }
  c.classList.remove('off');
}, [title, lines]);
const uncard = async () => { await page.evaluate(() => document.getElementById('demoCard')?.classList.add('off')); await sleep(700); };
let stepNo = 0;
const caption = async text => {
  stepNo++;
  await page.evaluate(([n, t]) => {
    const c = document.getElementById('demoCaption');
    c.classList.remove('on');
    setTimeout(() => { c.innerHTML = '<b></b><span></span>'; c.querySelector('b').textContent = n; c.querySelector('span').textContent = t; c.classList.add('on'); }, 250);
  }, [String(stepNo), text]);
  await sleep(700);
};
async function pointAt(locator, { click = true } = {}) {
  const box = await locator.boundingBox();
  if (!box) return;
  const x = Math.round(box.x + Math.min(box.width / 2, 60)), y = Math.round(box.y + box.height / 2);
  await page.evaluate(([x, y]) => { document.getElementById('demoCursor').style.transform = `translate(${x}px, ${y}px)`; }, [x, y]);
  await sleep(650);
  if (click) {
    await page.evaluate(() => { const c = document.getElementById('demoCursor'); c.classList.remove('click'); void c.offsetWidth; c.classList.add('click'); });
    await locator.click();
    await sleep(200);
  }
}
async function settle(maxMs = 30000) {
  const start = Date.now(); let last = -1, stable = Date.now();
  while (Date.now() - start < maxMs) {
    const [n, busy] = await page.evaluate(() => [document.querySelectorAll('#stream .msg').length, document.getElementById('send').disabled]);
    if (n !== last || busy) { last = n; stable = Date.now(); }
    if (Date.now() - stable > 1000) return;
    await sleep(120);
  }
}
const rows = () => page.evaluate(() => [...document.querySelectorAll('#stream .msg')].map(d => d.className.replace('msg ', '') + ': ' + d.innerText.replace(/\s+/g, ' ').slice(0, 400)));
// Natural typing: about 45 ms per key on short lines, faster on long prompts so
// no line takes more than about 5 s; small jitter, short pause after spaces.
async function typeText(text) {
  const per = Math.max(14, Math.min(45, Math.floor(5000 / text.length)));
  for (const ch of text) {
    await page.keyboard.type(ch);
    await sleep(per + Math.floor(Math.random() * per * 0.6) + (ch === ' ' ? per * 0.5 : 0));
  }
}
async function type(line, { pause = 1700, maxMs } = {}) {
  const before = (await rows()).length;
  await page.fill('#input', '');
  await typeText(line);
  await sleep(400);
  await page.keyboard.press('Enter');
  await settle(maxMs);
  await sleep(pause);
  const after = await rows();
  log.push({ input: line, rows: after.slice(Math.min(before, after.length)) });
}

// Sign in and replace the setup code before recording starts.
await page.goto(base + '/');
await page.waitForSelector('#hAuthUser', { state: 'visible', timeout: 60000 });
const setup = (await readFile(join(home, 'setup-code'), 'utf8')).trim();
await page.fill('#hAuthUser', 'admin'); await page.fill('#hAuthPass', setup); await page.click('#hAuthLogin');
await page.waitForFunction(() => document.getElementById('passwordDialog').open, null, { timeout: 30000 });
await page.fill('#passwordCurrent', setup); await page.fill('#passwordNew', 'demo-password-1234'); await page.fill('#passwordConfirm', 'demo-password-1234');
await page.evaluate(() => document.getElementById('passwordForm').requestSubmit());
await page.waitForFunction(() => !document.getElementById('passwordDialog').open, null, { timeout: 30000 });
await page.waitForSelector('#input', { state: 'visible' });
await overlay();
await card('Slash commands, replayed', ['The HVAC session from the bug report, typed into a fresh console', 'CG Agent Harness · PR #508 · fresh home, no URL allowlist']);
await sleep(600);
await writeFile(readyFile, 'go');           // the recorder starts on this
while (existsSync(readyFile)) await sleep(50); // and removes it once ffmpeg runs
await sleep(2600);
await uncard();
await pointAt(page.locator('#input'));

await caption('Start a session, then the model typo from the report');
await type('/session new hvac research');
await type('/model use qwen3.827b-mlx-cg', { pause: 2400 });
await type('/model use qwen3.8:27b-mlx-cg');
await type('yoo');
await caption('Set the goal; one flag lifts fetch-only mode');
await type('/goal find the best hvac repair service thats not too expensive (mid-tier) near cumming, georgia or zip code 30041', { pause: 2200 });
await type('/goal --web all', { pause: 2200 });
await caption('/loop auto now says it starts nothing yet');
await type('/loop auto', { pause: 2200 });
await caption('The research request names the missing allowlist and the fix');
await type('use /web search and /web research to research goal and reply with detailed and unique, customized and citeable findings of the top 2-3 hvac repair vendors that have offices within 50 miles from 30041', { pause: 2800 });
await type('/web status', { pause: 2600 });
await caption('The scheme-less allow the model taught is accepted (https:// added)');
await type('/web allow *.google.com google.com/* --group google', { pause: 3000 });
await type('/web allow https://www.google.com/* https://*.google.com/* --group google', { pause: 2800 });
await caption('A command pasted from a code block keeps working (edge line break trimmed)');
await page.fill('#input', '');
await page.evaluate(t => {
  const dt = new DataTransfer(); dt.setData('text/plain', t);
  const el = document.getElementById('input');
  if (el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }))) el.setRangeText(t.replace(/\n/g, ''), el.selectionStart, el.selectionEnd, 'end');
}, '/web check https://www.google.com/search?q=hvac\n');
await sleep(1600); await page.keyboard.press('Enter'); await settle(); await sleep(2200);
log.push({ input: 'paste "/web check https://www.google.com/search?q=hvac\\n"', rows: (await rows()).slice(-2) });
await caption('Forgot the slash? One click inserts the command');
await type('web check https://www.google.com', { pause: 1400 });
await pointAt(page.locator('#stream button.cmd-item', { hasText: '/web check https://www.google.com' }).last());
await sleep(900); await pointAt(page.locator('#input'), { click: false });
await page.keyboard.press('Enter'); await settle(); await sleep(2000);
log.push({ input: 'clicked the suggestion, then Enter', rows: (await rows()).slice(-2) });
await caption('Search and research run, stopping at this sandbox\'s network boundary');
await type('/web search --count 5 mid-tier hvac repair near cumming ga 30041', { pause: 2600, maxMs: 40000 });
await type('/web research --url https://www.google.com/search?q=hvac+repair+cumming+ga+30041 top 2-3 mid-tier hvac repair vendors with offices within 50 miles of 30041; cite sources', { pause: 2600, maxMs: 90000 });
await caption('Two bounded loop turns toward the goal');
await type('/loop 2', { pause: 3000, maxMs: 90000 });
await type('/loop stop', { pause: 1600 });
await caption('ArrowUp recalls every line, refused and slashless ones included');
await page.fill('#input', '');
const recalled = [];
for (let i = 0; i < 6; i++) { await page.keyboard.press('ArrowUp'); await sleep(900); recalled.push(await page.inputValue('#input')); }
log.push({ input: 'ArrowUp x6', recalled });
await sleep(1600);
await page.fill('#input', '');
await sleep(900);
await card('Every line typed is kept', ['Refused, failed and slashless lines come back with ArrowUp and survive a reload', 'Tested: 10/10 transcript steps, 151/151 command and flag steps']);
await sleep(3800);
await page.screenshot({ path: logPath.replace(/\.json$/, '-final.png') });
await writeFile(readyFile, 'stop');
await sleep(800);
await writeFile(logPath, JSON.stringify(log, null, 1));
await context.close();
console.log('demo done');
