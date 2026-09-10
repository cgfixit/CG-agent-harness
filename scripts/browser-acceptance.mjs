// Native Chrome acceptance against an explicitly supplied disposable arithmetic
// fixture server. No npm packages, real user profile, or stored API credentials.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {readFile, writeFile, mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
const base=process.env.CGAH_TEST_BASE_URL;
const home=resolve(process.env.CGAH_TEST_HOME);
assert.equal(process.env.CGAH_TEST_FIXTURE,'disposable-arithmetic');
assert.equal(new URL(base).hostname,'127.0.0.1');
assert.equal((await (await fetch(base+'/api/status')).json()).home,home);
const key=''; // Exercise default local access without a key or login.
const profile=await mkdtemp(join(tmpdir(),'cgah-browser-'));
const binary=process.env.CHROME_BIN || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const chrome=spawn(binary,['--headless=new','--no-first-run','--disable-background-networking','--disable-sync','--disable-extensions','--remote-debugging-port=0','--user-data-dir='+profile,'about:blank'],{stdio:'ignore'});
const pause=ms=>new Promise(r=>setTimeout(r,ms));
let ws;
try {
 let port;for(let i=0;i<100;i++){try{port=(await readFile(profile+'/DevToolsActivePort','utf8')).split('\n')[0];break;}catch{await pause(100);}}
 assert.ok(port,'Chrome must start; no skip');
 const targets=await(await fetch('http://127.0.0.1:'+port+'/json')).json();
 ws=new WebSocket(targets.find(t=>t.type==='page').webSocketDebuggerUrl);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j;});
 let seq=0;const pending=new Map();const requests=[];
 ws.onmessage=e=>{const v=JSON.parse(e.data);if(v.method==='Network.requestWillBeSent')requests.push({url:v.params.request.url,method:v.params.request.method});if(v.id){const p=pending.get(v.id);pending.delete(v.id);v.error?p.reject(Error(JSON.stringify(v.error))):p.resolve(v.result);}};
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));});
 const evaluate=async expression=>{const v=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(v.exceptionDetails)throw Error(JSON.stringify(v.exceptionDetails));return v.result.value;};
 const until=async (expression,timeout=15000)=>{const start=Date.now();while(Date.now()-start<timeout){const value=await evaluate(expression);if(value)return value;await pause(100);}throw Error('Browser expectation timed out: '+expression);};
 const send=async command=>{await until('!document.getElementById("send").disabled');await evaluate('document.getElementById("input").value='+JSON.stringify(command)+';document.getElementById("send").click()');await pause(100);await until('!document.getElementById("send").disabled');};
 await call('Page.enable');await call('Network.enable');await call('Page.navigate',{url:base+'/'});await until('!!document.getElementById("apiKey")');
 assert.equal(await evaluate('fetch("/api/agent/jobs").then(r=>r.status)'),403);
 await evaluate('document.getElementById("apiKey").value='+JSON.stringify(key));
 assert.equal(await evaluate('fetch("/api/agent/jobs",{method:"POST",headers:{"Authorization":"Bearer "+document.getElementById("apiKey").value,"Content-Type":"application/json"},body:"{}"}).then(r=>r.status)'),403);
 await send('/agent run codex/browser-'+Date.now()+' Fix add to return a plus b in src/lib.rs using one exact edit; preserve all other bytes and tests.');
 assert.equal(await evaluate('pendingAgentRun.checks'),null);
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("server default check: cargo-test")'));
 await send('/agent checks not-a-profile');assert.equal(await evaluate('pendingAgentRun.checks'),null);
 await send('/agent checks cargo-test');assert.deepEqual(await evaluate('pendingAgentRun.checks'),['cargo-test']);
 await send('/agent read src/lib.rs#L599-L604');
 await send('/agent confirm');assert.equal(requests.filter(r=>r.url.endsWith('/api/agent/jobs')&&r.method==='POST').length,1,'only deliberate missing-CSRF probe so far');
 const start=Date.now();await send('/agent confirm Verify disposable browser workflow');
 const job=await until('rememberedAgentJob()');assert.ok(Date.now()-start<15000,'job acknowledgement must be immediate');
 assert.ok(requests.some(r=>r.url.endsWith('/api/agent/jobs')&&r.method==='POST'));
 assert.ok(!requests.some(r=>r.url.endsWith('/api/agent/run')&&r.method==='POST'));
 await call('Page.reload',{ignoreCache:true});await until('!!document.getElementById("apiKey")');await pause(200);
 assert.equal(await evaluate('rememberedAgentJob()'),job);
 assert.equal(await evaluate('document.getElementById("apiKey").value'),'','key must not survive refresh');
 await evaluate('document.getElementById("apiKey").value='+JSON.stringify(key));await send('/agent job '+job);
 await until('document.getElementById("stream").innerText.includes("job '+job+': finished") || document.getElementById("stream").innerText.includes("job '+job+': failed")',300000);
 const outcome=await evaluate('api("/api/agent/jobs/'+job+'")');
 assert.equal(outcome.status,'finished',JSON.stringify(outcome.error||outcome.result));
 const run=outcome.result.parsed;assert.equal(run.status,'pending_decision');
 await send('/agent status '+run.run_id);
 const inspected=await evaluate('(async()=>agentRecord(await api("/api/agent/runs/'+run.run_id+'")))()');
 assert.ok(inspected.diff.includes('-pub fn add(a: i32, b: i32) -> i32 { a - b }'));
 assert.ok(inspected.diff.includes('+pub fn add(a: i32, b: i32) -> i32 { a + b }'));
 assert.ok(!inspected.diff.includes('[diff truncated'));
 assert.deepEqual(inspected.changed_files,['src/lib.rs']);
 // Complete known fixture diff has been inspected above; approve local commit.
 await send('/agent approve '+run.run_id+' Reviewed complete one-expression fixture diff and passing Cargo check');
 let decided=await evaluate('(async()=>agentRecord(await api("/api/agent/runs/'+run.run_id+'")))()');assert.equal(decided.status,'approved');assert.equal(decided.pushed,false);
 await send('/agent push '+run.run_id+' Separately authorize disposable local bare push');
 decided=await evaluate('(async()=>agentRecord(await api("/api/agent/runs/'+run.run_id+'")))()');assert.equal(decided.pushed,true);
 await send('/agent publish '+run.run_id+' Publish only the reviewed fixture template');
 assert.ok(!requests.some(r=>r.url.endsWith('/publish')&&r.method==='POST'),'no publication before loading a reviewed body');
 const body='## Proposed changes\nCorrect fixture addition.\n\n## Further comments\nReal Cargo passed; complete diff reviewed.\n';
 const bodyFile=join(profile,'reviewed-pr.md');await writeFile(bodyFile,body);
 await send('/agent pr-body '+run.run_id);
 const document=await call('DOM.getDocument');const input=await call('DOM.querySelector',{nodeId:document.root.nodeId,selector:'#agentPRBodyFile'});
 await call('DOM.setFileInputFiles',{nodeId:input.nodeId,files:[bodyFile]});
 await until('reviewedPRBodies.has('+JSON.stringify(run.run_id)+')');
 assert.equal(await evaluate('reviewedPRBodies.get('+JSON.stringify(run.run_id)+')'),body);
 await send('/agent publish '+run.run_id+' Separately authorize mock draft publication with reviewed body');
 decided=await evaluate('(async()=>agentRecord(await api("/api/agent/runs/'+run.run_id+'")))()');assert.equal(decided.pr_url,'https://example.invalid/pull/1');
 assert.equal(await readFile(resolve(home,'../published-body'),'utf8'),body);
 await send('/agent run codex/cancel-'+Date.now()+' Fix add to return a plus b in src/lib.rs with an exact edit.');await send('/agent read src/lib.rs#L599-L604');await send('/agent confirm Exercise cancellation of disposable job');
 const cancelled=await until('rememberedAgentJob()!=='+JSON.stringify(job)+'?rememberedAgentJob():null');
 await send('/agent run codex/staged-only No execution');await send('/agent cancel');assert.equal(await evaluate('pendingAgentRun'),null);
 await send('/agent stop '+cancelled);await pause(500);
 assert.equal((await evaluate('api("/api/agent/jobs/'+cancelled+'")')).status,'cancelled');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("may still survive")'));
 console.log(JSON.stringify({passed:true,job,run:run.run_id,cancelled,coverage:['real Chrome','authentication','CSRF','server defaults','check selection','staging','job creation','refresh recovery','complete fixture diff','explicit approval','separate local push','reviewed PR body','mock draft publication','staged cancel','active request cancel']}));
} finally {
 if(ws)ws.close();chrome.kill('SIGTERM');await new Promise(r=>{chrome.once('exit',r);setTimeout(r,2000);});await rm(profile,{recursive:true,force:true});
}
