// Exact report bytes and complete datasets: real module ABI, never mocked facts.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { join, basename, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { createLoader, connect } from './cqx-loader.mjs';
import { MODULES } from './wasm-catalog.mjs';

const [directory, ...corpora] = process.argv.slice(2);
const beforeIndex = corpora.indexOf('--before');
const beforePath = beforeIndex < 0 ? null : corpora.splice(beforeIndex, 2)[1];
let before;
if (beforePath) {
  const { instance } = await WebAssembly.instantiate(await readFile(beforePath), { cqx: { parsing_total() {}, parsed_one() {} } });
  before = connect(instance);
}
const manifest = JSON.parse(await readFile(join(directory, 'manifest.json')));
const loaded = [];
const loader = createLoader({ manifest, fetchBytes: async (file, id) => {
  loaded.push(id); return readFile(join(directory, file));
} });
const { instance } = await WebAssembly.instantiate(await readFile(join(directory, 'cqx_wasm.wasm')), { cqx: { parsing_total() {}, parsed_one() {} } });
const mono = connect(instance);
const hash = text => createHash('sha256').update(text).digest('hex');
const evidence = { manifest, cases: {} };
async function parity(name, files, config = '') {
  mono.call('cqx_reset', name);
  for (const file of files) mono.call('cqx_add_file', ...file);
  const expected = mono.call('cqx_quote', mono.call('cqx_score', config));
  const begin = performance.now();
  const actual = await loader.scan(files, { repo: name, label: name, config });
  assert.equal(actual.reportJson, expected, `${name}: complete report bytes`);
  assert.deepEqual(actual.dataset, JSON.parse(mono.call('cqx_dataset', name, config)), `${name}: complete dataset`);
  const facts = mono.call('cqx_facts').split('\n').filter(Boolean).map(line => JSON.parse(line));
  const coverage = {};
  for (const node of facts.filter(n => n.t === 'node' && n.kind === 'file')) {
    const language = node.attrs?.language ?? 'rust';
    coverage[language] ??= { parsed: 0, skipped: 0 };
    coverage[language][node.attrs?.skipped ? 'skipped' : 'parsed']++;
  }
  evidence.cases[name] = { report_sha256: hash(expected), full_report_bytes_identical: true, full_dataset_equal: true, ms: performance.now() - begin, scores: actual.dataset.score.scores, product_lines: actual.dataset.score.lines, coverage, rules: actual.dataset.score.rules.filter(r => r.total_findings).map(r => ({ language: r.language, rule: r.rule, findings: r.total_findings, deducted: r.deducted })) };
  console.log(`${name}: exact report bytes and full dataset passed`);
  return actual;
}
async function fixture(root) {
  const files = [];
  async function walk(relative = '') {
    for (const item of await readdir(join(root, relative), { withFileTypes: true })) {
      const path = relative ? `${relative}/${item.name}` : item.name;
      if (item.isDirectory()) await walk(path);
      else if (item.isFile()) files.push([path, await readFile(join(root, path), 'utf8')]);
    }
  }
  await walk();
  return files;
}
// Core-only snapshots must never fetch the heavy grammars.
for (const language of ['rust-compat', 'typescript', 'go', 'java', 'kotlin', 'swift', 'zig', 'python', 'php']) {
  await parity(language, await fixture(resolve('fixtures', language)), language === 'rust-compat' ? '{"exclude":["src/vendor/"]}' : '');
}
assert.deepEqual(loaded, ['core']);
await parity('c-cpp', await fixture(resolve('fixtures/c-cpp')));
await parity('csharp', await fixture(resolve('fixtures/csharp')));
await parity('heavy-discovery', await fixture(resolve('fixtures/heavy-discovery')));

