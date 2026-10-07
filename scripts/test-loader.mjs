import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { createLoader, connect, verifyManifest } from './cqx-loader.mjs';

const directory = process.argv[2];
const manifestBytes = await readFile(join(directory, 'manifest.json'));
const pin = value => createHash('sha256').update(JSON.stringify(value, null, 2) + '\n').digest('hex');
const manifestSha256 = createHash('sha256').update(manifestBytes).digest('hex');
const { manifest } = await verifyManifest(manifestBytes, `${manifestSha256}  manifest.json\n`);
assert.throws(() => createLoader({ manifest }), /trusted release manifest SHA-256 is required/);
await assert.rejects(verifyManifest(manifestBytes, `${'0'.repeat(64)}  manifest.json\n`), /manifest SHA-256 mismatch/);
await assert.rejects(verifyManifest(manifestBytes, ''), /exactly one manifest/);
await assert.rejects(verifyManifest(manifestBytes, `${manifestSha256}  manifest.json\n`.repeat(2)), /exactly one manifest/);
const loads = [];
const fetchBytes = async (file, id) => { loads.push(id); return readFile(join(directory, file)); };
const loader = createLoader({ manifest, manifestSha256, fetchBytes });
const files = [
  ['package.json', '{"bin":"src/main.ts"}'],
  ['cqx.json', '{"rules":{"typescript/exit-in-library":{"free":0,"full":1}}}'],
  ['src/lib.ts', 'export function stop() { process.exit(1); }'],
  ['src/main.ts', 'process.exit(0);'],
];
const {dataset: first, reportJson} = await loader.scan(files, { repo: 'fixture' });
assert.equal(first.score.rules.find(r => r.language === 'typescript' && r.rule === 'exit-in-library')?.total_findings, 1);
assert.equal(first.score.scores.containment, 70);
const { instance } = await WebAssembly.instantiate(await readFile(join(directory, 'core.wasm')), { cqx: { parsing_total() {}, parsed_one() {} } });
const core = connect(instance);
core.call('cqx_reset', 'fixture');
for (const file of files) core.call('cqx_add_file', ...file);
assert.deepEqual(first, JSON.parse(core.call('cqx_dataset', 'fixture', '')), 'complete routed dataset equals monolithic core');
assert.equal(reportJson, core.call('cqx_quote', core.call('cqx_score', '')), 'complete report bytes equal monolithic core');
assert.deepEqual((await loader.scan(files, { repo: 'fixture' })).dataset.score, first.score);
assert.deepEqual(loads, ['core'], 'compiled-module cache reused and unrelated modules never loaded');
await Promise.all([loader.scan(files), loader.scan([])]); // Fresh instances isolate simultaneous snapshots.

const changed = structuredClone(manifest);
changed.modules.core.abi_version++;
assert.throws(() => createLoader({ manifest: changed, manifestSha256: pin(changed), fetchBytes }), /version\/ABI mismatch/);
const stale = structuredClone(manifest);
stale.version = '0.0.0';
for (const entry of Object.values(stale.modules)) entry.version = stale.version;
await assert.rejects(createLoader({ manifest: stale, manifestSha256: pin(stale), fetchBytes }).scan(files), /binary version\/ABI\/identity mismatch/);
await assert.rejects(createLoader({ manifest, manifestSha256, fetchBytes: async () => new Uint8Array([0]) }).scan(files), /SHA-256 mismatch/);
await assert.rejects(createLoader({ manifest, manifestSha256, fetchBytes: async () => { throw new Error('offline'); } }).scan(files), /load core: offline/);
if (!manifest.modules.c) await assert.rejects(loader.scan([['lib.c', 'int f(void){return 1;}']]), /required module c is missing/);
// A compromised CDN can replace both module and advertised hash; the independent
// release pin must reject that manifest before fetching any module.
const replaced = structuredClone(manifest);
replaced.modules.core.sha256 = '0'.repeat(64);
let hostileFetches = 0;
await assert.rejects(createLoader({ manifest: replaced, manifestSha256, fetchBytes: async () => { hostileFetches++; return new Uint8Array([0]); } }).scan(files), /manifest SHA-256 mismatch/);
assert.equal(hostileFetches, 0);
const precompiled = new WebAssembly.Module(await readFile(join(directory, 'core.wasm')));
assert.deepEqual((await createLoader({ manifest, manifestSha256, compiledModules: { core: { module: precompiled, sha256: manifest.modules.core.sha256 } } }).scan(files, { repo: 'fixture' })).dataset.score, first.score);
console.log('Loader actual WASM routing, dataset bytes, config, caching, concurrency, Worker binding and failure controls passed');
