const { test } = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

test('installer embeds the shared WebView2 bootstrapper instead of a private runtime', () => {
  const conf = JSON.parse(fs.readFileSync(path.join(__dirname, '../src-tauri/tauri.conf.json'), 'utf8'));
  assert.equal(conf.bundle.windows.webviewInstallMode.type, 'embedBootstrapper');
  assert.equal(conf.bundle.windows.webviewInstallMode.silent, true);
});

test('portable archive carries the 1.8MB bootstrapper and launcher', () => {
  const version = JSON.parse(fs.readFileSync(path.join(__dirname, '../package.json'), 'utf8')).version;
  const zip = path.join(__dirname, `../dist/Dev-Tools-${version}-windows-x64-portable.zip`);
  if (!fs.existsSync(zip)) return;
  const listing = execFileSync('tar', ['-tvf', zip], { encoding: 'utf8' });
  const bootstrapper = listing.split(/\r?\n/).find(line => line.includes('MicrosoftEdgeWebview2Setup.exe'));
  assert.ok(bootstrapper, listing);
  const size = Number(bootstrapper.match(/\s(\d{6,})\s/)[1]);
  assert.ok(size > 500 * 1024 && size < 20 * 1024 * 1024, bootstrapper);
  assert.match(listing, /Dev-Tools\.cmd/);
  assert.doesNotMatch(listing, /FixedVersion|msedgewebview2\.exe/i);
});
