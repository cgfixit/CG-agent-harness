// Exercise the actual delivery UI handlers across logout and late replay refresh.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html=readFileSync(new URL('../assets/static/harness.html',import.meta.url),'utf8');
const nodes=new Map();let serial=0, calls=[], waitRefresh;
const node=id=>{if(!nodes.has(id))nodes.set(id,{value:'',checked:false,disabled:false,open:true,textContent:'',children:[],
 addEventListener(e,f){this[e]=f;},append(x){this.children.push(x);},replaceChildren(){this.children=[];},close(){this.open=false;},showModal(){this.open=true;}});return nodes.get(id);};
const data={enabled:true,destinations:[],deliveries:[{destination_id:'owned',state:'failed',job_status:'finished',attempts:3,replays:0,job_id:'job',delivery_id:'delivery'}]};
const context=vm.createContext({$:node,document:{createElement:()=>node('element'+serial++),createTextNode:s=>s},sessionRevision:1,encodeURIComponent,
 api:async(path,method,body)=>{calls.push({path,method,body});if(method==='POST')return {};if(waitRefresh)return new Promise(r=>waitRefresh=r);return data;}});
const start=html.indexOf('/* ── owner-scoped delivery status and explicit replay ── */'),end=html.indexOf('/* ── owned schedule preview and activation ── */',start);
vm.runInContext(html.slice(start,end),context);
await context.refreshDeliveries();
let row=node('notificationRows').children[0],button=row.children.at(-1),reason=row.children.at(-3).children[0],confirm=row.children.at(-2).children[0];
await button.click();assert.equal(calls.filter(x=>x.method==='POST').length,0);
reason.value='reviewed retry';confirm.checked=true;waitRefresh=true;
let replay=button.click();await new Promise(r=>setImmediate(r));assert.equal(typeof waitRefresh,'function');
context.sessionRevision++;context.resetNotificationView();waitRefresh(data);await replay;
assert.equal(node('notificationRows').children.length,0);assert.equal(node('notificationStatus').textContent,'');
assert.equal(node('notificationDialog').open,false,'late refresh must not reopen the old account view');
await button.click();assert.equal(calls.filter(x=>x.method==='POST').length,1,'detached old account buttons cannot replay');
console.log('delivery review: confirmation, stale replay/refresh and logout isolation passed');
