// Exercise the shipped header refresh, including out-of-order network replies.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const nodes = new Map();
const $ = id => { if (!nodes.has(id)) nodes.set(id, {textContent:'',style:{}}); return nodes.get(id); };
let session = {tokens:{total:0}}, fail = false;
const ctx = vm.createContext({$, Number, currentSession:'new', sessionRevision:0,
  statusTimer:null, statusBackoffMs:15000, STATUS_BASE_INTERVAL:15000, STATUS_MAX_INTERVAL:120000,
  apiKeyInput:null, window:{}, soulEnabled:false, soulStatus:{}, memoryEnabled:false,
  paintSoul(){}, paintMemory(){}, paintStyle(){}, refreshStyle:async()=>{}, scheduleStatusRefresh(){},
  api:async path => { if (path === '/api/status') return {total_tokens:999}; if (fail) throw Error('unavailable'); return session; },
});
const helperStart = html.indexOf('let sessionTokenRevision =');
if (helperStart !== -1) vm.runInContext(html.slice(helperStart, html.indexOf('async function refreshStatus()', helperStart)), ctx);
vm.runInContext(html.slice(html.indexOf('async function refreshStatus()'), html.indexOf('function scheduleStatusRefresh()')), ctx);
await ctx.refreshStatus();
assert.equal($('hTokens').textContent, '0', 'new session must not display the all-session aggregate');
session = {tokens:{total:1234}};
await ctx.refreshStatus();
assert.equal($('hTokens').textContent, (1234).toLocaleString());
ctx.currentSession = null; ctx.sessionRevision++;
await ctx.refreshStatus();
assert.equal($('hTokens').textContent, '0', 'no selected session, including logout, starts at zero');
ctx.currentSession = 'older'; ctx.sessionRevision++;
await ctx.refreshStatus();
assert.equal($('hTokens').textContent, (1234).toLocaleString());
fail = true; await ctx.refreshStatus();
assert.equal($('hTokens').textContent, '—', 'unavailable usage is not a fabricated zero');
fail = false; session = {}; await ctx.refreshStatus();
assert.equal($('hTokens').textContent, '—', 'missing tally remains unknown');
const deferred = () => { let resolve; const promise = new Promise(r => {resolve=r;}); return {promise,resolve}; };
for (const boundary of ['switch','completion','logout','newer refresh']) {
  ctx.currentSession = 'old'; ctx.sessionRevision++;
  const old = deferred();
  ctx.api = path => path === '/api/status' ? Promise.resolve({total_tokens:999}) : old.promise;
  const pending = ctx.refreshSessionTokens();
  if (boundary === 'switch') { ctx.currentSession = 'new'; ctx.sessionRevision++; ctx.paintSessionTokens({total:0}); }
  if (boundary === 'completion') ctx.paintSessionTokens({total:42});
  if (boundary === 'logout') { ctx.currentSession = null; ctx.sessionRevision++; ctx.paintSessionTokens({total:0}); }
  if (boundary === 'newer refresh') { ctx.api = async()=>({tokens:{total:42}}); await ctx.refreshSessionTokens(); }
  old.resolve({tokens:{total:17}}); await pending;
  assert.equal($('hTokens').textContent, ['switch','logout'].includes(boundary) ? '0' : '42', boundary+' must invalidate late usage');
}
console.log('session tokens: selected tally, zero, unavailable, switch, completion, logout and stale refresh passed');
