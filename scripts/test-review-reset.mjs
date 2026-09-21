// Exercise the real console review handlers without running jobs or publishing.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(process.env.CGAH_CONSOLE || new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const source = (start, end) => html.slice(html.indexOf(start), html.indexOf(end));
const nodes = new Map(), messages = [], calls = [];
let analyticsClears = 0, scheduleClears = 0, notificationClears = 0;
const node = id => {
  if (!nodes.has(id)) nodes.set(id, {value:'', files:[], textContent:'', replaceChildren(){this.textContent='';}, addEventListener(event, handler){this[event]=handler;}});
  return nodes.get(id);
};
const context = vm.createContext({
  document:{getElementById:node}, $:node, window:{}, TextEncoder,
  stream:node('stream'), hAuthLogout:node('logout'), CSRF_TOKEN:'fixture',
  reviewRevision:0, sessionRevision:1, sessionClearing:false, agentWatchGeneration:0,
  pendingAgentRun:null, shownAgentDiffs:new Map(), reviewedSoulProposal:null,
  currentSession:'fixture-session', sessionGoal:'fixture goal', promptHistory:{},
  input:node('input'), loopAuto:false, retryAttachmentIds:null, stopLoop(){}, paintGoalLoop(){}, resetScheduleReview(){++scheduleClears;}, resetNotificationView(){++notificationClears;},
  currentStyle:'off', currentStyleIssue:null, inflightChat:null, sendBtn:{disabled:false},
  isLoopStopCommand:()=>false, AGENT_CLI_TIMEOUT_MS:100, sys:text=>messages.push(text), table:rows=>JSON.stringify(rows),
  abortInflightSearch(){}, clearPendingAttachments(){}, clearSpend(){}, clearAnalytics(){++analyticsClears;}, paintStyle(){},
  refreshStatus:async()=>{}, refreshHarnessAuth:async()=>{},
  fetchWithTimeout:async()=>({ok:true}), agentRecord:result=>result.parsed,
  api:async(...args)=>{calls.push(args);return context.respond(...args);},
});
vm.runInContext([
  source('let sessionTokenRevision =', 'async function refreshStatus()'),
  source('const reviewedPRBodies =', 'function isReviewableAgentDiff('),
  source('function isReviewableAgentDiff(', "$('sessionClearCancel')"),
  source('async function runSlash(', '/* ── chat ── */'),
  source('async function runSlashMaybeFuzzy(', '/* ── wiring ── */'),
  source('async function watchAgentJob(', 'function showPendingAgentRun('),
  source('function showPendingAgentRun(', "agentPlanInput.addEventListener('change'"),
  source('if (hAuthLogout) {', 'const hAuthSetupBtn'),
].join('\n'), context);
const run = code => vm.runInContext(code, context);
const seed = () => run(`pendingAgentRun={branch:'codex/review',instruction:'reviewed instruction',read_files:[]}; shownAgentDiffs.set('run','diff'); reviewedPRBodies.set('run','reviewed body'); prBodyTarget='run'; reviewedSoulProposal={id:'proposal',revision:'one'};`);
const reset = () => {
  assert.equal(context.pendingAgentRun,null);
  assert.equal(context.shownAgentDiffs.size,0);
  assert.equal(context.reviewedSoulProposal,null);
  assert.equal(run('reviewedPRBodies.size'),0);
  assert.equal(run('prBodyTarget'),'');
};
seed(); context.clearTranscript(); reset();
seed(); await context.hAuthLogout.click(); reset();
assert.equal(analyticsClears, 1, 'logout must invalidate analytics');
assert.equal(scheduleClears, 2, 'clear transcript and logout must invalidate schedule review');
assert.equal(notificationClears, 2, 'clear transcript and logout must invalidate delivery rows');
assert.equal(context.currentSession,null);
assert.equal(context.sessionGoal,'');
assert.equal(context.input.value,'');
context.respond = () => {throw new Error('hidden reviews must refuse before any request');};
calls.length=0;
for (const line of ['/agent confirm reason','/agent publish run reason','/soul apply proposal reason']) await context.runSlash(line);
assert.equal(calls.length,0,'cleared reviews cannot authorize writes');

