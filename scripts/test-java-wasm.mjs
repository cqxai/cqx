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
 ['Cargo.toml','[package]\nname="mixed"\nversion="0.1.0"\n'],
 ['src/lib.rs','pub fn stop() { std::process::exit(1); }'],
 ['lib.ts','eval(input);'],
 ['lib.go','package lib\nimport "log"\nfunc Stop() { log.Fatal("bad") }'],
 ['Library.java','class Library { void stop(String input) { System.exit(1); new ProcessBuilder(input); } }'],
 ['App.java','class App { public static void main(String[] args) { System.exit(1); } }'],
 ['Broken.java','class { broken ('],
 ['cqx.json','{"rules":{"java/exit-in-library":{"weight":7}}}'],
];
snapshot(files);
const report = JSON.parse(call('cqx_score',''));
assert.equal(report.scores.containment,81);
assert.equal(report.scores.security,91);
assert.equal(report.rules.find(r=>r.language==='java'&&r.rule==='exit-in-library').total_findings,1);
assert.equal(report.skipped_files[0].file,'Broken.java');
assert.equal(report.languages.rust.scores.containment, 70);
assert.equal(report.languages.typescript.scores.security, 70);
assert.equal(report.languages.java.scores.containment, 93);
assert.equal(report.languages.java.scores.security, 85);
const dataset=JSON.parse(call('cqx_dataset','fixture',''));
assert.equal(expected,6); assert.equal(parsed,expected);
assert.deepEqual(dataset.score, JSON.parse(call('cqx_quote', JSON.stringify(report))));
const metadata=call('cqx_manifests');api.cqx_merge_reset();
const slices=[files.slice(0,4),files.slice(4)];
for(const files of slices){snapshot(files);call('cqx_merge_add',call('cqx_gather',metadata));}
const gathered=call('cqx_merge_done');api.cqx_fold_reset();
for(const files of slices){snapshot(files);call('cqx_gather',metadata);call('cqx_fold_add',call('cqx_emit',gathered));}
const folded=JSON.parse(call('cqx_fold_done','fixture',files.at(-1)[1]));
assert.deepEqual(folded.score.scores,report.scores);
assert.deepEqual(folded.score.rules,report.rules);
assert.deepEqual(folded.score.languages, report.languages);
assert.deepEqual(folded.score.skipped_files,report.skipped_files);
console.log('Java WASM parsing, mixed scores, config, entrypoints, progress, skips and sharding: passed');

const reviewFiles = [
 ['module/build.gradle.kts', '// project root'],
 ['module/src/main/java/com/acme/build/Library.java', 'class Library { void stop() { System.exit(1); } }'],
 ['module/src/main/java/com/acme/TestimonialService.java', 'class TestimonialService { void stop() { System.exit(1); } }'],
 ['module/target/generated-sources/Output.java', 'class Output { void stop() { System.exit(1); } }'],
 ['module/src/test/java/LibraryIT.java', 'class LibraryIT { void stop() { System.exit(1); } }'],
 ['App.java', 'class App { void main() { System.exit(1); } } class Library { void stop() { System.exit(2); } }'],
 ['Suppressions.java', '@SuppressWarnings("all") class Suppressions { @SuppressWarnings("unchecked") void work() {} }'],
];
snapshot(reviewFiles);
const reviewed = JSON.parse(call('cqx_score', ''));
const findingCount = (report, rule) => report.rules.find(r => r.language === 'java' && r.rule === rule).total_findings;
assert.equal(findingCount(reviewed, 'exit-in-library'), 3);
assert.equal(findingCount(reviewed, 'undocumented-suppressions'), 1);
assert.equal(reviewed.lines, 5);
const reviewMeta = call('cqx_manifests');
const reviewShards = [[reviewFiles[0]], reviewFiles.slice(1)];
api.cqx_merge_reset();
for (const shard of reviewShards) {
  snapshot(shard);
  call('cqx_merge_add', call('cqx_gather', reviewMeta));
}
const reviewMerged = call('cqx_merge_done');
api.cqx_fold_reset();
for (const shard of reviewShards) {
  snapshot(shard);
  call('cqx_gather', reviewMeta);
  call('cqx_fold_add', call('cqx_emit', reviewMerged));
}
const reviewFolded = JSON.parse(call('cqx_fold_done', 'fixture', '')).score;
assert.deepEqual(reviewFolded.scores, reviewed.scores);
assert.deepEqual(reviewFolded.rules, reviewed.rules);
assert.equal(reviewFolded.lines, reviewed.lines);
console.log('Java project-root exclusions across readers, product names, scoped main and broad suppressions: passed');
