// Exercise the shipped Setup script without starting a webview or an account.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const nodes = new Map();
const node = id => {
  if (!nodes.has(id)) nodes.set(id, {textContent: '', disabled: false, listeners: {},
    addEventListener(event, handler) { this.listeners[event] = handler; }});
  return nodes.get(id);
};
let snapshot = {message: 'Local model inventory checked.', details: '{"chat":"installed"}', setup_code: null};
let rejectModels, statusResponse, statusUnavailable = false, poll;
const context = vm.createContext({
  document: {getElementById: node, querySelectorAll: () => ['retry', 'models', 'prepare'].map(node)},
  window: {__TAURI_INTERNALS__: {invoke(command, args) {
    if (command === 'desktop_status') {
      const response = statusResponse;
      statusResponse = undefined;
      return response ?? (statusUnavailable ? Promise.reject('unavailable') : Promise.resolve(snapshot));
    }
    if (command === 'retry_backend') { assert.equal(args.initializeKey, null); return Promise.resolve(); }
    assert.equal(command, 'check_models');
    return new Promise((_, reject) => { rejectModels = reject; });
  }}},
  setInterval(callback, milliseconds) { assert.equal(milliseconds, 2000); poll = callback; },
});
vm.runInContext(readFileSync(new URL('../desktop/ui/setup.js', import.meta.url), 'utf8'), context);
const settle = () => new Promise(resolve => setImmediate(resolve));
await settle();
assert.equal(node('details').textContent, snapshot.details);

const pending = node('models').listeners.click();
assert.equal(node('status').textContent, 'Working…');
assert.equal(node('details').textContent, '', 'a new operation must hide the previous model result');
assert.ok(node('models').disabled);
rejectModels('Backend response timed out. Retry from setup.');
await pending;
assert.equal(node('status').textContent, 'Backend response timed out. Retry from setup.');
assert.equal(node('details').textContent, '', 'a failed operation must not show an old success');
assert.ok(!node('models').disabled);
await poll();
assert.match(node('status').textContent, /timed out/, 'polling must preserve the action error');

// A successful retry clears the action error; subsequent polls read current account state.
snapshot = {message: 'Backend ready. Sign in to the harness.', details: 'Owned endpoint: https://127.0.0.1:12345', setup_code: 'fixture-code'};
await node('retry').listeners.click();
await settle();
assert.match(node('status').textContent, /user admin, setup code fixture-code/);
snapshot = {...snapshot, setup_code: null};
await poll();
assert.equal(node('status').textContent, snapshot.message, 'retired first-sign-in guidance must disappear on refresh');

// Status requests started before an action cannot overwrite its pending or failed state.
for (const duringAction of [true, false]) {
  for (const rejected of [false, true]) {
    await node('retry').listeners.click();
    await settle();
    let finishStatus;
    statusResponse = new Promise((resolve, reject) => {
      finishStatus = () => rejected ? reject('unavailable') : resolve(snapshot);
    });
    const staleRefresh = poll();
    const pendingAction = node('models').listeners.click();
    const error = 'Backend response timed out. Retry from setup.';
    if (!duringAction) { rejectModels(error); await pendingAction; }
    finishStatus();
    await staleRefresh;
    assert.equal(node('status').textContent, duringAction ? 'Working…' : error,
      'a stale status response must not overwrite the current action');
    assert.equal(node('details').textContent, '', 'a stale status response must not restore old details');
    if (duringAction) { rejectModels(error); await pendingAction; }
  }
}
await node('retry').listeners.click();
await settle();
statusUnavailable = true;
await poll();
assert.equal(node('status').textContent, 'Setup connection unavailable. Quit and reopen the app.');
assert.equal(node('details').textContent, '', 'an unavailable status must not leave stale readiness details');
console.log('Desktop Setup state transitions passed.');
