// Deterministic actual-browser slash-command tests with a local mock HTTP API.
// Protocol/UI evidence only; this is not WKWebView or a real model acceptance.
import assert from 'node:assert/strict';
import {constants as fsConstants} from 'node:fs';
import {createServer} from 'node:http';
import {spawn} from 'node:child_process';
import {access, readFile, mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const html = await readFile(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
let persona=''; let clearFails=false;
let authFixture=false, signedIn=false, mustChange=true, savedKey='', authRole='admin', authUsername='admin';
const sessions = new Map(); const requests=[]; let sequence=0, mode='normal', tokens=2;
const server=createServer(async(req,res)=>{
 let data=''; for await (const chunk of req) data+=chunk;
 const body=data?JSON.parse(data):{}; const path=req.url; requests.push([req.method,path,body]);
 const reply=(value,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
 if(path==='/'){res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'self' 'nonce-fixture'; style-src 'nonce-fixture'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'");res.end(html.replaceAll('__CYCLAW_CSRF_TOKEN__','fixture').replaceAll('__CYCLAW_CSP_NONCE__','fixture'));return;}
 if(path.startsWith('/static/')){res.end('');return;}
 if(path==='/api/auth/whoami'){reply(authFixture&&signedIn?{username:authUsername,role:authRole,must_change_password:mustChange}:{},authFixture?(signedIn?200:401):503);return;}
 if(path==='/api/auth/setup-status'){reply({needs_password:false});return;}
 if(path==='/api/auth/login'){
  const auditor=body.username==='audit-fixture'&&body.password==='browser-auditor-password';
  signedIn=auditor||(body.username==='admin'&&body.password==='admin');
  authUsername=auditor?'audit-fixture':'admin';authRole=auditor?'audit':'admin';
  if(auditor)mustChange=false;
  reply({username:authUsername,role:authRole,must_change_password:mustChange},signedIn?200:401);return;
 }
 if(path==='/api/auth/password'){assert.equal(body.current_password,'admin');assert.ok(body.password.length>=12);mustChange=false;reply({changed:true});return;}
 if(path==='/api/auth/logout'){signedIn=false;reply({logged_out:true});return;}
 if(path==='/api/keys'){
  if(!signedIn || mustChange || authRole!=='admin'){reply({detail:{code:'AUTH_PERMISSION_DENIED',message:'denied'}},403);return;}
  if(req.method==='POST'){if(body.clear?.length)savedKey='';else savedKey=body.keys.DEEPAGENT_API_KEY;}
  reply({keys:[{name:'DEEPAGENT_API_KEY',label:'Planner key',detail:'Optional fixture provider',saved_configured:!!savedKey,saved_masked:savedKey?'••••••••1234':'',active_configured:false,active_masked:'',active_source:'unset',pending_restart:!!savedKey,environment_override:false}]});return;
 }
 if(path==='/api/status'&&authFixture&&(!signedIn||mustChange||authRole==='audit')){reply({version:'fixture',auth_enabled:true});return;}
 if(path==='/api/status'){reply({model:'mock',provider:'mock',home:'/fixture',soul_enabled:true,soul:{loaded:false,unavailable_reason:'missing'},total_tokens:0});return;}
 if(path==='/api/soul/document'){
  if(req.method==='POST'){if(!body.confirm){reply({detail:{code:'SOUL_CONFIRM',message:'Confirmation required'}},400);return;}persona=body.content;reply({saved:true,revision:'1'.repeat(64)});return;}
  reply({content:persona,revision:persona?'1'.repeat(64):'missing',max_chars:8000,versions:[]});return;
 }
 if(path==='/api/prompt/preview'){reply({scope:'Chat preview',prompt:'Discipline contract\n'+(body.soul_content??persona)});return;}
 if(path==='/api/registry'){reply({skills:[],tools:[],connectors:[]});return;}
 if(authFixture&&signedIn&&authRole==='audit'&&(path==='/api/sessions'||path.startsWith('/api/sessions/'))){reply({detail:{code:'AUTH_PERMISSION_DENIED',message:'Your role does not permit access to sessions.'}},403);return;}
 if(path==='/api/sessions/clear'){
  if(clearFails){reply({detail:{code:'HARNESS_SESSION_PERSIST_ERROR',message:'fixture storage failure'}},502);return;}
  if(!body.confirm || !body.reason?.trim()){reply({},422);return;}
  sessions.clear();reply({deleted_sessions:1});return;
 }
 if(path==='/api/sessions'){
  if(req.method==='GET'){reply({sessions:[...sessions.values()]});return;}
  const session={session_id:String(++sequence).padStart(12,'0'),goal:'',messages:[],title:'fixture',tokens:{total:0}};sessions.set(session.session_id,session);reply(session,201);return;
 }
 const match=path.match(/^\/api\/sessions\/([^/]+)(\/goal)?$/);
 if(match){const s=sessions.get(match[1]);if(match[2])s.goal=body.goal;reply(s);return;}
 if(path.match(/^\/api\/sessions\/[^/]+\/goal-stage$/)){
  const session=sessions.get(path.split('/')[3]);
  if(req.method==='POST')session.stage={stage_id:'a'.repeat(32),goal:session.goal,request:{instruction:session.goal,branch:body.branch,commit_message:'Fixture goal',checks:null,read_files:[],max_iterations:1,goal_stage:{session_id:session.session_id,stage_id:'a'.repeat(32)}}};
  reply({stage:session.stage,request:session.stage.request,status:'staged',executed:false,next:'Review and explicitly confirm'});return;
 }
 if(path.match(/^\/api\/sessions\/[^/]+\/skills$/)){
  const session=sessions.get(path.split('/')[3]);
  if(req.method==='POST')session.selected=body.ids;
  reply({selected:session.selected||[],last_result:[],ready:true});return;
 }
 if(path==='/api/agent/checks'){reply({profiles:[{name:'cargo-fmt'}],default_profile:'cargo-test',capabilities:{jobs:true},planner_model:'mock'});return;}
 if(path==='/api/skills/check'){
  if(body.id!=='check:cargo-fmt'){reply({detail:{code:'SKILL_ID',message:'Unknown fixed check'}},400);return;}
  reply({id:body.id,profile:'cargo-fmt',executed:false});return;
 }
 if(path==='/api/chat/cancel'){reply({cancelled:true});return;}
 if(path==='/api/chat'){
  if(mode==='delay'){setTimeout(()=>reply({reply:'LATE_OLD_REPLY',session_id:body.session_id,model:'mock',usage:{prompt_tokens:1,completion_tokens:2},tally:{total:3}}),500).unref();return;}
  if(mode==='rate'){reply({detail:{code:'LOOP_RATE_LIMIT',message:'rate fixture'}},429);return;}
  if(mode==='failure'){reply({detail:{code:'HARNESS_LLM_ERROR',message:'failure fixture'}},502);return;}
  reply({reply:mode==='done'?'GOAL_DONE':mode==='repeat'?'same':'reply '+requests.length,session_id:body.session_id,model:'mock',usage:{prompt_tokens:1,completion_tokens:tokens},tally:{total:tokens}});return;
 }
 reply({});
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const base='http://127.0.0.1:'+server.address().port;
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const chromeBinary=process.env.CHROME_BIN||(process.platform==='darwin'?'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome':'/usr/bin/google-chrome');
await access(chromeBinary,fsConstants.X_OK).catch(()=>{throw new Error('Chrome binary missing or not executable: '+chromeBinary);});
const chromeArgs=profile=>[
 '--headless=new',
 ...(process.platform==='linux'?['--no-sandbox','--disable-setuid-sandbox']:[]),
 '--no-first-run','--disable-background-networking','--disable-extensions',
 '--disable-gpu','--disable-dev-shm-usage','--disable-software-rasterizer','--disable-breakpad',
 '--metrics-recording-only','--remote-debugging-port=0','--remote-allow-origins=*',
 '--crash-dumps-dir='+profile,'--user-data-dir='+profile,'about:blank',
];
const stopChrome=async chrome=>{
 if(!chrome||chrome.exitCode!==null||chrome.signalCode)return;
 chrome.kill('SIGTERM');
 await new Promise(r=>{chrome.once('exit',r);setTimeout(r,2000);});
};
const launchChrome=async()=>{
 const profile=await mkdtemp(join(tmpdir(),'cgah-chat-browser-'));
 const stderrChunks=[];
 let exitInfo;
 const chrome=spawn(chromeBinary,chromeArgs(profile),{stdio:['ignore','ignore','pipe']});
 if(chrome.stderr)chrome.stderr.on('data',chunk=>{if(Buffer.concat(stderrChunks).length<16384)stderrChunks.push(chunk);});
 chrome.once('error',err=>{exitInfo={error:err.message};});
 chrome.once('exit',(code,signal)=>{exitInfo={code,signal};});
 let port;
 for(let i=0;i<300;i++){
  if(exitInfo)break;
  try{const line=(await readFile(join(profile,'DevToolsActivePort'),'utf8')).split('\n')[0].trim();if(line){port=line;break;}}catch{await pause(100);}
 }
 return {chrome,profile,port,exitInfo,stderr:Buffer.concat(stderrChunks).toString('utf8').slice(-4000)};
};
const describeLaunch=launched=>{
 const head=launched.exitInfo?JSON.stringify(launched.exitInfo):'DevToolsActivePort missing';
 return launched.stderr?head+'\n'+launched.stderr:head;
};
let launched=await launchChrome();
if(!launched.port){
 await stopChrome(launched.chrome);
 await rm(launched.profile,{recursive:true,force:true,maxRetries:10,retryDelay:200});
 const first=launched;
 launched=await launchChrome();
 if(!launched.port){
  await stopChrome(launched.chrome);
  await rm(launched.profile,{recursive:true,force:true,maxRetries:10,retryDelay:200});
  assert.ok(false,'Chrome must start ('+chromeBinary+'); first '+describeLaunch(first)+'; retry '+describeLaunch(launched));
 }
}
const {chrome,profile}=launched;const port=launched.port;let ws;
try {
 const targets=await(await fetch('http://127.0.0.1:'+port+'/json')).json();
 ws=new WebSocket(targets.find(t=>t.type==='page').webSocketDebuggerUrl);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j;});
 let seq=0;const pending=new Map();
 ws.onmessage=e=>{const v=JSON.parse(e.data);if(v.id){const p=pending.get(v.id);pending.delete(v.id);v.error?p.reject(Error(JSON.stringify(v.error))):p.resolve(v.result);}};
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));});
 const evaluate=async expression=>{const v=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(v.exceptionDetails)throw Error(JSON.stringify(v.exceptionDetails));return v.result.value;};
 const until=async expression=>{for(let i=0;i<150;i++){const v=await evaluate(expression);if(v)return v;await pause(50);}throw Error('Timed out: '+expression);};
 const send=command=>evaluate('document.getElementById("input").value='+JSON.stringify(command)+';onSend()');
 const chatCount=()=>requests.filter(r=>r[1]==='/api/chat').length;
 await call('Page.navigate',{url:base});await until('typeof onSend === "function"');
 await until('document.getElementById("sSoulV").textContent === "missing"');
 await until('document.getElementById("hAuthWho").textContent.includes("authentication disabled")');
 assert.equal(await evaluate('document.getElementById("hAuthHint").hidden'),true,'disabled-auth homes must not advertise a login');
 assert.ok(await evaluate('document.body.innerText.includes("Chat starts without an assigned repository")'));
 assert.equal(await evaluate('document.body.innerText.includes("agentic GitHub coding")'),false);
 await evaluate('agent("Branding fixture")');
 assert.ok(await evaluate('document.body.innerText.includes("CG Agent Harness")'));
 assert.equal(await evaluate('document.body.innerText.toLowerCase().includes("cyclaw")'),false);

 await send('/soul edit');assert.equal(await evaluate('document.getElementById("soulEditor").open'),true);
 await evaluate('document.getElementById("soulContent").value="BROWSER_PERSONA";document.getElementById("soulReason").value="Reviewed UI edit";document.getElementById("soulPreview").click()');
 await until('document.getElementById("soulFeedback").textContent.includes("BROWSER_PERSONA")');assert.equal(persona,'');
 await evaluate('document.getElementById("soulSave").click()');await until('document.getElementById("soulFeedback").textContent.includes("Confirmation required")');assert.equal(persona,'');
 await evaluate('document.getElementById("soulConfirm").checked=true;document.getElementById("soulSave").click()');await until('document.getElementById("soulFeedback").textContent.startsWith("Saved.")');assert.equal(persona,'BROWSER_PERSONA');
 await evaluate('document.getElementById("soulClose").click()');assert.equal(await evaluate('document.getElementById("soulContent").value'),'');
 await send('/prompt');assert.ok(await evaluate('document.getElementById("stream").innerText.includes("BROWSER_PERSONA")'));
 await send('/goal Review a patch');assert.equal(sessions.size,1);assert.equal(await evaluate('sessionGoal'),'Review a patch');
 await send('/goal');assert.ok(await evaluate('document.getElementById("stream").innerText.includes("current goal:")'));
 let before=chatCount();await send('/loop 2');assert.equal(chatCount(),before+1);assert.equal(await evaluate('loopState.remaining'),1);
 await send('/loop');assert.equal(chatCount(),before+2);assert.equal(await evaluate('loopState'),null);
 mode='repeat';await send('/loop 3');await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='done';await send('/loop');assert.equal(await evaluate('loopState'),null);assert.ok(await evaluate('document.getElementById("stream").innerText.includes("unverified")'));
 mode='normal';tokens=999999;await send('/loop');assert.equal(await evaluate('loopState'),null);tokens=2;
 mode='rate';await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='failure';await send('/loop');await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='delay';const generation=send('/loop');await until('!!inflightChat');await send('/loop stop');await generation;assert.equal(await evaluate('loopState'),null);
 mode='normal';await send('/loop auto');before=chatCount();const auto=send('/loop 3');await until('loopState && loopState.remaining === 2');await send('/loop stop');await auto;assert.equal(chatCount(),before+1,'stop during cooldown prevents another turn');
 await send('/loop auto');await send('/loop 2');await send('/goal clear');assert.equal(await evaluate('loopState'),null);assert.equal(await evaluate('sessionGoal'),'');
 await send('/goal Another goal');await send('/loop 2');await send('/session new');assert.equal(await evaluate('loopState'),null);
 const firstSession=[...sessions.values()][0];
 firstSession.messages=Array.from({length:12},(_,i)=>({role:i%2?'assistant':'user',content:'OLD_MESSAGE_'+i}));
 await send('/session use '+firstSession.session_id);
 assert.equal(await evaluate('document.querySelectorAll("#stream .msg.user, #stream .msg.agent").length'),12,'switch renders the selected session, including its older messages');
 await send('/session use '+firstSession.session_id);
 assert.equal(await evaluate('document.querySelectorAll("#stream .msg.user, #stream .msg.agent").length'),12,'reselecting a session must not duplicate its transcript');
 await send('/agent run codex/old Old staged work');
 await evaluate('shownAgentDiffs.set("old", "diff"); reviewedSoulProposal={id:"old"}');
 await evaluate('document.querySelector("#pane-sessions .cmd-item").click()');
 await until('currentSession !== '+JSON.stringify(firstSession.session_id));
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("OLD_MESSAGE_")'),false,'new session clears the old transcript');
 assert.equal(await evaluate('pendingAgentRun'),null);assert.equal(await evaluate('shownAgentDiffs.size'),0);assert.equal(await evaluate('reviewedSoulProposal'),null);
 // Model replies can finish despite cancellation; they must not switch the selected session back.
 await evaluate('window.originalFetch=window.fetch; window.fetch=(url,opts)=>window.originalFetch(url,String(url)==="/api/chat"?{...opts,signal:undefined}:opts)');
 mode='delay';const oldReply=send('OLD_INFLIGHT_MESSAGE');await until('!!inflightChat');
 const oldSession=await evaluate('currentSession');
 await evaluate('document.querySelector("#pane-sessions .cmd-item").click()');
 await until('currentSession !== '+JSON.stringify(oldSession));
 const newSession=await evaluate('currentSession');
 await oldReply;
 assert.equal(await evaluate('currentSession'),newSession,'late reply must not restore the old session ID');
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("LATE_OLD_REPLY")'),false);
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("OLD_INFLIGHT_MESSAGE")'),false);
 await evaluate('window.fetch=window.originalFetch');mode='normal';
 await send('NEW_SESSION_MESSAGE');assert.equal(requests.filter(r=>r[1]==='/api/chat').at(-1)[2].session_id,newSession);
 // The destructive control sits directly below New Session and requires a separate confirmation.
 assert.equal(await evaluate('document.querySelector("#pane-sessions .cmd-item").nextElementSibling.id'),'clearSessionHistory');
 const clearCount=()=>requests.filter(r=>r[1]==='/api/sessions/clear').length;
 const beforeClear=clearCount();const savedCount=sessions.size;
 await evaluate('document.getElementById("clearSessionHistory").click()');
 assert.equal(await evaluate('document.getElementById("sessionClearDialog").open'),true);
 await evaluate('document.getElementById("sessionClearCancel").click()');
 assert.equal(clearCount(),beforeClear);assert.equal(sessions.size,savedCount);
 clearFails=true;
 await evaluate('document.getElementById("clearSessionHistory").click();document.getElementById("sessionClearConfirm").click()');
 await until('document.getElementById("sessionClearFeedback").textContent.includes("fixture storage failure") && !sessionClearing');
 assert.equal(sessions.size,savedCount);assert.equal(await evaluate('currentSession'),newSession);
 await evaluate('document.getElementById("sessionClearCancel").click()');clearFails=false;
 await evaluate('window.fetch=(url,opts)=>window.originalFetch(url,String(url)==="/api/chat"?{...opts,signal:undefined}:opts)');
 mode='delay';const erasedReply=send('ERASE_INFLIGHT_MESSAGE');await until('!!inflightChat');
 await evaluate('document.getElementById("clearSessionHistory").click();document.getElementById("sessionClearConfirm").click()');
 await until('!sessionClearing && currentSession===null');await erasedReply;
 assert.equal(sessions.size,0);assert.equal(await evaluate('document.querySelectorAll("#pane-sessions .sess-item").length'),0);
 assert.equal(await evaluate('sessionGoal'),'');assert.equal(await evaluate('loopState'),null);
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("LATE_OLD_REPLY")'),false);
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("ERASE_INFLIGHT_MESSAGE")'),false);
 assert.equal(await evaluate('document.getElementById("sessionClearDialog").open'),false);
 assert.equal(persona,'BROWSER_PERSONA','clearing chats preserves persona');
 await evaluate('window.fetch=window.originalFetch');mode='normal';
 await send('/session new');await send('AFTER_ERASE_MESSAGE');assert.equal(sessions.size,1);
 await send('/skill use custom');assert.deepEqual([...sessions.values()].at(-1).selected,['custom']);
 await send('/skill clear');assert.deepEqual([...sessions.values()].at(-1).selected,[]);
 await send('/agent run codex/fixture Review only');await send('/skill check:cargo-fmt');assert.deepEqual(await evaluate('pendingAgentRun.checks'),['cargo-fmt']);
 await send('/skill check:unknown');assert.deepEqual(await evaluate('pendingAgentRun.checks'),['cargo-fmt']);await send('/agent cancel');
 await send('/goal Implement the fixture');await send('/goal stage codex/goal-fixture');assert.equal(await evaluate('pendingAgentRun.instruction'),'Implement the fixture');
 const goalSession=await evaluate('currentSession');assert.equal(await evaluate('pendingAgentRun.max_iterations'),1);
 await evaluate('window.__cgahLoaded=1');
 await call('Page.reload',{ignoreCache:true});
 await until('typeof onSend === "function" && window.__cgahLoaded!==1');
 await send('/session use '+goalSession);await send('/goal task');assert.equal(await evaluate('pendingAgentRun.goal_stage.session_id'),goalSession);
 await send('/agent confirm');assert.equal(await evaluate('pendingAgentRun.instruction'),'Implement the fixture','missing reason keeps request staged');
 assert.ok(!requests.some(r=>r[0]==='POST' && ['/api/agent/jobs','/api/agent/run'].includes(r[1])),'chat and skill staging never execute coding work');
 // The real browser must execute the minimal-status login and key editor flows.
 authFixture=true;await call('Page.reload',{ignoreCache:true});
 await until('!document.getElementById("hAuthLoginBox").hidden');
 assert.equal(await evaluate('document.getElementById("hAuthHint")?.hidden'),false,'show the fresh-install login hint before authentication');
 assert.match(await evaluate('document.getElementById("hAuthHint").innerText'),/Default login is User: admin \/ Password: admin/);
 assert.ok(await evaluate('document.querySelector(".logo").compareDocumentPosition(document.getElementById("hAuthHint")) & Node.DOCUMENT_POSITION_FOLLOWING'));
 assert.ok(await evaluate('document.getElementById("hAuthHint").compareDocumentPosition(document.getElementById("harnessAuth")) & Node.DOCUMENT_POSITION_FOLLOWING'));
 await until('document.getElementById("sProvider").textContent === "sign in"');
 assert.equal(await evaluate('document.querySelector("[data-pane=registry]")'),null);
 await evaluate('document.getElementById("hAuthUser").value="admin";document.getElementById("hAuthPass").value="admin";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("passwordDialog").open');
 assert.equal(await evaluate('document.getElementById("hAuthHint").hidden'),true,'hide bootstrap guidance after login');
 assert.match(await evaluate('document.getElementById("passwordHelp").innerText'),/Replace the initial admin password/);
 await evaluate('document.getElementById("passwordCurrent").value="admin";document.getElementById("passwordNew").value="browser-fixture-password";document.getElementById("passwordConfirm").value="browser-fixture-password";document.getElementById("passwordForm").requestSubmit()');
 await until('!document.getElementById("passwordDialog").open');assert.equal(mustChange,false);
 await evaluate('document.querySelector("[data-pane=api-keys]").click()');
 await until('!!document.getElementById("saved-DEEPAGENT_API_KEY")');
 await evaluate('document.getElementById("saved-DEEPAGENT_API_KEY").value="fixture-secret-value-1234";document.getElementById("saved-DEEPAGENT_API_KEY").form.requestSubmit()');
 await until('document.getElementById("pane-api-keys").textContent.includes("••••••••1234")');
 assert.equal(savedKey,'fixture-secret-value-1234');
 assert.equal(await evaluate('document.getElementById("saved-DEEPAGENT_API_KEY").value'),'');
 assert.equal(await evaluate('document.body.textContent.includes("fixture-secret-value-1234")'),false);
 await evaluate('Array.from(document.querySelectorAll("#pane-api-keys button")).find(b=>b.textContent==="Clear saved value").click()');
 await until('document.getElementById("pane-api-keys").textContent.includes("Saved: unset")');assert.equal(savedKey,'');
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');assert.equal(await evaluate('currentSession'),null);
 assert.equal(await evaluate('document.getElementById("hAuthHint").hidden'),false,'restore login guidance after logout');
 // Authentication succeeds for an auditor even though the server denies operational sessions.
 await evaluate('document.getElementById("hAuthUser").value="audit-fixture";document.getElementById("hAuthPass").value="browser-auditor-password";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("pane-sessions").textContent.includes("Your role does not permit access to sessions.")');
 assert.equal(await evaluate('document.getElementById("hAuthWho").textContent'),'audit-fixture · audit','a session permission denial must not turn successful login into a network error');
 assert.equal(await evaluate('document.querySelectorAll("#pane-sessions button").length'),0,'auditors must not see New Session or Clear History controls');
 assert.equal(await evaluate('document.getElementById("hAuthHint").hidden'),true);
 await evaluate('document.getElementById("stream").replaceChildren()');await send('/status');
 const auditorStatus=await evaluate('document.getElementById("stream").innerText');
 assert.equal(auditorStatus.includes('undefined'),false,'redacted status must not render missing fields');
 assert.match(auditorStatus,/read.only/i,'auditor status must explain its read-only scope');
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');assert.equal(signedIn,false);
 assert.equal(await evaluate('document.getElementById("hAuthHint").hidden'),false);
 assert.equal(await evaluate('document.getElementById("sProvider").textContent'),'sign in');
 console.log(JSON.stringify({passed:true,coverage:['minimal anonymous status','forced password change','API Keys catalog save/clear/masked status','auditor login with denied sessions and redacted status','logout clears UI','fresh transcript','full session restore without duplication','late reply session isolation','staged approval reset','goal coding staging','refresh recovery','no implicit confirmation','prompt skill selection/clear','fixed check staging/refusal','persona editor','preview without write','explicit save confirmation','prompt viewer','fresh soul missing','first session','goal set/show/clear','manual continuation','auto cooldown stop','generation cancellation','session switch','repeat stop','GOAL_DONE advisory','aggregate budget','rate limit','model failures','no agent execution']}));
} finally {
 if(ws)ws.close();chrome.kill('SIGTERM');await new Promise(r=>{chrome.once('exit',r);setTimeout(r,2000);});server.closeAllConnections();server.close();await rm(profile,{recursive:true,force:true,maxRetries:10,retryDelay:200});
}