const deferred = () => {let resolve;const promise=new Promise(r=>{resolve=r;});return {promise,resolve};};
const proposal={id:'proposal',revision:'one'};
const record={run_id:'run',status:'pending_decision',diff:'complete fixture diff'};
for (const [line,reply] of [
  ['/soul review proposal',proposal],
  ['/agent run codex/review instruction',{capabilities:{jobs:true},default_profile:'cargo-test'}],
  ['/agent status run',{parsed:record}],
  ['/agent approve run reason',{parsed:record}],
  ['/goal stage codex/review',{request:{instruction:'late request'}}],
  ['/goal task',{status:'staged',stage:{request:{instruction:'late request'}}}],
]) {
  const pending=deferred();context.respond=()=>pending.promise;
  context.currentSession='fixture-session';context.sessionGoal='fixture goal';
  context.shownAgentDiffs.set('run',record.diff);
  calls.length=0;
  const request=context.runSlash(line);
  context.clearTranscript();pending.resolve(reply);await request;
  reset();assert.equal(calls.length,1,'stale approval must not send a decision: '+line);
}

// A delayed fixed-check selection cannot mutate a newly staged request.
seed();
const checkReply=deferred();context.respond=()=>checkReply.promise;
const selectingCheck=context.runSlash('/skill check:ruff');
context.clearTranscript();
context.pendingAgentRun={checks:['cargo-test'],read_files:[]};
checkReply.resolve({profile:'ruff'});await selectingCheck;
assert.deepEqual(context.pendingAgentRun.checks,['cargo-test']);
context.respond=()=>({profile:'ruff'});await context.runSlash('/skill check:ruff');
assert.equal(context.pendingAgentRun.checks[0],'ruff','fresh fixed-check selection still works');
context.clearTranscript();

// A local file can finish reading after logout or /clear, without another API.
run("prBodyTarget='run'");
const fileRead=deferred();
node('agentPRBodyFile').files=[{size:12,text:()=>fileRead.promise}];
const loading=node('agentPRBodyFile').change();
context.clearTranscript();fileRead.resolve('late PR body');await loading;reset();

// An in-flight job poll must not display and authorize a diff after clearing.
const poll=deferred(), pollStarted=deferred();
context.respond=path=>{if(path==='/api/agent/checks')return {poll_interval_ms:1};pollStarted.resolve();return poll.promise;};
const watching=context.watchAgentJob('job');await pollStarted.promise;
assert.equal(calls.at(-1)[0],'/api/agent/jobs/job','the deferred job response must be in flight');
context.clearTranscript();poll.resolve({status:'complete',result:{parsed:record}});await watching;reset();

// Positive controls: fresh visible review and explicit approval still work.
context.respond=()=>proposal;await context.runSlash('/soul review proposal');
assert.equal(context.reviewedSoulProposal.id,'proposal');
context.respond=()=>({parsed:record});await context.runSlash('/agent status run');
assert.equal(context.shownAgentDiffs.get('run'),record.diff);
calls.length=0;await context.runSlash('/agent approve run reason');
assert.equal(calls[1][0],'/api/agent/runs/run/decision');
assert.equal(calls[1][2].confirm,true);
run("prBodyTarget='run'");node('agentPRBodyFile').files=[{size:12,text:async()=>'fresh PR body'}];
await node('agentPRBodyFile').change();assert.equal(run("reviewedPRBodies.get('run')"),'fresh PR body');
console.log('review reset: logout, clear, stale requests/files/polls, write refusal and fresh review passed');

// Parsing is asynchronous too: a command cannot acquire a new session/account
// or survive clearing its originating view while its parse response is pending.
for (const boundary of ['session', 'review']) {
  const pending=deferred(); calls.length=0;
  context.respond=path=>path==='/api/slash/parse'?pending.promise:{count:0};
  const command=context.runSlashMaybeFuzzy('/memory clear');
  if(boundary==='session') context.sessionRevision++; else context.clearTranscript();
  pending.resolve({kind:'dispatch',dispatch:true,canonical:'/memory clear'});
  await command;
  assert.equal(calls.filter(call=>call[0]==='/api/memory/clear').length,0,boundary+' boundary must invalidate pending slash dispatch');
}
calls.length=0;
context.respond=path=>path==='/api/slash/parse'?{kind:'dispatch',dispatch:true,canonical:'/memory clear'}:{count:0};
await context.runSlashMaybeFuzzy('/memory clear');
assert.equal(calls.filter(call=>call[0]==='/api/memory/clear').length,1,'unchanged context still dispatches an exact command');
console.log('pending slash responses respect session and review boundaries');
