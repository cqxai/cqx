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
const files = [
  ['Cargo.toml', '[package]\nname="mixed"\nversion="0.1.0"\n'],
  ['src/lib.rs', 'pub fn stop() { std::process::exit(1); }'],
  ['lib.ts', 'eval(input);'],
  ['lib.go', 'package lib\nimport "log"\nfunc Stop() { log.Fatal("bad") }'],
  ['lib.c', '#include <stdlib.h>\nvoid stop(void) { exit(1); }'],
  ['include/lib.h', 'namespace lib { void stop() { std::exit(1); } }'],
  ['lib.cpp', '#include <cstdlib>\nvoid stop() { std::exit(1); }'],
  ['broken.cpp', 'void f( { ;'],
  ['cqx.json', '{"rules":{"c/exit-in-library":{"weight":7}}}'],
];
snapshot(files);
const report = JSON.parse(call('cqx_score', ''));
assert.equal(report.scores.containment, 78); // Weighted independent language scores over 10 product lines.
assert.equal(report.scores.security, 97);
assert.equal(report.skipped_files[0].file, 'broken.cpp');
assert.equal(report.rules.find(r => r.language === 'c' && r.rule === 'exit-in-library').total_findings, 1);
assert.equal(report.rules.find(r => r.language === 'cpp' && r.rule === 'exit-in-library').total_findings, 1);
const metadata = call('cqx_manifests');
api.cqx_merge_reset();
const slices = [files.slice(0, 5), files.slice(5)];
for (const files of slices) {
  snapshot(files);
  call('cqx_merge_add', call('cqx_gather', metadata));
}
const gathered = call('cqx_merge_done');
api.cqx_fold_reset();
for (const files of slices) {
  snapshot(files);
  call('cqx_gather', metadata);
  call('cqx_fold_add', call('cqx_emit', gathered));
}
const folded = JSON.parse(call('cqx_fold_done', 'fixture', files.at(-1)[1]));
assert.deepEqual(folded.score.scores, report.scores);
assert.deepEqual(folded.score.rules, report.rules);
assert.deepEqual(folded.score.skipped_files, report.skipped_files);
console.log('C/C++ WASM parsing, mixed scores, configuration, skips and sharding: passed');

// Macro normalization/recovery metadata survive both native browser scoring
// and the production reader/coordinator fold, with original source offsets.
const recovery = [
  ['cJSON.c', '#include <stdio.h>\nCJSON_PUBLIC(void) f(void) {\n @@@\n char b[15]; sprintf(b, "%d", 1);\n}\n'],
  ['broken.c', '@\n'.repeat(20)],
];
snapshot(recovery);
const recovered = JSON.parse(call('cqx_score', ''));
assert.equal(recovered.skipped, 1);
assert.equal(recovered.recovered, 1);
assert.equal(recovered.coverage.c.partial, true);
assert.equal(recovered.coverage.c.scored_files, 1);
assert.equal(recovered.rules.find(r => r.language === 'c' && r.rule === 'unsafe-buffer-calls').findings[0].line, 4);
const recoveryMetadata = call('cqx_manifests');
api.cqx_merge_reset();
for (const file of recovery) {
  snapshot([file]);
  call('cqx_merge_add', call('cqx_gather', recoveryMetadata));
}
const recoveryGathered = call('cqx_merge_done');
api.cqx_fold_reset();
for (const file of recovery) {
  snapshot([file]);
  call('cqx_gather', recoveryMetadata);
  call('cqx_fold_add', call('cqx_emit', recoveryGathered));
}
const recoveryFolded = JSON.parse(call('cqx_fold_done', 'recovery', '{}')).score;
assert.deepEqual(recoveryFolded, recovered);
console.log('C/C++ macro recovery, coverage honesty and sharded metadata: passed');
