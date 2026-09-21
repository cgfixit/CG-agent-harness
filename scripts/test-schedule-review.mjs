// Real schedule UI handlers: stale responses and changed requests cannot activate.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html',import.meta.url),'utf8');
const nodes = new Map();
const node = id => {
  if (!nodes.has(id)) nodes.set(id,{value:'',defaultValue:'fixture-default',checked:false,disabled:false,open:true,textContent:'',children:[],
    addEventListener(e,f){this[e]=f;},replaceChildren(){this.children=[];},append(x){this.children.push(x);},close(){this.open=false;},showModal(){this.open=true;}});
  return nodes.get(id);
};
let resolve, calls=[];
const context = vm.createContext({$:node,document:{createElement:()=>node('new-'+nodes.size)},Date,JSON,Error,Number,
  sessionRevision:1,reviewRevision:1,pendingAgentRun:{goal_stage:{session_id:'owned',stage_id:'reviewed'},instruction:'review this'},
  api:async(path,method,body)=>{calls.push({path,body});if(path.endsWith('/preview'))return new Promise(r=>resolve=r);return {schedules:[]};}});
const start=html.indexOf('/* ── owned schedule preview and activation ── */'), end=html.indexOf("$('openSpend').addEventListener",start);
vm.runInContext(html.slice(start,end),context);
const seed = () => {node('scheduleKind').value='interval';node('scheduleSeconds').value='60';node('scheduleReason').value='fixture review';node('scheduleConfirm').checked=true;node('scheduleDialog').open=true;};
const result = () => ({preview_id:'receipt',expires_at:Date.now()/1000+60,occurrences:[{at:Date.now()/1000+60,local_time:'fixture'}]});
const activations = () => calls.filter(x=>x.path==='/api/agent/schedules'&&x.body);
seed();
const beforeEdit=calls.length;
for(let i=0;i<80;i++) node('scheduleReason').input();
assert.equal(calls.length,beforeEdit,'typing must not issue requests or exhaust the shared API rate limit');
let pending=node('previewSchedule').click();
context.sessionRevision++;
resolve(result());await pending;
assert.equal(node('activateSchedule').disabled,true,'account/session change must discard late preview');
seed();pending=node('previewSchedule').click();context.pendingAgentRun.instruction='changed while previewing';resolve(result());await pending;
assert.equal(node('activateSchedule').disabled,true,'changed staged request must discard preview');
seed();pending=node('previewSchedule').click();resolve(result());await pending;
assert.equal(node('activateSchedule').disabled,false);
node('scheduleSeconds').value='120';
await node('activateSchedule').click();
assert.equal(activations().length,0,'changed timing cannot use old receipt even without input event');
seed();pending=node('previewSchedule').click();resolve(result());await pending;
await node('activateSchedule').click();
assert.equal(activations().length,1);
assert.equal(activations()[0].body.preview_id,'receipt');
await node('activateSchedule').click();
assert.equal(activations().length,1,'successful activation consumes UI receipt');
node('scheduleKind').value='cron';node('scheduleExpression').value='private timing';node('scheduleTimezone').value='private zone';
context.resetScheduleReview(true);
assert.equal(node('scheduleKind').value,'interval');
for (const id of ['scheduleSeconds','scheduleExpression','scheduleTimezone']) assert.equal(node(id).value,node(id).defaultValue,'logout must erase prior account timing choices');
assert.equal(node('scheduleCronFields').hidden,true);
assert.equal(node('scheduleDialog').open,false);
assert.equal(node('scheduleConfirm').checked,false);
assert.equal(node('scheduleReason').value,'');
console.log('schedule review: stale session/request/timing refusals, activation, single use and clear passed');
