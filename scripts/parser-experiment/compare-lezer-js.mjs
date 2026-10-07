import {parser} from '../../.experiment/lezer-js/node_modules/@lezer/cpp/dist/index.js';
import {readFile,writeFile} from 'node:fs/promises';
import {performance} from 'node:perf_hooks';
import path from 'node:path';
const [out,...repos]=process.argv.slice(2),results={};
for(const repo of repos){
 const name=path.basename(repo);
 const rust=JSON.parse(await readFile(`.experiment/lezer-results.json.${name}.json`));
 const facts=(await readFile(`.experiment/results.json.${name}.clang.facts.ndjson`,'utf8')).trim().split('\n').map(JSON.parse);
 const eligible=facts.filter(n=>n.t==='node'&&n.kind==='file'&&['c','cpp'].includes(n.attrs?.language));
 const files=new Set(eligible.map(n=>n.attrs.path));
 const rows=[],start=performance.now();let parseMs=0;
 for(const r of rust.filter(r=>files.has(r.path))){
  const text=await readFile(path.join(repo,r.path),'utf8'),t=performance.now(),tree=parser.parse(text);
  let errors=0;tree.iterate({enter(node){if(node.type.isError)errors++;}});parseMs+=performance.now()-t;
  rows.push({path:r.path,rust_errors:r.errors,js_errors:errors});
 }
 results[name]={eligible:rows.length,rust_clean:rows.filter(r=>r.rust_errors===0).length,js_clean:rows.filter(r=>r.js_errors===0).length,js_parse_walk_ms:parseMs,wall_ms:performance.now()-start,clean_status_mismatches:rows.filter(r=>(r.rust_errors===0)!==(r.js_errors===0))};
}
await writeFile(out,JSON.stringify(results,null,2)+'\n');console.log(JSON.stringify(results,null,2));
