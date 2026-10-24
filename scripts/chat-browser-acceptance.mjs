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
const slashSource = await readFile(new URL('../src/server/slash.rs', import.meta.url), 'utf8');
const canonicalCommands = [...slashSource.match(/const COMMANDS: &[\s\S]*?= &\[([\s\S]*?)\];/)[1].matchAll(/"([a-z]+)"/g)].map(match=>'/'+match[1]).sort();
// The fresh-home credential is restricted until the password route replaces it.
const FIXTURE_SETUP_CODE='fx2a-b3cd-e4fg-h5jk';
let adminPassword=FIXTURE_SETUP_CODE;
const userIds={admin:'1'.repeat(32),'audit-fixture':'2'.repeat(32),'memory-second':'3'.repeat(32)}; // random_hex(16) owner ids
let persona=''; let clearFails=false, slashMode='dispatch';
let authFixture=false, signedIn=false, logoutStatus=200, whoamiDrop=false, mustChange=true, savedKey='', savedSearchKey='', authRole='admin', authUsername='admin';
let automationFixture=null; // null keeps the unknown-path {} reply for the header feature probes
const headerState='["openAnalytics","openDeliveries","openSchedules"].filter(id=>!document.getElementById(id).hidden).join()';
const sessions = new Map(); const requests=[]; let sequence=0, mode='normal', tokens=2;
let memoryEnabled=false, structuredOpen=true;
let suggestionRun=null;
let automaticSuggestions=null, memoryOwnerFixture=null, holdMemoryHistory=false, memoryHistoryHeld=null, rejectHeldMemoryHistory=null;
let holdMemoryFacts=false, memoryFactsHeld=null, releaseHeldMemoryFacts=null;
let holdMemoryDecision=false, memoryDecisionHeld=null, releaseHeldMemoryDecision=null, rateLimitMemoryOnce=false, failMemoryOnce=false;
let latestEpisode={id:'episode_labeled_latest',outcome:'completed',semantic_summary:null};
const memoryProposals=[
 {id:'proposal_labeled_apply',revision:'revision_apply',action:'add',category:'pref',content:'Prefer metric units <script>throw new Error("unsafe")</script>',source_episode_ids:['episode_labeled_latest'],status:'pending'},
 {id:'proposal_labeled_reject',revision:'revision_reject',action:'add',category:'pref',content:'Prefer concise answers.',source_episode_ids:['episode_labeled_latest'],status:'pending'},
 {id:'proposal_labeled_old',revision:'revision_old',action:'add',content:'Already decided',status:'applied'}
];
const savedFacts=[{id:'fact_labeled_saved',owner_id:'user_fixture_owner',revision:1,content_digest:'fixture_saved_digest',category:'insight',content:'Saved fact <script>throw new Error("unsafe fact")</script>',active:true}];
const structuredGates={episode_capture:false,explicit_recall:false,retrieval:false,auto_retrieval:false,consolidation:false,auto_consolidation:false,auto_suggest_chat:false,auto_suggest_coding:false};
const ftsHit={id:'fact_labeled_alpha',revision:1,category:'pref',content:'Prefer metric units in examples.',active:true,score:1,provenance:'fts5',type:'untrusted_background_context'};
const memoryPayload=()=>({
 enabled:memoryEnabled,count:0,max_notes:8,max_chars:500,notes:[],
 rag:{enabled:false,writable_from_harness:false},
 structured_memory:{separate:true,enabled:true,episode_capture:structuredGates.episode_capture,explicit_recall:structuredGates.explicit_recall,retrieval:structuredGates.retrieval,auto_retrieval:structuredGates.auto_retrieval,consolidation:structuredGates.consolidation,auto_consolidation:structuredGates.auto_consolidation,auto_suggest_chat:structuredGates.episode_capture&&structuredGates.auto_suggest_chat,auto_suggest_coding:structuredGates.episode_capture&&structuredGates.auto_suggest_coding,status_path:'/api/structured-memory',prompt_toggle:'harness.json.memory_enabled remains pinned-note inclusion only'}
});
const gatePayload=()=>({
 owner_id:'user_fixture_owner',operator_gates:{...structuredGates},
 episode_capture:structuredGates.episode_capture,explicit_recall:structuredGates.explicit_recall,
 retrieval:structuredGates.retrieval,auto_retrieval:structuredGates.retrieval&&structuredGates.auto_retrieval,consolidation:structuredGates.consolidation,auto_consolidation:structuredGates.consolidation&&structuredGates.auto_consolidation,auto_suggest_chat:structuredGates.episode_capture&&structuredGates.auto_suggest_chat,auto_suggest_coding:structuredGates.episode_capture&&structuredGates.auto_suggest_coding,memory_on_unchanged:true
});
const server=createServer(async(req,res)=>{
 let data=''; for await (const chunk of req) data+=chunk;
 const body=data?JSON.parse(data):{}; const path=req.url; requests.push([req.method,path,body]);
 const reply=(value,status=200)=>{if(!res.headersSent)res.writeHead(status,{'Content-Type':'application/json','Cache-Control':'no-store'});res.end(JSON.stringify(value));};
 if(path==='/'){res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'self' 'nonce-fixture'; style-src 'nonce-fixture'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; frame-src 'self'");res.end(html.replaceAll('__CYCLAW_CSRF_TOKEN__','fixture').replaceAll('__CYCLAW_CSP_NONCE__','fixture'));return;}
 if(path.startsWith('/preview/html')){res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'; frame-src 'none'");res.setHeader('X-Frame-Options','SAMEORIGIN');res.end('<!DOCTYPE html><html><body><p>Run a page to preview it here.</p></body></html>');return;}
 if(path.startsWith('/static/')){res.end('');return;}
 // Mirrors account_gate (src/server/guards.rs): an audit account reaches only the public
 // routes, its own whoami/password/logout and GET /api/audit.
 const route=path.split('?')[0];
 if(authFixture&&signedIn&&mustChange&&route.startsWith('/api/')&&!['/api/status','/api/auth/setup-status','/api/auth/login','/api/auth/whoami','/api/auth/password','/api/auth/logout'].includes(route)){reply({detail:{code:'AUTH_PASSWORD_CHANGE_REQUIRED',message:'replace the bootstrap password before using the harness'}},403);return;}
 if(authFixture&&signedIn&&authRole==='audit'&&route.startsWith('/api/')&&!['/api/status','/api/auth/setup-status','/api/auth/login','/api/auth/whoami','/api/auth/password','/api/auth/logout'].includes(route)&&!(route==='/api/audit'&&req.method==='GET')){reply({detail:{code:'AUTH_PERMISSION_DENIED',message:'denied'}},403);return;}
 // Browser dispatch contract fixture; actual grammar is covered by Rust slash tests.
 if(path==='/api/slash/parse'){
  if(slashMode==='error'){reply({detail:{code:'UNAVAILABLE',message:'parser fixture unavailable'}},503);return;}
  if(slashMode==='invalid'){reply({});return;}
  if(slashMode==='suggest'){reply({kind:'suggest',dispatch:false,suggestions:[],notice:'inspection only; not dispatched'});return;}
  // Mirrors the parser's control and hidden-format refusal (src/server/slash.rs).
  const refused=n=>n<0x20||(n>=0x7f&&n<=0x9f)||[0x2028,0x2029,0xfeff,0x200b,0x180e].includes(n)||(n>=0x2060&&n<=0x2064)||(n>=0x202a&&n<=0x202e)||(n>=0x2066&&n<=0x2069);
  if([...body.line].some(c=>refused(c.codePointAt(0)))){reply({kind:'suggest',dispatch:false,suggestions:[],notice:'not dispatched'});return;}
  if(body.line==='/web help'){reply({kind:'dispatch',dispatch:true,canonical:'/help web'});return;}
  if(body.line.startsWith('/web ')){
   const [,action,...words]=body.line.split(' ');
   if(action==='inject'&&!words.length){reply({kind:'dispatch',dispatch:true,canonical:body.line,web:{action,body:{}}});return;}
   if(['pages','search'].includes(action)&&words.length){reply({kind:'dispatch',dispatch:true,canonical:body.line,web:{action,body:{query:words.join(' '),engine:action==='search'?'google':'pages',group:null,count:5}}});return;}
   reply({kind:'suggest',dispatch:false,suggestions:[{line:'/help web'}],notice:'Missing arguments for /web '+action+'; use /help web.'});return;
  }
  // Mirrors echoable_args: the parser never returns an /api set credential.
  const apiSet=body.line.trim().match(/^\/api\s+set\s+(\S+)/i);
  if(apiSet){reply({kind:'dispatch',dispatch:true,canonical:'/api set '+apiSet[1],command:'api',sub:'set',rest:apiSet[1]});return;}
  reply({kind:'dispatch',dispatch:true,canonical:body.line});return;
 }
 if(path==='/api/auth/whoami'){if(whoamiDrop){req.socket.destroy();return;}reply(authFixture&&signedIn?{username:authUsername,role:authRole,user_id:userIds[authUsername],must_change_password:mustChange}:{},authFixture?(signedIn?200:401):503);return;}
 if(path==='/api/auth/setup-status'){reply({enabled:true,default_password:false,setup_code:authFixture&&adminPassword===FIXTURE_SETUP_CODE});return;}
 if(path==='/api/auth/login'){
  const auditor=body.username==='audit-fixture'&&body.password==='browser-auditor-password';
  const secondOwner=body.username==='memory-second'&&body.password==='browser-memory-password';
  signedIn=auditor||secondOwner||(body.username==='admin'&&body.password===adminPassword);
  authUsername=auditor?'audit-fixture':secondOwner?'memory-second':'admin';authRole=auditor?'audit':'admin';
  if(auditor)mustChange=false;
  reply({username:authUsername,role:authRole,user_id:userIds[authUsername],must_change_password:mustChange},signedIn?200:401);return;
 }
 if(path==='/api/auth/password'){assert.equal(body.current_password,adminPassword);assert.ok(body.password.length>=12);adminPassword=body.password;mustChange=false;reply({ok:true,must_change_password:false});return;}
 if(path==='/api/auth/logout'){if(logoutStatus!==200){reply({detail:{code:'AUTH_REQUIRED',message:'session expired'}},logoutStatus);return;}signedIn=false;reply({ok:true});return;}
 if(path==='/api/keys'){
  if(!signedIn || mustChange || authRole!=='admin'){reply({detail:{code:'AUTH_PERMISSION_DENIED',message:'denied'}},403);return;}
  if(req.method==='POST'){if(body.clear?.includes('DEEPAGENT_API_KEY'))savedKey='';if(body.clear?.includes('SERPAPI_API_KEY'))savedSearchKey='';if(body.keys?.DEEPAGENT_API_KEY)savedKey=body.keys.DEEPAGENT_API_KEY;if(body.keys?.SERPAPI_API_KEY)savedSearchKey=body.keys.SERPAPI_API_KEY;}
  reply({...(req.method==='POST'&&body.keys?{written:Object.keys(body.keys),path:'fixture-store'}:{}),keys:[{name:'SERPAPI_API_KEY',label:'Google results (SerpAPI)',detail:'Public Google fallback when unset',saved_configured:!!savedSearchKey,saved_masked:savedSearchKey?'••••••••5678':'',active_configured:false,active_masked:'',active_source:'unset',pending_restart:!!savedSearchKey,environment_override:false},{name:'DEEPAGENT_API_KEY',label:'Planner key',detail:'Optional fixture provider',saved_configured:!!savedKey,saved_masked:savedKey?'••••••••1234':'',active_configured:false,active_masked:'',active_source:'unset',pending_restart:!!savedKey,environment_override:false}]});return;
 }
 if(path==='/api/status'&&authFixture&&(!signedIn||mustChange||authRole==='audit')){reply({version:'fixture',auth_enabled:true});return;}
 if(path==='/api/status'){reply({model:'mock',provider:'mock',home:'/fixture',soul_enabled:true,soul:{loaded:false,unavailable_reason:'missing'},total_tokens:987654});return;}
 if(path==='/api/soul/document'){
  if(req.method==='POST'){if(!body.confirm){reply({detail:{code:'SOUL_CONFIRM',message:'Confirmation required'}},400);return;}persona=body.content;reply({saved:true,revision:'1'.repeat(64)});return;}
  reply({content:persona,revision:persona?'1'.repeat(64):'missing',max_chars:8000,versions:[]});return;
 }
 if(path==='/api/prompt/preview'){reply({scope:'Chat preview',prompt:'Discipline contract\n'+(body.soul_content??persona)});return;}
 if(path==='/api/registry'){reply({skills:[],tools:[],connectors:[]});return;}
 if(path.startsWith('/api/style')){
  const sessionId=req.method==='POST'?body.session_id:new URL(path,'http://fixture').searchParams.get('session_id');
  const session=sessionId?sessions.get(sessionId):null;
  if(sessionId&&!session){reply({detail:{code:'SESSION_NOT_FOUND',message:'No such session'}},404);return;}
  if(req.method==='POST')session.style=body.name==='off'?null:body.name;
  reply({styles:['concise','technical'].map(id=>({id})),session_id:sessionId,style:session?.style||null,loaded:!!session?.style});return;
 }
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
 if(match){const s=sessions.get(match[1]);if(match[2]){s.goal=body.goal;s.goal_web_mode=s.goal?(body.web_mode??'fetch'):'all';}reply(s);return;}
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
 if(path==='/api/notifications'||path==='/api/tools'){
  if(authFixture&&!signedIn){reply({detail:{code:'AUTH_REQUIRED',message:'authentication required'}},401);return;}
  if(authFixture&&mustChange){reply({detail:{code:'AUTH_PERMISSION_DENIED',message:'denied'}},403);return;}
  if(automationFixture){reply(path==='/api/notifications'?{enabled:automationFixture.notifications,destinations:[],deliveries:[]}:{tools:[{name:'agent-schedule-start',path:'/api/agent/schedules',enabled:automationFixture.agentic}],wired:1,total:1});return;}
 }
 if(path==='/api/spend/predict'){
  assert.equal(req.method,'POST');assert.equal(req.headers['x-cyclaw-csrf'],'fixture');
  reply({model:'mock-cloud',usd:0.000125,input_tokens:4,reserved_output_tokens:16,estimate_source:'vendor_count',max_usd_per_call:null,budget_applies:false,budget_exceeded:false,priced_as_of:'2026-09-20',rates_stale:false});return;
 }
 if(path==='/api/skills/check'){
  if(body.id!=='check:cargo-fmt'){reply({detail:{code:'SKILL_ID',message:'Unknown fixed check'}},400);return;}
  reply({id:body.id,profile:'cargo-fmt',executed:false});return;
 }
 if(path==='/api/web/search'){reply(body.engine==='google'?{provider:'google-serpapi',results:[{rank:1,title:'Fixture Google result',url:'https://veeam.com/kb1',snippet:'Search listing only'}],notice:'Linked page not fetched'}:{hits:[],coverage:{searched:[],failed:[]}});return;}
 if(path==='/api/chat/cancel'){reply({cancelled:true});return;}
 if(path==='/api/memory'||path.startsWith('/api/memory?')){
  if(req.method==='POST')memoryEnabled=!!body.enabled;
  reply(memoryPayload());return;
 }
 if(path==='/api/memory/add'||path==='/api/memory/forget'||path==='/api/memory/clear'){reply(memoryPayload());return;}
 if(path==='/api/structured-memory'){
  if(rateLimitMemoryOnce){rateLimitMemoryOnce=false;res.setHeader('Retry-After','1');reply({detail:{code:'RATE_LIMIT',message:'Fixture rate limit',details:{retry_after_sec:1}}},429);return;}
  if(failMemoryOnce){failMemoryOnce=false;reply({detail:{code:'UNAVAILABLE',message:'Fixture transient memory failure'}},503);return;}
  reply({enabled:structuredOpen,fact_count:structuredOpen?(memoryOwnerFixture?.facts||savedFacts).length:0,pending_proposal_count:structuredOpen?(memoryOwnerFixture?.proposals||memoryProposals).filter(p=>p.status==='pending').length:0,limits:{max_reason_chars:1000,max_facts_per_owner:64},...(automaticSuggestions?{automatic_suggestions:automaticSuggestions}:{}),config_drift:['structured_memory.retrieval','structured_memory.auto_retrieval']});return;
 }
 if(path==='/api/structured-memory/facts'&&req.method==='GET'){
  if(!structuredOpen){reply({detail:{code:'STRUCTURED_MEMORY_DISABLED',message:'structured memory is disabled'}},409);return;}
  const facts=memoryOwnerFixture?.facts||savedFacts;
  if(holdMemoryFacts){
   holdMemoryFacts=false;const snapshot=JSON.parse(JSON.stringify(facts));
   // Match the real API's no-store headers before delaying the body. Otherwise
   // Chromium serializes identical GETs behind its pending cache transaction.
   res.writeHead(200,{'Content-Type':'application/json','Cache-Control':'no-store'});res.flushHeaders();
   releaseHeldMemoryFacts=()=>reply({owner_id:'user_fixture_owner',facts:snapshot,count:snapshot.length});memoryFactsHeld();return;
  }
  reply({owner_id:memoryOwnerFixture?.owner||'user_fixture_owner',facts,count:facts.length,search:false,retrieval:false,fts:false});return;
 }
 if(path==='/api/structured-memory/facts'&&req.method==='POST'){
  if(!structuredOpen){reply({detail:{code:'STRUCTURED_MEMORY_DISABLED',message:'structured memory is disabled'}},409);return;}
  assert.equal(req.headers['x-cyclaw-csrf'],'fixture');assert.equal(body.confirm,true);assert.ok(body.reason.trim());
  reply({id:'manual_fixture_fact',content:body.content});return;
 }
 if(path==='/api/structured-memory/episodes?latest_completed=true'){
  if(!structuredOpen){reply({detail:{code:'STRUCTURED_MEMORY_DISABLED',message:'structured memory is disabled'}},409);return;}
  reply({episodes:latestEpisode?[latestEpisode]:[],available:structuredGates.episode_capture});return;
 }
 if(path==='/api/structured-memory/episodes/episode_labeled_latest/summary'){
  assert.equal(req.method,'POST');assert.equal(req.headers['x-cyclaw-csrf'],'fixture');
  assert.equal(body.confirm,true);assert.ok(body.reason.trim());
  latestEpisode.semantic_summary=body.summary;reply(latestEpisode);return;
 }
 if(path==='/api/structured-memory/proposals?status=pending'){reply({proposals:(memoryOwnerFixture?.proposals||memoryProposals).filter(p=>p.status==='pending')});return;}
 if(path.startsWith('/api/structured-memory/proposals/')){
  const proposal=memoryProposals.find(p=>p.id===path.split('/').at(-1));
  assert.equal(req.method,'POST');assert.equal(req.headers['x-cyclaw-csrf'],'fixture');
  assert.equal(body.confirm,true);assert.ok(body.reason.trim());assert.equal(body.revision,proposal.revision);
  const decide=()=>{
   proposal.status=body.apply?'applied':'rejected';
   if(body.apply&&proposal.action==='add')savedFacts.unshift({id:'fact_from_'+proposal.id,owner_id:'user_fixture_owner',revision:1,category:proposal.category,content:proposal.content,active:true});
   if(body.apply&&proposal.action==='deactivate'){const index=savedFacts.findIndex(f=>f.id===proposal.target_fact_id);if(index>=0)savedFacts.splice(index,1);}
   reply(proposal);
  };
  if(holdMemoryDecision){holdMemoryDecision=false;releaseHeldMemoryDecision=decide;memoryDecisionHeld();return;}
  decide();return;
 }
 if(path==='/api/structured-memory/gates'){
  if(req.method==='POST'){
   if(!Object.hasOwn(structuredGates,body.gate)){reply({detail:{code:'STRUCTURED_MEMORY_GATE',message:'gate must be episode_capture, explicit_recall, retrieval, auto_retrieval, consolidation, or auto_consolidation'}},422);return;}
   structuredGates[body.gate]=!!body.enabled;
  }
  reply(gatePayload());return;
 }
 if(path==='/api/structured-memory/consolidation'||path.startsWith('/api/structured-memory/consolidation?')){
  if(req.method==='GET'){
   if(holdMemoryHistory){
    holdMemoryHistory=false;
    // The next owner's history read must not wait on a cached GET transaction.
    res.writeHead(503,{'Content-Type':'application/json','Cache-Control':'no-store'});res.flushHeaders();
    rejectHeldMemoryHistory=()=>reply({detail:{code:'FIXTURE_HISTORY_ERROR',message:'Old account history failed'}},503);memoryHistoryHeld();return;
   }
   reply({runs:suggestionRun?[suggestionRun]:[],count:suggestionRun?1:0});return;
  }
  if(!structuredGates.consolidation){reply({detail:{code:'STRUCTURED_MEMORY_DISABLED',message:'structured consolidation is disabled; selected episodes are not summarized'}},409);return;}
  reply({id:'run_labeled_alpha',state:'done',proposal_count:0,candidate_count:0,rejected_count:0,auto:false});return;
 }
 if(path.startsWith('/api/structured-memory/search')){
  if(!structuredGates.retrieval){reply({detail:{code:'STRUCTURED_MEMORY_RETRIEVAL_DISABLED',message:'structured retrieval is disabled; FTS search requires the retrieval gate'}},409);return;}
  const q=new URL(path,'http://fixture.invalid').searchParams.get('q')||'';
  const hits=q.trim()?[ftsHit]:[];
  reply({owner_id:'user_fixture_owner',hits,count:hits.length,retrieval:true,fts:true,type:'untrusted_background_context',scope:'search candidates only; not injected unless retrieve/auto_retrieval picks them'});return;
 }
 if(path==='/api/chat'){
  if(mode==='stream' || mode==='stream-error'){
   assert.equal(req.headers.accept,'text/event-stream');
   const fail=mode==='stream-error';
   res.writeHead(200,{'Content-Type':'text/event-stream'});
   const first=Buffer.from('data: '+JSON.stringify({type:'delta',text:'STREAM_PARTIAL hé'})+'\n\n');
   const split=first.indexOf(Buffer.from('é'))+1;
   res.write(first.subarray(0,split));
   setTimeout(()=>res.write(first.subarray(split)),20).unref();
   setTimeout(()=>res.end('data: '+JSON.stringify(fail?{type:'error',error:{detail:{code:'HARNESS_LLM_ERROR',message:'stream failure',details:{}}},headers:[]}:{type:'done',data:{reply:'STREAM_COMPLETE',session_id:body.session_id,model:'mock',usage:{prompt_tokens:1,completion_tokens:2},tally:{total:3}}})+'\n\n'),500).unref();
   return;
  }
  if(mode==='delay'){setTimeout(()=>reply({reply:'LATE_OLD_REPLY',session_id:body.session_id,model:'mock',usage:{prompt_tokens:1,completion_tokens:2},tally:{total:3}}),500).unref();return;}
  if(mode==='rate'){reply({detail:{code:'LOOP_RATE_LIMIT',message:'rate fixture'}},429);return;}
  if(mode==='failure'){reply({detail:{code:'HARNESS_LLM_ERROR',message:'failure fixture'}},502);return;}
  const chat={reply:mode==='done'?'GOAL_DONE':mode==='repeat'?'same':'reply '+requests.length,session_id:body.session_id,model:'mock',usage:{prompt_tokens:1,completion_tokens:tokens},tally:{total:tokens}};
  if(sessions.has(body.session_id)) {
   const s=sessions.get(body.session_id); s.tokens.total+=1+tokens; chat.tally={...s.tokens};
  }
  if(body.retrieve)chat.structured_facts={injected:structuredGates.retrieval?[ftsHit]:[],source:'fts',retrieval:!!structuredGates.retrieval};
  reply(chat);return;
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
 let seq=0;const pending=new Map();let loadResolve=null;const pageErrors=[];
 ws.onmessage=e=>{
  const v=JSON.parse(e.data);
  if(v.method==='Page.loadEventFired'&&loadResolve){const done=loadResolve;loadResolve=null;done();}
  if(v.method==='Runtime.exceptionThrown')pageErrors.push((v.params&&v.params.exceptionDetails&&(v.params.exceptionDetails.text||v.params.exceptionDetails.exception&&v.params.exceptionDetails.exception.description))||'exception');
  if(v.id){const p=pending.get(v.id);if(!p)return;pending.delete(v.id);v.error?p.reject(Error(JSON.stringify(v.error))):p.resolve(v.result);}
 };
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params}));});
 const afterLoad=async work=>{const loaded=new Promise(r=>{loadResolve=r;});await work();await Promise.race([loaded,pause(8000)]);};
 const evaluate=async expression=>{
  for(let attempt=0;attempt<10;attempt++){
   try{
    const v=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});
    if(v.exceptionDetails)throw Error(JSON.stringify(v.exceptionDetails));
    return v.result.value;
   }catch(err){
    const msg=String(err&&err.message||err);
    if(attempt<9&&/Inspected target navigated or closed/.test(msg)){await pause(80);continue;}
    throw err;
   }
  }
 };
 const until=async expression=>{for(let i=0;i<150;i++){const v=await evaluate(expression);if(v)return v;await pause(50);}throw Error('Timed out: '+expression);};
 const send=command=>evaluate('document.getElementById("input").value='+JSON.stringify(command)+';onSend()');
 const chatCount=()=>requests.filter(r=>r[1]==='/api/chat').length;
 await call('Page.enable');await call('Runtime.enable');
 await afterLoad(()=>call('Page.navigate',{url:base}));
 await until('typeof onSend === "function"');
 await until('document.getElementById("sSoulV").textContent === "unset"');
 await until('document.getElementById("hAuthWho").textContent.includes("authentication disabled")');
 assert.equal(await evaluate('document.getElementById("signInScreen").hidden'),true,'disabled-auth homes must not advertise a login');
 assert.equal(await evaluate('document.getElementById("passwordDialog").open'),false,'disabled-auth homes must not offer setup');
 {
  await call('Emulation.setDeviceMetricsOverride',{width:1200,height:900,deviceScaleFactor:1,mobile:false});
  const width=()=>evaluate('document.querySelector(".sidebar").getBoundingClientRect().width');
  const drag=async x=>{
   const edge=await evaluate('(()=>{const r=document.getElementById("sidebarResize").getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+80};})()');
   await call('Input.dispatchMouseEvent',{type:'mousePressed',x:edge.x,y:edge.y,button:'left',clickCount:1});
   await call('Input.dispatchMouseEvent',{type:'mouseMoved',x,y:edge.y,button:'left',buttons:1});
   await call('Input.dispatchMouseEvent',{type:'mouseReleased',x,y:edge.y,button:'left'});
  };
  assert.equal(await evaluate('document.getElementById("sidebarResize")?.getAttribute("role")'),'separator');
  await drag(420);
  assert.ok(Math.abs(await width()-420)<10,'drag right expands the sidebar');
  await drag(260);
  assert.ok(Math.abs(await width()-260)<10,'drag left shrinks the sidebar');
  await evaluate('selectSideTab("memory");document.getElementById("pane-memory").innerHTML="<input id=reviewDraft value=keep-my-reason>"');
  await drag(30);
  assert.equal(await width(),0,'drag far left hides the pane');
  assert.equal(await evaluate('document.getElementById("sidebarToggle").textContent'),'>>');
  await evaluate('document.getElementById("sidebarToggle").click()');
  assert.ok(Math.abs(await width()-260)<10,'restore retains the last chosen width');
  assert.equal(await evaluate('document.getElementById("reviewDraft").value'),'keep-my-reason','collapse preserves the active pane and draft');
  const beforeKeyboard=await width();
  await evaluate('document.getElementById("sidebarResize").focus()');
  await call('Input.dispatchKeyEvent',{type:'keyDown',key:'ArrowRight'});
  assert.equal(await width(),beforeKeyboard+24,'keyboard arrows resize the sidebar');
  await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Home'});
  assert.equal(await width(),0);
  assert.equal(await evaluate('document.activeElement.id'),'sidebarToggle','collapse leaves a visible keyboard focus target');
  await evaluate('document.getElementById("sidebarToggle").click()');
  await call('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  assert.ok(await width()<=390,'the narrow layout overrides the desktop width');
  assert.equal(await evaluate('getComputedStyle(document.getElementById("sidebarResize")).display'),'none','horizontal resizing is unavailable in the stacked layout');
  assert.equal(pageErrors.length,0,'sidebar must not throw: '+pageErrors.join('; '));
  await call('Emulation.clearDeviceMetricsOverride');
  await evaluate('sidebarWidth=null;sidebar.style.removeProperty("--sidebar-width");selectSideTab("commands")');
  console.log('Sidebar browser acceptance passed: pointer resize, collapse/restore, review draft, keyboard and narrow layout.');
 }
 if(!process.argv.includes('--sidebar-only')) {
 // Header: Analytics once the account state is known; automation only when its probe reports it enabled.
 await until('!document.getElementById("openAnalytics").hidden');
 const firstProbe=requests.findIndex(r=>r[1]==='/api/notifications'||r[1]==='/api/tools');
 assert.ok(firstProbe>requests.findIndex(r=>r[1]==='/api/auth/whoami'),'feature probes wait for the account state');
 assert.equal(await evaluate('refreshHeaderFeatures(true).then(()=>'+headerState+')'),'openAnalytics','unknown {} probe replies keep automation hidden');
 assert.equal(await evaluate('document.getElementById("openSpend")'),null,'Spend is folded into Analytics');
 assert.equal(await evaluate('document.getElementById("openDeliveries").textContent'),'Job webhooks');
 assert.ok(await evaluate('document.getElementById("analyticsTokensPanel").contains(document.getElementById("predictSpend"))'));
 await evaluate('document.getElementById("input").value="estimate this draft";document.getElementById("openAnalytics").click()');
 await until('document.getElementById("analyticsDialog").open && document.getElementById("analyticsStatus").textContent.startsWith("Updated")');
 await evaluate('document.getElementById("predictSpend").click()');
 await until('document.getElementById("spendPrediction").textContent.startsWith("mock-cloud: $0.000125")');
 assert.equal(requests.filter(r=>r[1]==='/api/spend/predict').at(-1)[2].message,'estimate this draft');
 await evaluate('document.getElementById("closeAnalytics").click()');
 assert.equal(await evaluate('document.getElementById("spendPrediction").textContent'),'','closing Analytics clears the estimate');
 assert.equal(await evaluate('document.activeElement.id'),'openAnalytics','focus returns to the header button');
 await evaluate('document.getElementById("input").value=""');
 assert.ok(await evaluate('document.body.innerText.includes("Chat starts without an assigned repository")'));
 assert.equal(await evaluate('document.body.innerText.includes("agentic GitHub coding")'),false);
 await evaluate('agent("Branding fixture")');
 assert.ok(await evaluate('document.body.innerText.includes("CG Agent Harness")'));
 assert.equal(await evaluate('document.body.innerText.toLowerCase().includes("cyclaw")'),false);
 // A fresh console has no session, but choosing a style starts one and saves the choice.
 await until('[...document.getElementById("sStyle").options].some(option=>option.value==="concise")');
 assert.equal(await evaluate('currentSession'),null);
 assert.equal(await evaluate('document.getElementById("sStyle").disabled'),false,'style menu opens before the first session');
 await evaluate('document.getElementById("sStyle").value="concise";document.getElementById("sStyle").dispatchEvent(new Event("change",{bubbles:true}))');
 await until('currentSession && document.getElementById("sStyle").value==="concise"');
 assert.equal(sessions.size,1,'choosing a style creates one session');
 assert.equal([...sessions.values()][0].style,'concise','the new session persists the selected style');
 assert.equal(requests.filter(r=>r[0]==='POST'&&r[1]==='/api/style').length,1,'the chosen style is posted once');
 await evaluate('document.getElementById("sStyle").value="technical";document.getElementById("sStyle").dispatchEvent(new Event("change",{bubbles:true}))');
 await until('currentStyle==="technical" && document.getElementById("sStyle").disabled===false');
 assert.equal(sessions.size,1,'later style changes reuse the session');
 assert.equal([...sessions.values()][0].style,'technical');
 sessions.clear();sequence=0;
 await afterLoad(()=>call('Page.reload',{ignoreCache:true}));
 await until('typeof onSend === "function" && currentSession===null');

 const menu = await evaluate('[...document.querySelectorAll("#pane-commands .c")].map(node=>node.textContent)');
 assert.deepEqual([...new Set(menu.map(command=>command.split(' ')[0]))].sort(),canonicalCommands,'every canonical slash root must be discoverable');
 assert.deepEqual(menu,[...menu].sort(),'command rows must render alphabetically with related variants together');
 for(const usage of ['/agent approve <run-id> <reason>','/agent confirm <reason>','/agent jobs','/agent pr-body <run-id>','/agent publish <run-id> <reason>','/agent push <run-id> <reason>','/agent stop [job-id]','/memory capture|recall|retrieval on|off','/session info','/style [<name>|off]','/web pages [--group name] <query>'])assert.ok(menu.includes(usage),'missing command usage: '+usage);
 const beforeMenuWrites=requests.filter(r=>r[0]==='POST').length;
 await evaluate('[...document.querySelectorAll("#pane-commands .cmd-item")].find(node=>node.querySelector(".c").textContent==="/agent approve <run-id> <reason>").click()');
 assert.equal(await evaluate('document.getElementById("input").value'),'/agent approve ','menu clicks retain the subcommand for editing');
 assert.equal(await evaluate('pendingAgentRun'),null);
 assert.equal(requests.filter(r=>r[0]==='POST').length,beforeMenuWrites,'menu clicks must not dispatch commands');
 // The pane folds the catalog by family; every row's ? opens that command's manual, which only inserts.
 assert.ok(await evaluate('document.querySelectorAll("#pane-commands .cmd-family:not(.cmd-tips)").length')>1,'the sidebar folds commands by family');
 assert.ok(await evaluate('document.querySelectorAll("#pane-commands .cmd-help").length')>=menu.length,'every catalog row has a help button');
 await evaluate('[...document.querySelectorAll("#pane-commands .cmd-help")].find(b=>b.getAttribute("aria-label")==="Help for /agent approve <run-id> <reason>").click()');
 assert.equal(await evaluate('document.getElementById("commandHelpDialog").open'),true,'? opens the command manual');
 assert.equal(await evaluate('document.getElementById("commandHelpTitle").textContent'),'/agent approve <run-id> <reason>');
 const manual=await evaluate('document.getElementById("commandHelpBody").innerText');
 for(const part of ['SYNOPSIS','DESCRIPTION','ARGUMENTS','<run-id>','<reason>','asks you to repeat it','SEE ALSO','/agent status <run-id>'])assert.ok(manual.includes(part),'manual missing: '+part);
 await evaluate('[...document.querySelectorAll("#commandHelpBody button")].find(b=>b.textContent==="/agent status <run-id>").click()');
 assert.equal(await evaluate('document.getElementById("commandHelpTitle").textContent'),'/agent status <run-id>','See also opens the related manual in place');
 assert.equal(requests.filter(r=>r[0]==='POST').length,beforeMenuWrites,'reading a manual sends nothing');
 await evaluate('document.getElementById("commandHelpInsert").click()');
 assert.equal(await evaluate('document.getElementById("commandHelpDialog").open'),false,'Insert closes the manual');
 assert.equal(await evaluate('document.getElementById("input").value'),'/agent status ','Insert fills the composer with the fixed prefix, never sends');
 assert.equal(await evaluate('document.activeElement.id'),'input','focus moves to the composer');
 await evaluate('[...document.querySelectorAll("#pane-commands .cmd-help")].find(b=>b.getAttribute("aria-label")==="Help for /skill check:<profile>").click()');
 await evaluate('document.getElementById("commandHelpInsert").click()');
 assert.equal(await evaluate('document.getElementById("input").value'),'/skill check:','a one-word form keeps its colon on insert');
 await evaluate('[...document.querySelectorAll("#pane-commands .cmd-item")].find(node=>node.querySelector(".c").textContent==="/skill check:<profile>").click()');
 assert.equal(await evaluate('document.getElementById("input").value'),'/skill check:','the catalog row inserts the same form');
 assert.equal(requests.filter(r=>r[0]==='POST').length,beforeMenuWrites,'the manual never dispatches');
 await evaluate('document.getElementById("input").value=""');
 await send('/help');
 assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("Choose a topic")'),'default help must be an overview, not the complete table');
 await send('/help all');
 assert.deepEqual(await evaluate('[...document.querySelectorAll("#stream .msg:last-child .c")].map(node=>node.textContent)'),menu,'help all and the sidebar must expose the same catalog');
 await evaluate('(()=>{const search=document.querySelector("#stream .msg:last-child input[type=search]");search.value="--group";search.dispatchEvent(new Event("input"));})()');
 assert.ok(await evaluate('[...document.querySelectorAll("#stream .msg:last-child .c")].every(node=>node.textContent.includes("--group"))'),'help filters flags');
 await send('/agent');
 assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("/agent runs")'),'agent help must include retained runs');
 await send('/web help');
 assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("/web deny <id-or-pattern>")'));
 for(const command of ['/web deny','/web pages','/web research']){
  await send(command);assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes('+JSON.stringify(command)+')'),'refusal must identify the requested command');
 }
 const beforeSuggestion=requests.length;
 await evaluate('document.querySelector("#stream .msg:last-child button").click()');
 assert.equal(await evaluate('input.value'),'/help web');
 assert.equal(requests.length,beforeSuggestion,'suggestion click only inserts, never executes');
 await send('/web inject');
 assert.equal(requests.filter(r=>r[1]==='/api/web/inject').length,1,'inject must reach its fixed endpoint');
 assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("first successful page")'),'inject must explain its single-page scope');
 assert.equal(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("0 chars")'),false,'missing API metadata is not a zero-length extract');
 assert.equal(requests.some(r=>r[0]==='POST'&&r[1].startsWith('/api/agent/')),false,'discovery must not execute agent work');

 await send('/agent run codex/parser-fixture Keep this test request staged');
 assert.ok(await evaluate('!!pendingAgentRun'),'exercise parser refusals with an executable request staged');
 // Saving a typed line for ArrowUp recall is not command dispatch.
 const historySave=r=>r[1].endsWith('/prompt-history');
 const mutationRequests=()=>requests.filter(r=>r[0]==='POST'&&r[1]!=='/api/slash/parse'&&!historySave(r));
 const beforeParserRefusals=mutationRequests().length;
 for(const response of ['suggest','error','invalid']){
  slashMode=response;
  for(const command of ['/memory clear --help','/memory capture off --dry-run','/memory please retrieve private preference','/agent confirm --help'])await send(command);
  assert.equal(mutationRequests().length,beforeParserRefusals,'parser '+response+' must not fall back to raw mutation dispatch');
  assert.ok(await evaluate('!!pendingAgentRun'),'declining a command must preserve the staged request');
 }
 assert.ok(await evaluate('document.querySelector("#stream .msg:last-child").textContent.includes("command not dispatched")'));
 assert.ok(requests.some(r=>historySave(r)&&r[2].text==='/agent confirm --help')||await evaluate('promptHistory.prompts.includes("/agent confirm --help")'),'a refused command stays recallable');
 slashMode='suggest';
 await evaluate('runSlashMaybeFuzzy("/memory clear\\n")');
 assert.equal(requests.filter(r=>r[1]==='/api/slash/parse').at(-1)[2].line,'/memory clear\n','slash wrapper must preserve command boundaries');
 for(const pasted of ['/memory\n clear','/memory\tclear','/web allow https://example.org/*\n/web on','/agent\nconfirm operator-approved','/api\tclear GROK_API_KEY','/session\u2028new','/agent push\ufeffabc --dry-run','\ufeff/model list','/memory save a\u202eb :: why','/session rename a\u200bb']) {
  assert.equal(await evaluate('(()=>{input.value="";const data=new DataTransfer();data.setData("text/plain",'+JSON.stringify(pasted)+');return input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,cancelable:true}));})()'),false,'ambiguous slash-command paste must be prevented before native input normalization');
 }
 assert.equal(await evaluate('(()=>{input.value="/memory ";input.setSelectionRange(8,8);const data=new DataTransfer();data.setData("text/plain","clear\\n");return input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,cancelable:true}));})()'),false,'partial paste must consider existing command text');
 // A line copied from a reply's code block keeps its command, minus the edge line break.
 assert.equal(await evaluate('(()=>{input.value="";const data=new DataTransfer();data.setData("text/plain","\\n/memory clear\\n");input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,cancelable:true}));return input.value;})()'),'/memory clear','edge line breaks are trimmed, not refused');
 assert.equal(await evaluate('(()=>{input.value="";const data=new DataTransfer();data.setData("text/plain","/agent cancel");return input.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,cancelable:true}));})()'),true,'exact single-line paste remains available');
 slashMode='dispatch';await send('/agent cancel');

 mode='stream';const streamed=send('stream fixture');
 await until('document.getElementById("stream").innerText.includes("STREAM_PARTIAL hé")');
 assert.ok(await evaluate('!!inflightChat'));
 await streamed;
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("STREAM_COMPLETE")'));
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("STREAM_PARTIAL")'),false);
 mode='stream-error';await send('stream error fixture');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("stream failure")'));
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("STREAM_PARTIAL")'),false);
 mode='normal';

 await send('/soul edit');assert.equal(await evaluate('document.getElementById("soulEditor").open'),true);
 await evaluate('document.getElementById("soulContent").value="BROWSER_PERSONA";document.getElementById("soulReason").value="Reviewed UI edit";document.getElementById("soulPreview").click()');
 await until('document.getElementById("soulFeedback").textContent.includes("BROWSER_PERSONA")');assert.equal(persona,'');
 await evaluate('document.getElementById("soulSave").click()');await until('document.getElementById("soulFeedback").textContent.includes("Confirmation required")');assert.equal(persona,'');
 await evaluate('document.getElementById("soulConfirm").checked=true;document.getElementById("soulSave").click()');await until('document.getElementById("soulFeedback").textContent.startsWith("Saved.")');assert.equal(persona,'BROWSER_PERSONA');
 await evaluate('document.getElementById("soulClose").click()');assert.equal(await evaluate('document.getElementById("soulContent").value'),'');
 await send('/prompt');assert.ok(await evaluate('document.getElementById("stream").innerText.includes("BROWSER_PERSONA")'));
 await send('/goal Review a patch');assert.equal(sessions.size,1);assert.equal(await evaluate('sessionGoal'),'Review a patch');
 const beforeMemoryOn=requests.filter(r=>r[1]==='/api/structured-memory/gates').length;
 await send('/memory on');
 assert.equal(requests.filter(r=>r[1]==='/api/structured-memory/gates').length,beforeMemoryOn,'/memory on must not flip structured gates');
 assert.equal(structuredGates.retrieval,false);
 for(const line of ['/memory','/memory status']){
  await evaluate('document.getElementById("stream").replaceChildren()');
  await send(line);
  await until('document.getElementById("stream").innerText.includes("pending proposals")');
  const readout=await evaluate('document.getElementById("stream").innerText');
  assert.equal(readout.includes('RAG'),false,line+' must not show the always-false RAG rows');
  assert.equal(readout.includes('undefined'),false,line+' must not render missing fields');
  assert.ok(readout.includes('1 / 64'),line+' shows the owner fact count against the cap');
  assert.ok(readout.includes('omits 2 setting(s) that ship on')&&readout.includes('structured_memory.auto_retrieval'),line+' warns about config drift');
 }
 const factReads=()=>requests.filter(r=>r[0]==='GET'&&r[1]==='/api/structured-memory/facts').length;
 const nonParsePosts=()=>requests.filter(r=>r[0]==='POST'&&r[1]!=='/api/slash/parse'&&!historySave(r)).length;
 const beforeFactsWrites=nonParsePosts();
 await evaluate('document.getElementById("stream").replaceChildren()');
 await send('/memory facts');
 await until('document.getElementById("stream").innerText.includes("fact_labeled_saved")');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("<script>throw")'),'fact content renders as literal text');
 assert.equal(await evaluate('document.querySelectorAll("#stream script").length'),0,'fact content must stay inert text');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("not pinned notes")'));
 assert.equal(nonParsePosts(),beforeFactsWrites,'/memory facts is read-only');
 const gateCommands=[['capture','episode_capture'],['recall','explicit_recall'],['retrieval','retrieval'],['auto-retrieve','auto_retrieval'],['consolidation','consolidation'],['auto-consolidate','auto_consolidation'],['auto-suggest-chat','auto_suggest_chat'],['auto-suggest-coding','auto_suggest_coding']];
 for(const [command,gate] of gateCommands){await send('/memory '+command+' on');assert.equal(structuredGates[gate],true,command+' on must persist the mapped gate');}
 for(const [command,gate] of gateCommands.toReversed()){await send('/memory '+command+' off');assert.equal(structuredGates[gate],false,command+' off must persist the mapped gate');}
 const beforeRememberChat=chatCount();
 const memoryWrites=()=>requests.filter(r=>r[0]==='POST'&&r[1].startsWith('/api/structured-memory/')).length;
 const beforeSave=memoryWrites();
 await send('/memory save Durable fact.');await send('/memory save Durable fact. ::   ');
 assert.equal(memoryWrites(),beforeSave);
 await send('/memory save Prefer  metric units. :: Explicit operator save');
 assert.deepEqual(requests.filter(r=>r[0]==='POST'&&r[1]==='/api/structured-memory/facts').at(-1)[2],{content:'Prefer  metric units.',category:'manual',reason:'Explicit operator save',confirm:true});
 assert.equal(chatCount(),beforeRememberChat,'manual save is not chat or consolidation');
 let beforeRemember=memoryWrites();
 await send('/memory remember Prefer metric units.');
 await send('/memory remember Prefer metric units. ::   ');
 await send('/memory remember :: operator reason');
 assert.equal(memoryWrites(),beforeRemember,'missing summary/reason must not write');
 await send('/memory remember Prefer  metric units. :: operator reviewed');
 assert.equal(latestEpisode.semantic_summary,'Prefer  metric units.','remember preserves the sentence remainder');
 assert.equal(requests.filter(r=>r[1].endsWith('/summary')).at(-1)[2].reason,'operator reviewed');
 assert.equal(chatCount(),beforeRememberChat,'remember must not send a chat or consolidate');
 assert.equal(requests.filter(r=>r[1]==='/api/structured-memory/consolidation').length,0);
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("run /memory consolidate episode_labeled_latest")'));
 beforeRemember=memoryWrites();latestEpisode=null;
 await send('/memory remember Prefer metric units. :: operator reviewed');
 assert.equal(memoryWrites(),beforeRemember);
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("Capture is off")'));
 structuredOpen=false;
 await send('/memory remember Prefer metric units. :: operator reviewed');
 assert.equal(memoryWrites(),beforeRemember);
 await send('/memory proposals');
 await until('document.getElementById("pane-memory").textContent.includes("Saved memory is unavailable")');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory form").length'),0);
 const closedFactReads=factReads();
 await send('/memory facts');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("store is closed; no facts to list")'));
 assert.equal(factReads(),closedFactReads,'a closed store is reported without reading facts');
 structuredOpen=true;
 await send('/memory proposals');
 await until('document.querySelectorAll("#pane-memory form").length === 2');
 await until('document.getElementById("pane-memory").textContent.includes("Saved facts (1)")');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory section.saved-facts form, #pane-memory section.saved-facts button").length'),0,'saved facts are listed read-only, outside the proposal forms');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory script").length'),0,'proposal and fact content must stay inert text');
 // Completion polling must update outcomes alongside existing proposals without rebuilding review inputs.
 automaticSuggestions={chat:true,coding:true,mode:'both',queued:1,running:false,poll_interval_ms:250,last_attempt:{source:'chat',created_ts:1,episode_id:'episode_polled',queued:true,run_id:'run_polled'}};
 await evaluate('refreshMemoryProposals()');
 await evaluate('window.keptMemoryInput=document.querySelector("#pane-memory form input");keptMemoryInput.value="Review still being written";keptMemoryInput.focus();document.querySelector("#pane-memory form details").open=true;window.keptFactDetail=document.querySelector("#pane-memory .saved-facts details");keptFactDetail.open=true');
 const arrived={id:'proposal_polled',revision:'revision_polled',action:'add',category:'insight',content:'New proposal arrived after generation.',source_episode_ids:['episode_polled'],status:'pending'};
 memoryProposals.push(arrived);
 automaticSuggestions={...automaticSuggestions,queued:0};
 suggestionRun={id:'run_polled',created_ts:1,episode_ids:['episode_polled'],model_id:'fixture-local-b',summarizer_version:'completion-suggestions-v1',state:'done',proposal_ids:['proposal_polled'],proposal_count:1,candidate_count:1,rejected_count:0};
 await until('document.querySelectorAll("#pane-memory form").length === 3 && document.getElementById("pane-memory").textContent.includes("fixture-local-b")');
 assert.equal(await evaluate('document.querySelector("#pane-memory form input")===keptMemoryInput && keptMemoryInput.value==="Review still being written" && document.activeElement===keptMemoryInput'),true,'automatic arrival preserves the actual input, typed reason and focus');
 assert.equal(await evaluate('document.querySelector("#pane-memory form details").open && keptFactDetail.isConnected && keptFactDetail.open'),true,'unchanged proposal and saved-fact details stay expanded across polling');
 await evaluate('clearTimeout(memoryRefreshTimer)');await until('!memoryRefreshBusy');await evaluate('clearTimeout(memoryRefreshTimer)');
 automaticSuggestions={...automaticSuggestions,queued:1,poll_interval_ms:30000,last_attempt:{source:'chat',created_ts:2,queued:false,reason:'source_disabled'}};
 suggestionRun={...suggestionRun,id:'newer_summary',created_ts:3,summarizer_version:'structured-memory-v1'};
 await evaluate('refreshMemoryProposals()');
 assert.equal(await evaluate('document.querySelector("#pane-memory .memory-latest").textContent'), 'Latest activity: Created 1 suggestion.','a newer summary result replaces the older completion skip, even if an older queue entry remains');
 automaticSuggestions.last_attempt={source:'chat',created_ts:4,queued:false,reason:'source_disabled'};
 await evaluate('refreshMemoryProposals()');
 assert.ok(await evaluate('document.querySelector("#pane-memory .memory-latest").textContent.includes("Automatic suggestions are off")'),'a newer skipped completion stays correlated instead of displaying an old summary or unrelated queue');
 automaticSuggestions=null;
 await evaluate('clearTimeout(memoryRefreshTimer)');await until('!memoryRefreshBusy');await evaluate('clearTimeout(memoryRefreshTimer)');
 arrived.status='rejected';suggestionRun=null;
 await evaluate('refreshMemoryProposals();keptMemoryInput.value="";keptMemoryInput.blur()');
 await until('document.querySelectorAll("#pane-memory form").length === 2 && !memoryRefreshBusy');
 // A delayed facts snapshot must not overwrite a newer post-Apply read.
 holdMemoryFacts=true;const factsRequestSeen=new Promise(resolve=>{memoryFactsHeld=resolve;});
 await evaluate('window.oldFactsLoad=appendSavedFacts(document.getElementById("pane-memory"));true');
 await Promise.race([factsRequestSeen,pause(5000).then(()=>{throw Error('facts fixture request not held');})]);
 savedFacts.unshift({id:'fact_after_newer_read',revision:1,category:'insight',content:'NEWER_FACT_SNAPSHOT',active:true});
 await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
 releaseHeldMemoryFacts();await evaluate('window.oldFactsLoad');
 assert.ok(await evaluate('document.getElementById("pane-memory").textContent.includes("NEWER_FACT_SNAPSHOT")'),'an older facts response cannot erase a newer read');
 savedFacts.shift();await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
 const decisions=()=>requests.filter(r=>r[0]==='POST'&&r[1].startsWith('/api/structured-memory/proposals/'));
 await evaluate('document.querySelector("#pane-memory form button").click()');
 assert.equal(decisions().length,0,'empty reason cannot apply');
 await evaluate('document.querySelector("#pane-memory form input").value=" ";document.querySelector("#pane-memory form button").click()');
 assert.equal(decisions().length,0,'whitespace reason cannot apply');
 await evaluate('document.querySelector("#pane-memory form input").value="Reviewed preference";document.querySelector("#pane-memory form button").click()');
 await until('document.getElementById("pane-memory").textContent.includes("Memory saved.")');
 assert.ok(await evaluate('document.getElementById("pane-memory").textContent.includes("Saved facts below")'),'Apply names where the fact went');
 await until('document.getElementById("pane-memory").textContent.includes("Saved facts (2)")');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory section.saved-facts").length'),1,'Apply re-lists saved facts in place');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory script").length'),0,'an applied fact stays inert text');
 assert.equal(decisions().at(-1)[2].apply,true);
 await evaluate('document.querySelectorAll("#pane-memory form")[1].querySelector("input").value="Not durable";document.querySelectorAll("#pane-memory form")[1].querySelectorAll("button")[1].click()');
 await until('document.getElementById("pane-memory").textContent.includes("Suggestion dismissed.")');
 assert.equal(decisions().at(-1)[2].apply,false);
 assert.equal(chatCount(),beforeRememberChat);
 suggestionRun={id:'run_empty',created_ts:1,episode_ids:['episode_labeled_latest'],model_id:'fixture-local-b',proposal_ids:[],summarizer_version:'completion-suggestions-v1',state:'done',proposal_count:0,candidate_count:0,rejected_count:0};
 await evaluate('refreshMemoryProposals()');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory form").length'),0,'one refresh removes every adjacent decided proposal');
 assert.ok(await evaluate('document.getElementById("pane-memory").textContent.includes("No lasting details found")'));
 suggestionRun={...suggestionRun,state:'failed',error_class:'STRUCTURED_MEMORY_SCHEMA'};
 await evaluate('refreshMemoryProposals()');
 assert.ok(await evaluate('document.getElementById("pane-memory").textContent.includes("STRUCTURED_MEMORY_SCHEMA")'));
 await evaluate('Promise.all([refreshMemoryProposals(),refreshMemoryProposals()])');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory section.saved-facts").length'),1,'overlapping refreshes leave one saved-facts section');
 // A change/removal needs a readable target, including when the facts snapshot
 // has not changed since the preceding refresh. Loading must preserve typing.
 const originalTarget=savedFacts.find(f=>f.id==='fact_labeled_saved');
 const targetProposals=[
  {id:'proposal_target_update',revision:'target_update',action:'update',target_fact_id:originalTarget.id,expected_revision:originalTarget.revision,expected_digest:originalTarget.content_digest,content:'Updated preference',status:'pending'},
  {id:'proposal_target_remove',revision:'target_remove',action:'deactivate',target_fact_id:originalTarget.id,expected_revision:originalTarget.revision,expected_digest:originalTarget.content_digest,content:null,status:'pending'},
  {id:'proposal_target_missing',revision:'target_missing',action:'deactivate',target_fact_id:'fact_no_longer_active',expected_revision:1,expected_digest:'missing_digest',content:null,status:'pending'},
 ];
 memoryProposals.push(...targetProposals);
 holdMemoryFacts=true;const targetReadSeen=new Promise(resolve=>{memoryFactsHeld=resolve;});
 await evaluate('window.targetRefresh=refreshMemoryProposals();true');
 await Promise.race([targetReadSeen,pause(5000).then(()=>{throw Error('target facts request not held');})]);
 await evaluate('window.targetUpdate=document.querySelector(\'[data-proposal="proposal_target_update:target_update"]\');window.targetRemove=document.querySelector(\'[data-proposal="proposal_target_remove:target_remove"]\');window.targetMissing=document.querySelector(\'[data-proposal="proposal_target_missing:target_missing"]\');window.targetReason=targetUpdate.querySelector("input");targetReason.value="Keep the corrected preference";targetReason.focus();targetUpdate.querySelector("details").open=true');
 assert.equal(await evaluate('[targetUpdate,targetRemove,targetMissing].every(form=>form.querySelector(".memory-save").disabled)'),true,'target-changing actions wait for the facts read');
 releaseHeldMemoryFacts();await evaluate('window.targetRefresh');
 assert.equal(await evaluate('targetUpdate.querySelector(".memory-save").textContent==="Update memory" && targetRemove.querySelector(".memory-save").textContent==="Remove memory"'),true,'supported update/deactivate actions have distinct approval labels');
 assert.equal(await evaluate('[targetUpdate,targetRemove].every(form=>!form.querySelector(".memory-save").disabled && form.querySelector(".memory-target").textContent.includes("Saved fact <script>"))'),true,'the actual saved target is readable as inert text outside technical details');
 assert.equal(await evaluate('targetMissing.querySelector(".memory-save").disabled && targetMissing.querySelector(".memory-target").textContent.includes("unavailable")'),true,'an absent target cannot be approved');
 assert.equal(await evaluate('targetReason===targetUpdate.querySelector("input") && document.activeElement===targetReason && targetReason.value==="Keep the corrected preference" && targetUpdate.querySelector("details").open'),true,'loading target text preserves the reason node, focus and expanded details');
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory script").length'),0);
 for(const [field,changed] of [['revision',2],['content_digest','changed_digest']]){
  const original=originalTarget[field];originalTarget[field]=changed;
  await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
  assert.equal(await evaluate('[targetUpdate,targetRemove].every(form=>form.querySelector(".memory-save").disabled && !form.querySelectorAll("button")[1].disabled && form.querySelector(".memory-target").textContent.includes("changed since the suggestion"))'),true,'a stale '+field+' blocks changes while allowing dismissal');
  assert.equal(await evaluate('targetReason===targetUpdate.querySelector("input") && document.activeElement===targetReason && targetReason.value==="Keep the corrected preference" && targetUpdate.querySelector("details").open'),true,'stale-target updates preserve the review in progress');
  originalTarget[field]=original;
 }
 await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
 assert.equal(await evaluate('[targetUpdate,targetRemove].every(form=>!form.querySelector(".memory-save").disabled)'),true,'only the exact proposal target revision and digest enable a change');
 holdMemoryDecision=true;const decisionSeen=new Promise(resolve=>{memoryDecisionHeld=resolve;});
 await evaluate('targetRemove.querySelector("input").value="No longer relevant";targetRemove.querySelector(".memory-save").click()');
 await Promise.race([decisionSeen,pause(5000).then(()=>{throw Error('target decision not held');})]);
 await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
 assert.equal(await evaluate('Array.from(targetRemove.querySelectorAll("button")).every(button=>button.disabled)'),true,'a facts poll cannot unlock an in-flight decision');
 releaseHeldMemoryDecision();await until('targetRemove.dataset.reviewState === "decided"');
 await evaluate('appendSavedFacts(document.getElementById("pane-memory"))');
 assert.equal(await evaluate('Array.from(targetRemove.querySelectorAll("button")).every(button=>button.disabled) && targetRemove.querySelector(".memory-target").textContent.includes("Saved fact <script>")'),true,'a completed removal stays locked and retains the text that was reviewed');
 for(const proposal of targetProposals)proposal.status='rejected';
 savedFacts.push(originalTarget);await evaluate('refreshMemoryProposals()');
 // Failure before receiving the polling interval must still schedule a retry.
 await evaluate('clearTimeout(statusTimer);clearTimeout(memoryRefreshTimer);statusBackoffMs=250');
 failMemoryOnce=true;await evaluate('refreshMemoryProposals()');
 assert.ok(await evaluate('document.querySelector("#pane-memory .memory-feedback").textContent.includes("Could not refresh")'));
 const readsBeforeRetry=requests.filter(row=>row[1]==='/api/structured-memory').length;
 for(let i=0;i<100&&requests.filter(row=>row[1]==='/api/structured-memory').length===readsBeforeRetry;i++)await pause(50);
 assert.ok(requests.filter(row=>row[1]==='/api/structured-memory').length>readsBeforeRetry,'a transient first status failure retries without manual Refresh');
 await until('!memoryRefreshBusy');
 await evaluate('statusBackoffMs=STATUS_BASE_INTERVAL');
 // One real 429 must silence both background loops until its Retry-After.
 await evaluate('clearTimeout(statusTimer);clearTimeout(memoryRefreshTimer)');
 rateLimitMemoryOnce=true;await evaluate('refreshMemoryProposals()');
 assert.ok(await evaluate('apiRetryDelay()>0'));
 const routeReads=path=>requests.filter(row=>row[1]===path).length;
 const statusBeforeCooldown=routeReads('/api/status'),memoryBeforeCooldown=routeReads('/api/structured-memory');
 await evaluate('statusBackoffMs=50;scheduleStatusRefresh();refreshStatus();refreshMemoryProposals();true');
 await pause(400);
 assert.equal(routeReads('/api/status'),statusBeforeCooldown,'status polling respects the memory request Retry-After');
 assert.equal(routeReads('/api/structured-memory'),memoryBeforeCooldown,'memory polling does not retry early');
 for(let i=0;i<100&&(routeReads('/api/status')===statusBeforeCooldown||routeReads('/api/structured-memory')===memoryBeforeCooldown);i++)await pause(50);
 assert.ok(routeReads('/api/status')>statusBeforeCooldown&&routeReads('/api/structured-memory')>memoryBeforeCooldown,'both loops resume after the shared cooldown');
 await until('!memoryRefreshBusy && statusBackoffMs === STATUS_BASE_INTERVAL');
 await evaluate('finishChat({reply:"synthetic reply",model:"mock",memory_suggestion:{queued:true}})');
 assert.ok(await evaluate('document.getElementById("stream").textContent.includes("Checking this activity for useful memories")'));
 await evaluate('document.getElementById("stream").replaceChildren()');
 await evaluate('finishChat({reply:"second reply",model:"mock",memory_suggestion:{queued:true}})');
 assert.equal(await evaluate('document.getElementById("stream").textContent.includes("Checking this activity for useful memories")'),false,'the queued note shows once per session, not after every turn');
 await evaluate(`document.querySelector('[data-pane="commands"]').click()`);
 const beforeDisabledSearch=chatCount();
 await send('/memory search metric');
 assert.equal(chatCount(),beforeDisabledSearch,'search is not inject when retrieval is off');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("STRUCTURED_MEMORY_RETRIEVAL_DISABLED")'));
 await send('/memory retrieval on');
 assert.equal(structuredGates.retrieval,true);assert.equal(structuredGates.auto_retrieval,false);assert.equal(structuredGates.explicit_recall,false);
 const beforeSearch=chatCount();
 await send('/memory search metric');
 assert.equal(chatCount(),beforeSearch,'FTS search stays candidates-only');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("search is not inject")'));
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("fact_labeled_alpha")'));
 const searchReq=requests.filter(r=>String(r[1]).startsWith('/api/structured-memory/search')).at(-1);
 assert.equal(searchReq[0],'GET');assert.ok(String(searchReq[1]).includes('q=metric'));
 const beforeRetrieve=chatCount();
 await send('/memory retrieve metric');
 assert.equal(chatCount(),beforeRetrieve+1);
 const retrieveReq=requests.filter(r=>r[1]==='/api/chat').at(-1)[2];
 assert.equal(retrieveReq.retrieve,true);assert.equal(retrieveReq.retrieve_query,undefined,'natural-language matching, not all-terms');assert.equal(retrieveReq.message,'metric');
 assert.ok(retrieveReq.session_id,'force-include chat keeps the current session');
 await send('/goal');assert.ok(await evaluate('document.getElementById("stream").innerText.includes("current goal (web: fetch):")'));
 let before=chatCount();await send('/loop 2');assert.equal(chatCount(),before+1);assert.equal(await evaluate('loopState.remaining'),1);
 await send('/loop');assert.equal(chatCount(),before+2);assert.equal(await evaluate('loopState'),null);
 mode='repeat';await send('/loop 3');await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='done';await send('/loop');assert.equal(await evaluate('loopState'),null);assert.ok(await evaluate('document.getElementById("stream").innerText.includes("unverified")'));
 mode='normal';tokens=999999;await evaluate('MAX_LOOP_TOTAL_COMPLETION_TOKENS = 1000');await send('/loop');assert.equal(await evaluate('loopState'),null);await evaluate('MAX_LOOP_TOTAL_COMPLETION_TOKENS = 0');tokens=2;
 mode='rate';await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='failure';await send('/loop');await send('/loop');assert.equal(await evaluate('loopState'),null);
 mode='delay';const generation=send('/loop');await until('!!inflightChat');
 const beforeHelpStop=requests.filter(r=>r[1]==='/api/chat/cancel').length;
 for(const command of ['/loop stop --help','/ loop stop'])await send(command);
 // A text input strips newlines, so exercise that raw wrapper input directly.
 slashMode='error';await evaluate('runSlashMaybeFuzzy('+JSON.stringify('/\nloop stop')+')');slashMode='dispatch';
 // JS \s splits on U+FEFF but the parser does not: hidden characters never earn the local stop bypass.
 for(const code of [0xfeff,0x200b,0x202e])await evaluate('runSlashMaybeFuzzy('+JSON.stringify('/loop'+String.fromCharCode(code)+'stop')+')');
 assert.equal(requests.filter(r=>r[1]==='/api/chat/cancel').length,beforeHelpStop,'malformed stop commands must not trigger the cancellation exception');
 assert.ok(await evaluate('!!inflightChat'));
 await send('/LOOP   STOP');await generation;assert.equal(await evaluate('loopState'),null);
 mode='normal';await send('/loop auto');before=chatCount();const auto=send('/loop 3');await until('loopState && loopState.remaining === 2');slashMode='error';await send('  /loop stop  ');await auto;assert.equal(chatCount(),before+1,'exact stop during cooldown works even when slash parsing is unavailable');slashMode='dispatch';
 await send('/loop auto');await send('/loop 2');await send('/goal clear');assert.equal(await evaluate('loopState'),null);assert.equal(await evaluate('sessionGoal'),'');
 before=chatCount();await send('/loop 2 Compare two vendors');assert.equal(await evaluate('sessionGoal'),'Compare two vendors','goal text sets the goal in the same command');
 assert.equal(chatCount(),before+1);assert.equal(await evaluate('loopState.remaining'),1);await send('/loop stop');
 await send('/loop auto');assert.ok(await evaluate('!!autoLoopStart'),'auto on with a goal schedules a start');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("starts in 5 s")'));
 before=chatCount();await send('/loop stop');assert.equal(await evaluate('autoLoopStart'),null);await pause(5500);assert.equal(chatCount(),before,'a stopped countdown never starts');
 await send('/loop auto');assert.equal(await evaluate('loopAuto'),false,'stop leaves auto on; the next toggle turns it off');
 await send('/loop auto');before=chatCount();await pause(3000);assert.equal(chatCount(),before,'nothing starts inside the countdown');
 await until('loopState && loopState.auto && loopState.total - loopState.remaining >= 1');assert.ok(chatCount()>before,'the countdown starts the loop by itself');
 await send('/loop stop');await send('/loop auto');assert.equal(await evaluate('loopAuto'),false);
 await send('/goal Another goal');await send('/loop 2');await send('/session new');assert.equal(await evaluate('loopState'),null);
 const firstSession=[...sessions.values()][0];
 assert.equal(await evaluate('document.getElementById("hTokens").textContent'),'0','new session starts at zero despite nonzero aggregate');
 const secondSession=await evaluate('currentSession');
 await send('session token fixture');
 assert.equal(await evaluate('document.getElementById("hTokens").textContent'),'3','completed chat updates its own tally');
 firstSession.tokens={total:4321};
 firstSession.messages=Array.from({length:12},(_,i)=>({role:i%2?'assistant':'user',content:'OLD_MESSAGE_'+i}));
 await send('/session use '+firstSession.session_id);
 assert.equal(await evaluate('document.getElementById("hTokens").textContent'),(4321).toLocaleString(),'reopened session restores its own count');
 await evaluate('refreshStatus()');
 assert.equal(await evaluate('document.getElementById("hTokens").textContent'),(4321).toLocaleString(),'status polling cannot overwrite with aggregate');
 await send('/session use '+secondSession);
 assert.equal(await evaluate('document.getElementById("hTokens").textContent'),'3');
 await send('/session use '+firstSession.session_id);
 assert.equal(await evaluate('document.querySelectorAll("#stream .msg.user, #stream .msg.agent").length'),12,'switch renders the selected session, including its older messages');
 assert.equal(await evaluate('document.querySelectorAll("#stream [aria-live=off] .msg.user, #stream [aria-live=off] .msg.agent").length'),12,'restored history sits outside the polite live log');
 assert.equal(await evaluate('document.getElementById("stream").getAttribute("aria-live")'),'polite');
 await send('/session use '+firstSession.session_id);
 assert.equal(await evaluate('document.querySelectorAll("#stream .msg.user, #stream .msg.agent").length'),12,'reselecting a session must not duplicate its transcript');
 // Keyboard recall loads saved prompts, restores drafts, and never crosses session boundaries.
 const arrow=async key=>{
   await evaluate('document.getElementById("input").focus()');
   await call('Input.dispatchKeyEvent',{type:'keyDown',key,code:key});
   await call('Input.dispatchKeyEvent',{type:'keyUp',key,code:key});
   await until('!promptHistory.loading');
 };
 await evaluate('input.value="unsent draft"');
 await arrow('ArrowUp');assert.equal(await evaluate('input.value'),'OLD_MESSAGE_10');
 await arrow('ArrowUp');assert.equal(await evaluate('input.value'),'OLD_MESSAGE_8');
 await arrow('ArrowDown');await arrow('ArrowDown');assert.equal(await evaluate('input.value'),'unsent draft');
 firstSession.prompt_history=Array.from({length:55},(_,i)=>'SAVED_PROMPT_'+i);
 await send('/session use '+firstSession.session_id);
 await evaluate('input.value="draft retained"');
 await arrow('ArrowUp');assert.equal(await evaluate('input.value'),'SAVED_PROMPT_54');
 for(let i=0;i<55;i++)await arrow('ArrowUp');
 assert.equal(await evaluate('input.value'),'SAVED_PROMPT_5','recall is capped at fifty');
 for(let i=0;i<50;i++)await arrow('ArrowDown');
 assert.equal(await evaluate('input.value'),'draft retained');
 await arrow('ArrowUp');
 await evaluate('input.value="edited draft";input.dispatchEvent(new Event("input"))');
 await arrow('ArrowUp');await arrow('ArrowDown');assert.equal(await evaluate('input.value'),'edited draft');
 await send('/session new');await arrow('ArrowUp');assert.equal(await evaluate('input.value'),'','new session cannot recall old prompts');
 await send('/session use '+firstSession.session_id);
 await afterLoad(()=>call('Page.reload',{ignoreCache:true}));
 await until('!!document.getElementById("input")');
 await send('/session use '+firstSession.session_id);await arrow('ArrowUp');
 assert.equal(await evaluate('input.value'),'SAVED_PROMPT_54','recall survives a page reload');
 await send('/agent run codex/old Old staged work');
 await evaluate('shownAgentDiffs.set("old", "diff"); reviewedSoulProposal={id:"old"}; reviewedPRBodies.set("old", "body"); prBodyTarget="old"');
 await evaluate('document.querySelector("#pane-sessions .cmd-item").click()');
 await until('currentSession !== '+JSON.stringify(firstSession.session_id));
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("OLD_MESSAGE_")'),false,'new session clears the old transcript');
 assert.equal(await evaluate('pendingAgentRun'),null);assert.equal(await evaluate('shownAgentDiffs.size'),0);assert.equal(await evaluate('reviewedSoulProposal'),null);
 assert.equal(await evaluate('reviewedPRBodies.size'),0);assert.equal(await evaluate('prBodyTarget'),'');
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
 await afterLoad(()=>call('Page.reload',{ignoreCache:true}));
 await until('typeof onSend === "function" && window.__cgahLoaded!==1');
 await send('/session use '+goalSession);await send('/goal task');assert.equal(await evaluate('pendingAgentRun.goal_stage.session_id'),goalSession);
 await send('/agent confirm');assert.equal(await evaluate('pendingAgentRun.instruction'),'Implement the fixture','missing reason keeps request staged');
 await send('/web search veeam software cve');assert.equal(requests.filter(r=>r[1]==='/api/web/search').at(-1)[2].engine,'google');assert.ok(await evaluate('document.getElementById("stream").innerText.includes("Fixture Google result")'));
 await send('/web pages veeam');assert.equal(requests.filter(r=>r[1]==='/api/web/search').at(-1)[2].engine,'pages');
 await evaluate('renderChatWebTools([{tool:"web_fetch",ok:true,result:{url:"https://veeam.com/kb1",source_chars:123,notice:"Fetched fixture page"}},{tool:"web_search",ok:false,code:"WEB_GOOGLE_CHALLENGE",message:"Google requires interaction"}])');
 assert.ok(await evaluate('document.getElementById("stream").innerText.includes("WEB_GOOGLE_CHALLENGE")'));
 assert.ok(!requests.some(r=>r[0]==='POST' && ['/api/agent/jobs','/api/agent/run'].includes(r[1])),'chat and skill staging never execute coding work');
 // The real browser must execute the minimal-status login and key editor flows.
 const signedOutMark=requests.length;
 authFixture=true;automationFixture={notifications:true,agentic:true};await afterLoad(()=>call('Page.reload',{ignoreCache:true}));
 await until('document.getElementById("passwordDialog").open');
 assert.equal(await evaluate(headerState),'','signed-out consoles show no header automation buttons');
 assert.equal(requests.slice(signedOutMark).some(r=>['/api/notifications','/api/tools'].includes(r[1])),false,'signed-out consoles send no feature probes');
 assert.equal(await evaluate('document.getElementById("passwordTitle").textContent'),'Set your password');
 assert.equal(await evaluate('document.getElementById("passwordCurrentLabel").textContent'),'Setup code','fresh setup requires the private code');
 assert.equal(await evaluate('document.getElementById("passwordCurrent").disabled'),false);
 assert.equal(requests.slice(signedOutMark).some(r=>r[0]==='POST'&&r[1].startsWith('/api/auth/')),false,'opening setup must not create a session');
 assert.equal(await evaluate('document.getElementById("hAuthHint")'),null,'the old setup-code instructions are absent');
 assert.equal(await evaluate('document.querySelector(".main").offsetParent'),null,'signed out, the console is not drawn');
 assert.equal(await evaluate('document.querySelector("label[for=hAuthUser]").textContent'),'Username');
 assert.equal(await evaluate('document.querySelector("[data-pane=registry]")'),null);
 assert.equal(await evaluate(headerState),'','a pending password replacement shows no header automation buttons');
 // Closing the required dialog lands on the lock card, never on the console; the card reopens it.
 await evaluate('document.getElementById("passwordClose").click()');
 await until('!document.getElementById("passwordDialog").open && !document.getElementById("passwordLockScreen").hidden');
 assert.equal(await evaluate('document.querySelector(".main").offsetParent'),null,'a closed password dialog does not reveal the console');
 await evaluate('document.getElementById("passwordLockOpen").click()');
 await until('document.getElementById("passwordDialog").open');
 await evaluate('document.getElementById("passwordCurrent").value='+JSON.stringify(FIXTURE_SETUP_CODE)+';document.getElementById("passwordNew").value="browser-fixture-password";document.getElementById("passwordConfirm").value="browser-fixture-password";document.getElementById("passwordForm").requestSubmit()');
 await until('!document.getElementById("passwordDialog").open');assert.equal(mustChange,false);
 assert.deepEqual(requests.slice(signedOutMark).filter(r=>r[0]==='POST'&&r[1].startsWith('/api/auth/')).map(r=>[r[1],r[2]]),[
  ['/api/auth/login',{username:'admin',password:FIXTURE_SETUP_CODE}],
  ['/api/auth/password',{current_password:FIXTURE_SETUP_CODE,password:'browser-fixture-password'}]
 ],'one explicit save performs the restricted login and protected password replacement');
 await until(headerState+'==="openAnalytics,openDeliveries,openSchedules"');
 assert.match(await evaluate('document.getElementById("openDeliveries").title'),/metadata-only.*coding job finishes/);
 await evaluate('document.querySelector("[data-pane=api-keys]").click()');
 await until('!!document.getElementById("saved-DEEPAGENT_API_KEY")');
 await evaluate('document.getElementById("saved-DEEPAGENT_API_KEY").value="fixture-secret-value-1234";document.getElementById("saved-DEEPAGENT_API_KEY").form.requestSubmit()');
 await until('document.getElementById("pane-api-keys").textContent.includes("••••••••1234")');
 assert.equal(savedKey,'fixture-secret-value-1234');
 await evaluate('document.getElementById("saved-SERPAPI_API_KEY").value="fixture-search-key-5678";document.getElementById("saved-SERPAPI_API_KEY").form.requestSubmit()');
 await until('document.getElementById("pane-api-keys").textContent.includes("••••••••5678")');
 assert.equal(savedSearchKey,'fixture-search-key-5678');
 await evaluate('Array.from(document.getElementById("saved-SERPAPI_API_KEY").form.querySelectorAll("button")).find(b=>b.textContent==="Test Google search").click()');
 await until('document.getElementById("pane-api-keys").textContent.includes("Connected via google-serpapi")');
 assert.equal(await evaluate('document.body.textContent.includes("fixture-search-key-5678")'),false);
 await evaluate('Array.from(document.getElementById("saved-SERPAPI_API_KEY").form.querySelectorAll("button")).find(b=>b.textContent==="Clear saved value").click()');
 await until('document.getElementById("saved-SERPAPI_API_KEY") && !document.getElementById("pane-api-keys").textContent.includes("••••••••5678")');assert.equal(savedSearchKey,'');

 assert.equal(await evaluate('document.getElementById("saved-DEEPAGENT_API_KEY").value'),'');
 assert.equal(await evaluate('document.body.textContent.includes("fixture-secret-value-1234")'),false);
 // /api set takes the value from the operator's line, not the parse echo,
 // and the transcript never prints it (no "normalized:" copy).
 await send('/API  set DEEPAGENT_API_KEY slash-secret-value-4321');
 await until('document.getElementById("stream").innerText.includes("saved: DEEPAGENT_API_KEY")');
 assert.equal(savedKey,'slash-secret-value-4321');
 assert.equal(requests.filter(r=>r[1]==='/api/slash/parse').at(-1)[2].line,'/API  set DEEPAGENT_API_KEY slash-secret-value-4321');
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("slash-secret-value-4321")'),false);
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("normalized: /api set")'),false);
 assert.equal(requests.some(r=>historySave(r)&&JSON.stringify(r[2]).includes('slash-secret-value-4321')),false,'history never saves an /api set value');
 assert.equal(await evaluate('promptHistory.prompts.some(p=>p.includes("slash-secret-value-4321"))'),false);
 await evaluate('Array.from(document.getElementById("saved-DEEPAGENT_API_KEY").form.querySelectorAll("button")).find(b=>b.textContent==="Clear saved value").click()');
 // The cleared SerpAPI row already reads "Saved: unset", so wait on this row's status (the <p> before its input).
 await until('document.getElementById("saved-DEEPAGENT_API_KEY")?.previousElementSibling?.textContent.startsWith("Saved: unset")');assert.equal(savedKey,'');
 // A rejected history request from the previous account must not render its already-loaded proposals into the next account.
 memoryOwnerFixture={owner:'user_prior',facts:[],proposals:[{id:'proposal_prior_owner',revision:'revision_prior',action:'add',category:'insight',content:'PRIVATE_PRIOR_OWNER_PROPOSAL',status:'pending'}]};
 holdMemoryHistory=true;const historyRequestSeen=new Promise(resolve=>{memoryHistoryHeld=resolve;});
 await evaluate('selectSideTab("memory");window.oldMemoryLoad=refreshMemoryProposals();true');
 await Promise.race([historyRequestSeen,pause(5000).then(()=>{throw Error('history fixture request not held');})]);
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');
 memoryOwnerFixture={owner:'user_next',facts:[{id:'fact_next_owner',revision:1,category:'insight',content:'NEXT_OWNER_FACT',active:true}],proposals:[{id:'proposal_next_owner',revision:'revision_next',action:'add',category:'insight',content:'NEXT_OWNER_PROPOSAL',status:'pending'}]};
 await evaluate('document.getElementById("hAuthUser").value="memory-second";document.getElementById("hAuthPass").value="browser-memory-password";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("hAuthLoginBox").hidden');
 // Login must refresh the retained active pane without another tab click or slash command.
 await until('document.getElementById("pane-memory").textContent.includes("NEXT_OWNER_PROPOSAL") && document.getElementById("pane-memory").textContent.includes("NEXT_OWNER_FACT")');
 assert.equal(await evaluate('document.getElementById("pane-memory").classList.contains("active")'),true,'successful login reloads the retained Memory tab');
 rejectHeldMemoryHistory();await evaluate('window.oldMemoryLoad');
 assert.equal(await evaluate('document.getElementById("pane-memory").textContent.includes("PRIVATE_PRIOR_OWNER_PROPOSAL")'),false,'old-account history failure cannot reveal its captured proposals after account switch');
 assert.ok(await evaluate('document.getElementById("pane-memory").textContent.includes("NEXT_OWNER_PROPOSAL")'));
 assert.equal(await evaluate('document.querySelectorAll("#pane-memory form").length'),1,'the current account keeps its own proposal after the rejected stale request');
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');memoryOwnerFixture=null;
 await evaluate('document.getElementById("hAuthUser").value="admin";document.getElementById("hAuthPass").value="browser-fixture-password";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("hAuthLoginBox").hidden');
 // An expired session rejects logout (401); the Memory pane must still be cleared before anyone signs in again.
 await send('/memory facts');
 await until('document.getElementById("stream").innerText.includes("fact_labeled_saved")');
 await send('/memory proposals');
 await until('document.getElementById("pane-memory").textContent.includes("Saved facts (2)")');
 await evaluate('currentSession="session_fixture_prior";sessionGoal="Prior goal";document.getElementById("input").value="unsent draft"');
 // The follow-up whoami probe also fails here, so the reset must not depend on it.
 signedIn=false;logoutStatus=401;whoamiDrop=true;
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('document.getElementById("hAuthWho").textContent==="logout rejected (401)"');
 assert.equal(await evaluate('document.getElementById("pane-memory").childElementCount'),0,'a 401 logout clears facts even when the follow-up probe fails');
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("fact_labeled_saved")'),false);
 assert.equal(await evaluate('currentSession'),null,'a 401 logout drops the session even when the follow-up probe fails');
 whoamiDrop=false;
 await evaluate('refreshHarnessAuth()');
 await until('!document.getElementById("hAuthLoginBox").hidden');
 assert.equal(await evaluate('document.getElementById("pane-memory").childElementCount'),0,'a rejected logout still clears account-private facts');
 assert.equal(await evaluate('document.getElementById("stream").innerText.includes("fact_labeled_saved")'),false,'a rejected logout still clears listed facts from the transcript');
 assert.equal(await evaluate('currentSession'),null,'a rejected logout drops the active session like a successful one');
 assert.equal(await evaluate('sessionGoal'),'');assert.equal(await evaluate('document.getElementById("input").value'),'');
 assert.equal(await evaluate('document.getElementById("sSession").textContent'),'none');
 logoutStatus=200;const factsBeforeRelogin=factReads();
 await evaluate('document.getElementById("hAuthUser").value="admin";document.getElementById("hAuthPass").value="browser-fixture-password";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("hAuthLoginBox").hidden');
 // Logout cleared the old view above; successful login now reloads the retained
 // Memory tab through the current account instead of leaving it empty.
 await until('document.getElementById("pane-memory").textContent.includes("Saved facts (2)") && !memoryRefreshBusy');
 assert.ok(factReads()>factsBeforeRelogin,'sign-in loads saved facts through a fresh owner-scoped read');
 await send('/agent run codex/logout Reviewed before logout');
 await evaluate('shownAgentDiffs.set("old", "diff"); reviewedSoulProposal={id:"old"}; reviewedPRBodies.set("old", "body"); prBodyTarget="old"');
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');assert.equal(await evaluate('currentSession'),null);
 assert.equal(await evaluate(headerState),'','logout hides header automation buttons');
 assert.equal(await evaluate('pendingAgentRun'),null);assert.equal(await evaluate('shownAgentDiffs.size'),0);
 assert.equal(await evaluate('reviewedSoulProposal'),null);assert.equal(await evaluate('reviewedPRBodies.size'),0);
 assert.equal(await evaluate('prBodyTarget'),'');
 assert.equal(await evaluate('document.getElementById("passwordDialog").open'),false,'setup stays closed after logout');
 assert.equal(await evaluate('document.getElementById("signInScreen").hidden'),false,'logout returns to the sign-in card');
 // Authentication succeeds for an auditor even though the server denies operational sessions.
 await evaluate('document.getElementById("hAuthUser").value="audit-fixture";document.getElementById("hAuthPass").value="browser-auditor-password";document.getElementById("hAuthLogin").click()');
 await until('document.getElementById("pane-sessions").textContent.includes("AUTH_PERMISSION_DENIED: denied")');
 assert.equal(await evaluate('document.getElementById("hAuthWho").textContent'),'audit-fixture · audit · '+userIds['audit-fixture'],'a session permission denial must not turn successful login into a network error');
 assert.equal(await evaluate('document.getElementById("sStyle").disabled'),true,'auditors cannot select a session style');
 assert.equal(await evaluate('document.querySelectorAll("#pane-sessions button").length'),0,'auditors must not see New Session or Clear History controls');
 assert.equal(await evaluate('refreshHeaderFeatures(true).then(()=>["openDeliveries","openSchedules"].every(id=>document.getElementById(id).hidden))'),true,'refused (403) probes keep automation hidden');
 assert.equal(await evaluate('document.getElementById("passwordDialog").open'),false);
 // account_gate denies an auditor /api/slash/parse, so a typed /status is refused unsent;
 // drive the reduced status view through the dispatcher instead.
 await evaluate('document.getElementById("stream").replaceChildren()');await evaluate('runSlash("/status")');
 const auditorStatus=await evaluate('document.getElementById("stream").innerText');
 assert.equal(auditorStatus.includes('undefined'),false,'redacted status must not render missing fields');
 assert.match(auditorStatus,/read.only/i,'auditor status must explain its read-only scope');
 await evaluate('document.getElementById("hAuthLogout").click()');
 await until('!document.getElementById("hAuthLoginBox").hidden');assert.equal(signedIn,false);
 assert.equal(await evaluate(headerState),'');
 assert.equal(await evaluate('document.getElementById("passwordDialog").open'),false);
 assert.equal(await evaluate('document.getElementById("sProvider").textContent'),'sign in');
 assert.equal(pageErrors.length,0,'page must not throw: '+pageErrors.join('; '));
 console.log(JSON.stringify({passed:true,coverage:['complete alphabetical command menu and matching help; staging without execution','folded command families and per-command manual that only inserts','parser refusal and outage never dispatch raw commands; exact cancellation remains available','incremental SSE with split UTF-8 and provisional-text cleanup','Google and page search routing','web tool sources and failures','SerpAPI key masked save and clear','minimal anonymous status','direct first-run password setup without implicit login; protected replacement and lock/reopen','API Keys catalog save/clear/masked status','auditor login with denied sessions and redacted status','logout clears UI','fresh transcript','full session restore without duplication','last 50 prompt recall, draft restoration and per-session isolation','late reply session isolation','staged approval reset','goal coding staging','refresh recovery','no implicit confirmation','prompt skill selection/clear','fixed check staging/refusal','persona editor','preview without write','explicit save confirmation','prompt viewer','unset persona diagnostics','first session','goal set/show/clear','manual continuation','goal text in /loop','auto start after a 5 s countdown and its cancellation','auto cooldown stop','generation cancellation','session switch','repeat stop','GOAL_DONE advisory','aggregate budget','rate limit','model failures','no agent execution','all memory gate slash mappings on and off','memory remember confirmation and reason','memory pending proposal Apply/Reject with reason and revision','memory store-closed refusal','memory facts listing, status alias and owner counts; Apply names the fact store','rejected logout resets the transcript, Memory pane and active session like a successful logout, even when the follow-up probe fails','memory search candidates only','memory retrieve force-include','header buttons follow account state and feature probes','Estimate draft inside Analytics']}));
 }
} finally {
 if(ws)ws.close();chrome.kill('SIGTERM');await new Promise(r=>{chrome.once('exit',r);setTimeout(r,2000);});server.closeAllConnections();server.close();await rm(profile,{recursive:true,force:true,maxRetries:10,retryDelay:200});
}
