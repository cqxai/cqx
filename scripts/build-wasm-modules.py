#!/usr/bin/env python3
"""Build the production modules and a monolithic parity/compatibility reference."""
import pathlib
import subprocess
import sys
ROOT = pathlib.Path(__file__).resolve().parents[1]
DIST = ROOT / '.target/wasm-dist'
for features, name in [('core','core'),('c','c'),('csharp','csharp'),('core,c,csharp','cqx_wasm')]:
    subprocess.run([sys.executable, str(ROOT / 'scripts/build-tree-wasm.py'), '--features', features, '--output', str(DIST / f'{name}.wasm')], check=True)
subprocess.run(['node', str(ROOT / 'scripts/wasm-manifest.mjs'), str(DIST), 'core', 'c', 'csharp'], check=True)
