// Browser, Node and Worker host: immutable compiled modules, fresh scan instances.
import { ABI_VERSION, MODULES, moduleFor } from './wasm-catalog.mjs';

const encoder = new TextEncoder(), decoder = new TextDecoder();
function fail(message) { throw new Error(`cqx WASM: ${message}`); }
export function connect(instance) {
  const api = instance.exports;
  for (const name of ['memory', 'cqx_alloc', 'cqx_free', 'cqx_reset', 'cqx_add_file', 'cqx_module_info', 'cqx_gather', 'cqx_emit']) {
    if (!api[name]) fail(`missing ABI export ${name}`);
  }
  return {
    api,
    call(name, ...strings) {
      const inputs = [];
      try {
        for (const text of strings) {
          const bytes = encoder.encode(text), ptr = api.cqx_alloc(bytes.length);
          inputs.push([ptr, bytes.length]);
          new Uint8Array(api.memory.buffer, ptr, bytes.length).set(bytes);
        }
        if (typeof api[name] !== 'function') fail(`missing ABI export ${name}`);
        const ptr = api[name](...inputs.flat());
        if (!ptr) return;
        const size = new DataView(api.memory.buffer).getUint32(ptr, true);
        try { return decoder.decode(new Uint8Array(api.memory.buffer, ptr + 4, size)); }
        finally { api.cqx_free(ptr, size + 4); }
      } finally { for (const [ptr, size] of inputs) api.cqx_free(ptr, size); }
    },
  };
}
function checked(text, operation) {
  // NDJSON fact streams can contain an extraction error in place of facts.
  for (const line of text.split('\n').filter(Boolean)) {
    let item;
    try { item = JSON.parse(line); } catch { fail(`${operation}: invalid JSON`); }
    if (item.error || item.t === 'error') fail(`${operation}: ${item.error ?? item.message}`);
  }
  return text;
}
function snapshot(host, label, files) {
  host.call('cqx_reset', label);
  for (const [path, source] of files) host.call('cqx_add_file', path, source);
}

/** createLoader({manifest, baseURL, fetchBytes?, compiledModules?, onProgress?})
 * compiledModules is a trusted deployment binding: {id: {module, sha256}}.
 * Byte downloads are SHA-256 verified before compilation. All module identities
 * are checked even for precompiled Worker bindings. No partial result on error.
 */
export function createLoader({ manifest, baseURL, fetchBytes, compiledModules = {}, onProgress = () => {} }) {
  if (manifest?.abi_version !== ABI_VERSION || typeof manifest.version !== 'string' || !manifest.modules?.core) fail('unsupported manifest version/ABI or missing core');
  for (const [id, entry] of Object.entries(manifest.modules)) {
    const descriptor = MODULES[id];
    if (!descriptor || !entry || entry.version !== manifest.version || entry.abi_version !== ABI_VERSION) fail(`module ${id}: version/ABI mismatch`);
    if (!/^[a-f0-9]{64}$/.test(entry.sha256) || entry.file !== `${id}.wasm`) fail(`module ${id}: invalid file/hash`);
    for (const key of ['extensions', 'languages']) {
      if (JSON.stringify(entry[key]) !== JSON.stringify(descriptor[key])) fail(`module ${id}: invalid ${key} routing`);
    }
  }
  const cache = new Map();
  async function compiled(id) {
    const entry = manifest.modules[id];
    if (!entry) fail(`required module ${id} is missing from manifest`);
    const key = `${id}:${entry.version}:${entry.abi_version}:${entry.sha256}`;
    if (!cache.has(key)) {
      const promise = (async () => {
        const binding = compiledModules[id];
        if (binding) {
          if (!(binding.module instanceof WebAssembly.Module) || binding.sha256 !== entry.sha256) fail(`module ${id}: precompiled binding hash mismatch`);
          return binding.module;
        }
        let bytes;
        if (fetchBytes) bytes = await fetchBytes(entry.file, id);
        else {
          const response = await fetch(new URL(entry.file, baseURL));
          if (!response.ok) fail(`module ${id}: HTTP ${response.status}`);
          bytes = await response.arrayBuffer();
        }
        const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes);
        const hash = Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, '0')).join('');
        if (hash !== entry.sha256) fail(`module ${id}: SHA-256 mismatch`);
        return WebAssembly.compile(bytes);
      })().catch(error => { cache.delete(key); fail(`load ${id}: ${error.message}`); });
      cache.set(key, promise);
    }
    return cache.get(key);
  }
  async function load(id) {
    try {
      const instance = await WebAssembly.instantiate(await compiled(id), { cqx: {
        parsing_total: total => onProgress({ module: id, total }),
        parsed_one: () => onProgress({ module: id, parsed: 1 }),
      } });
      const host = connect(instance);
      const info = JSON.parse(host.call('cqx_module_info'));
      if (info.module !== id || info.version !== manifest.version || info.abi_version !== ABI_VERSION) fail(`module ${id}: binary version/ABI/identity mismatch (got ${JSON.stringify(info)})`);
      return host;
    } catch (error) { fail(`load ${id}: ${error.message}`); }
  }
  return {
    /** files: iterable [snapshot-relative path, UTF-8 source]. Returns {dataset, reportJson} from one core score. */
    async scan(files, { repo = '', label = repo, config = '' } = {}) {
      // Match Vfs normalization, deduplication and ordering before partitioning.
      const normalized = new Map();
      for (const [path, source] of files) normalized.set(path.replaceAll('\\', '/').replace(/^(?:\.\/)+/, '').replace(/^\/+/, ''), source);
      const all = [...normalized].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
      const selected = ['core', ...['c', 'csharp'].filter(id => all.some(([path]) => moduleFor(path) === id))];
      for (const id of selected) if (!manifest.modules[id]) fail(`required module ${id} is missing from manifest`);
      const hosts = new Map();
      for (const id of selected) hosts.set(id, await load(id));
      const core = hosts.get('core');
      // Coordinator sees all paths, but only core sources/config/manifests.
      // Empty foreign sources preserve header dialect and implicit target discovery.
      snapshot(core, label, all.map(([path, text]) => [path, moduleFor(path) === 'core' ? text : '']));
      const metadata = checked(core.call('cqx_manifests'), 'manifests');
      core.api.cqx_merge_reset();
      for (const [id, host] of hosts) {
        snapshot(host, label, all.filter(([path]) => moduleFor(path) === id));
        const gathered = checked(host.call('cqx_gather', metadata), `gather ${id}`);
        checked(core.call('cqx_merge_add', gathered), `merge ${id}`);
      }
      const shared = checked(core.call('cqx_merge_done'), 'resolve');
      core.api.cqx_fold_reset();
      for (const [id, host] of hosts) checked(core.call('cqx_fold_add', checked(host.call('cqx_emit', shared), `emit ${id}`)), `fold ${id}`);
      const finalized = JSON.parse(checked(core.call('cqx_finalize', repo, config), 'score'));
      const dataset = finalized.dataset;
      let report = finalized.report_json;
      for (const host of hosts.values()) report = checked(host.call('cqx_quote', report), 'quote');
      dataset.score = JSON.parse(report);
      return { dataset, reportJson: report };
    },
  };
}
