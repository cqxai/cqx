import {load} from './measure.mjs';
import {readFile,writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {performance} from 'node:perf_hooks';
const [artifacts,out,...repos]=process.argv.slice(2), results={};
for(const repo of repos){
 const cs=path.basename(repo)==='humanizer', name=cs?'lezer_cs':'lezer_c';
 const module=await load(`${artifacts}/${name}.wasm`);
 const paths=execFileSync('git',['-C',repo,'ls-files','-z']).toString().split('\0').filter(p=>cs?/\.(cs|csx)$/.test(p):/\.(c|h|cc|cpp|cxx|hpp|hh|hxx|C|H)$/.test(p)).sort();
 const baseline=(await readFile(`.experiment/results.json.${path.basename(repo)}.clang.facts.ndjson`,'utf8')).trim().split('\n').map(JSON.parse);
 const eligible=new Set(baseline.filter(n=>n.t==='node'&&n.kind==='file'&&(cs?n.attrs?.language==='csharp':['c','cpp'].includes(n.attrs?.language))).map(n=>n.attrs.path));
 const files=[];for(const p of paths.filter(p=>eligible.has(p)))files.push([p,await readFile(path.join(repo,p),'utf8')]);
 const times=[];let raw;
 for(let i=0;i<3;i++){module.snapshot(files);const start=performance.now();raw=module.call('cqx_summary');times.push(performance.now()-start);}
 const ms=[...times].sort((a,b)=>a-b)[1];
 const parsed=JSON.parse(raw);await writeFile(`${out}.${path.basename(repo)}.json`,JSON.stringify(parsed,null,2));
 results[path.basename(repo)]={bytes:module.size,cold_ms:module.coldMs,parse_and_cst_walk_ms:ms,parse_and_cst_walk_ms_runs:times,files:parsed.length,clean:parsed.filter(f=>f.errors===0).length,errors:parsed.filter(f=>f.errors>0).map(f=>f.path)};
}
const m=await load(`${artifacts}/lezer_c.wasm`);
m.snapshot([['example.c','#include <stdlib.h>\nvoid stop(char *s) { exit(1); system(s); }'],['macro.c','#ifdef X\nvoid foo(\n#else\nvoid bar(\n#endif\nint x) {}']]);
await writeFile(`${out}.samples.json`,m.call('cqx_facts'));
await writeFile(out,JSON.stringify(results,null,2)+'\n');console.log(JSON.stringify(results,null,2));
