// Execute the actual inline workflow scripts against isolated GitHub API fixtures.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
function scripts(file) {
  const text = readFileSync(new URL(`../.github/workflows/${file}`, import.meta.url), 'utf8');
  return [...text.matchAll(/^          script: \|\n((?:            .*\n|\n)+)/gm)]
    .map(match => new AsyncFunction('github', 'context', 'core', 'process',
      match[1].split('\n').map(line => line.slice(12)).join('\n')));
}
const [resolve] = scripts('review-gate.yml');
const [mark, evaluate, publish] = scripts('review-gate-evaluate.yml');
assert.equal(scripts('review-gate.yml').length, 1);
assert.equal(scripts('review-gate-evaluate.yml').length, 3);

const sha = 'a'.repeat(40);
const repo = { full_name: 'owner/repo', id: 1 };
const pr = { number: 23, state: 'open', head: { sha, ref: 'topic', repo }, base: { repo } };
const run = {
  id: 80, run_attempt: 1, workflow_id: 9, repository: repo, head_repository: repo,
  head_sha: sha, head_branch: 'topic', path: '.github/workflows/review-gate-request.yml',
  event: 'pull_request', status: 'completed', conclusion: 'success', pull_requests: [],
};
const context = { repo: { owner: 'owner', repo: 'repo' }, eventName: 'workflow_run',
  payload: { workflow_run: run }, runId: 90, runAttempt: 1 };
const env = {
  PR_NUMBER: '23', EXPECTED_HEAD_SHA: sha, MARKED_HEAD_SHA: sha, CHECK_NAME: 'review gate',
  CHECK_RUN_ID: '44', MARKER_RESULT: 'success', EVAL_RESULT: 'success', FINISHED: 'true',
  CONCLUSION: 'success', THREAD_COUNT: '0', RUNNING_COUNT: '0', REVIEW_BOTS: 'chatgpt-codex-connector[bot]',
  GITHUB_RUN_ATTEMPT: '1',
  POLL_MINUTES: '0', GRACE_SECONDS: '0', WAIT_FOR_BOT: 'true',
};
const bot = 'chatgpt-codex-connector[bot]';
function fixture(options = {}) {
  const outputs = {}, failures = [], writes = [], pages = [];
  const core = { setOutput: (key, value) => { outputs[key] = value; },
    setFailed: message => failures.push(message), info: () => {},
    summary: { addRaw: () => ({ write: async () => {} }) } };
  const github = {
    rest: {
      actions: { getWorkflowRun: async () => ({ data: { ...run, ...options.run } }),
        getWorkflow: async () => ({ data: { id: 9 } }) },
      pulls: { list: 'pulls', get: async () => ({ data: options.livePr || pr }) },
      issues: { listComments: 'comments' },
      checks: {
        create: async request => { writes.push(request); return { data: { id: 44 } }; },
        get: async () => ({ data: { name: 'review gate', head_sha: sha, external_id: 'review-gate:90:1', ...options.check } }),
        update: async request => {
          if (options.publishError) throw new Error('API unavailable');
          writes.push(request);
        },
      },
    },
    paginate: async method => method === 'pulls' ? options.prs || [pr] : options.comments || [],
    graphql: async (_query, variables) => {
      pages.push(variables.after);
      if (options.readError) throw new Error('API unavailable');
      const nodes = options.threads || [];
      return { repository: { pullRequest: { reviewThreads: {
        nodes: variables.after ? nodes.slice(1) : nodes.slice(0, 1),
        pageInfo: { hasNextPage: !variables.after && nodes.length > 1, endCursor: 'page2' },
      } } } };
    },
  };
  return { outputs, failures, writes, pages, execute: script => script(github,
    { ...context, ...options.context }, core, { env: { ...env, ...options.env } }) };
}
let cases = 0;
async function check(script, options, verify) {
  const f = fixture(options);
  await f.execute(script);
  verify(f);
  cases++;
}

// The relay's result is never a verdict. Forks need no artifact or populated PR array.
for (const event of ['pull_request', 'pull_request_review', 'pull_request_review_comment']) {
  for (const conclusion of ['success', 'failure', 'cancelled']) {
    await check(resolve, { run: { event, conclusion } }, f => {
      assert.equal(f.outputs.pr_number, '23');
      assert.equal(f.outputs.head_sha, sha);
      assert.equal(f.outputs.wait_for_bot, String(event === 'pull_request'));
    });
  }
}
const fork = { id: 2, full_name: 'contributor/repo' };
await check(resolve, { run: { head_repository: fork, actor: { login: 'contributor' } },
  prs: [{ ...pr, head: { ...pr.head, repo: fork } }],
  livePr: { ...pr, head: { ...pr.head, repo: fork } } }, f => assert.equal(f.outputs.head_sha, sha));
await check(resolve, { run: { actor: { login: 'dependabot[bot]' } } }, f => assert.equal(f.outputs.pr_number, '23'));
await check(resolve, { run: { path: '.github/workflows/review-gate-request.yml@refs/pull/23/merge' } },
  f => assert.equal(f.outputs.pr_number, '23'));
