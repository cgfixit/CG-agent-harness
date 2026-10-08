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
  '- GOAL_DONE', '__GOAL_DONE__', 'Summary.\n\n**GOAL_DONE**\n']) {
  assert.equal(replyHasGoalDone(done), true, JSON.stringify(done));
}
for (const notDone of ['', 'I will write GOAL_DONE when finished.', 'GOAL_DONE_LATER', 'NOT GOAL_DONE', 'goal_done', null]) {
  assert.equal(replyHasGoalDone(notDone), false, JSON.stringify(notDone));
}

const prompt = loopTurnPrompt(2, 3, 'Write   a\nparser');
assert.ok(prompt.startsWith('Loop turn 2 of 3. Session goal: Write a parser\n'), prompt);
assert.ok(prompt.includes('GOAL_DONE on its own line') && !/27b/i.test(prompt), 'model-agnostic marker instruction');
const long = loopTurnPrompt(1, 1, 'x'.repeat(2000));
assert.ok(long.includes('x'.repeat(239) + '…') && !long.includes('x'.repeat(240)), 'goal reminder is bounded');
const astral = loopTurnPrompt(1, 1, 'x'.repeat(238) + '😀😀 tail');
assert.ok(astral.includes('x'.repeat(238) + '😀…'), astral);
assert.ok(!/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(astral), 'no lone surrogate reaches JSON');

const answer = 'The parser reads tokens from the lexer, builds an AST for each statement, and reports the first syntax error with its line number and column.';
assert.equal(repliesNearlyRepeat(answer, answer), true);
assert.equal(repliesNearlyRepeat(answer, '**' + answer.toUpperCase() + '**'), true, 'formatting and case are not progress');
assert.equal(repliesNearlyRepeat(answer, answer.replace('first syntax error', 'first syntax errors')), true, 'a one-word edit of a long reply is a stall');
assert.equal(repliesNearlyRepeat(answer, 'Next, add error recovery: skip to the next semicolon after a syntax error and keep parsing so later errors are reported too.'), false);
assert.equal(repliesNearlyRepeat('Step 1 done.', 'Step 2 done.'), false, 'short replies compare word for word');
console.log('loop continuation: GOAL_DONE decorations, bounded model-agnostic prompt and near-repeat stall passed');
