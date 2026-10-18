const { test, before } = require('node:test');
const assert = require('node:assert/strict');
let createCollector, parseQuery, routePath;
before(async () => { ({ createCollector, parseQuery, routePath } = await import('../frontend/tools/model.js')); });

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
