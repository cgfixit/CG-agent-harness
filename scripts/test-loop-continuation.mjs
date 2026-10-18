// Exercise the shipped /loop turn prompt, GOAL_DONE marker and stall checks.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const start = html.indexOf('/* ── chat ── */');
const end = html.indexOf('function sleepMs(', start);
assert.ok(start !== -1 && end > start, 'loop helpers are present in the console');
const ctx = vm.createContext({});
vm.runInContext(html.slice(start, end) + ';this.api={replyHasGoalDone,loopTurnPrompt,repliesNearlyRepeat,loopBlockingWebCode,loopGoalFromMessages};', ctx);
const {replyHasGoalDone, loopTurnPrompt, repliesNearlyRepeat, loopBlockingWebCode, loopGoalFromMessages} = ctx.api;

for (const done of ['GOAL_DONE', '**GOAL_DONE**', '`GOAL_DONE`', '## GOAL_DONE', 'GOAL_DONE.', '> GOAL_DONE',
  '- GOAL_DONE', '__GOAL_DONE__', 'Summary.\n\n**GOAL_DONE**\n', '1. GOAL_DONE', '3) **GOAL_DONE**']) {
  assert.equal(replyHasGoalDone(done), true, JSON.stringify(done));
}
for (const notDone of ['', 'I will write GOAL_DONE when finished.', '1. Write GOAL_DONE at the end.', '2024 GOAL_DONE', 'GOAL_DONE_LATER', 'NOT GOAL_DONE', 'goal_done', null]) {
  assert.equal(replyHasGoalDone(notDone), false, JSON.stringify(notDone));
}

const prompt = loopTurnPrompt(2, 3, 'Write   a\nparser');
assert.ok(prompt.startsWith('Loop turn 2 of 3. Session goal: Write a parser\n'), prompt);
assert.ok(prompt.includes('GOAL_DONE on its own line') && !/27b/i.test(prompt), 'model-agnostic marker instruction');
const fullGoal = 'x'.repeat(1930) + ' FINAL_CONSTRAINT: do not search the web; CHECKED_441';
assert.ok(loopTurnPrompt(1, 1, fullGoal).includes(fullGoal), 'late constraints in every valid goal reach the latest user turn');
const long = loopTurnPrompt(1, 1, 'x'.repeat(2001));
assert.ok(long.includes('x'.repeat(1999) + '…') && !long.includes('x'.repeat(2000)), 'goal reminder is bounded by the server goal limit');
const astral = loopTurnPrompt(1, 1, 'x'.repeat(1998) + '😀😀 tail');
assert.ok(astral.includes('x'.repeat(1998) + '😀…'), astral);
assert.ok(!/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(astral), 'no lone surrogate reaches JSON');

const answer = 'The parser reads tokens from the lexer, builds an AST for each statement, and reports the first syntax error with its line number and column.';
assert.equal(repliesNearlyRepeat(answer, answer), true);
assert.equal(repliesNearlyRepeat(answer, '**' + answer.toUpperCase() + '**'), true, 'formatting and case are not progress');
assert.equal(repliesNearlyRepeat(answer, answer.replace('first syntax error', 'first syntax errors')), true, 'a one-word edit of a long reply is a stall');
assert.equal(repliesNearlyRepeat(answer, 'Next, add error recovery: skip to the next semicolon after a syntax error and keep parsing so later errors are reported too.'), false);
assert.equal(repliesNearlyRepeat('Step 1 done.', 'Step 2 done.'), false, 'short replies compare word for word');
console.log('loop continuation: GOAL_DONE decorations, bounded model-agnostic prompt and near-repeat stall passed');

// Only a refusal that repeats on every turn stops an unattended loop; one read's failure does not.
const page = {tool:'web_fetch', ok:true, result:{url:'https://docs.example/a', text:'x'}};
assert.equal(loopBlockingWebCode([page, {tool:'web_fetch', ok:false, code:'WEB_HOST_DENIED', message:'denied'}]), null, 'one denied URL beside a read');
assert.equal(loopBlockingWebCode([{tool:'web_fetch', ok:false, code:'WEB_DNS', message:'DNS lookup failed'}]), null, 'an unreachable URL with no other read');
assert.equal(loopBlockingWebCode([{tool:'web_fetch', ok:false, code:'WEB_TOKEN_BUDGET', message:'no room left for this result'}, page]), null, 'one result that did not fit');
assert.equal(loopBlockingWebCode([{tool:'web', ok:false, code:'WEB_TOKEN_BUDGET', message:'no room for a tool round'}]), 'WEB_TOKEN_BUDGET', 'a round the budget refused outright');
assert.equal(loopBlockingWebCode([{tool:'web', ok:true, code:'WEB_TOOLS_WITHHELD', message:'final answer reserved'}]), null, 'reserving the answer is not a failure');
assert.equal(loopBlockingWebCode([page, {tool:'web_search', ok:false, code:'WEB_ALLOWLIST_EMPTY', message:'no URL permitted'}]), 'WEB_ALLOWLIST_EMPTY');
assert.equal(loopBlockingWebCode([{tool:'web_search', ok:false, code:'WEB_GOOGLE_PERMISSION', message:'search needs a key'}]), 'WEB_GOOGLE_PERMISSION');
assert.equal(loopBlockingWebCode([]), null); assert.equal(loopBlockingWebCode(null), null);
console.log('Loop web-tool stop policy: per-read failures continue, repeating refusals stop.');

