const { test, before } = require('node:test');
const assert = require('node:assert/strict');
let createCollector, parseQuery, routePath;
before(async () => { ({ createCollector, parseQuery, routePath } = await import('../frontend/tools/model.js')); });

test('log identities survive history trimming without merging identical lines', async () => {
  const { logKey } = await import('../frontend/tools/log-row.js');
  const first = { timestamp: '2026-10-19T05:00:00Z', stream: 'stdout', text: 'same line' };
  const repeated = { ...first };
  const history = [first, repeated];
  const retainedKey = logKey(repeated);
  assert.notEqual(logKey(first), retainedKey);
  history.shift();
  assert.equal(logKey(history[0]), retainedKey);
  assert.notEqual(logKey({ ...repeated }), retainedKey);
});

test('log caches keep ordered recent rows with per-service and global capacity limits', async () => {
  const { updateLogCache, LOG_CACHE_LIMITS } = await import('../frontend/tools/log-cache.js');
  const entry = n => ({ timestamp: 'now', stream: 'stdout', text: `${n}:` + '中'.repeat(8192) });
  let cache = {};
  for (let service = 0; service < 10; service++) {
    cache = updateLogCache(cache, `service-${service}`, Array.from({ length: 1000 }, (_, n) => entry(n)));
  }
  const cost = rows => rows.reduce((sum, row) => sum + row.text.length + row.timestamp.length + row.stream.length, 0);
  assert.ok(Object.values(cache).reduce((sum, rows) => sum + cost(rows), 0) <= LOG_CACHE_LIMITS.total);
  assert.ok(Object.values(cache).every(rows => cost(rows) <= LOG_CACHE_LIMITS.perService));
  assert.ok(cache['service-9'].length < 1000);
  assert.match(cache['service-9'].at(-1).text, /^999:/);
  const latest = entry(1000);
  cache = updateLogCache(cache, 'service-9', [latest]);
  assert.equal(cache['service-9'].at(-1), latest);
  assert.ok(Number.parseInt(cache['service-9'][0].text) < 1000);
  cache = updateLogCache(cache, 'service-9', [entry(7)], true);
  assert.equal(cache['service-9'].length, 1);
  assert.match(cache['service-9'][0].text, /^7:/);
});

test('ordinary logs retain 1000 lines and cache updates do not mutate other views', async () => {
  const { updateLogCache } = await import('../frontend/tools/log-cache.js');
  const rows = Array.from({ length: 1200 }, (_, n) => ({ text: String(n), timestamp: 'now', stream: 'stdout' }));
  const prior = { web: rows.slice(0, 2) };
  const cache = updateLogCache(prior, 'api', rows);
  assert.equal(cache.api.length, 1000);
  assert.equal(cache.api[0].text, '200');
  assert.equal(cache.api.at(-1).text, '1199');
  assert.equal(cache.web, prior.web);
  assert.equal(prior.api, undefined);
});

test('background output evicts inactive caches before the service being viewed', async () => {
  const { updateLogCache, LOG_CACHE_LIMITS } = await import('../frontend/tools/log-cache.js');
  const quiet = [{ timestamp: 'now', stream: 'stdout', text: 'ready' }];
  let cache = { quiet };
  for (let service = 0; service < 10; service++) {
    const rows = Array.from({ length: 1000 }, () => ({ timestamp: 'now', stream: 'stdout', text: 'x'.repeat(8192) }));
    cache = updateLogCache(cache, `busy-${service}`, rows, false, new Set(['quiet']));
  }
  assert.equal(cache.quiet, quiet);
  assert.ok(cache['busy-9'].length > 0);
  const total = Object.values(cache).flat().reduce((n, entry) => n + entry.text.length + entry.timestamp.length + entry.stream.length, 0);
  assert.ok(total <= LOG_CACHE_LIMITS.total);
});

function add(collector, text, serviceId = 'api', timestamp = 'now') {
  collector.addBatch({ serviceId, entries: [{ text, timestamp }] });
}

test('collects known errors and merges non-adjacent repetitions', () => {
  const c = createCollector();
  add(c, 'Port 7779 was already in use');
  add(c, 'BUILD FAILURE');
  add(c, 'Port 7779 was already in use');
  assert.equal(c.items.length, 2);
  assert.equal(c.items[0].count, 2);
  assert.match(c.items[0].hint, /端口 7779/);
});

test('keeps different services separate and deduplicates long original text', () => {
  const c = createCollector();
  const text = "Cannot find module '" + 'a'.repeat(400) + "'";
  add(c, text); add(c, text); add(c, text, 'web');
  assert.equal(c.items.length, 2);
  assert.equal(c.items.find(i => i.serviceId === 'api').count, 2);
  assert.equal(c.items[0].text, text);
});

test('strips terminal escapes and ignores tool noise without losing unknown errors', () => {
  const c = createCollector();
  add(c, '\u001b[31m[ERROR] unknown custom failure\u001b[0m');
  add(c, '[ERROR] To see the full stack trace, re-run Maven');
  add(c, '[INFO] BUILD SUCCESS');
  assert.equal(c.items.length, 1);
  assert.equal(c.items[0].text, '[ERROR] unknown custom failure');
  assert.match(c.items[0].hint, /日志/);
});

test('handles malformed events and bounds retained categories', () => {
  const c = createCollector();
  for (const payload of [null, {}, { entries: {} }, { entries: [null, 4, {}] }]) assert.doesNotThrow(() => c.addBatch(payload));
  for (let i = 0; i < 80; i++) add(c, '[ERROR] failure ' + i);
  assert.equal(c.items.length, 60);
  assert.equal(c.items[0].text, '[ERROR] failure 79');
  c.clear();
  assert.equal(c.items.length, 0);
});

test('search distinguishes valid ports, Windows paths and process names', () => {
  assert.deepEqual(parseQuery('7779'), { kind: 'port', value: 7779 });
  assert.equal(parseQuery('0').kind, 'invalid');
  assert.equal(parseQuery('65536').kind, 'invalid');
  assert.equal(parseQuery('999999').kind, 'invalid');
  assert.deepEqual(parseQuery(' "D:\\my project" '), { kind: 'path', value: 'D:\\my project' });
  assert.equal(parseQuery('\\\\server\\share').kind, 'path');
  assert.deepEqual(parseQuery('node.exe'), { kind: 'process', value: 'node.exe' });
  assert.equal(parseQuery(' ').kind, 'empty');
});

test('routes global tools using the actual hash router', () => {
  assert.equal(routePath({ hash: '#/projects/one?tab=services', pathname: '/' }), '/projects/one');
  assert.equal(routePath({ hash: '', pathname: '/logs' }), '/logs');
});

test('preserves actual Maven errors with Help links and explains locked packaging artifacts', () => {
  const c = createCollector();
  add(c, "[ERROR] Failed to execute goal repackage: Unable to rename 'target/app.jar' to 'target/app.jar.original' -> [Help 1]");
  add(c, '[ERROR] [Help 1] http://cwiki.apache.org/help');
  assert.equal(c.items.length, 1);
  assert.match(c.items[0].hint, /无法重命名/);
});
