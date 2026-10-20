// Execute the shipped account UI and API helper against a restricted account fixture.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import {randomBytes} from 'node:crypto';
const setupCode = randomBytes(24).toString('hex');

const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
function between(start, end) {
  const from = html.indexOf(start), to = html.indexOf(end, from);
  assert.ok(from >= 0 && to > from, 'missing source boundaries: ' + start);
  return html.slice(from, to);
}
const nodes = new Map();
const node = id => {
  if (!nodes.has(id)) nodes.set(id, {id, value:'', textContent:'', hidden:false, disabled:false, open:false, listeners:{},
    addEventListener(event, listener) { this.listeners[event] = listener; },
    focus() { page.activeElement = this; }, contains() { return false; }, replaceChildren() { this.textContent = ''; },
    showModal() { this.open = true; }, close() { this.open = false; this.listeners.close?.(); },
    reset() { for (const field of ['passwordCurrent', 'passwordNew', 'passwordConfirm']) node(field).value = ''; },
  });
  return nodes.get(id);
};
const classes = () => { const values = new Set(); return {
  contains: name => values.has(name), add: name => values.add(name), remove: name => values.delete(name),
  toggle(name, enabled) { if (enabled) values.add(name); else values.delete(name); },
}; };
const page = {getElementById:node, body:{classList:classes()}, documentElement:{classList:classes()}, activeElement:null};
page.documentElement.classList.add('auth-pending');
let pending = true, session = null, savedPassword = setupCode, failProbe = false;
const requests = [];
const response = (status, body) => new Response(JSON.stringify(body), {status, headers:{'Content-Type':'application/json'}});
const account = () => ({username:'admin', role:'admin', must_change_password:pending});
async function request(path, options = {}) {
  const body = options.body ? JSON.parse(options.body) : undefined;
  requests.push({path, method:options.method || 'GET', body, headers:options.headers});
  if (path === '/api/auth/whoami') {
    if (failProbe) throw new TypeError('Failed to fetch');
    return session ? response(200, account()) : response(401, {});
  }
  if (path === '/api/auth/setup-status') return response(200, {default_password:false, setup_code:pending});
  if (path === '/api/auth/login') {
    if (body.username !== 'admin' || body.password !== savedPassword) return response(401, {});
    session = pending ? 'restricted' : 'normal';
    return response(200, account());
  }
  if (path === '/api/auth/password') {
    assert.equal(options.headers['X-CyClaw-CSRF'], 'fixture-page-csrf', 'the protected request retains page CSRF');
    if (!session) return response(401, {detail:{message:'Sign in again.'}});
    if (body.current_password !== savedPassword) return response(401, {detail:{message:'Current password did not match.'}});
    if (body.password.length < 12) return response(400, {detail:{message:'Use at least 12 characters.'}});
    savedPassword = body.password; pending = false; session = 'normal';
    return response(200, {ok:true});
  }
  assert.equal(session, 'normal', 'no operational request while setup is pending: ' + path);
  if (path === '/api/notifications') return response(200, {enabled:false});
  if (path === '/api/tools') return response(200, {tools:[]});
  assert.fail('unexpected request ' + path);
}
const context = vm.createContext({
  $:node, document:page, window:{location:{href:'https://127.0.0.1:9000/harness'}},
  fetchWithTimeout:request, fetchHold:async (path, options) => ({resp:await request(path, options), release() {}}),
  CSRF_TOKEN:'fixture-page-csrf', apiKeyInput:null, inflightChat:null, FormData, URL,
  clearAnalytics() {}, clearPendingAttachments() {}, clearTimeout() {}, clearTranscript() {}, stopLoop() {},
  paintSessionTokens() {}, paintStyle() {}, abortInflightSearch() {}, sys() {},
  refreshStatus:async () => {}, refreshSessions:async () => {}, refreshMemoryProposals:async () => {},
  memoryRefreshTimer:null, memoryViewRevision:0, memoryRefreshBusy:false, netconnectRevision:0,
  currentSession:null, sessionGoal:'', loopAuto:false, retryAttachmentIds:null, promptHistory:{},
  currentStyle:'off', currentStyleIssue:null, sessionRevision:0, sendBtn:{disabled:false}, input:node('input'),
});
vm.runInContext(between('let apiRetryUntil = 0;', 'async function readChatStream(') + '\n' +
  between('const hAuthLogin = document.getElementById(', 'refreshHarnessAuth();\nlet memoryRefreshTimer'), context);
const settle = () => new Promise(setImmediate);
const submit = async (password, confirmation = password) => {
  if (pending || session === 'normal' && node('passwordCurrentLabel').textContent === 'Setup code') node('passwordCurrent').value = setupCode;
  node('passwordNew').value = password; node('passwordConfirm').value = confirmation;
  await node('passwordForm').listeners.submit({preventDefault() {}}); await settle();
};

await context.refreshHarnessAuth(); await settle();
assert.deepEqual(requests.map(r => r.path), ['/api/auth/whoami', '/api/auth/setup-status']);
assert.ok(requests.every(r => r.method === 'GET'), 'opening setup creates no login or session');
assert.equal(session, null);
assert.equal(node('passwordTitle').textContent, 'Set your password');
assert.ok(node('passwordDialog').open);
assert.equal(node('passwordCurrentLabel').textContent, 'Setup code');
assert.equal(page.activeElement.id, 'passwordCurrent');
assert.ok(node('hAuthLoginBox').hidden);
assert.ok(page.body.classList.contains('password-required'));

