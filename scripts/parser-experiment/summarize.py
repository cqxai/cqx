#!/usr/bin/env python3
"""Produce compact receipts, including evidence that skip policy loses findings."""
import hashlib
import json
from pathlib import Path

root=Path(__file__).resolve().parents[2];scratch=root/'.experiment'
read=lambda name:json.loads((scratch/name).read_text())
data={'measurements':read('results.json'),'tree_parse_walk':read('tree-times.json'),'ubuntu':read('ubuntu-parity.json'),'individual_parity':read('individual-parity.json'),'negative_controls':read('negative-controls.json'),'lezer_js':read('lezer-js-results.json')}
data['lezer']={}
for repo,r in read('lezer-eligible.json').items():
    errors=set(r.pop('errors'));r['skipped']=len(errors)
    report=read(f'results.json.{repo}.clang.report.json')
    r['baseline_finding_locations_in_rejected_files']={rule['rule']:sum(f['file'] in errors for f in rule['findings']) for rule in report['rules'] if rule['language'] in ['c','cpp','csharp'] and rule['total_findings']}
    data['lezer'][repo]=r
for r in data['lezer_js'].values():
    mismatches=r.pop('clean_status_mismatches');r['clean_status_mismatch_count']=len(mismatches);r['mismatch_examples']=mismatches[:5]
data['artifacts']={p.name:{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted((scratch/'artifacts').glob('*.wasm'))}
data['inputs']={'base':'519c1a303bfb41f2d22e5dcf2c7d967bb62728c4','c_cpp':'dd7801fa8b7cb611e659a5c54ffa8060c049e3d0','csharp':'ee16c2cc498f6b05a18c2179c9a760436135eea6','redis':'e1d873123a15a37c65c876cdf7cb6450a0b5a58b','double_conversion':'080217fda2b360f4d8466680b17d8900870b4940','humanizer':'e4da08c5e631975e15bb98ae020aede819d376ca','rust':'1.98.1','zig':'0.15.2','clang':'23.1.2','node':'26.3.1','lezer_cpp':'1.1.6','rezel_generator':'0.0.0','rezel_lr':'0.0.0','lezer_csharp':'23b0bda779e091f31d6581d7f4c46125fd084df6'}
out=root/'docs/parser-experiment/results.json';out.parent.mkdir(parents=True,exist_ok=True)
out.write_text(json.dumps(data,indent=2)+'\n')
print(out)
