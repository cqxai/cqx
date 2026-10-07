import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, mkdtemp, copyFile, readdir, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { packageWasm } from './package-wasm.mjs';
import { shipLegacy } from './wasm-compat.mjs';

const source = resolve(process.argv[2]);
await mkdir('.tmp', { recursive: true });
const temporary = await mkdtemp(resolve('.tmp/package-'));
try {
  const first = join(temporary, 'first'), later = join(temporary, 'later');
  const oldRelease = { tag_name: 'v0.1.24', assets: [{ name: 'cqx_wasm.wasm' }] };
  const firstLegacy = shipLegacy('v0.1.25', [oldRelease]);
  await packageWasm(source, first, { legacy: firstLegacy });
  const firstManifest = JSON.parse(await readFile(join(first, 'manifest.json')));
  assert.ok(firstManifest.deprecated_artifacts['cqx_wasm.wasm']);
  assert.deepEqual(await readFile(join(first, 'cqx_wasm.wasm')), await readFile(join(source, 'cqx_wasm.wasm')), 'compatibility is the actual full monolith');
  const firstRelease = { tag_name: 'v0.1.25', assets: [{ name: 'manifest.json' }, { name: 'cqx_wasm.wasm' }] };
  assert.equal(shipLegacy('v0.1.25', [firstRelease], firstManifest), true, 'first modular rerun retains compatibility');
  const laterLegacy = shipLegacy('v0.1.26', [firstRelease]);
  await packageWasm(source, later, { legacy: laterLegacy });
  assert.deepEqual((await readdir(later)).sort(), ['c.wasm', 'core.wasm', 'cqx-loader.mjs', 'csharp.wasm', 'manifest.json', 'wasm-catalog.mjs'].sort(), 'next release publishes only selective modules and loader');
  const laterManifest = JSON.parse(await readFile(join(later, 'manifest.json')));
  assert.equal(laterManifest.deprecated_artifacts, undefined);
  assert.equal(shipLegacy('v0.1.25', [{ tag_name: 'v0.1.26', assets: [{ name: 'manifest.json' }] }], firstManifest), true, 'historical transition rerun retains its own published policy');
  assert.equal(shipLegacy('v0.1.26', [{ tag_name: 'v0.1.26', assets: [{ name: 'manifest.json' }] }], laterManifest), false);
  assert.throws(() => shipLegacy('v0.1.25', [firstRelease]), /rerun requires/);
  const { createLoader } = await import(pathToFileURL(join(later, 'cqx-loader.mjs')));
  const manifestSha256 = createHash('sha256').update(await readFile(join(later, 'manifest.json'))).digest('hex');
  const loader = createLoader({ manifest: laterManifest, manifestSha256, fetchBytes: file => readFile(join(later, file)) });
  const actual = await loader.scan([['Lib.cs', 'class L { void F(){System.Environment.Exit(1);} }']]);
  assert.equal(actual.dataset.score.scores.containment, 70, 'published loader and C# module execute one core score');
  await assert.rejects(packageWasm(source, first), /nonempty upload directory/);

  const corrupt = join(temporary, 'corrupt'); await mkdir(corrupt);
  for (const file of ['manifest.json', 'core.wasm', 'c.wasm', 'csharp.wasm']) await copyFile(join(source, file), join(corrupt, file));
  // Valid harmless custom section: only integrity validation can reject it.
  const original = await readFile(join(corrupt, 'core.wasm'));
  await writeFile(join(corrupt, 'core.wasm'), Buffer.concat([original, Buffer.from([0, 4, 1, 120, 1, 2])]));
  await assert.rejects(packageWasm(corrupt, join(temporary, 'bad-hash')), /SHA-256 mismatch/);
  await writeFile(join(corrupt, 'core.wasm'), original);
  await rm(join(corrupt, 'csharp.wasm'));
  await assert.rejects(packageWasm(corrupt, join(temporary, 'missing-module')), /csharp.wasm/);
  console.log('Actual release files, first-release/rerun/next-release cutoff, copied loader execution, integrity and missing-artifact gates passed');
} finally { await rm(temporary, { recursive: true, force: true }); }
