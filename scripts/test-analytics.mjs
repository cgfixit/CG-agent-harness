import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
class Element {
  constructor(tag = 'div') { this.tag = tag; this.children = []; this.listeners = {}; this.open = true; this.disabled = false; this.text = ''; this.attributes = {}; this.value = ''; }
  set textContent(text) { this.text = String(text); this.children = []; }
  get textContent() { return this.text + this.children.map(c => c.textContent).join(''); }
  appendChild(child) { this.children.push(child); return child; }
  replaceChildren(...children) { this.text = ''; this.children = children; }
  setAttribute(key, value) { this.attributes[key] = value; }
  focus() { this.focused = true; }
  showModal() { this.open = true; }
  addEventListener(event, callback) { this.listeners[event] = callback; }
  close() { this.open = false; }
  querySelectorAll(tag) { return this.children.flatMap(c => [...(c.tag === tag ? [c] : []), ...c.querySelectorAll(tag)]); }
}
const elements = new Map();
const $ = id => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
const fixture = {
  spend:{complete:false, skipped_rows:1, rates_stale:true,days:[{day:'2026-09-21',provider:'local',model:'<script>literal</script>',calls:1,input_tokens:11,output_tokens:3}]},
  sessions:{sessions:[{session_id:'abcdef123456',title:'<img src=x onerror=alert(1)>',message_count:2,tokens:{prompt_tokens:11,completion_tokens:3,total:14,exchanges:1},created_ts:1789948800}]},
  status:{model:'fixture',provider:'local',total_tokens:14},
  code:{runs:[{run_id:'a',status:'exhausted',iterations:3,changed_file_count:2,reject_code:'VERIFY_FAILED_LOOP'},{run_id:'b',status:'unreadable'}],truncated:true},
  session_days:[{day:'2026-09-21',count:1}],
};
let calls = [], reply = fixture;
const ctx = vm.createContext({$, input:$('chatInput'), document:{createElement:tag => new Element(tag)}, Intl, AGENT_CLI_TIMEOUT_MS:130000, api:async (path, method) => {
  calls.push([path,method]); return reply;
}});
vm.runInContext(html.slice(html.indexOf('function table('), html.indexOf('function renderToolsDiagram(')), ctx);
vm.runInContext(html.slice(html.indexOf('function spendSummaryView('), html.indexOf('let spendRequest =')), ctx);
vm.runInContext(html.slice(html.indexOf('/* Analytics:'), html.indexOf('/* End analytics. */')), ctx);
await ctx.refreshAnalytics();
assert.deepEqual(calls, [['/api/analytics/summary','GET']]);
assert.match($('analyticsTokensStatus').textContent, /partial.*Skipped rows: 1.*STALE/);
assert.match($('analyticsTokens').textContent, /<script>literal<\/script>.*Unpriced \(local\)/);
assert.match($('analyticsSessions').textContent, /<img src=x onerror=alert\(1\)>/);
assert.match($('analyticsCodeStatus').textContent, /Partial.*Iterations: Unknown/);
assert.equal($('analyticsTotalTokens').textContent, '14');
assert.equal($('analyticsTotalSessions').textContent, '1');
assert.equal($('analyticsTotalRuns').textContent, '2+');
assert.equal($('analyticsDialog').attributes['aria-busy'], 'false');
assert.equal($('analyticsOutcomes').querySelectorAll('meter').length, 2);
// Each section keeps its page/filter; short tables never disappear when sessions page.
reply = {...fixture, sessions:{sessions:Array.from({length:55}, (_,i) => ({session_id:'id'+i,title:'row'+i,tokens:{total:i},created_ts:1789948800-i*86400}))}};
await ctx.refreshAnalytics();
$('analyticsSessionsTab').listeners.click();
assert.equal($('analyticsSessions').querySelectorAll('td').length, 175);
assert.match($('analyticsSessions').querySelectorAll('td')[0].textContent, /^row54/);
$('analyticsNext').listeners.click();
assert.match($('analyticsSessions').querySelectorAll('td')[0].textContent, /^row29/);
$('analyticsTokensTab').listeners.click();
assert.equal($('analyticsTokens').querySelectorAll('td').length, 7);
assert.equal($('analyticsNext').disabled, true);
$('analyticsSessionsTab').listeners.click();
assert.match($('analyticsPage').textContent, /Page 2 of 3/);
$('analyticsNext').listeners.click();
assert.equal($('analyticsSessions').querySelectorAll('td').length, 35);
assert.equal($('analyticsNext').disabled, true);
$('analyticsFilter').value = 'id42'; $('analyticsFilter').listeners.input();
assert.match($('analyticsSessions').textContent, /row42/);
assert.match($('analyticsPage').textContent, /1–1 of 1.*filtered from 55/);
$('analyticsCodeTab').listeners.click();
assert.equal($('analyticsFilter').value, '');
$('analyticsSessionsTab').listeners.click();
assert.equal($('analyticsFilter').value, 'id42');
$('analyticsFilter').value = 'no such record'; $('analyticsFilter').listeners.input();
assert.equal($('analyticsEmpty').hidden, false);
assert.match($('analyticsEmpty').textContent, /No matching records/);
$('analyticsClearFilter').listeners.click();
assert.equal($('analyticsFilter').value, '');
$('analyticsSort').value = 'recent'; $('analyticsSort').listeners.change();
assert.match($('analyticsSessions').querySelectorAll('td')[0].textContent, /^row0 /);
$('analyticsSort').value = 'title'; $('analyticsSort').listeners.change();
assert.match($('analyticsSessions').querySelectorAll('td')[14].textContent, /^row10 /);
// Arrow keys/Home/End move the active tab and focus; only one tab enters the tab order.
let prevented = false;
$('analyticsSessionsTab').listeners.keydown({key:'End',preventDefault(){prevented=true;}});
assert.equal(prevented, true);
assert.equal($('analyticsCodeTab').attributes['aria-selected'], 'true');
assert.equal($('analyticsCodeTab').focused, true);
assert.equal($('analyticsSessionsTab').tabIndex, -1);
assert.equal($('analyticsSessionsPanel').hidden, true);
// Refresh keeps filters and clamps a no-longer-existing page after retained data shrinks.
$('analyticsSessionsTab').listeners.click();
$('analyticsNext').listeners.click();
reply = fixture; await ctx.refreshAnalytics();
assert.match($('analyticsPage').textContent, /Page 1 of 1/);
reply = {...fixture, sessions:{sessions:[]},code:{runs:[],truncated:false}}; await ctx.refreshAnalytics();
assert.match($('analyticsEmpty').textContent, /No readable sessions/);
assert.equal($('analyticsTotalSessions').textContent, '0');
assert.equal($('analyticsTotalRuns').textContent, '0');
// Closing/logout invalidates both stored data and late async completions.
let finish;
ctx.api = () => new Promise(resolve => { finish = resolve; });
$('analyticsFilter').value = 'private query'; $('analyticsFilter').listeners.input();
const pending = ctx.refreshAnalytics();
assert.equal($('refreshAnalytics').disabled, true);
assert.equal($('analyticsDialog').attributes['aria-busy'], 'true');
ctx.clearAnalytics(); finish(fixture); await pending;
assert.equal($('analyticsDialog').open, false);
assert.equal($('analyticsSessions').textContent, '');
assert.equal($('analyticsStatus').textContent, '');
assert.equal($('analyticsFilter').value, '');
assert.equal($('analyticsTotalTokens').textContent, '');
assert.equal($('refreshAnalytics').disabled, false);
const logout = html.slice(html.indexOf("if (hAuthLogout) {"), html.indexOf("if (hAuthSetupBtn) {"));
assert.ok(logout.indexOf('clearAnalytics();') < logout.indexOf("fetchWithTimeout('/api/auth/logout'"));
// Latest refresh wins even while the dialog remains open.
$('analyticsDialog').open = true;
let finishOld;
ctx.api = () => new Promise(resolve => { finishOld = resolve; });
const old = ctx.refreshAnalytics();
ctx.api = async () => fixture;
await ctx.refreshAnalytics();
finishOld({...fixture,status:{model:'STALE'}}); await old;
assert.doesNotMatch($('analyticsStatus').textContent, /STALE/);
assert.equal($('analyticsTotalTokens').textContent, '14');
// Only an unsupported aggregate (404) falls back, in parallel, to the four GET sources.
calls = []; const releases = [];
ctx.api = (path, method) => {
  calls.push([path,method]);
  if (path === '/api/analytics/summary') return Promise.reject(Object.assign(new Error('HTTP 404'),{status:404}));
  return new Promise(resolve => releases.push(resolve));
};
const fallback = ctx.refreshAnalytics();
await new Promise(setImmediate);
assert.equal(releases.length, 4);
[fixture.spend, fixture.sessions, fixture.status, {ok:true,parsed:fixture.code}].forEach((v,i) => releases[i](v));
await fallback;
assert.deepEqual(calls.slice(1), [['/api/spend/summary','GET'],['/api/sessions','GET'],['/api/status','GET'],['/api/agent/runs','GET']]);
assert.match($('analyticsDays').textContent, /2026-09-21: 1/);
for (const status of [401,403,429,500]) {
  calls = []; ctx.api = async path => { calls.push(path); throw Object.assign(new Error('denied'),{status}); };
  await ctx.refreshAnalytics(); assert.equal(calls.length, 1); assert.equal($('analyticsSessions').textContent, '');
}
ctx.api = async path => {
  if (path === '/api/analytics/summary') throw Object.assign(new Error('404'),{status:404});
  if (path === '/api/agent/runs') throw new Error('coding disabled');
  return path === '/api/spend/summary' ? fixture.spend : path === '/api/sessions' ? fixture.sessions : fixture.status;
};
await ctx.refreshAnalytics();
assert.match($('analyticsCodeStatus').textContent, /Unavailable: coding disabled/);
assert.match($('analyticsSessionsStatus').textContent, /1 readable/);
// Revocation during fallback clears all sections, even when other reads succeeded.
ctx.api = async path => {
  if (path === '/api/analytics/summary') throw Object.assign(new Error('404'),{status:404});
  if (path === '/api/agent/runs') throw Object.assign(new Error('revoked'),{status:403});
  return fixture.sessions;
};
await ctx.refreshAnalytics();
assert.match($('analyticsStatus').textContent, /revoked/);
assert.equal($('analyticsSessions').textContent, '');
assert.equal(ctx.analyticsCode({ok:false,parsed:{runs:[]}}).runs, undefined);
assert.equal(ctx.analyticsSessionDays([{created_ts:0},{}]).length, 0, 'legacy missing dates are unknown');
// Native WebKit does not consistently return focus after dialog.close().
ctx.api = async () => fixture;
ctx.clearAnalytics();
await ctx.openAnalytics($('openAnalytics')); ctx.clearAnalytics();
assert.equal($('openAnalytics').focused, true);
await ctx.openAnalytics(); ctx.clearAnalytics();
assert.equal($('chatInput').focused, true);
assert.equal(vm.runInContext('analyticsCreated({})', ctx), 'Unknown');
assert.equal(vm.runInContext('analyticsCreated({created_ts:0})', ctx), 'Unknown');
assert.equal(vm.runInContext('analyticsCreated({created_ts:Infinity})', ctx), 'Unknown');
assert.ok(!html.slice(html.indexOf('/* Analytics:'), html.indexOf('/* End analytics. */')).includes('innerHTML'));
console.log('analytics: retained metrics, literal text, independent paging/filtering/sorting, keyboard tabs, empty/loading states, 404 fallback, revocation and stale-response clearing passed');
