// Execute the real CLI with gh on PATH replaced by a recording release server.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { shipLegacy } from './wasm-compat.mjs';

await mkdir(resolve('.tmp'), { recursive: true, mode: 0o700 });
const temporary = await mkdtemp(resolve('.tmp/compat-'));
const fixture = join(temporary, 'fixture.json'), calls = join(temporary, 'calls.ndjson');
const release = (tag, modular = false, extra = {}) => ({ tag_name: tag, assets: modular ? [{ name: 'manifest.json' }] : [{ name: 'cqx_wasm.wasm' }], ...extra });
const old = release('v0.1.24'), first = release('v0.1.25', true), later = release('v0.1.26', true);
const manifest = legacy => ({ abi_version: 1, version: '0.1.25', modules: { core: {}, c: {}, csharp: {} }, ...(legacy ? { deprecated_artifacts: { 'cqx_wasm.wasm': {} } } : {}) });
await writeFile(join(temporary, 'gh'), `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2), fixture = JSON.parse(fs.readFileSync(process.env.CQX_COMPAT_FIXTURE));
fs.appendFileSync(process.env.CQX_COMPAT_CALLS, JSON.stringify(args)+'\\n');
if (fixture.fail) { process.stderr.write('inventory unavailable'); process.exit(1); }
if (args[0] === 'api' && args[1].endsWith('/releases/latest')) console.log(JSON.stringify(fixture.latest));
else if (args[0] === 'api' && args[1].includes('/releases/tags/')) {
  const tag = decodeURIComponent(args[1].split('/tags/')[1]);
  const current = fixture.pages.flat().find(r => r.tag_name === tag);
  if (current) console.log(JSON.stringify(current));
  else { process.stderr.write('gh: Not Found (HTTP 404)'); process.exit(1); }
} else if (args.join(' ') === 'api repos/cqxai/cqx/releases --paginate --slurp') console.log(JSON.stringify(fixture.pages));
else if (args[0] === 'release' && args[1] === 'download') {
  if (fixture.downloadFail) { process.stderr.write('manifest unavailable'); process.exit(1); }
  console.log(JSON.stringify(fixture.manifest));
} else { process.stderr.write('unexpected gh arguments: '+JSON.stringify(args)); process.exit(1); }
`, { mode: 0o755 });
const env = { ...process.env, PATH: `${temporary}:${process.env.PATH}`, CQX_COMPAT_FIXTURE: fixture, CQX_COMPAT_CALLS: calls };
async function run(name, tag, data, expected) {
  await writeFile(fixture, JSON.stringify(data));
  await writeFile(calls, '');
  assert.equal(execFileSync('node', ['scripts/wasm-compat.mjs', tag, 'cqxai/cqx'], { env, encoding: 'utf8' }).trim(), String(expected), name);
  const requests = (await readFile(calls, 'utf8')).trim().split('\n').map(JSON.parse);
  assert.deepEqual(requests[0], ['api', 'repos/cqxai/cqx/releases', '--paginate', '--slurp'], `${name}: full inventory, not latest`);
  assert.ok(!requests.some(args => args[1]?.endsWith('/releases/latest')), name);
  return requests;
}
try {
  await run('out-of-order older hotfix retains monolith', 'v0.1.25', { pages: [[old], [release('v0.2.0', true)]], latest: release('v0.2.0', true) }, true);
  await run('next modular tag across pages', 'v0.1.26', { pages: [[old], [first]], latest: old }, false);
  await run('first modular tag', 'v0.1.25', { pages: [[old]], latest: old }, true);
  await run('prerelease predecessor is visible', 'v0.1.25-rc.2', { pages: [[old], [release('v0.1.25-rc.1', true, { prerelease: true })]], latest: old }, false);
  await run('draft predecessor is visible', 'v0.1.26', { pages: [[old], [release('v0.1.25', true, { draft: true })]], latest: old }, false);
  await run('older minor-line hotfix', 'v0.1.23', { pages: [[first], [release('v0.1.22')]], latest: first }, true);
  await run('older hotfix after its own modular predecessor', 'v0.1.24', { pages: [[later], [release('v0.1.23', true)]], latest: later }, false);
  const rerun = await run('historical transition rerun', 'v0.1.25', { pages: [[later], [first]], latest: later, manifest: manifest(true) }, true);
  assert.deepEqual(rerun[1], ['release', 'download', 'v0.1.25', '--repo', 'cqxai/cqx', '--pattern', 'manifest.json', '--output', '-']);
  await run('later rerun omits compatibility', 'v0.1.26', { pages: [[later], [first]], latest: later, manifest: manifest(false) }, false);
  for (const data of [{ pages: [], fail: true }, { pages: [[{ tag_name: 'v0.1.24' }]], latest: old }, { pages: [[first]], latest: first, downloadFail: true }]) {
    await writeFile(fixture, JSON.stringify(data));
    assert.throws(() => execFileSync('node', ['scripts/wasm-compat.mjs', 'v0.1.25', 'cqxai/cqx'], { env, stdio: 'pipe' }));
  }
  // SemVer ordering is numeric, and stable follows its prereleases.
  assert.equal(shipLegacy('v0.1.25-rc.10', [release('v0.1.25-rc.2', true)]), false);
  assert.equal(shipLegacy('v0.1.25-rc.2', [release('v0.1.25-rc.10', true)]), true);
  assert.equal(shipLegacy('v0.1.25', [release('v0.1.25-rc.1', true)]), false);
  assert.equal(shipLegacy('v0.1.25-rc.1', [first]), true);
  assert.equal(shipLegacy('v0.1.26+build.2', [first]), false);
  assert.equal(shipLegacy('v0.1.25', [release('nightly', true)]), true);
  assert.throws(() => shipLegacy('v0.1.25-rc.01', []), /invalid release version tag/);
  console.log('Actual gh CLI: complete paginated tags, prereleases/drafts, older hotfixes, historical reruns and fail-closed inventory passed');
} finally { await rm(temporary, { recursive: true, force: true }); }
