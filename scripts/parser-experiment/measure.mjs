// A repository is scored once, after all selected readers emit their facts.
import {readFile, writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import path from 'node:path';

const encoder = new TextEncoder(), decoder = new TextDecoder();
export async function load(file) {
  const bytes = await readFile(file);
  const begin = performance.now();
  const {instance} = await WebAssembly.instantiate(bytes, {cqx:{parsing_total(){},parsed_one(){}}});
  const coldMs = performance.now()-begin, api = instance.exports;
  function put(text) {
    const bytes = encoder.encode(text), ptr=api.cqx_alloc(bytes.length);
    new Uint8Array(api.memory.buffer,ptr,bytes.length).set(bytes);
    return [ptr,bytes.length];
  }
  function call(name,...strings) {
    const inputs=strings.map(put);
    const ptr=api[name](...inputs.flat());
    for(const [p,n] of inputs) api.cqx_free(p,n);
    if(!ptr) return;
    const n=new DataView(api.memory.buffer).getUint32(ptr,true);
    const text=decoder.decode(new Uint8Array(api.memory.buffer,ptr+4,n));
    api.cqx_free(ptr,n+4);
    return text;
  }
  function snapshot(files,label='experiment') {
    call('cqx_reset',label);
    for(const [p,s] of files) call('cqx_add_file',p,s);
  }
  return {api,call,snapshot,coldMs,size:bytes.length};
}
export function selected(files) {
  const modules=['core'];
  if(files.some(([p])=>/\.(c|h|cc|cpp|cxx|hpp|hh|hxx|C|H)$/.test(p))) modules.push('c');
  if(files.some(([p])=>/\.(cs|csx)$/.test(p))) modules.push('cs');
  return modules;
}
export async function splitScore(dir,files,label='experiment') {
  const readers=[];
  for(const name of selected(files)) {
    const reader=await load(path.join(dir,`${name}.wasm`));
    reader.snapshot(files,label);
    readers.push(reader);
  }
  const core=readers[0];
  const metadata=JSON.parse(core.call('cqx_manifests'));
  for(const reader of readers.slice(1)) Object.assign(metadata,JSON.parse(reader.call('cqx_manifests')));
  core.api.cqx_merge_reset();
  for(const reader of readers) core.call('cqx_merge_add',reader.call('cqx_gather',JSON.stringify(metadata)));
  const shared=core.call('cqx_merge_done');
  core.api.cqx_fold_reset();
  for(const reader of readers) core.call('cqx_fold_add',reader.call('cqx_emit',shared));
  const report=JSON.parse(core.call('cqx_fold_done',label,''));
  assert.ok(!report.error,JSON.stringify(report));
  return {report:report.score,modules:selected(files)};
}
async function corpus(dir) {
  const paths=execFileSync('git',['-C',dir,'ls-files','-z']).toString().split('\0').filter(Boolean).sort();
  const files=[];
  for(const p of paths) {
    if(!/\.(rs|ts|tsx|js|jsx|mjs|cjs|mts|cts|go|c|h|cc|cpp|cxx|hpp|hh|hxx|C|H|cs|csx)$/.test(p)
       && !/(^|\/)(Cargo.toml|package.json|go.mod|cqx.json)$/.test(p)) continue;
    files.push([p,await readFile(path.join(dir,p),'utf8')]);
  }
  return files;
}
const hash=text=>createHash('sha256').update(text).digest('hex');
if(process.argv[2]==='--cold') {
  const module=await load(process.argv[3]);
  console.log(JSON.stringify({coldMs:module.coldMs,size:module.size}));
} else if(process.argv[2]==='--measure') {
  const [dir,out,...repos]=process.argv.slice(3), results={modules:{},repos:{}};
  for(const name of ['clang','zig','core','c','cs']) {
    const runs=Array.from({length:5},()=>JSON.parse(execFileSync(process.execPath,[import.meta.filename,'--cold',path.join(dir,`${name}.wasm`)])));
    const times=runs.map(r=>r.coldMs).sort((a,b)=>a-b);
    results.modules[name]={bytes:runs[0].size,cold_ms_median:times[2],cold_ms_runs:runs.map(r=>r.coldMs)};
  }
  for(const repo of repos) {
    const files=await corpus(repo), label=path.basename(repo), data={snapshot_files:files.length};
    const artifacts={};
    for(const name of ['clang','zig']) {
      const module=await load(path.join(dir,`${name}.wasm`));module.snapshot(files,label);
      const start=performance.now(), facts=module.call('cqx_facts'), ms=performance.now()-start;
      const report=module.call('cqx_score','');
      assert.ok(!JSON.parse(report).error,report);
      artifacts[name]={facts,report};
      const nodes=facts.trim().split('\n').map(s=>JSON.parse(s)).filter(n=>n.t==='node'&&n.kind==='file');
      const coverage={};
      for(const node of nodes) {const a=node.attrs??{};const language=a.language??'rust';coverage[language]??={parsed:0,skipped:0};coverage[language][a.skipped?'skipped':'parsed']++;}
      data[name]={extract_ms:ms,facts_sha256:hash(facts),report_sha256:hash(report),coverage,scores:JSON.parse(report).scores};
      await writeFile(`${out}.${label}.${name}.report.json`,report);
      await writeFile(`${out}.${label}.${name}.facts.ndjson`,facts);
    }
    assert.equal(artifacts.clang.facts,artifacts.zig.facts,`${label}: compiler facts parity`);
    assert.equal(artifacts.clang.report,artifacts.zig.report,`${label}: compiler full report parity`);
    const start=performance.now(), split=await splitScore(dir,files,label);
    assert.deepEqual(split.report,JSON.parse(artifacts.clang.report),`${label}: split complete report parity`);
    data.split={ms_including_load:performance.now()-start,modules:split.modules,full_report_parity:true};
    results.repos[label]=data;
  }
  const mixed=[['Cargo.toml','[package]\nname="mixed"\nversion="0.1.0"\n'],['src/lib.rs','pub fn stop(){std::process::exit(1);}'],['lib.ts','eval(input);'],['lib.go','package lib\nimport "log"\nfunc Stop(){log.Fatal("bad")}'],['lib.c','#include <stdlib.h>\nvoid stop(){exit(1);}'],['lib.cpp','#include <cstdlib>\nvoid stop(){std::exit(1);}'],['Lib.cs','class Lib { void Stop() { System.Environment.Exit(1); } }'],['cqx.json','{"rules":{"c/exit-in-library":{"weight":7}}}']];
  const mono=await load(path.join(dir,'clang.wasm'));mono.snapshot(mixed);
  const split=await splitScore(dir,mixed);
  assert.deepEqual(split.report,JSON.parse(mono.call('cqx_score','')),'mixed complete report parity');
  assert.equal(selected([['lib.ts','']]).join(','),'core');
  results.mixed={modules:split.modules,full_report_parity:true,scores:split.report.scores};
  await writeFile(out,JSON.stringify(results,null,2)+'\n');
  console.log(JSON.stringify(results,null,2));
}
