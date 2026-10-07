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
 ['Library.php','<?php function stop(string $input){exit(1);system($input);}'],
 ['public/index.php','<?php exit(0);'],
 ['Broken.php','<?php function broken( {'],
 ['pkg/composer.json','{"bin":["tools/cli"],"require":{"php":">=8.1"}}'],
 ['pkg/tools/cli','#!/usr/bin/env php\n<?php exit(0);'],
 ['cqx.json','{"rules":{"php/exit-in-library":{"weight":7}}}'],
];
snapshot(files);
const report = JSON.parse(call('cqx_score',''));
assert.equal(report.scores.containment,33);
assert.equal(report.scores.security,55);
assert.equal(report.rules.find(r=>r.language==='php'&&r.rule==='exit-in-library').total_findings,1);
assert.equal(report.skipped_files[0].file,'Broken.php');
const dataset=JSON.parse(call('cqx_dataset','fixture',''));
assert.equal(expected,7); assert.equal(parsed,expected);
assert.deepEqual(dataset.score.scores,report.scores);
const metadata=call('cqx_manifests');api.cqx_merge_reset();
const slices=[files.slice(0,4),files.slice(4,8),files.slice(8)];
for(const files of slices){snapshot(files);call('cqx_merge_add',call('cqx_gather',metadata));}
const gathered=call('cqx_merge_done');api.cqx_fold_reset();
for(const files of slices){snapshot(files);call('cqx_gather',metadata);call('cqx_fold_add',call('cqx_emit',gathered));}
const folded=JSON.parse(call('cqx_fold_done','fixture',files.at(-1)[1]));
assert.deepEqual(folded.score.scores,report.scores);
assert.deepEqual(folded.score.rules,report.rules);
assert.deepEqual(folded.score.skipped_files,report.skipped_files);
console.log('PHP WASM parsing, mixed scores, config, entrypoints, progress, skips and sharding: passed');

// Optional declarations are facts, and legacy execution semantics are explicit.
snapshot([['Library.php','<?php declare(strict_types=1);function f(string $input): void {assert($input);create_function(\"$a\",$input);}'],['composer.json','{"require":{"php":"^7.4"}}']]);
const legacy=JSON.parse(call('cqx_score',''));
assert.equal(legacy.rules.find(r=>r.language==='php'&&r.rule==='dynamic-code').total_findings,2);
const facts=call('cqx_facts');
assert.ok(facts.includes('php:strict_types'));
assert.ok(facts.includes('php:parameters'));
console.log('PHP legacy-version semantics and optional type facts: passed');
