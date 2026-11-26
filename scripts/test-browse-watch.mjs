// /browse follows its task through a read that got no answer. The real
// watchBrowseJob runs in a VM against a scripted API: a dropped connection,
// a rate limit or a 5xx is tried again, so a waiting request's card still
// appears; a gone job, a refused sign-in or a switched view stop at once.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const from = html.indexOf("let browseJobId = '';"), to = html.indexOf('async function watchResearchJob(', from);
assert.ok(from >= 0 && to > from, 'console markers must bound the watcher slice');
const poll = 'const BROWSE_POLL_MS = 1500;';
const slice = html.slice(from, to);
assert.ok(slice.includes(poll), 'the poll interval is where this test expects it');

const failure = (code, status, extra = {}) => Object.assign(new Error(code + ': ' + status), {code, status, ...extra});
const unanswered = () => failure('NETWORK', undefined);
function harness(replies) {
  const store = new Map(), lines = [], cards = [], results = [];
  let reads = 0;
  const context = vm.createContext({
    sessionStorage: {getItem: k => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k)},
    sys: text => lines.push(String(text)),
    showBrowseApproval: (id, pending) => cards.push([id, pending.request]),
    renderBrowseResult: result => results.push(result),
    apiRetryDelay: () => 0,
    api: async () => {
      const reply = replies[Math.min(reads, replies.length - 1)];
      reads += 1;
      if (typeof reply === 'function') return reply();
      if (reply instanceof Error) throw reply;
      return reply;
    },
    setTimeout, JSON, encodeURIComponent,
  });
  vm.runInContext('var sessionRevision = 0, currentSession = null;\n' + slice.replace(poll, 'const BROWSE_POLL_MS = 1;'), context);
  const run = code => vm.runInContext(code, context);
  return {run, lines, cards, results, store, reads: () => reads};
}
const JOB = 'b'.repeat(32);
const waiting = {status: 'waiting', pending: {request: 3, url: 'https://shop.example/cart', sha256: 'c'.repeat(64)}};

// A dropped connection, a rate limit and a 502 in a row: the watcher keeps
// going, shows the waiting request's card, and renders the result.
{
  const h = harness([unanswered(), failure('RATE_LIMITED', 429, {retryAfter: 0}), failure('', 502), waiting,
    {status: 'finished', result: {finished: true}}]);
  h.run(`rememberBrowseJob(${JSON.stringify(JOB)}, null)`);
  await h.run(`watchBrowseJob(${JSON.stringify(JOB)})`);
  assert.deepEqual(h.cards, [[JOB, 3]], 'the waiting request is shown after the failed reads');
  assert.equal(h.results.length, 1);
  assert.equal(h.lines.filter(l => /Lost contact/.test(l)).length, 1, 'one notice for one run of failures');
  assert.ok(h.lines.some(l => /Following the browser task again/.test(l)), h.lines.join('\n'));
  assert.equal(h.store.size, 0, 'a finished task is forgotten');
  assert.equal(h.run('browseJobId'), '');
}

// A job the server no longer has, or a refused sign-in, is not tried again.
for (const [refusal, kept] of [[failure('BROWSER_JOB_NOT_FOUND', 404), false], [failure('AUTH_REQUIRED', 401), true]]) {
  const h = harness([refusal, waiting]);
  h.run(`rememberBrowseJob(${JSON.stringify(JOB)}, null)`);
  await h.run(`watchBrowseJob(${JSON.stringify(JOB)})`);
  assert.equal(h.reads(), 1, refusal.code + ' is read once');
  assert.deepEqual(h.cards, []);
  assert.equal(h.store.size === 1, kept, refusal.code + ': the stored pair');
}

// Reads that keep getting no answer stop after BROWSE_POLL_RETRIES, and say
// the task still runs; the pair stays so a reload follows it again.
{
  const h = harness([failure('', 503)]);
  h.run(`rememberBrowseJob(${JSON.stringify(JOB)}, null)`);
  await h.run(`watchBrowseJob(${JSON.stringify(JOB)})`);
  assert.equal(h.reads(), h.run('BROWSE_POLL_RETRIES') + 1);
  assert.match(h.lines.at(-1), /Stopped following the browser task/);
  assert.equal(h.store.size, 1, 'the pair is kept for a reload');
}

// A view switched while a read is being retried ends that watcher silently.
{
  let h;
  const switched = () => { h.run('sessionRevision++'); throw unanswered(); };
  h = harness([unanswered(), switched, waiting]);
  await h.run(`watchBrowseJob(${JSON.stringify(JOB)})`);
  assert.deepEqual(h.cards, [], 'a stale watcher shows nothing');
  assert.equal(h.lines.filter(l => /Stopped following/.test(l)).length, 0);
}

console.log('browse watch: unanswered reads retried, card still shown, gone job and refused sign-in stop at once, retries bounded, stale view silent passed');
