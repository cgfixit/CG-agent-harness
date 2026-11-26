// #1123 phase 4: finding this tab's /browse task again. The real
// resumeBrowseJob and its storage helpers run in a VM against a stub API whose
// reply is held until the test releases it, so the selection can change while
// the load-time read is still in flight.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const between = (start, end) => {
  const from = html.indexOf(start), to = html.indexOf(end, from);
  assert.ok(from >= 0 && to > from, 'console markers must bound the slice: ' + start + ' … ' + end);
  return html.slice(from, to);
};

const store = new Map();
const lines = [], watched = [];
let pending = null;
const context = vm.createContext({
  sessionStorage: {getItem: k => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k)},
  sys: text => lines.push(String(text)),
  api: () => new Promise((resolve, reject) => { pending = {resolve, reject}; }),
  JSON, encodeURIComponent,
});
vm.runInContext('var sessionRevision = 0, currentSession = null;\n'
  + between("let browseJobId = '';", 'async function watchBrowseJob(')
  + '\nfunction watchBrowseJob(id, resumed) { watchedJobs.push([id, resumed, sessionRevision]); browseJobId = id; browseWatchRevision = sessionRevision; }', Object.assign(context, {watchedJobs: watched}));
const run = code => vm.runInContext(code, context);
const JOB = 'a'.repeat(32);
const stored = () => store.get(run('BROWSE_JOB_KEY')) ?? null;
const settle = () => new Promise(setImmediate);

// Only the pair is kept, and it is read back as written.
run(`rememberBrowseJob(${JSON.stringify(JOB)}, 'sess-a')`);
assert.deepEqual(JSON.parse(stored()), {session_id: 'sess-a', job_id: JOB});

// At load, a task left running in another session is pointed to.
let resume = run('resumeBrowseJob(true)');
await settle();
pending.resolve({status: 'waiting'});
await resume;
assert.equal(lines.length, 1);
assert.match(lines[0], /started in session sess-a is waiting/);

// The selection changes while the load-time read is in flight: the reply is
// dropped, success or failure, and nothing is printed for the old view.
lines.length = 0;
resume = run('resumeBrowseJob(true)');
await settle();
run('sessionRevision++');
pending.resolve({status: 'waiting'});
await resume;
assert.deepEqual(lines, [], 'a pointer that lands after a session switch or logout is dropped');
resume = run('resumeBrowseJob(true)');
await settle();
run('sessionRevision++');
pending.reject(Object.assign(new Error('no such browser job'), {code: 'BROWSER_JOB_NOT_FOUND'}));
await resume;
assert.deepEqual(lines, [], 'an "unavailable" reply that lands after a switch is dropped');

// Unchanged selection: a job the server no longer has is forgotten and reported.
resume = run('resumeBrowseJob(true)');
await settle();
pending.reject(Object.assign(new Error('no such browser job'), {code: 'BROWSER_JOB_NOT_FOUND'}));
await resume;
assert.equal(stored(), null, 'an unknown job is forgotten');
assert.match(lines.at(-1), /unavailable, nothing was resumed/);

// The task's own session: a watcher started for the view now shown is kept;
// one left from before the last navigation is replaced.
run(`rememberBrowseJob(${JSON.stringify(JOB)}, null)`);
await run('resumeBrowseJob(false)');
assert.equal(watched.length, 1, 'the shown session starts a watcher');
await run('resumeBrowseJob(false)');
assert.equal(watched.length, 1, 'a watcher for this view already follows the task');
run('sessionRevision++');
await run('resumeBrowseJob(false)');
assert.equal(watched.length, 2, 'a watcher from before the switch, still between polls, does not count');
assert.equal(watched[1][2], run('sessionRevision'));

console.log('browse rediscovery: pair-only storage, pointer, stale load-time replies dropped, unknown job forgotten, stale watcher replaced passed');
