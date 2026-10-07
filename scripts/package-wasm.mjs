// Validate every artifact before producing the upload directory.
import { readFile, writeFile, mkdir, readdir, copyFile, constants } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createLoader, connect } from './cqx-loader.mjs';
import { ABI_VERSION } from './wasm-catalog.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const version = (await readFile(join(root, 'Cargo.toml'), 'utf8')).match(/^version = "([^"]+)"/m)[1];
export async function packageWasm(source, output, { legacy = false } = {}) {
  const manifest = JSON.parse(await readFile(join(source, 'manifest.json'), 'utf8'));
  createLoader({ manifest }); // Shared manifest/routing/version/ABI validation.
  if (manifest.version !== version) throw Error(`WASM version ${manifest.version} disagrees with source ${version}`);
  if (Object.keys(manifest.modules).sort().join(',') !== 'c,core,csharp') throw Error('release requires core, c and csharp modules');
  const assets = [];
  for (const [id, entry] of Object.entries(manifest.modules)) {
    const bytes = await readFile(join(source, entry.file));
    const actual = createHash('sha256').update(bytes).digest('hex');
    if (actual !== entry.sha256) throw Error(`module ${id}: SHA-256 mismatch`);
    const { instance } = await WebAssembly.instantiate(bytes, { cqx: { parsing_total() {}, parsed_one() {} } });
    const info = JSON.parse(connect(instance).call('cqx_module_info'));
    if (info.module !== id || info.version !== version || info.abi_version !== ABI_VERSION) throw Error(`module ${id}: binary version/ABI mismatch`);
    assets.push(entry.file);
  }
  delete manifest.deprecated_artifacts;
  if (legacy) {
    const bytes = await readFile(join(source, 'cqx_wasm.wasm'));
    const { instance } = await WebAssembly.instantiate(bytes, { cqx: { parsing_total() {}, parsed_one() {} } });
    const info = JSON.parse(connect(instance).call('cqx_module_info'));
    if (info.module !== 'monolith' || info.version !== version || info.abi_version !== ABI_VERSION) throw Error('legacy binary version/ABI mismatch');
    manifest.deprecated_artifacts = { 'cqx_wasm.wasm': {
      version, abi_version: ABI_VERSION, sha256: createHash('sha256').update(bytes).digest('hex'),
      note: 'Single-file compatibility for the first modular release only. Use cqx-loader.mjs and manifest.json.',
      replacement: ['core.wasm', 'c.wasm', 'csharp.wasm', 'manifest.json'],
    } };
    assets.push('cqx_wasm.wasm');
  }
  await mkdir(output, { recursive: true });
  if ((await readdir(output)).length) throw Error(`refusing nonempty upload directory ${output}`);
  for (const file of assets) await copyFile(join(source, file), join(output, file), constants.COPYFILE_EXCL);
  await writeFile(join(output, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx' });
  for (const file of ['cqx-loader.mjs', 'wasm-catalog.mjs']) await copyFile(join(root, 'scripts', file), join(output, file), constants.COPYFILE_EXCL);
  if (legacy) await writeFile(join(output, 'WASM-DEPRECATION.md'), '# Single-file WASM compatibility\n\n`cqx_wasm.wasm` remains the complete monolith for this first modular release.\nThe next release removes it. Use `cqx-loader.mjs` with `manifest.json` to load\n`core.wasm` and the required C/C++ or C# modules and produce one report.\n', { flag: 'wx' });
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const [source, output, flag] = process.argv.slice(2);
  if (!source || !output || (flag && flag !== '--legacy')) throw Error('usage: node scripts/package-wasm.mjs SOURCE OUTPUT [--legacy]');
  await packageWasm(source, output, { legacy: flag === '--legacy' });
}
