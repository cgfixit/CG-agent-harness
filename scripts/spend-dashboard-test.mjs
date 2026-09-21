import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('function spendSummaryView('), html.indexOf('let spendRequest ='));
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
assert.equal(rows[1][6], '$0.00');
assert.equal(rows[2][6], 'Unpriced / incomplete');
assert.ok(html.includes('td.textContent = String(value'));
console.log('spend dashboard: completeness, unknown/local costs, stale rates, literal text passed');

const elements = Object.fromEntries(['predictSpend', 'spendPrediction', 'spendDialog'].map(id => [id, {textContent:'', disabled:false, open:true}]));
const input = {value:'a draft'};
let response = {model:'grok-4.6', usd:0.01, input_tokens:2, reserved_output_tokens:16, estimate_source:'heuristic', max_usd_per_call:0.005, budget_applies:true, budget_exceeded:true, priced_as_of:'2026-09-19'};
let calls = 0;
const predictionContext = vm.createContext({input, $: id => elements[id], api: async (path, method, body) => {
  assert.equal(path, '/api/spend/predict'); assert.equal(method, 'POST'); assert.equal(body.message, 'a draft'); ++calls; return response;
}});
vm.runInContext(html.slice(html.indexOf('let spendPredictionRequest ='), html.indexOf('function paintSpendPage(')), predictionContext);
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
const pending = predictionContext.predictSpend();
input.value = 'changed draft';
complete(response); await pending;
assert.match(elements.spendPrediction.textContent, /Draft changed/);
console.log('spend prediction: estimate sources, cap decisions, unknown rates, command refusal and stale drafts passed');
