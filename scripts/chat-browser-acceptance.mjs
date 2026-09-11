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
const sessions = new Map(); const requests=[]; let sequence=0, mode='normal', tokens=2;
const server=createServer(async(req,res)=>{
 let data=''; for await (const chunk of req) data+=chunk;
 const body=data?JSON.parse(data):{}; const path=req.url; requests.push([req.method,path,body]);
 const reply=(value,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
 if(path==='/'){res.end(html.replaceAll('__CYCLAW_CSRF_TOKEN__','fixture').replaceAll('__CYCLAW_CSP_NONCE__','fixture'));return;}
 if(path.startsWith('/static/')){res.end('');return;}
 if(path==='/api/status'){reply({model:'mock',provider:'mock',home:'/fixture',soul_enabled:true,soul:{loaded:false,unavailable_reason:'missing'},total_tokens:0});return;}
 if(path==='/api/registry'){reply({skills:[],tools:[],connectors:[]});return;}
 if(path==='/api/sessions'){
  if(req.method==='GET'){reply({sessions:[...sessions.values()]});return;}
  const session={session_id:String(++sequence).padStart(12,'0'),goal:'',messages:[],title:'fixture',tokens:{total:0}};sessions.set(session.session_id,session);reply(session,201);return;
 }
 const match=path.match(/^\/api\/sessions\/([^/]+)(\/goal)?$/);
 if(match){const s=sessions.get(match[1]);if(match[2])s.goal=body.goal;reply(s);return;}
 if(path==='/api/chat/cancel'){reply({cancelled:true});return;}
 if(path==='/api/chat'){
  if(mode==='delay'){setTimeout(()=>reply({reply:'late'}),2500).unref();return;}
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
 assert.ok(!requests.some(r=>r[1].startsWith('/api/agent/')),'ordinary chat continuation never executes coding work');
 console.log(JSON.stringify({passed:true,coverage:['fresh soul missing','first session','goal set/show/clear','manual continuation','auto cooldown stop','generation cancellation','session switch','repeat stop','GOAL_DONE advisory','aggregate budget','rate limit','model failures','no agent execution']}));
} finally {
 if(ws)ws.close();chrome.kill('SIGTERM');await new Promise(r=>{chrome.once('exit',r);setTimeout(r,2000);});server.closeAllConnections();server.close();await rm(profile,{recursive:true,force:true,maxRetries:10,retryDelay:200});
}
