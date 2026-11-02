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

test('database path buttons open the matching system picker', async () => {
  const { pickerOptions, selectedPath } = await import('../frontend/tools/infra-picker.js');
  const program = pickerOptions('mysql', 'executable', '  ');
  assert.equal(program.directory, undefined);
  assert.equal(program.multiple, false);
  assert.equal(program.defaultPath, undefined);
  assert.equal(program.filters[0].name, 'mysqld.exe');
  assert.deepEqual(program.filters[0].extensions, ['exe']);
  const directory = pickerOptions('redis', 'dataDir', 'D:\\redis\\data');
  assert.equal(directory.directory, true);
  assert.equal(directory.title, '选择数据目录');
  assert.equal(directory.defaultPath, 'D:\\redis\\data');
  const config = pickerOptions('redis', 'configFile', '');
  assert.equal(config.directory, undefined);
  assert.deepEqual(config.filters[0].extensions, ['ini', 'cnf', 'conf']);
  assert.equal(selectedPath(null), '');
  assert.equal(selectedPath(['D:\\redis\\redis-server.exe']), '');
  assert.equal(selectedPath('D:\\mysql\\bin\\mysqld.exe'), 'D:\\mysql\\bin\\mysqld.exe');
});

test('homepage and software table use the Xiaopi status layout', async () => {
  const { homeBlock, settingsTabs, softwareTable } = await import('../frontend/tools/infra-board.js');
  const table = softwareTable([
    { kind: 'mysql', state: 'running', port: 3306, executable: 'C:\\mysql\\bin\\mysqld.exe' },
    { kind: 'redis', state: 'stopped', port: 6379, executable: '' },
  ]);
  assert.match(table, /软件名称/);
  assert.match(table, /状态/);
  assert.match(table, /端口/);
  assert.match(table, /操作/);
  assert.match(table, /xp-state is-on/);
  assert.match(table, />启动</);
  assert.match(table, /xp-state is-off/);
  assert.match(table, />停止</);
  assert.match(homeBlock([]), /数据库/);
  assert.match(softwareTable([{
    kind: 'mysql', state: 'stopped', port: 3306, executable: 'D:\\custom\\mysqld.exe',
    resolvedExecutable: 'C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysqld.exe',
    executableSource: 'default',
  }]), /默认安装/);
  const { serviceListState } = await import('../frontend/tools/infra-board.js');
  const programs = [{
    kind: 'mysql',
    resolvedExecutable: 'C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysqld.exe',
    executable: '',
  }];
  const same = serviceListState(programs, [{
    name: 'pxczxn',
    binaryPath: '"C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysqld.exe" --defaults-file="C:\\ProgramData\\MySQL\\MySQL Server 8.0\\my.ini"',
  }]);
  assert.equal(same.visible, false);
  assert.deepEqual(same.services, []);
  const other = serviceListState(programs, [{
    name: 'other-mysql',
    binaryPath: 'D:\\db\\bin\\mysqld.exe --console',
  }]);
  assert.equal(other.visible, true);
  assert.equal(other.services[0].name, 'other-mysql');
  assert.deepEqual(settingsTabs('mysql'), ['端口', '存储位置', '配置文件', '当前状态']);
  assert.deepEqual(settingsTabs('redis'), ['性能调整', '配置文件', '当前状态']);
});

test('restart current terminal uses only the selected service', async () => {
  const { readFileSync } = require('node:fs');
  const { join } = require('node:path');
  const { restartCurrentTerminal } = await import('../frontend/tools/log-restart.js');
  const bundle = readFileSync(join(__dirname, '../frontend/assets/index-BuLY3L2r.js'), 'utf8');
  const at = bundle.indexOf('children:"重启当前终端"');
  assert.ok(at > 0);
  assert.equal(bundle.indexOf('children:"重启当前终端"', at + 1), -1);
  const around = bundle.slice(Math.max(0, at - 260), at + 40);
  assert.match(around, /xdRestartCurrentTerminal\?\.\(N,\{unsubscribe:u,clear:v,subscribe:o\}\)/);
  assert.doesNotMatch(around, /restart_process|start_process/);
  const steps = [];
  assert.equal(await restartCurrentTerminal('', { clear: async id => steps.push(id) }), 'empty');
  assert.equal(steps.length, 0);
  assert.equal(await restartCurrentTerminal('svc-a', {
    unsubscribe: async id => steps.push(['off', id]),
    clear: async id => steps.push(['clear', id]),
    subscribe: async id => steps.push(['on', id]),
  }), 'restarted');
  assert.deepEqual(steps, [['off', 'svc-a'], ['clear', 'svc-a'], ['on', 'svc-a']]);
  assert.equal(await restartCurrentTerminal('svc-b', {
    clear: async () => { throw new Error('缓冲还在'); },
    report: message => steps.push(message),
  }), 'failed');
  assert.match(steps.at(-1), /缓冲还在/);
});

test('preserves actual Maven errors with Help links and explains locked packaging artifacts', () => {
  const c = createCollector();
  add(c, "[ERROR] Failed to execute goal repackage: Unable to rename 'target/app.jar' to 'target/app.jar.original' -> [Help 1]");
  add(c, '[ERROR] [Help 1] http://cwiki.apache.org/help');
  assert.equal(c.items.length, 1);
  assert.match(c.items[0].hint, /无法重命名/);
});

test('settings about can check, explain, and refuse an unsigned update', async () => {
  const { readFileSync, rmSync } = require('node:fs');
  const { join } = require('node:path');
  const { spawnSync } = require('node:child_process');
  const html = readFileSync(join(__dirname, '../frontend/index.html'), 'utf8');
  const source = readFileSync(join(__dirname, '../frontend/tools/update.js'), 'utf8');
  const workflow = readFileSync(join(__dirname, '../.github/workflows/release-windows.yml'), 'utf8');
  assert.match(html, /tools\/update\.js/);
  assert.match(source, /get_app_version/);
  assert.match(source, /update_check/);
  assert.match(source, /暂时无法连接更新服务器，当前版本仍可离线使用/);
  assert.match(source, /下载并安装/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY/);
  assert.match(workflow, /dist\/latest\.json/);
  assert.match(workflow, /windows-x64-portable\.zip\.sig/);
  assert.match(workflow, /-setup\.exe\.sig/);
  const output = join(require('node:os').tmpdir(), 'dev-tools-latest-test.json');
  const run = spawnSync('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(__dirname, '../scripts/write-latest-json.ps1'), '-Version', '1.2.3', '-PortableFileName', 'Dev-Tools-1.2.3-windows-x64-portable.zip', '-SetupFileName', 'Dev-Tools_1.2.3_x64-setup.exe', '-PortableSignature', 'portable-sig', '-SetupSignature', 'setup-sig', '-Notes', '修复启动', '-Output', output], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  const manifest = JSON.parse(readFileSync(output, 'utf8'));
  assert.equal(manifest.version, '1.2.3');
  assert.equal(manifest.notes, '修复启动');
  assert.equal(manifest.platforms['windows-x86_64-portable'].signature, 'portable-sig');
  assert.equal(manifest.platforms['windows-x86_64'].signature, 'setup-sig');
  assert.match(manifest.platforms['windows-x86_64-portable'].url, /\/v1\.2\.3\/Dev-Tools-1\.2\.3-windows-x64-portable\.zip$/);
  assert.match(manifest.platforms['windows-x86_64'].url, /\/v1\.2\.3\/Dev-Tools_1\.2\.3_x64-setup\.exe$/);
  rmSync(output, { force: true });
});
