import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
class Element {
  constructor(tag = 'div') { this.tag = tag; this.children = []; this.listeners = {}; this.open = true; this.disabled = false; this.text = ''; }
  set textContent(text) { this.text = String(text); this.children = []; }
  get textContent() { return this.text + this.children.map(c => c.textContent).join(''); }
  appendChild(child) { this.children.push(child); return child; }
  replaceChildren(...children) { this.text = ''; this.children = children; }
  setAttribute() {}
  addEventListener(event, callback) { this.listeners[event] = callback; }
  close() { this.open = false; }
  querySelectorAll(tag) { return this.children.flatMap(c => [...(c.tag === tag ? [c] : []), ...c.querySelectorAll(tag)]); }
}
const elements = new Map();
const $ = id => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
const fixture = {
  spend:{complete:false, skipped_rows:1, rates_stale:true,days:[{day:'2026-09-21',provider:'local',model:'<script>literal</script>',calls:1,input_tokens:11,output_tokens:3}]},
  sessions:{sessions:[{title:'<img src=x onerror=alert(1)>',message_count:2,tokens:{prompt_tokens:11,completion_tokens:3,total:14,exchanges:1},created_ts:1789948800}]},
  status:{model:'fixture',provider:'local',total_tokens:14},
  code:{runs:[{run_id:'a',status:'exhausted',iterations:3,changed_file_count:2,reject_code:'VERIFY_FAILED_LOOP'},{run_id:'b',status:'unreadable'}],truncated:true},
  session_days:[{day:'2026-09-21',count:1}],
};
let calls = [], reply = fixture;
const ctx = vm.createContext({$, document:{createElement:tag => new Element(tag)}, Intl, AGENT_CLI_TIMEOUT_MS:130000, api:async (path, method) => {
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
assert.match($('analyticsStatus').textContent, /tokens: 14.*not added/);
assert.equal($('analyticsOutcomes').querySelectorAll('meter').length, 2);
// All three tables are bounded and sessions sort by descending token totals.
reply = {...fixture, sessions:{sessions:Array.from({length:205}, (_,i) => ({title:'row'+i,tokens:{total:i}}))}};
await ctx.refreshAnalytics();
assert.equal($('analyticsSessions').querySelectorAll('td').length, 600);
assert.equal($('analyticsSessions').querySelectorAll('td')[0].textContent, 'row204');
$('analyticsNext').listeners.click();
assert.equal($('analyticsSessions').querySelectorAll('td')[0].textContent, 'row104');
$('analyticsNext').listeners.click();
assert.equal($('analyticsSessions').querySelectorAll('td').length, 30);
assert.equal($('analyticsNext').disabled, true);
// Closing/logout invalidates both stored data and late async completions.
let finish;
ctx.api = () => new Promise(resolve => { finish = resolve; });
const pending = ctx.refreshAnalytics();
ctx.clearAnalytics(); finish(fixture); await pending;
assert.equal($('analyticsDialog').open, false);
assert.equal($('analyticsSessions').textContent, '');
assert.equal($('analyticsStatus').textContent, '');
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
assert.ok(!html.slice(html.indexOf('/* Analytics:'), html.indexOf('/* End analytics. */')).includes('innerHTML'));
console.log('analytics: retained metrics, literal text, pagination, 404 fallback, partial failures, revocation and stale-response clearing passed');
