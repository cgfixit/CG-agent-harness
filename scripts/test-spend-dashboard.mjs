import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const between = (start, end) => {
  const from = html.indexOf(start), to = html.indexOf(end, from);
  assert.ok(from >= 0 && to > from, 'console markers must bound the slice: ' + start + ' … ' + end);
  return html.slice(from, to);
};
const source = between('function spendSummaryView(', 'let spendPredictionRequest =');
const context = vm.createContext({Intl});
vm.runInContext(source, context);
const view = data => context.spendSummaryView(data);
assert.match(view({}).status, /completeness unknown/);
assert.match(view({complete:false, skipped_rows:3, rates_stale:true}).status, /partial.*Skipped rows: 3.*STALE.*not proof of zero spend/);
assert.match(view({complete:true,days:[]}).status, /No inference rows/);
const rows = view({complete:true,days:[
  {day:'2026-09-20',provider:'local',model:'<script>alert(1)</script>',calls:1,estimate:{usd:99}},
  {provider:'grok',estimate:{usd:0}}, {provider:'claude',estimate:{usd:null}},
]}).rows;
assert.equal(rows[0][2], '<script>alert(1)</script>');
assert.equal(rows[0][4], 'Unknown');
assert.equal(rows[0][6], 'Unpriced (local)');
assert.equal(rows[1][0], 'Unknown', 'a missing day renders as Unknown, not blank');
assert.equal(rows[1][2], 'Unknown', 'a missing model renders as Unknown, not blank');
assert.equal(rows[1][6], '$0.00');
assert.equal(rows[2][6], 'Unpriced / incomplete');
// One ledger renderer: Analytics' Tokens and cost tab, through the shared text-only table builder.
const cells = [];
const cell = tag => ({tag, children:[], appendChild(child) { this.children.push(child); return child; },
  set textContent(value) { cells.push(value); }, set innerHTML(value) { throw new Error('ledger cells must never be parsed as HTML'); }});
const tableContext = vm.createContext({document:{createElement:cell}});
vm.runInContext(between('function table(', 'function renderToolsDiagram('), tableContext);
tableContext.table(rows, ['UTC day','Provider','Model','Calls','Input tokens','Output tokens','USD']);
assert.ok(cells.includes('<script>alert(1)</script>'), 'model names render as literal text');
const analytics = between('/* Analytics:', '/* End analytics. */');
assert.ok(analytics.includes('spendSummaryView(data.spend || {})') && analytics.includes("analyticsTable('analyticsTokens', spend.rows"));
assert.ok(analytics.includes('const node = table('));
assert.ok(!html.includes('spendDialog') && !html.includes('id="openSpend"'), 'no second, divergent ledger view');
console.log('spend dashboard: completeness, unknown/local costs, stale rates, literal text passed');

const elements = Object.fromEntries(['predictSpend', 'spendPrediction', 'analyticsDialog'].map(id => [id, {textContent:'', disabled:false, open:true}]));
const input = {value:'a draft'};
let response = {model:'grok-4.6', usd:0.01, input_tokens:2, reserved_output_tokens:16, estimate_source:'heuristic', max_usd_per_call:0.005, budget_applies:true, budget_exceeded:true, priced_as_of:'2026-09-19'};
let calls = 0;
const predictionContext = vm.createContext({input, $: id => elements[id], api: async (path, method, body) => {
  assert.equal(path, '/api/spend/predict'); assert.equal(method, 'POST'); assert.equal(body.message, 'a draft'); ++calls; return response;
}});
vm.runInContext(between('let spendPredictionRequest =', '/* End spend helpers. */'), predictionContext);
await predictionContext.predictSpend();
assert.match(elements.spendPrediction.textContent, /\$0.010000.*may undercount CJK.*generation would be refused/);
assert.equal(elements.predictSpend.disabled, false);
response = {...response, model:'<script>literal model</script>', estimate_source:'vendor_count', budget_exceeded:false};
await predictionContext.predictSpend();
assert.match(elements.spendPrediction.textContent, /<script>literal model<\/script>.*Provider token estimate.*Within configured cap/);
response = {...response, usd:null, budget_applies:false};
await predictionContext.predictSpend();
assert.match(elements.spendPrediction.textContent, /Unpriced.*cap does not apply/);
input.value = '/agent private instruction';
await predictionContext.predictSpend();
assert.equal(calls, 3);
let complete;
predictionContext.api = () => new Promise(resolve => { complete = resolve; });
input.value = 'a draft';
let pending = predictionContext.predictSpend();
input.value = 'changed draft';
complete(response); await pending;
assert.match(elements.spendPrediction.textContent, /Draft changed/);
// The estimate belongs to the open Analytics dialog: closing it discards a late reply.
input.value = 'a draft'; elements.spendPrediction.textContent = '';
pending = predictionContext.predictSpend();
assert.equal(elements.predictSpend.disabled, true);
elements.analyticsDialog.open = false;
complete(response); await pending;
assert.doesNotMatch(elements.spendPrediction.textContent, /literal model/);
assert.equal(elements.predictSpend.disabled, false);
// Clearing (close, Escape, login and logout via clearAnalytics) invalidates an in-flight estimate.
elements.analyticsDialog.open = true;
pending = predictionContext.predictSpend();
predictionContext.clearSpendPrediction();
assert.equal(elements.spendPrediction.textContent, '');
assert.equal(elements.predictSpend.disabled, false);
complete(response); await pending;
assert.equal(elements.spendPrediction.textContent, '', 'a cleared estimate cannot be repainted by its late reply');
console.log('spend prediction: estimate sources, cap decisions, unknown rates, command refusal, stale drafts and dialog-bound clearing passed');