// Exercise asynchronous session ownership in the shipped loop runner.
const source=html.slice(html.indexOf('async function runLoopTurns()'),html.indexOf('function requestLoopStop('));
function fixture() {
 let resolve, reject, started; const reached=new Promise(a=>{started=a;}); const upload=new Promise((a,b)=>{resolve=a;reject=b;});
 const ctx=vm.createContext({MAX_LOOP_FAILURES:2,loopState:{remaining:1,total:1},currentSession:'old',sessionRevision:1,sessionGoal:'old goal',paintGoalLoop(){},api:async()=>({goal:'old goal'}),loopTurnPrompt:()=>'',inflightSearch:null,inflightChat:null,retryAttachmentIds:null,attachInput:{files:['file']},sendBtn:{disabled:false},uploadChatAttachments:()=>{started();return upload;},releaseSendGate(){ctx.releases++;},releases:0,showError(){ctx.errors++;},errors:0,stopLoop(){ctx.loopState=null;},paintAttach(){},sendChat:async()=>'',sleepMs:async()=>{}});
 vm.runInContext(source,ctx);
 return {ctx,resolve,reject,reached};
}
for(const fail of [false,true]){
 const {ctx,resolve,reject,reached}=fixture();
 const pending=ctx.runLoopTurns();
 await reached;
 const fresh={remaining:2,total:2};ctx.loopState=fresh;ctx.currentSession='new';ctx.sessionRevision=2;ctx.retryAttachmentIds={session:'new',ids:['new-file']};
 if(fail) reject(Error('old upload failed'));else resolve(['old-file']);
 await pending;
 assert.equal(ctx.loopState,fresh);assert.equal(ctx.retryAttachmentIds.ids[0],'new-file');assert.equal(ctx.releases,0);assert.equal(ctx.errors,0);
}
console.log('Old loop upload success/failure preserves the new session and loop.');

// A turn that fails without a verdict (timeout, truncated reply, network error) is retried by an
// unattended loop up to MAX_LOOP_FAILURES; an attended loop hands the retry to the operator; a
// refusal that already stopped the loop inside sendChat is never retried.
function retryFixture(auto, sendChat) {
 const box={MAX_LOOP_FAILURES:2,LOOP_COOLDOWN_MS:0,MAX_LOOP_TOTAL_COMPLETION_TOKENS:12000,WEB_TOOL_NEXT_STEP:{},loopState:{remaining:1,total:1,auto,failures:0,completionTokens:0,lastReply:''},currentSession:'s',sessionRevision:1,sessionGoal:'goal',paintGoalLoop(){},api:async()=>({goal:'goal'}),loopTurnPrompt:()=>'turn',inflightSearch:null,inflightChat:null,retryAttachmentIds:null,attachInput:{files:[]},sendBtn:{disabled:false},releaseSendGate(){},showError(){},paintAttach(){},sends:0,notices:[],stops:[],sleepMs:async()=>{},repliesNearlyRepeat:()=>false,replyHasGoalDone:()=>false};
 box.sendChat=async()=>{box.sends++;return sendChat(box);};
 box.sys=m=>{box.notices.push(m);};
 box.stopLoop=r=>{box.stops.push(r);box.loopState=null;};
 const ctx=vm.createContext(box); vm.runInContext(source,ctx); return ctx;
}
{
 const once=retryFixture(true,box=>box.sends===1?null:'Progress on the goal.'); await once.runLoopTurns();
 assert.equal(once.sends,2,'auto retries the failed turn'); assert.ok(once.notices.some(n=>n.includes('retrying this turn')),once.notices.join('|'));
 assert.deepEqual(once.stops,['loop budget exhausted (1 turn(s))']);
 const twice=retryFixture(true,()=>null); await twice.runLoopTurns();
 assert.equal(twice.sends,2,'auto gives up after MAX_LOOP_FAILURES failures'); assert.match(twice.stops[0],/^loop aborted — 2 consecutive chat failures/);
 const attended=retryFixture(false,()=>null); await attended.runLoopTurns();
 assert.equal(attended.sends,1,'an attended loop does not retry by itself'); assert.deepEqual(attended.stops,[]); assert.equal(attended.loopState.failures,1);
 assert.ok(attended.notices.at(-1).includes('/loop to retry'),attended.notices.join('|'));
 const refused=retryFixture(true,box=>{box.stopLoop('loop stopped — WEB_TOOL_DENIED');return null;}); await refused.runLoopTurns();
 assert.equal(refused.sends,1,'a refusal that stopped the loop is not retried'); assert.deepEqual(refused.stops,['loop stopped — WEB_TOOL_DENIED']);
}
console.log('Unattended loop retries a verdict-less turn up to MAX_LOOP_FAILURES; attended and refused loops do not.');

