// Header automation buttons: hidden until the account state is known, and only for enabled features.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const between = (start, end) => {
  const from = html.indexOf(start), to = html.indexOf(end, from);
  assert.ok(from >= 0 && to > from, 'console markers must bound the slice: ' + start + ' … ' + end);
  return html.slice(from, to);
};
const buttons = ['openAnalytics', 'openDeliveries', 'openSchedules'];
for (const id of buttons) assert.match(html, new RegExp('<button [^>]*id="' + id + '"[^>]* hidden[ >]'), id + ' must start hidden');
assert.match(html, /<button [^>]*id="openDeliveries"[^>]*title="Outgoing, metadata-only [^"]*coding job finishes"[^>]*>Job webhooks<\/button>/);
assert.ok(!html.includes('id="openSpend"'), 'Spend is folded into Analytics');

const nodes = new Map();
const node = id => {
  if (!nodes.has(id)) nodes.set(id, {id, hidden:buttons.includes(id), textContent:'', value:'', disabled:false, listeners:{},
    addEventListener(event, handler) { this.listeners[event] = handler; }, focus() {}, contains() { return false; },
    replaceChildren() { this.textContent = ''; }});
  return nodes.get(id);
};
const shown = () => buttons.filter(id => !node(id).hidden);
const reply = (status, body = {}) => ({status, ok:status >= 200 && status < 300, json:async () => body});
const refused = status => Object.assign(new Error('HTTP ' + status), {status});
const shipped = {'/api/notifications':{enabled:false,destinations:[],deliveries:[]}, '/api/tools':{tools:[{path:'/api/agent/checks',enabled:null},{path:'/api/agent/schedules',enabled:false}]}};
const enabled = {'/api/notifications':{enabled:true,destinations:[],deliveries:[]}, '/api/tools':{tools:[{path:'/api/agent/checks',enabled:null},{path:'/api/agent/schedules',enabled:true}]}};
let replies = shipped, auth = {whoami:reply(503), setup:reply(200, {needs_password:false})}, probes = [];
const classes = () => { const set = new Set(); return {add:n => set.add(n), remove:n => set.delete(n), contains:n => set.has(n),
  toggle:(n, on) => { if (on) set.add(n); else set.delete(n); return on; }}; };
const page = {getElementById:node, body:{classList:classes()}, documentElement:{classList:classes()}, activeElement:null};
const passwordPrompts = []; let transcriptClears = 0;
const context = vm.createContext({
  $:node, document:page, window:{}, CSRF_TOKEN:'fixture', sessionRevision:1, sendBtn:{disabled:false}, inflightChat:null,
  api:async path => { probes.push(path); const value = replies[path]; if (value instanceof Error) throw value; return typeof value === 'function' ? value() : value; },
  fetchWithTimeout:async url => {
    const value = {'/api/auth/whoami':auth.whoami, '/api/auth/setup-status':auth.setup, '/api/auth/login':auth.login,
      '/api/auth/logout':auth.logout, '/api/auth/bootstrap-password':auth.bootstrap}[url];
    assert.ok(value, 'unexpected request ' + url);
    return typeof value === 'function' ? value() : value;
  },
  clearAnalytics() {}, clearPendingAttachments() {}, abortInflightSearch() {}, openPasswordChange(required) { passwordPrompts.push(required); },
  refreshStatus:async () => {}, refreshSessions:async () => {},
  // resetAccountView() state, so logout paths run the real reset.
  clearTranscript() { transcriptClears++; }, stopLoop() {}, paintSessionTokens() {}, paintStyle() {}, input:node('input'),
  netconnectRevision:0, currentSession:'fixture-session', sessionGoal:'fixture goal', loopAuto:false, retryAttachmentIds:null,
  promptHistory:{}, currentStyle:'off', currentStyleIssue:null,
});
vm.runInContext([
  between('const hAuthLogin = document.getElementById(', 'if (hAuthLogin) {'),
  between('if (hAuthLogin) {', 'if (hAuthLogout) {'),
  between('if (hAuthLogout) {', 'const hAuthSetupBtn'),
  between('const hAuthSetupBtn', 'function openPasswordChange('),
].join('\n'), context);
// Await every probe the real handlers start, without timers.
const started = [], original = context.refreshHeaderFeatures;
context.refreshHeaderFeatures = usable => { const probe = original(usable); started.push(probe); return probe; };
const settle = async () => { while (started.length) await started.shift(); };
const paint = async (usable, fixture) => { replies = fixture; context.refreshHeaderFeatures(usable); await settle(); return shown(); };

// Probes start only after whoami answers; disabled auth is a usable local account.
let answer;
auth.whoami = () => new Promise(resolve => { answer = resolve; });
const refreshing = context.refreshHarnessAuth();
await new Promise(setImmediate);
assert.deepEqual(probes, [], 'no probe before the account state is known');
assert.deepEqual(shown(), []);
answer(reply(503)); await refreshing; await settle();
assert.deepEqual(shown(), ['openAnalytics'], 'shipped defaults: notifications and coding pipeline disabled');
assert.deepEqual(probes.sort(), ['/api/notifications', '/api/tools']);

