// Every console command family and its documented flags, driven through the real UI.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const text = ms => ms.map(m => m.kind + ': ' + m.body + (m.buttons.length ? ' [buttons: ' + m.buttons.join(' | ') + ']' : '')).join('\n');
// Markers that mean the console never ran a command it should have.
const PARSER_FAIL = /command not dispatched|unknown command|slash parsing unavailable|Unsupported flag|unknown subcommand|Did not dispatch/i;
const modelCalls = () => readFileSync(process.env.MODEL_LOG, 'utf8').trim().split('\n').map(l => JSON.parse(l))
  .filter(r => r.path.endsWith('/chat/completions') && Array.isArray(r.body.tools));
const lastChat = () => modelCalls().at(-1).body;
const steps = [];
// run one line; `has` and `not` are regexes checked against the new transcript rows.
const cmd = (id, area, line, { has = [], not = [], parserOk = true, quietMs, maxMs } = {}) => steps.push({
  id, area, input: line, run: async a => {
    const out = text(await a.send(line, { quietMs, maxMs }));
    if (parserOk) assert.doesNotMatch(out, PARSER_FAIL, 'parser refused a documented form');
    for (const r of has) assert.match(out, r);
    for (const r of not) assert.doesNotMatch(out, r);
    return out;
  },
});
const custom = (id, area, input, run) => steps.push({ id, area, input, run });

// ── meta and help
cmd('H01', 'help', '/help', { has: [/Command guide/] });
cmd('H02', 'help', '/help web', { has: [/Web research/] });
cmd('H03', 'help', '/help all', { has: [/\/web allow/] });
cmd('H04', 'help', '/web --help', { has: [/Web research/] });
cmd('H05', 'help', '/model HELP', { has: [/Chat & models|\/model/] });
cmd('H06', 'help', '/memory search --help', { has: [/Memory & persona/] });
cmd('H07', 'meta', '/status', { has: [/model|session/i] });
cmd('H08', 'meta', '/tokens', { has: [/token|no active session/i] });
cmd('H09', 'meta', '/prompt');
cmd('H10', 'meta', '/tools', { not: [/\/model pull\b/] });
cmd('H11', 'meta', '/tools all', { has: [/\/model list/], not: [/\/model inventory|\/model pull/] });
cmd('H12', 'meta', '/skills');
cmd('H13', 'meta', '/connectors');
cmd('H14', 'meta', '/registry');
cmd('H15', 'meta', '/harness');
cmd('H16', 'meta', '/github');
cmd('H17', 'meta', '/analytics');
custom('H18', 'meta', '/users', async a => {
  await a.page.keyboard.press('Escape'); // /analytics (H17) leaves its modal dialog open
  await a.page.evaluate(() => document.querySelectorAll('dialog[open]').forEach(d => d.close()));
  await a.send('/users');
  await a.page.waitForFunction(() => document.getElementById('stream').innerText.includes('users panel'), null, { timeout: 15000 });
  return 'users panel opened';
});
cmd('H19', 'meta', '/net status');
cmd('H20', 'meta', '/net devices');

// ── sessions
cmd('S01', 'session', '/session new hvac research', { has: [/session created/] });
cmd('S02', 'session', '/session info');
cmd('S03', 'session', '/session list');
cmd('S04', 'session', '/session rename HVAC near Cumming');
cmd('S05', 'session', '/session unpin', { has: [/Cleared 0 attachment pin/] });

// ── model and style
cmd('M01', 'model', '/model', { has: [/model/i] });
cmd('M02', 'model', '/model list', { has: [/qwen3\.8:27b-mlx-cg/] });
cmd('M03', 'model', '/model use fake-alt:7b', { has: [/chat model selected: fake-alt:7b/] });
cmd('M04', 'model', '/model use qwen3.8:27b-mlx-cg', { has: [/chat model selected: qwen3\.8:27b-mlx-cg/] });
cmd('M05', 'model', '/model profile');
cmd('M06', 'model', '/model qwen3:8b', { parserOk: false, has: [/\/model use qwen3:8b/], not: [/chat model selected/] });
cmd('M07', 'model', '/model use', { has: [/usage: \/model use/] });
cmd('M08', 'model', '/model use --help', { parserOk: false, not: [/chat model selected/] });
cmd('M09', 'style', '/style', { has: [/available: .*concise/] });
cmd('M10', 'style', '/style concise', { has: [/style concise/] });
cmd('M11', 'style', '/style bogus', { has: [/Available: /] });
cmd('M12', 'style', '/style off', { has: [/style off/] });