const mixed = [
  ['Cargo.toml', '[package]\nname="mixed"\nversion="0.1.0"\n'],
  ['src/lib.rs', 'pub fn stop(){std::process::exit(1);}'],
  ['lib.ts', 'eval(input);'],
  ['lib.go', 'package lib\nimport "log"\nfunc Stop(){log.Fatal("bad")}'],
  ['lib.c', '#include <stdlib.h>\nvoid stop(){exit(1);}'],
  ['lib.cpp', '#include <cstdlib>\nvoid stop(){std::exit(1);}'],
  ['Library.cs', 'class Lib { void Stop(){System.Environment.Exit(1);} }'],
  ['cqx.json', '{"rules":{"c/exit-in-library":{"weight":7}}}'],
];
const combined = await parity('six-language', mixed);
assert.equal(combined.dataset.score.scores.containment, 0);
assert.equal(combined.dataset.score.scores.security, 70);
assert.deepEqual(loaded, ['core', 'c', 'csharp']);
// Selective metadata: no manifests are supplied to language readers. .h keeps
// the coordinator dialect even when its shard contains no .cpp translation unit.
await parity('header-and-layout', [
  ['nested/CMakeLists.txt', ''], ['nested/Library.csproj', ''], ['nested/dialect.cpp', 'int x;'],
  ['nested/include/lib.h', '#include <cstdlib>\nnamespace lib { void stop(){std::exit(1);} }'],
  ['nested/src/domain/build/lib.c', '#include <stdlib.h>\nvoid f(){exit(1);}'],
  ['nested/build/lib.c', 'invalid'],
  ['nested/src/domain/tests/Lib.cs', 'class L { static void Main(){Environment.Exit(0);} void Helper(){Environment.Exit(1);} }'],
  ['nested/tests/Lib.cs', 'class L { void F(){Environment.Exit(1);} }'],
]);
const missing = structuredClone(manifest); delete missing.modules.c;
await assert.rejects(createLoader({ manifest: missing }).scan(mixed), /required module c is missing/);
await assert.rejects(createLoader({ manifest, fetchBytes: async (file, id) => {
  if (id === 'csharp') throw Error('C# unavailable');
  return readFile(join(directory, file));
} }).scan(mixed), /load csharp: C# unavailable/);
const wrong = structuredClone(manifest); wrong.modules.csharp.sha256 = manifest.modules.core.sha256;
await assert.rejects(createLoader({ manifest: wrong, fetchBytes: async file => readFile(join(directory, file === 'csharp.wasm' ? 'core.wasm' : file)) }).scan(mixed), /binary version\/ABI\/identity mismatch/);

const extensions = Object.values(MODULES).flatMap(m => m.extensions);
for (const root of corpora) {
  const paths = execFileSync('git', ['-C', root, 'ls-files', '-z']).toString().split('\0').filter(Boolean).sort();
  const files = [];
  for (const path of paths) {
    const name = path.split('/').at(-1);
    if (!extensions.includes(name.split('.').at(-1)) && !['Cargo.toml', 'package.json', 'go.mod', 'cqx.json', 'CMakeLists.txt', 'Makefile', 'meson.build', 'configure.ac', 'composer.json', 'pyproject.toml', 'Package.swift', 'build.zig', 'pom.xml', 'build.gradle', 'build.gradle.kts'].includes(name) && !name.endsWith('.csproj')) continue;
    files.push([path, await readFile(join(root, path), 'utf8')]);
  }
  const name = basename(root);
  await parity(name, files);
  if (before) {
    before.call('cqx_reset', name);
    for (const file of files) before.call('cqx_add_file', ...file);
    const report = JSON.parse(before.call('cqx_score', ''));
    assert.ok(!report.error, JSON.stringify(report));
    evidence.cases[name].before = { scores: report.scores, product_lines: report.lines, skipped_files: report.skipped_files?.length ?? 0, rules: report.rules.filter(r => r.total_findings).map(r => ({ language: r.language, rule: r.rule, findings: r.total_findings, deducted: r.deducted })) };
    evidence.cases[name].before_frontend_pins = { c: 'dd7801fa8b7cb611e659a5c54ffa8060c049e3d0', csharp: 'ee16c2cc498f6b05a18c2179c9a760436135eea6' };
  }
  evidence.cases[name].revision = execFileSync('git', ['-C', root, 'rev-parse', 'HEAD']).toString().trim();
}
await writeFile(join(directory, 'parity-evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
console.log('Selective module loading, shared metadata, one score, goldens, six languages and failure controls passed');