for (const invalid of [{ repository: fork }, { workflow_id: 10 }, { path: '.github/workflows/other.yml' },
  { event: 'push' }, { status: 'in_progress' }, { run_attempt: 2 }, { head_sha: 'main' }]) {
  await assert.rejects(fixture({ run: invalid }).execute(resolve), /unrecognized/);
  cases++;
}
for (const invalid of [{ head: { ...pr.head, sha: 'b'.repeat(40) } }, { head: { ...pr.head, ref: 'other' } },
  { head: { ...pr.head, repo: fork } }, { head: { ...pr.head, repo: null } }]) {
  await check(resolve, { prs: [{ ...pr, ...invalid }] }, f => assert.deepEqual(f.outputs, {}));
}
await check(resolve, { prs: [] }, f => assert.deepEqual(f.outputs, {}));
await assert.rejects(fixture({ prs: [pr, { ...pr, number: 24 }] }).execute(resolve), /more than one/);
cases++;
await check(resolve, { livePr: { ...pr, head: { ...pr.head, sha: 'b'.repeat(40) } } }, f => assert.deepEqual(f.outputs, {}));

for (const login of [bot, 'human']) {
  await check(resolve, { context: { eventName: 'issue_comment', payload: {
    issue: { number: 23, state: 'open', pull_request: {} }, comment: { user: { login } },
  } } }, f => assert.equal(f.outputs.pr_number, login === bot ? '23' : undefined));
}
await check(resolve, { context: { eventName: 'issue_comment', payload: {
  issue: { number: 23, state: 'open' }, comment: { user: { login: bot } },
} } }, f => assert.deepEqual(f.outputs, {}));

await check(mark, {}, f => {
  assert.equal(f.writes[0].name, 'review gate');
  assert.equal(f.writes[0].head_sha, sha);
  assert.equal(f.writes[0].status, 'in_progress');
  assert.equal(f.outputs.marked, 'true');
});
for (const livePr of [{ ...pr, state: 'closed' }, { ...pr, head: { ...pr.head, sha: 'b'.repeat(40) } }]) {
  await check(mark, { livePr }, f => { assert.equal(f.writes.length, 0); assert.equal(f.outputs.marked, 'false'); });
}
for (const bad of [{ PR_NUMBER: '0' }, { PR_NUMBER: '23\ninjected=true' }, { EXPECTED_HEAD_SHA: 'main' }]) {
  await assert.rejects(fixture({ env: bad }).execute(mark), /invalid PR identity/);
  cases++;
}

await check(evaluate, {}, f => { assert.equal(f.outputs.conclusion, 'success'); assert.equal(f.outputs.finished, 'true'); });
const thread = { isResolved: false, isOutdated: true, path: 'src/file.rs', line: 1,
  comments: { nodes: [{ body: '<b>Finding</b>', author: { login: 'reviewer' }, url: 'https://example.test/thread' }] } };
await check(evaluate, { threads: [thread, { ...thread, isResolved: true }] }, f => {
  assert.equal(f.outputs.conclusion, 'neutral'); assert.equal(f.outputs.thread_count, '1');
  assert.deepEqual(f.pages, [null, 'page2']);
});
for (const login of [bot, 'human']) {
  await check(evaluate, { comments: [{ user: { login }, body: '<!-- codex-pull-request-review-summary --> **Running**' }] },
    f => assert.equal(f.outputs.conclusion, login === bot ? 'neutral' : 'success'));
}
await check(evaluate, { livePr: { ...pr, head: { ...pr.head, sha: 'b'.repeat(40) } } }, f => {
  assert.equal(f.failures.length, 1); assert.equal(f.outputs.finished, undefined);
});
await assert.rejects(fixture({ readError: true }).execute(evaluate), /API unavailable/);
cases++;

for (const [extra, conclusion] of [[{}, 'success'], [{ CONCLUSION: 'neutral', THREAD_COUNT: '1' }, 'neutral'],
  [{ EVAL_RESULT: 'failure' }, 'failure'], [{ EVAL_RESULT: 'cancelled' }, 'failure'],
  [{ FINISHED: 'false' }, 'failure'], [{ THREAD_COUNT: '-1' }, 'failure'],
  [{ THREAD_COUNT: '9007199254740992' }, 'failure'], [{ THREAD_COUNT: '1' }, 'failure']]) {
  await check(publish, { env: extra }, f => {
    assert.equal(f.writes[0].check_run_id, 44);
    assert.equal(f.writes[0].status, 'completed');
    assert.equal(f.writes[0].conclusion, conclusion);
  });
}
for (const key of ['MARKER_RESULT', 'EVAL_RESULT']) {
  await check(publish, { env: { [key]: 'skipped' } }, f => assert.equal(f.writes.length, 0));
}
for (const check of [{ head_sha: 'b'.repeat(40) }, { name: 'other' }, { external_id: 'review-gate:91:1' }]) {
  await assert.rejects(fixture({ check }).execute(publish), /not owned/);
  cases++;
}
await check(publish, { livePr: { ...pr, head: { ...pr.head, sha: 'b'.repeat(40) } } }, f => {
  assert.equal(f.writes[0].conclusion, 'failure');
  assert.match(f.writes[0].output.title, /head changed/);
});
await assert.rejects(fixture({ publishError: true }).execute(publish), /API unavailable/);
cases++;
console.log(`review-gate: ${cases} workflow-script scenarios passed`);
