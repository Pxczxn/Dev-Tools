const { test } = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = path.join(__dirname, '..');

function runVersionCheck(tag) {
  const result = execFileSync(
    'powershell',
    ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/check-release-version.ps1', '-Tag', tag],
    { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] },
  );
  return result;
}

test('release tag must match every packaged version', () => {
  const version = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  assert.match(runVersionCheck(`v${version}`), new RegExp(`Version ${version.replaceAll('.', '\\.')} matches`));
  assert.throws(() => runVersionCheck('v9.9.9'), error => {
    const output = `${error.stdout || ''}${error.stderr || ''}${error.message}`;
    return output.includes('does not match');
  });
});

test('checksum sidecar keeps the portable hash format', () => {
  const sample = path.join(os.tmpdir(), `dev-tools-sha-${process.pid}.bin`);
  fs.writeFileSync(sample, Buffer.from([1, 2, 3, 4]));
  const hash = execFileSync(
    'powershell',
    ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/write-sha256.ps1', '-Path', sample],
    { cwd: root, encoding: 'utf8' },
  ).trim();
  const sidecar = fs.readFileSync(`${sample}.sha256`);
  assert.equal(sidecar.toString('utf8'), `${hash}  ${path.basename(sample)}\r\n`);
  fs.rmSync(sample, { force: true });
  fs.rmSync(`${sample}.sha256`, { force: true });
});

test('tag workflow builds both Windows packages and publishes the release', () => {
  const workflow = fs.readFileSync(path.join(root, '.github/workflows/release-windows.yml'), 'utf8');
  assert.match(workflow, /tags:[\s\S]*- "v\*"/);
  assert.match(workflow, /npm run build:portable/);
  assert.match(workflow, /npx tauri build --bundles nsis/);
  assert.match(workflow, /softprops\/action-gh-release@v2/);
  assert.match(workflow, /Dev-Tools-\*-windows-x64-portable\.zip/);
  assert.match(workflow, /-setup\.exe/);
});
