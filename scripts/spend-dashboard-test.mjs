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