// ── API keys: values never reach history or the model
cmd('K01', 'api', '/api', { has: [/credential store/] });
cmd('K02', 'api', '/api set SERPAPI_API_KEY fixture-serp-secret-123', { has: [/applies now/], not: [/fixture-serp-secret-123/] });
cmd('K03', 'web', '/web status', { has: [/Google via SerpAPI — \/web search needs no URL rule/] });
cmd('K04', 'api', '/api clear SERPAPI_API_KEY', { has: [/applies now/] });
cmd('K05', 'api', 'api set GROK_API_KEY fixture-grok-secret-456', { parserOk: false, has: [/not sent: this looks like (an )?\/api/], not: [/^user:/m] });
custom('K06', 'api', 'secrets never enter history or model requests', async a => {
  const log = readFileSync(process.env.MODEL_LOG, 'utf8');
  assert.doesNotMatch(log, /fixture-(serp|grok)-secret/);
  const hist = await a.page.evaluate(() => promptHistory.prompts);
  assert.ok(!hist.some(h => /fixture-(serp|grok)-secret/.test(h)), JSON.stringify(hist));
  return hist.filter(h => /api/i.test(h));
});

// ── memory, including automatic retrieval
cmd('Y01', 'memory', '/memory', { has: [/notes|memory/i] });
cmd('Y02', 'memory', '/memory status');
cmd('Y03', 'memory', '/memory add prefers morning appointments');
cmd('Y04', 'memory', '/memory off');
cmd('Y05', 'memory', '/memory on');
cmd('Y06', 'memory', '/memory clear', { has: [/memory notes cleared; 0 remain/] });
cmd('Y07', 'memory', '/memory save Preferred HVAC contractor is Acme Heating in Cumming GA :: standing preference', { has: [/saved|fact/i] });
cmd('Y08', 'memory', '/memory facts', { has: [/Acme Heating/] });
cmd('Y09', 'memory', '/memory search hvac contractor', { has: [/Acme Heating/] });
cmd('Y10', 'memory', '/memory retrieve which hvac contractor do I prefer', { has: [/memory: included 1 saved fact/] });
custom('Y11', 'memory', 'auto-retrieval injects the fact into a plain chat turn', async a => {
  const out = text(await a.send('remind me which hvac contractor I like'));
  assert.match(out, /memory: included 1 saved fact/);
  assert.match(JSON.stringify(lastChat().messages[0]), /Acme Heating/);
  return out;
});
cmd('Y12', 'memory', '/memory proposals');
cmd('Y13', 'memory', '/memory consolidate', { has: [/usage: \/memory consolidate/] });
cmd('Y14', 'memory', '/memory list', { parserOk: false, has: [/\/memory facts/] });
for (const gate of ['capture', 'recall', 'retrieval', 'auto-retrieve', 'consolidation', 'auto-consolidate', 'auto-suggest-chat', 'auto-suggest-coding']) {
  cmd('YG-' + gate + '-off', 'memory', '/memory ' + gate + ' off');
  cmd('YG-' + gate + '-on', 'memory', '/memory ' + gate + ' on');
}
cmd('P01', 'soul', '/soul', { has: [/"enabled"/] });
cmd('P02', 'soul', '/soul status');
cmd('P03', 'soul', '/soul off');
cmd('P04', 'soul', '/soul on');
cmd('P05', 'soul', '/soul history');
cmd('P06', 'soul', '/soul review', { has: [/usage: \/soul review <id>/] });

// ── goals and loops
cmd('G01', 'goal', '/goal', { has: [/no goal set/] });
cmd('G02', 'goal', '/goal find the best hvac repair service near cumming, georgia 30041', { has: [/goal set \(web: fetch\)/, /Google search is off/] });
custom('G03', 'goal', 'fetch mode offers only web_fetch and tells the model how to restore search', async a => {
  const out = text(await a.send('search the web for hvac repair near 30041'));
  const req = lastChat();
  assert.deepEqual(req.tools.map(t => t.function.name), ['web_fetch']);
  assert.match(req.messages[0].content, /\/goal --web all/);
  return out;
});
cmd('G04', 'goal', '/goal --web all', { has: [/goal set \(web: all\)/, /find the best hvac repair/] });
custom('G05', 'goal', 'all mode offers web_search again', async a => {
  const out = text(await a.send('search the web for hvac repair near 30041'));
  assert.deepEqual(lastChat().tools.map(t => t.function.name).sort(), ['web_fetch', 'web_search']);
  return out;
});
cmd('G06', 'goal', '/goal Task list for hvac vendors', { has: [/goal set \(web: fetch\):\nTask list for hvac vendors/] });
cmd('G07', 'goal', '/goal --web fetch find the best hvac repair service near cumming, georgia 30041', { has: [/goal set \(web: fetch\)/] });
cmd('L01', 'loop', '/loop auto', { has: [/nothing is running yet/] });
cmd('L02', 'loop', '/loop auto', { has: [/OFF/] });
cmd('L03', 'loop', '/loop --web all 1', { has: [/goal web mode all/], maxMs: 60000 });
custom('L04', 'loop', 'loop turns retrieve memory by the goal, and stay out of ArrowUp history', async a => {
  const loopReq = modelCalls().filter(r => /Loop turn/.test(JSON.stringify(r.body.messages.at(-1)))).at(-1);
  assert.ok(loopReq, 'a loop turn reached the model');
  assert.match(loopReq.body.messages[0].content, /Acme Heating/, 'the goal-matching fact was retrieved');
  const hist = await a.page.evaluate(() => promptHistory.prompts);
  assert.ok(!hist.some(h => h.startsWith('Loop turn')), JSON.stringify(hist));
  return 'loop request carried the fact; history: ' + JSON.stringify(hist.slice(-4));
});
cmd('L05', 'loop', '/loop --web fetch 1', { has: [/goal web mode fetch/], maxMs: 60000 });
cmd('L06', 'loop', '/loop --web bogus', { parserOk: false, has: [/usage: \/loop/] });
cmd('L07', 'loop', '/loop 0', { parserOk: false, has: [/usage: \/loop/] });
cmd('L08', 'loop', '/loop stop');
cmd('L09', 'loop', '/goal clear', { has: [/goal cleared/] });
cmd('L10', 'loop', '/loop', { has: [/set a goal first/] });

