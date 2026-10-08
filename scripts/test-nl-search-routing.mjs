// Exercise the shipped natural-language search predicate against its server mirror's phrasings.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const start = html.indexOf('function looksLikeNlSearch(text)');
const end = html.indexOf('var MAX_WEB_QUERY_LEN', start);
assert.ok(start !== -1 && end > start, 'predicate is present in the console');
const ctx = vm.createContext({});
vm.runInContext(html.slice(start, end) + ';this.f=looksLikeNlSearch;', ctx);
for (const routed of ['search first 3 results for rust lifetimes', 'search top 3 results for rust lifetimes',
  'search the first 3 results for rust lifetimes', 'search first 2 pages for "rust"', 'search "tokio select"',
  'search top 3 pages of rust docs']) {
  assert.equal(ctx.f(routed), true, routed);
}
for (const chat of ['search the first 2 pages of the rust book for lifetimes', 'search top 10 movies',
  'search top 3 results for rust and summarize them', 'what is the top 3 results thing']) {
  assert.equal(ctx.f(chat), false, chat);
}
console.log('nl search routing: first/top count clauses, pages-of subjects and follow-ups passed');