assert.deepEqual(await paint(true, enabled), buttons);
for (const [label, failure] of [['401', refused(401)], ['403', refused(403)], ['network', new TypeError('Failed to fetch')]]) {
  assert.deepEqual(await paint(true, {'/api/notifications':failure, '/api/tools':failure}), ['openAnalytics'], label + ' keeps both hidden');
}
assert.deepEqual(await paint(true, {'/api/notifications':{}, '/api/tools':{}}), ['openAnalytics'], 'an empty reply is not an enabled feature');
assert.deepEqual(await paint(true, {'/api/notifications':{enabled:'true'},
  '/api/tools':{tools:[null, 7, {path:'/api/agent/runs',enabled:true}, {path:'/api/agent/schedules',enabled:'true'}]}}), ['openAnalytics'], 'only a literal true enables');
assert.deepEqual(await paint(true, {'/api/notifications':enabled['/api/notifications'], '/api/tools':refused(401)}), ['openAnalytics', 'openDeliveries']);
assert.deepEqual(await paint(true, {'/api/notifications':refused(401), '/api/tools':enabled['/api/tools']}), ['openAnalytics', 'openSchedules']);

// A probe that answers after logout cannot reveal a button.
let late;
replies = {'/api/notifications':() => new Promise(resolve => { late = resolve; }), '/api/tools':enabled['/api/tools']};
const stale = original(true);
context.refreshHeaderFeatures(false);
late(enabled['/api/notifications']); await stale; await settle();
assert.deepEqual(shown(), []);

// Signed out: nothing is shown and nothing is probed.
await paint(true, enabled);
probes = []; auth.whoami = reply(401);
await context.refreshHarnessAuth(); await settle();
assert.deepEqual(shown(), []);
assert.deepEqual(probes, [], 'signed-out consoles send no feature probes');
// Signed out, the sign-in card replaces the console; the default-password note needs the server's word.
assert.ok(page.body.classList.contains('signed-out'));
assert.equal(node('hAuthLoginBox').hidden, false);
assert.equal(node('hAuthHint').hidden, true, 'no note unless setup-status reports default_password');
auth.setup = reply(200, {needs_password:false, default_password:true});
await context.refreshHarnessAuth(); await settle();
assert.equal(node('hAuthHint').hidden, false, 'note shown while the shipped password is active');
auth.setup = reply(200, {needs_password:false, default_password:false});
await context.refreshHarnessAuth(); await settle();
assert.equal(node('hAuthHint').hidden, true, 'note hidden once it changed');
assert.ok(!page.documentElement.classList.contains('auth-pending'));

// Signed in: the account's features are probed; a pending password change shows nothing yet.
replies = enabled; auth.whoami = reply(200, {username:'admin', role:'admin', must_change_password:true});
await context.refreshHarnessAuth(); await settle();
assert.deepEqual(shown(), []);
assert.deepEqual(probes, []);
assert.deepEqual(passwordPrompts, [true]);
auth.whoami = reply(200, {username:'operator', role:'operator', must_change_password:false});
await context.refreshHarnessAuth(); await settle();
assert.deepEqual(shown(), buttons);
assert.ok(!page.body.classList.contains('signed-out'), 'a session removes the sign-in card');

// Login re-evaluates; so does bootstrap-password setup. Blank fields never reach the server.
context.refreshHeaderFeatures(false); await settle();
auth.login = () => assert.fail('a blank sign-in must not send a request');
await node('hAuthLogin').listeners.click(); await settle();
assert.equal(node('hAuthError').textContent, 'Enter your username and password.');
auth.login = reply(200, {username:'admin', role:'admin', must_change_password:false});
node('hAuthUser').value = 'admin'; node('hAuthPass').value = 'fixture-password';
await node('hAuthLogin').listeners.click(); await settle();
assert.deepEqual(shown(), buttons);
context.refreshHeaderFeatures(false); await settle();
auth.login = reply(401);
node('hAuthPass').value = 'wrong-password';
await node('hAuthLogin').listeners.click(); await settle();
assert.equal(node('hAuthError').textContent, "That username and password don't match. Check both and try again.");
assert.deepEqual(shown(), [], 'a failed login shows nothing');
auth.bootstrap = reply(200, {username:'admin', role:'admin'});
await node('hAuthSetupBtn').listeners.click(); await settle();
assert.deepEqual(shown(), buttons);

// Logout hides first; a rejected logout that kept the session restores and rechecks.
vm.runInContext("currentSession = 'fixture-session'", context);
auth.logout = reply(500);
await node('hAuthLogout').listeners.click(); await settle();
assert.deepEqual(shown(), buttons);
assert.equal(node('hAuthWho').textContent, 'logout rejected (500)');
assert.equal(context.currentSession, 'fixture-session', 'a 500 logout keeps the session and its view');
auth.logout = reply(401); auth.whoami = reply(401);
await node('hAuthLogout').listeners.click(); await settle();
assert.deepEqual(shown(), [], 'a 401 logout rechecks whoami and stays hidden when signed out');
assert.equal(context.currentSession, null, 'a 401 logout resets the account view');
assert.ok(transcriptClears >= 1, 'a 401 logout clears the transcript');
// The reset does not depend on the follow-up probe: a failed whoami still leaves it reset.
vm.runInContext("currentSession = 'second-session'", context);
auth.logout = reply(401); auth.whoami = () => { throw new TypeError('Failed to fetch'); };
await node('hAuthLogout').listeners.click(); await settle();
assert.equal(context.currentSession, null, 'a 401 logout resets even when the probe fails');
console.log('header buttons: hidden until auth is known, signed-out and failed probes stay hidden, enabled features shown, login/setup/logout re-evaluate, stale probes discarded, sign-in card and default-password note passed');