await submit('first-password-123', 'does-not-match');
assert.equal(requests.length, 2, 'a mismatched password never authenticates');
assert.equal(node('passwordFeedback').textContent, 'Passwords do not match.');
await submit('short');
assert.deepEqual(requests.slice(2).map(r => r.path), ['/api/auth/login', '/api/auth/password']);
assert.deepEqual(requests[2].body, {username:'admin', password:setupCode});
assert.equal(session, 'restricted');
assert.match(node('passwordFeedback').textContent, /at least 12/);
assert.ok(page.body.classList.contains('password-required'));
assert.ok(node('reloadLimits').hidden && node('webModeToggle').hidden);
node('passwordDialog').close();
assert.ok(page.body.classList.contains('password-required'), 'dismissal never unlocks the console');
node('passwordLockCard').listeners.submit({preventDefault() {}});
assert.equal(node('passwordCurrentLabel').textContent, 'Setup code', 'retry requires the private code again');

await submit('first-password-123');
assert.equal(requests.filter(r => r.path === '/api/auth/login').length, 1, 'policy retry reuses the restricted session');
assert.equal(session, 'normal');
assert.ok(!node('passwordDialog').open);
assert.ok(!page.body.classList.contains('password-required'));
assert.ok(!page.body.classList.contains('signed-out'));
assert.ok(!node('passwordSave').disabled);

// Reloading/signing out after completion presents ordinary login.
session = null;
await context.refreshHarnessAuth(); await settle();
assert.ok(!node('hAuthLoginBox').hidden);
assert.ok(!node('passwordDialog').open && node('passwordLockScreen').hidden);
assert.ok(!html.includes('id="hAuthHint"') && !html.includes('setupCodePending'), 'no retired shortcut hint survives');

// A second tab completing setup retires an already-open direct-setup form safely.
pending = true; savedPassword = setupCode;
await context.refreshHarnessAuth(); await settle();
pending = false; savedPassword = 'another-tab-password';
const beforeRace = requests.length;
await submit('this-tab-password');
assert.ok(!node('passwordDialog').open && !node('hAuthLoginBox').hidden);
assert.match(node('hAuthError').textContent, /Initial setup is complete/);
assert.ok(!requests.slice(beforeRace).some(r => r.path === '/api/auth/password'), 'a failed bootstrap never sets a password');

// Browser tabs share the normal cookie installed by the tab that completed setup.
pending = true; savedPassword = setupCode; session = null;
await context.refreshHarnessAuth(); await settle();
pending = false; savedPassword = 'shared-tab-password'; session = 'normal';
const beforeSharedCookie = requests.length;
await submit('stale-tab-password');
assert.ok(!node('passwordDialog').open && !page.body.classList.contains('password-required'));
assert.ok(!page.body.classList.contains('signed-out'));
assert.equal(savedPassword, 'shared-tab-password', 'stale form never replaces the other tab password');
assert.ok(!requests.slice(beforeSharedCookie).some(r => r.path === '/api/auth/password'));
node('hAuthPassword').listeners.click();
assert.ok(!node('passwordCurrentField').hidden && !node('passwordCurrent').disabled, 'ordinary change is restored');
node('passwordDialog').close();

// The same completion race can happen after a prior restricted-login retry.
pending = true; savedPassword = setupCode; session = 'restricted';
await context.refreshHarnessAuth(); await settle();
pending = false; savedPassword = 'completed-while-retrying'; session = 'normal';
await submit('stale-retry-password');
assert.equal(savedPassword, 'completed-while-retrying');
assert.ok(!node('passwordDialog').open && !page.body.classList.contains('password-required'));
assert.ok(!page.body.classList.contains('signed-out'));

// A restricted session expiring between retries returns to setup and can authenticate again.
pending = true; savedPassword = setupCode; session = 'restricted';
await context.refreshHarnessAuth(); await settle();
assert.equal(node('passwordCurrentLabel').textContent, 'Setup code');
session = null;
await submit('retry-after-expiry');
assert.ok(node('passwordDialog').open && page.body.classList.contains('password-required'));
await submit('retry-after-expiry');
assert.equal(savedPassword, 'retry-after-expiry');
assert.ok(!page.body.classList.contains('password-required'));

// Ordinary changes still ask for and send the account's current password.
node('hAuthPassword').listeners.click();
assert.ok(!node('passwordCurrentField').hidden && !node('passwordCurrent').disabled);
node('passwordCurrent').value = savedPassword;
await submit('second-password-123');
assert.equal(savedPassword, 'second-password-123');

// Existing accounts retain an ordinary sign-in path while admin setup is pending.
pending = true; savedPassword = setupCode; session = null;
await context.refreshHarnessAuth(); await settle();
node('passwordDialog').close();
node('passwordSignIn').listeners.click();
assert.ok(!node('hAuthLoginBox').hidden && !node('passwordDialog').open);
assert.ok(page.body.classList.contains('signed-out'));

// Knowing the account name is insufficient: a wrong setup code cannot claim it.
pending = true; savedPassword = setupCode; session = null;
await context.refreshHarnessAuth(); await settle();
const wrongCodeStart = requests.length;
node('passwordCurrent').value = 'admin';
node('passwordNew').value = node('passwordConfirm').value = 'attacker-password-123';
await node('passwordForm').listeners.submit({preventDefault() {}}); await settle();
assert.equal(session, null);
assert.equal(savedPassword, setupCode);
assert.ok(!requests.slice(wrongCodeStart).some(r => r.path === '/api/auth/password'));
assert.ok(page.body.classList.contains('password-required'));

// Unreachable startup falls back to sign-in, never to a usable console.
session = null; failProbe = true;
page.documentElement.classList.add('auth-pending');
await context.refreshHarnessAuth();
assert.ok(page.body.classList.contains('signed-out'));
assert.match(node('hAuthError').textContent, /Can't reach the harness/);
console.log('First-run password: explicit bootstrap, protected CSRF change, lock/retry, stale tab, expired session, completion, normal change and failure recovery passed.');
