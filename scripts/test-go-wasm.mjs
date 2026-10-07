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
const config = JSON.stringify({ version: 1, rules: { 'go/shell-invocation': { weight: 7 }, 'go/shell-argument-unchecked': { enabled: false } } });
const rust = [
  ['Cargo.toml', '[package]\nname="mixed"\nversion="0.1.0"\n'],
  ['src/lib.rs', 'pub fn library() {}'],
];
const go = [
  ['go.mod', 'module example.com/mixed\ngo 1.23\n'],
  ['internal/lib.go', 'package lib\nimport "os/exec"\nfunc Start(command string) { exec.Command("sh", "-c", command) }'],
  ['cmd/app/main.go', 'package main\nimport "os"\nfunc main() { os.Exit(1) }'],
  ['lib_test.go', 'package lib\nimport "os"\nfunc testHelper() { os.Exit(1) }'],
  ['generic.go', 'package lib\nfunc Identity[T any](x T) T { return x }'],
  ['cqx.json', config],
];
const ts = [['src/library.ts', 'export const answer = 42;']];
snapshot([...rust, ...go, ...ts]);
const facts = call('cqx_facts');
assert.match(facts, /go:rule/);
assert.match(facts, /"extractor":"rust"/);
assert.match(facts, /"extractor":"typescript"/);
const scored = JSON.parse(call('cqx_score', ''));
assert.deepEqual(Object.keys(scored.scores).sort(), ['containment', 'legibility', 'modularity', 'quality', 'security']);
assert.equal(scored.scores.security, 93);
assert.equal(scored.scores.containment, 100);
const dataset = JSON.parse(call('cqx_dataset', 'cqxai/fixture', ''));
assert.equal(expected, 6);
assert.equal(parsed, expected);
assert.deepEqual(dataset.score.scores, scored.scores);
const metadata = call('cqx_manifests');
assert.equal(JSON.parse(metadata).go_modules[''], 'example.com/mixed');
api.cqx_merge_reset();
const slices = [[...rust, go[0]], [go[1], go[2]], go.slice(3).concat(ts)];
for (const files of slices) {
  snapshot(files);
  const shared = call('cqx_gather', metadata);
  assert.equal(JSON.parse(shared).error, undefined);
  assert.equal(parsed, files.filter(([path]) => /\.(rs|go)$/.test(path)).length);
  assert.equal(JSON.parse(call('cqx_merge_add', shared)).error, undefined);
}
const merged = call('cqx_merge_done');
api.cqx_fold_reset();
for (const files of slices) {
  snapshot(files);
  call('cqx_gather', metadata);
  const emitted = call('cqx_emit', merged);
  assert.match(emitted, /"t":"header"/);
  if (files.some(([path]) => path === 'internal/lib.go')) assert.match(emitted, /example.com\/mixed/);
  assert.equal(JSON.parse(call('cqx_fold_add', emitted)).error, undefined);
}
const folded = JSON.parse(call('cqx_fold_done', 'cqxai/fixture', config));
assert.deepEqual(folded.score.scores, scored.scores);
assert.deepEqual(folded.score.rules, scored.rules);
snapshot([['broken.go', 'package p\nfunc = ;']]);
assert.match(JSON.parse(call('cqx_score', '')).error, /broken.go/);
console.log('WASM Go, three-language scoring, root config, progress, shards, generics, parse failure: passed');
