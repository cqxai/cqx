import {load} from './measure.mjs';
import {readFile,writeFile} from 'node:fs/promises';
import {performance} from 'node:perf_hooks';
import path from 'node:path';
import assert from 'node:assert/strict';
const [artifact,out,...repos]=process.argv.slice(2),results={};
for(const dir of repos){
 const repo=path.basename(dir);
 const facts=(await readFile(`.experiment/results.json.${repo}.clang.facts.ndjson`,'utf8')).trim().split('\n').map(JSON.parse);
 const eligible=facts.filter(n=>n.t==='node'&&n.kind==='file'&&['c','cpp','csharp'].includes(n.attrs?.language));
 const files=[];for(const n of eligible)files.push([n.attrs.path,await readFile(path.join(dir,n.attrs.path),'utf8')]);
 // The eligible subset omits vendor .cpp files. Preserve the coordinator's
 // already established .h dialect, rather than rediscovering it in a shard.
 if(eligible.some(n=>n.attrs.language==='cpp'&&n.attrs.path.endsWith('.h'))) files.push(['__experiment_header_dialect.cpp','']);
 const runs=[];
 for(let i=0;i<3;i++){
  const m=await load(artifact);m.snapshot(files);const t=performance.now(),rows=JSON.parse(m.call('cqx_facts'));runs.push(performance.now()-t);
  assert.equal(rows.length,eligible.length);
  assert.equal(rows.filter(n=>n.errors===0).length,eligible.filter(n=>!n.attrs.skipped).length);
 }
 results[repo]={files:eligible.length,parse_and_cst_walk_ms_runs:runs,median_ms:[...runs].sort((a,b)=>a-b)[1]};
}
await writeFile(out,JSON.stringify(results,null,2)+'\n');console.log(results);
