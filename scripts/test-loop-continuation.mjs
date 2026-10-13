// Exercise the shipped /loop turn prompt, GOAL_DONE marker and stall checks.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const start = html.indexOf('/* ── chat ── */');
const end = html.indexOf('function sleepMs(', start);
assert.ok(start !== -1 && end > start, 'loop helpers are present in the console');
const ctx = vm.createContext({});
vm.runInContext(html.slice(start, end) + ';this.api={replyHasGoalDone,loopTurnPrompt,repliesNearlyRepeat};', ctx);
const {replyHasGoalDone, loopTurnPrompt, repliesNearlyRepeat} = ctx.api;

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

// Exercise asynchronous session ownership in the shipped loop runner.
const source=html.slice(html.indexOf('async function runLoopTurns()'),html.indexOf('function requestLoopStop('));
function fixture() {
 let resolve, reject, started; const reached=new Promise(a=>{started=a;}); const upload=new Promise((a,b)=>{resolve=a;reject=b;});
 const ctx=vm.createContext({loopState:{remaining:1,total:1},currentSession:'old',sessionRevision:1,sessionGoal:'old goal',paintGoalLoop(){},api:async()=>({goal:'old goal'}),loopTurnPrompt:()=>'',inflightSearch:null,inflightChat:null,retryAttachmentIds:null,attachInput:{files:['file']},sendBtn:{disabled:false},uploadChatAttachments:()=>{started();return upload;},releaseSendGate(){ctx.releases++;},releases:0,showError(){ctx.errors++;},errors:0,stopLoop(){ctx.loopState=null;},paintAttach(){},sendChat:async()=>'',sleepMs:async()=>{}});
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

// Execute the actual slash branch too: preflight must use its own session guard.
const loopCase=html.slice(html.indexOf("      case 'loop': {"),html.indexOf("      case 'tokens': {"));
const command=vm.createContext({rest:['2'],loopState:null,loopAuto:false,currentSession:'owned',sessionGoal:'',staleReview:()=>false,api:async(path)=>path==='/api/status'?{provider:'ollama',chat_tools_available:true}:{goal:'stored goal',live_attachment_pins:1},paintGoalLoop(){},sys(){},MAX_LOOP_TURNS:8,ran:false,runLoopTurns:async()=>{command.ran=true;}});
await vm.runInContext('(async()=>{switch("loop"){'+loopCase+'}})()',command);
assert.equal(command.ran,true);
assert.equal(command.sessionGoal,'stored goal');
assert.equal(command.loopState.remaining,2);
console.log('Loop command preflight loads goal and starts the owned loop.');

// Execute goal option parsing, persisted payload and invalid-mode refusal.
const goalCase=html.slice(html.indexOf("      case 'goal': {"),html.indexOf("      case 'loop': {"));
for (const [arg, mode, goal] of [['--web off use attachment only','off','use attachment only'],['--web fetch read supplied URL','fetch','read supplied URL'],['ordinary goal','all','ordinary goal'],['--web all '+ '界'.repeat(2000),'all','界'.repeat(2000)],['--web nope test',null,null]]) {
  const calls=[];
  const command=vm.createContext({arg,rest:arg.split(/\s+/),currentSession:'owned',sessionGoal:'',MAX_GOAL_CHARS:2000,api:async(path,method,body)=>{calls.push({path,body});return {goal:body.goal,goal_web_mode:body.web_mode};},paintGoalLoop(){},sys(){},refreshSessions:async()=>{}});
  await vm.runInContext('(async()=>{switch("goal"){'+goalCase+'}})()',command);
  if(mode===null) assert.equal(calls.length,0); else {assert.equal(calls[0].body.web_mode,mode);assert.equal(calls[0].body.goal,goal);}
}
console.log('Goal command persists explicit web modes and keeps the Unicode goal limit.');