// Execute the actual slash branch too: preflight must use its own session guard.
const loopCase=html.slice(html.indexOf("      case 'loop': {"),html.indexOf("      case 'tokens': {"));
const command=vm.createContext({rest:['2'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,api:async(path)=>path==='/api/status'?{provider:'ollama',chat_tools_available:true}:{goal:'stored goal',live_attachment_pins:1},paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{command.ran=true;}});
await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',command);
assert.equal(command.ran,true);
assert.equal(command.sessionGoal,'stored goal');
assert.equal(command.loopState.remaining,2);
console.log('Loop command preflight loads goal and starts the owned loop.');

// Goal derivation: the last own prompt, never a loop turn or compaction note.
assert.equal(loopGoalFromMessages([{role:'user',content:'first'},{role:'assistant',content:'ack'},{role:'user',content:'fix the parser'}],2000),'fix the parser');
assert.equal(loopGoalFromMessages([{role:'user',content:'Loop turn 1 of 3. Session goal: x'},{role:'user',content:'[session-compacted]\nsummary'}],2000),'','loop turns and compaction notes are not goals');
assert.equal(loopGoalFromMessages([{role:'user',content:'  '},{role:'assistant',content:null}],2000),'');
assert.equal(loopGoalFromMessages(null,2000),'');
assert.equal(loopGoalFromMessages([{role:'user',content:'x'.repeat(3000)}],2000).length,2000,'bounded by the server goal limit');
console.log('Loop goal derivation takes the last own prompt and stays bounded.');

// A goal-less /loop derives the goal from the session and starts; with no
// prompt history at all it is still refused.
{
  const calls=[];
  const derive=vm.createContext({rest:['2'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,MAX_GOAL_CHARS:2000,loopGoalFromMessages,
    api:async(path,method,body)=>{calls.push({path,method,body});
      if(path==='/api/status')return{provider:'ollama',chat_tools_available:true};
      if(method==='POST')return{goal:body.goal,goal_web_mode:'all'};
      return {goal:'',messages:[{role:'user',content:'Loop turn 1 of 3. Session goal: x'},{role:'assistant',content:'ack'},{role:'user',content:'fix the parser'}],live_attachment_pins:0};},
    paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{derive.ran=true;}});
  await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',derive);
  const set=calls.find(c=>c.method==='POST');
  assert.equal(set.path,'/api/sessions/owned/goal');
  assert.equal(set.body.goal,'fix the parser','the last own prompt becomes the goal');
  assert.equal(derive.ran,true);
  const bare=vm.createContext({rest:['2'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,MAX_GOAL_CHARS:2000,loopGoalFromMessages,
    api:async(path)=>path==='/api/status'?{provider:'ollama',chat_tools_available:true}:{goal:'',messages:[],live_attachment_pins:0},
    paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{bare.ran=true;}});
  await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',bare);
  assert.equal(bare.ran,false,'no goal and no prompt history never starts a loop');
  console.log('Goal-less /loop derives the goal from the last prompt; empty history stays refused.');
}

// Context-pressure default: maybeAutoLoop continues a compacted turn toward
// the goal, derives a missing goal, and stays off for cloud or running loops.
{
  const autoSrc=html.slice(html.indexOf('async function maybeAutoLoop()'),html.indexOf('window.addEventListener',html.indexOf('async function maybeAutoLoop()')));
  function autoFixture(provider,goal,messages){
    const box={loopState:null,currentSession:'s',sessionGoal:goal,lastStatus:{provider,base_url:provider==='ollama'?'http://127.0.0.1:11434':null},
      MAX_GOAL_CHARS:2000,DEFAULT_LOOP_TURNS:3,loopGoalFromMessages,calls:[],
      api:async(path,method,body)=>{box.calls.push({path,method,body});return method==='POST'?{goal:body.goal}:{goal,messages};},
      paintGoalLoop(){},sys:m=>box.calls.push({sys:m}),showError:e=>{throw e;},runLoopTurns:async()=>{box.looped=true;}};
    const c=vm.createContext(box);vm.runInContext(autoSrc,c);return c;
  }
  const local=autoFixture('ollama','',['fix the parser'].map(content=>({role:'user',content})));
  await local.maybeAutoLoop();
  assert.equal(local.looped,true,'compacted local turn auto-continues');
  assert.equal(local.loopState.auto,true);
  assert.equal(local.calls.find(c=>c.method==='POST').body.goal,'fix the parser');
  const cloud=autoFixture('grok','goal',[]);
  await cloud.maybeAutoLoop();
  assert.equal(cloud.looped,undefined,'cloud never auto-loops');
  const running=autoFixture('ollama','goal',[]);
  running.loopState={running:true};
  await running.maybeAutoLoop();
  assert.equal(running.looped,undefined,'a running loop is left alone');
  const goalless=autoFixture('ollama','',[]);
  await goalless.maybeAutoLoop();
  assert.equal(goalless.looped,undefined,'nothing to continue stays idle');
  console.log('Auto-loop on compaction: local only, goal-derived, never overlaps.');
}

// Execute goal option parsing, persisted payload and invalid-mode refusal.
const goalCase=html.slice(html.indexOf("      case 'goal': {"),html.indexOf("      case 'loop': {"));
for (const [arg, mode, goal] of [['--web off use attachment only','off','use attachment only'],['--web fetch read supplied URL','fetch','read supplied URL'],['ordinary goal',undefined,'ordinary goal'],['--web all '+ '界'.repeat(2000),'all','界'.repeat(2000)],['--web nope test',null,null]]) {
  const calls=[];
  const command=vm.createContext({arg,rest:arg.split(/\s+/),currentSession:'owned',sessionGoal:'',MAX_GOAL_CHARS:2000,goalWebNote:()=>'',adoptPendingHistory(){},staleReview:()=>false,api:async(path,method,body)=>{calls.push({path,body});return {goal:body.goal,goal_web_mode:body.web_mode};},paintGoalLoop(){},sys(){},refreshSessions:async()=>{}});
  await vm.runInContext('(async()=>{switch("goal"){'+goalCase+'}})()',command);
  if(mode===null) assert.equal(calls.length,0); else {assert.equal(calls[0].body.web_mode,mode);assert.equal(calls[0].body.goal,goal);}
}
console.log('Goal command persists explicit web modes, omits the mode otherwise, and keeps the Unicode goal limit.');

// /loop --web <mode> re-sets the stored goal with that mode before the first turn.
{
  const calls=[];
  const command=vm.createContext({rest:['--web','all','2'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,api:async(path,method,body)=>{calls.push({path,method,body});return path==='/api/status'?{provider:'ollama',chat_tools_available:true}:method==='POST'?{goal:body.goal,goal_web_mode:body.web_mode}:{goal:'stored goal',goal_web_mode:'fetch',live_attachment_pins:0};},paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{command.ran=true;}});
  await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',command);
  const set=calls.find(c=>c.method==='POST');
  assert.equal(set.path,'/api/sessions/owned/goal');
  assert.equal(set.body.goal,'stored goal');
  assert.equal(set.body.web_mode,'all');
  assert.equal(command.ran,true);
  assert.equal(command.loopState.remaining,2);
  const bad=vm.createContext({rest:['--web','nope'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,api:async()=>{throw Error('no request on a bad mode');},paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{bad.ran=true;}});
  await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',bad);
  assert.equal(bad.ran,false);
  console.log('Loop --web persists the mode on the goal and refuses unknown modes.');
}

const notices=[];
const noticeContext=vm.createContext({sys:text=>notices.push(text),renderSearchResults:()=>{throw Error('a budget notice is not a source result');}});
vm.runInContext(html.slice(html.indexOf('function renderChatWebTools('),html.indexOf('function renderWebStatus(')),noticeContext);
noticeContext.renderChatWebTools([{tool:'web',ok:true,code:'WEB_TOOLS_WITHHELD',message:'Final answer reserved.'}]);
assert.equal(notices[0],'web: WEB_TOOLS_WITHHELD — Final answer reserved.');
console.log('Final-answer budget notice renders without inventing a source result.');
