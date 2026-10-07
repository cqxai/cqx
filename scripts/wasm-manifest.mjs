// Generate the manifest from the actual binaries, refusing stale/mislabeled builds.
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { ABI_VERSION, MODULES } from './wasm-catalog.mjs';
import { connect } from './cqx-loader.mjs';

const directory = process.argv[2];
const ids = process.argv.slice(3);
if (!directory || !ids.includes('core')) throw new Error('usage: node scripts/wasm-manifest.mjs DIRECTORY core [c csharp]');
const manifest = { version: null, abi_version: ABI_VERSION, modules: {} };
for (const id of ids) {
  if (!MODULES[id]) throw new Error(`unknown module ${id}`);
  const bytes = await readFile(join(directory, `${id}.wasm`));
  const { instance } = await WebAssembly.instantiate(bytes, { cqx: { parsing_total() {}, parsed_one() {} } });
  const info = JSON.parse(connect(instance).call('cqx_module_info'));
  manifest.version ??= info.version;
  if (info.module !== id || info.version !== manifest.version || info.abi_version !== ABI_VERSION) throw new Error(`module ${id}: binary version/ABI mismatch`);
  manifest.modules[id] = { file: `${id}.wasm`, ...MODULES[id], version: info.version, abi_version: info.abi_version, sha256: createHash('sha256').update(bytes).digest('hex') };
}
await writeFile(join(directory, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
