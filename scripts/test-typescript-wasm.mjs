// Exercise the actual browser ABI, including coordinator/reader dispatch.
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const bytes = await readFile(process.argv[2]);
let expected = 0, parsed = 0;
const { instance: { exports: api } } = await WebAssembly.instantiate(bytes, {
  cqx: { parsing_total(n) { expected = n; parsed = 0; }, parsed_one() { parsed++; } },
});
const encoder = new TextEncoder(), decoder = new TextDecoder();
function input(text) {
  const bytes = encoder.encode(text), ptr = api.cqx_alloc(bytes.length);
  new Uint8Array(api.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}
function call(name, ...strings) {
  const inputs = strings.map(input);
  const ptr = api[name](...inputs.flat());
  for (const [p, n] of inputs) api.cqx_free(p, n);
  if (!ptr) return;
  const n = new DataView(api.memory.buffer).getUint32(ptr, true);
  const text = decoder.decode(new Uint8Array(api.memory.buffer, ptr + 4, n));
  api.cqx_free(ptr, n + 4);
  return text;
}
function snapshot(files) {
  call('cqx_reset', 'mixed');
  for (const [path, content] of files) call('cqx_add_file', path, content);
  assert.equal(api.cqx_file_count(), files.length);
}
const config = JSON.stringify({ version: 1, rules: { 'typescript/dynamic-code': { weight: 17 } } });
const rust = [
  ['Cargo.toml', '[package]\nname="mixed"\nversion="0.1.0"\n'],
  ['src/lib.rs', 'pub fn shutdown() { std::process::exit(1); }'],
];
const ts = [
  ['package.json', '{"scripts":{"start":"node tools/start.js"}}'],
  ['src/library.tsx', 'export function render(s: string) { eval(s); return <p>{s}</p>; }'],
  ['tools/start.js', 'process.exit(1);'],
  ['module.jsx', 'export const view = <div/>;'],
  ['module.mjs', 'export const x = 1;'],
  ['module.cjs', 'module.exports = 1;'],
  ['module.ts', 'export type T = string;'],
  ['cqx.json', config],
];
snapshot([...rust, ...ts]);
const facts = call('cqx_facts');
assert.match(facts, /typescript:rule/);
assert.match(facts, /"extractor":"rust"/);
const scored = JSON.parse(call('cqx_score', ''));
assert.deepEqual(Object.keys(scored.scores).sort(), ['containment', 'legibility', 'modularity', 'quality', 'security']);
assert.equal(scored.scores.security, 83);
assert.equal(scored.scores.containment, 70);
const dataset = JSON.parse(call('cqx_dataset', 'cqxai/fixture', ''));
assert.equal(expected, 7); // One Rust file plus six TS/JS source files.
assert.equal(parsed, expected);
assert.deepEqual(dataset.score.scores, scored.scores);

// The shard's bin declaration is supplied through the full snapshot metadata,
// even if its package.json was allocated to another reader.
const metadata = call('cqx_manifests');
api.cqx_merge_reset();
const slices = [[...rust, ts[0]], ts.slice(1)];
for (const files of slices) {
  snapshot(files);
  const shared = call('cqx_gather', metadata);
  assert.equal(JSON.parse(call('cqx_merge_add', shared)).error, undefined);
}
const merged = call('cqx_merge_done');
api.cqx_fold_reset();
for (const files of slices) {
  snapshot(files);
  call('cqx_gather', metadata);
  const emitted = call('cqx_emit', merged);
  assert.equal(JSON.parse(call('cqx_fold_add', emitted)).error, undefined);
}
const folded = JSON.parse(call('cqx_fold_done', 'cqxai/fixture', config));
assert.deepEqual(folded.score.scores, scored.scores);

snapshot([...rust, ...ts, ['broken.ts', 'const = ;'], ['redeclaration.ts', 'let value; let value;']]);
const partial = JSON.parse(call('cqx_score', ''));
assert.deepEqual(partial.scores, scored.scores);
assert.deepEqual(partial.skipped_files.map(f => f.file), ['broken.ts', 'redeclaration.ts']);
const brokenMetadata = call('cqx_manifests');
api.cqx_merge_reset();
const brokenSlices = [[...rust, ts[0], ['broken.ts', 'const = ;']], [...ts.slice(1), ['redeclaration.ts', 'let value; let value;']]];
for (const files of brokenSlices) {
  snapshot(files);
  call('cqx_merge_add', call('cqx_gather', brokenMetadata));
}
const brokenMerged = call('cqx_merge_done');
api.cqx_fold_reset();
for (const files of brokenSlices) {
  snapshot(files);
  call('cqx_gather', brokenMetadata);
  call('cqx_fold_add', call('cqx_emit', brokenMerged));
}
const brokenFolded = JSON.parse(call('cqx_fold_done', 'cqxai/fixture', config));
assert.deepEqual(brokenFolded.score.scores, partial.scores);
assert.deepEqual(brokenFolded.score.skipped_files, partial.skipped_files);
console.log('WASM TS/JS, mixed scoring, root config, progress, shards, and reported parse skips: passed');

snapshot([
  ['npm/package.json', '{"scripts":{"build":"node ../npm/build.mjs","test":"node ./test.mjs"}}'],
  ['npm/build.mjs', 'if (!version) process.exit(2); process.exit(1);'],
  ['npm/test.mjs', 'if (!binary) process.exit(2); process.exit(failed ? 1 : 0);'],
  ['tools/check.mjs', 'if (!ready) process.exit(2);'],
]);
const scripts = JSON.parse(call('cqx_score', ''));
assert.equal(scripts.scores.containment, 100);
snapshot([
  ['src/lib.js', 'import "../npm/build.mjs";'],
  ['npm/build.mjs', 'process.exit(2);'],
]);
const imported = JSON.parse(call('cqx_score', ''));
assert.equal(imported.scores.containment, 70);
console.log('Nested package scripts and unimported top-level .mjs entries: passed');

for (const ext of ['js', 'cjs', 'mjs', 'ts', 'mts', 'cts']) {
  const path = `tools/check.${ext}`;
  snapshot([[path, 'if (!ready) process.exit(2);']]);
  assert.equal(JSON.parse(call('cqx_score', '')).scores.containment, 100, ext);
  snapshot([['src/lib.js', `import '../${path}';`], [path, 'process.exit(2);']]);
  assert.equal(JSON.parse(call('cqx_score', '')).scores.containment, 70, ext);
  // Import metadata must reach the reader containing the candidate script.
  const meta = call('cqx_manifests');
  api.cqx_merge_reset();
  const shards = [[['src/lib.js', `import '../${path}';`]], [[path, 'process.exit(2);']]];
  for (const files of shards) {
    snapshot(files);
    call('cqx_merge_add', call('cqx_gather', meta));
  }
  const merged = call('cqx_merge_done');
  api.cqx_fold_reset();
  for (const files of shards) {
    snapshot(files);
    call('cqx_gather', meta);
    call('cqx_fold_add', call('cqx_emit', merged));
  }
  assert.equal(JSON.parse(call('cqx_fold_done', 'fixture', '')).score.scores.containment, 70, ext);
}
for (const runner of ['tsx', 'bun']) {
  snapshot([
    ['packages/task/package.json', JSON.stringify({ scripts: { start: `${runner} ../shared/x.ts` } })],
    ['packages/shared/x.ts', 'function stop() { process.exit(2); } stop();'],
  ]);
  assert.equal(JSON.parse(call('cqx_score', '')).scores.containment, 100, runner);
}
snapshot([
  ['packages/task/package.json', '{"bin":{"app":"../../../outside.ts"}}'],
  ['outside.ts', 'export function stop() { process.exit(2); }'],
]);
assert.equal(JSON.parse(call('cqx_score', '')).scores.containment, 70);
console.log('Standalone JS/TS extensions, imported shards, nested tsx/bun, bounded bin paths: passed');
