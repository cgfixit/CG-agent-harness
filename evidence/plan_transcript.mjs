// Replays the operator's reported session against the real console.
import assert from 'node:assert/strict';
const text = ms => ms.map(m => m.kind + ': ' + m.body).join('\n');
const step = (id, area, input, run) => ({ id, area, input, run });

export default [
  step('T01', 'session', '/session new hvac', async a => text(await a.send('/session new hvac'))),
  step('T02', 'chat', 'yoo', async a => text(await a.send('yoo'))),
  step('T03', 'goal', '/goal find the best hvac repair service near cumming, georgia 30041', async a =>
    text(await a.send('/goal find the best hvac repair service thats not too expensive (mid-tier) near cumming, georgia or zip code 30041'))),
  step('T04', 'goal', 'chat: use /web search and /web research …', async a => {
    const out = text(await a.send('use /web search and /web research to research goal and reply with findings'));
    return out;
  }),
  step('T05', 'web', '/web status', async a => text(await a.send('/web status'))),
  step('T06', 'web', '/web allow *.google.com google.com/* --group google', async a => {
    const out = text(await a.send('/web allow *.google.com google.com/* --group google'));
    assert.doesNotMatch(out, /wildcards require an explicit host/);
    return out;
  }),
  step('T07', 'web', 'web allow /*.google.com google.com/* --group google (no slash)', async a => {
    const out = text(await a.send('web allow /*.google.com google.com/* --group google'));
    return out;
  }),
  step('T08', 'input', 'paste "/web allow https://www.google.com/*\\n" from a code block', async a => {
    const r = await a.paste('/web allow https://www.google.com/*\n');
    assert.doesNotMatch(text(r.added), /paste refused/);
    return r;
  }),
  step('T09', 'web', 'web check https://www.google.com (no slash)', async a => text(await a.send('web check https://www.google.com'))),
  step('T10', 'history', 'ArrowUp recalls every submitted line', async a => {
    await a.page.fill('#input', '');
    const seen = [];
    for (let i = 0; i < 6; i++) seen.push(await a.arrow('ArrowUp'));
    assert.ok(seen.some(v => v.startsWith('/web allow *.google.com')), 'refused slash command must be recallable: ' + JSON.stringify(seen));
    return seen;
  }),
];