// ── web
cmd('W01', 'web', '/web status', { has: [/permission rules\s+0 of 64/, /needs \/web allow https:\/\/www\.google\.com/] });
cmd('W02', 'web', '/web allow *.google.com google.com/* --group google', { has: [/permits only the root page https:\/\/\*\.google\.com\//, /permission rules\s+2 of 64/] });
cmd('W03', 'web', '/web allow https://www.google.com/*', { has: [/permission rules\s+3 of 64/] });
cmd('W04', 'web', '/web check www.google.com', { has: [/Permitted by URL policy/] });
cmd('W05', 'web', '/web check https://www.google.com/search?q=hvac', { has: [/Permitted by URL policy/] });
cmd('W06', 'web', '/web check https://maps.google.com/x', { has: [/Denied/] });
cmd('W07', 'web', '/web allow https://www.google.com, https://google.com/*', { parserOk: false, has: [/the host contains ','/] });
cmd('W08', 'web', '/web allow /*.google.com', { parserOk: false, has: [/Remove the leading \//] });
cmd('W09', 'web', '/web deny https://*.google.com/* --group google', { parserOk: false, has: [/with no flags/] });
cmd('W10', 'web', '/web deny https://never.example.org/*', { has: [/No rule has that ID or pattern/] });
cmd('W11', 'web', '/web deny *.google.com', { has: [/permission rules\s+2 of 64/] });
cmd('W12', 'web', '/web off', { has: [/no \(fail-closed\)/] });
cmd('W13', 'web', '/web on', { has: [/enabled\s+yes/] });
cmd('W14', 'web', '/web inject', { has: [/No saved extract yet/] });
cmd('W15', 'web', '/web forget');
cmd('W16', 'web', '/web cancel');
cmd('W17', 'web', '/web fetch https://example.org/', { has: [/outside the current shared-home allowlist|WEB_HOST_DENIED/] });
cmd('W18', 'web', '/web search --count 3 hvac repair cumming ga', { maxMs: 30000 });
cmd('W19', 'web', '/web pages --group google hvac repair', { maxMs: 30000 });
cmd('W20', 'web', '/web search --engine pages --group google hvac', { maxMs: 30000 });
cmd('W21', 'web', '/web research --url www.google.com which hvac repair is mid-tier near 30041', { maxMs: 90000 });

// ── agent family: documented forms dispatch; execution gates stay closed
for (const [id, line] of [['A01', '/agent'], ['A02', '/agent help'], ['A03', '/agent checks'], ['A04', '/agent jobs'], ['A05', '/agent runs'],
  ['A06', '/agent plan'], ['A07', '/agent iterations 2'], ['A08', '/agent cancel'], ['A09', '/agent status abcdef0123456789'],
  ['A10', '/agent run claude/hvac Compare hvac vendors'], ['A11', '/agent stop'], ['A12', '/agent job'],
  ['A13', '/skill status'], ['A14', '/skill clear'], ['A15', '/goal stage claude/hvac']]) cmd(id, 'agent', line);

// ── composer behaviour
custom('I01', 'input', 'command typed without its leading slash gets a non-executing hint', async a => {
  const out = text(await a.send('web check https://www.google.com'));
  assert.match(out, /has no leading \//);
  assert.match(out, /\[buttons: \/web check https:\/\/www\.google\.com\]/);
  return out;
});
custom('I02', 'input', 'paste of a code-block line keeps the command, minus its line break', async a => {
  const r = await a.paste('\n/web status\n');
  assert.equal(r.value, '/web status');
  assert.doesNotMatch(text(r.added), /paste refused/);
  return r;
});
custom('I03', 'input', 'multi-line slash paste is still refused', async a => {
  const r = await a.paste('/web allow https://example.org/*\n/web on');
  assert.equal(r.value, '');
  assert.match(text(r.added), /paste refused/);
  await a.page.fill('#input', '');
  return r;
});
custom('I04', 'input', 'busy gate says so instead of dropping the line', async a => {
  await a.page.fill('#input', 'SLOWREPLY please');
  await a.page.press('#input', 'Enter');
  await a.sleep(400);
  await a.page.fill('#input', '/status');
  await a.page.press('#input', 'Enter');
  await a.sleep(300);
  const out = text(await a.messages());
  assert.match(out, /still working on the current request/);
  assert.equal(await a.page.inputValue('#input'), '/status');
  await a.settle(900, 20000);
  await a.page.fill('#input', '');
  return 'notice shown; line kept';
});
custom('I05', 'history', 'ArrowUp recalls refused, failed and slash lines; history survives reload', async a => {
  await a.send('/web allow *.bad host');
  const seen = [];
  await a.page.fill('#input', '');
  for (let i = 0; i < 4; i++) seen.push(await a.arrow('ArrowUp'));
  assert.equal(seen[0], '/web allow *.bad host');
  const session = await a.page.evaluate(() => currentSession);
  await a.reload();
  await a.send('/session use ' + session);
  await a.page.fill('#input', '');
  const afterReload = [];
  for (let i = 0; i < 3; i++) afterReload.push(await a.arrow('ArrowUp'));
  assert.ok(afterReload.includes('/web allow *.bad host'), JSON.stringify(afterReload));
  return { beforeReload: seen, afterReload };
});
cmd('Z01', 'clear', '/loop auto');
custom('Z02', 'clear', '/clear also turns loop auto off', async a => {
  await a.send('/clear');
  assert.equal(await a.page.evaluate(() => loopAuto), false);
  return 'loopAuto=false';
});

// ── later verifier findings
cmd('V01', 'goal', '/goal --web=all find hvac', { parserOk: false, has: [/a space, not =/] });
cmd('V02', 'goal', '/goal Stage the rollout of the scheduler', { parserOk: false, has: [/reword it/] });
cmd('V03', 'agent', '/agent run fix/login-timeout Return 408 on upstream timeout', { has: [/branch must be <prefix>\/<topic>/] });
cmd('V04', 'agent', '/goal stage hvac-research', { has: [/branch must be <prefix>\/<topic>|Set \/goal <text> first/] });
cmd('V05', 'agent', '/agent schedules', { parserOk: false, has: [/unknown \/agent subcommand/] });
cmd('V06', 'soul', '/soul enable', { parserOk: false, has: [/unknown \/soul subcommand/] });
cmd('V07', 'meta', '/net help', { has: [/net|Network|LAN/i] });
cmd('V08', 'web', '/web check https://localhost/', { parserOk: false, has: [/https:\/\/localhost\/: local or metadata host refused/] });
cmd('V09', 'api', '`/api set GROK_API_KEY fixture-quoted-secret-789`', { parserOk: false, has: [/not sent: this looks like an \/api command/], not: [/^user:/m] });
cmd('V11', 'history', '/web fetch https://admin:hunter2@intranet.example.com/x', { parserOk: false });
custom('V12', 'history', 'URL credentials and loop framing never reach saved history', async a => {
  const sid = await a.page.evaluate(() => currentSession);
  await a.sleep(800);
  const hist = (await a.fetchJson('/api/sessions/' + sid)).json.prompt_history;
  assert.ok(!hist.some(h => /hunter2|^Loop turn/.test(h)), JSON.stringify(hist.slice(-6)));
  assert.ok(hist.includes('/web fetch https://intranet.example.com/x'), JSON.stringify(hist.slice(-6)));
  return hist.slice(-3);
});
custom('V10', 'history', 'a chat line starting with API is kept whole; tokens never kept', async a => {
  await a.send('API gateway vs service mesh for the scheduler?');
  await a.send('/api set fake-token-not-a-key-name');
  const sid = await a.page.evaluate(() => currentSession);
  await a.sleep(800);
  const hist = (await a.fetchJson('/api/sessions/' + sid)).json.prompt_history.slice(-4);
  assert.ok(hist.includes('API gateway vs service mesh for the scheduler?'), JSON.stringify(hist));
  assert.ok(!hist.some(h => /ghp_|fixture-quoted-secret/.test(h)), JSON.stringify(hist));
  assert.doesNotMatch(readFileSync(process.env.MODEL_LOG, 'utf8'), /fixture-quoted-secret|ghp_16C7/);
  return hist;
});

export default steps;
