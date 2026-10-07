#!/usr/bin/env python3
"""Generate real Rust parsers from pinned grammars; build standalone probes."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

root=Path(__file__).resolve().parents[2]
scratch=root/'.experiment'
scripts=root/'scripts/parser-experiment'
scratch.mkdir(exist_ok=True)
(root/'.tmp').mkdir(exist_ok=True)
def download(url,name,checksum):
    p=scratch/name
    if not p.exists():urllib.request.urlretrieve(url,p)
    assert hashlib.sha256(p.read_bytes()).hexdigest()==checksum, f'{name}: checksum mismatch'
    return p
generator=download('https://static.crates.io/crates/rezel-generator/rezel-generator-0.0.0.crate','rezel-generator.tar.gz','5ad4e5b6733b7b36b8510c6e5a0980bca3cdd561da04243e8f26f275663c274a')
cpp=download('https://registry.npmjs.org/@lezer/cpp/-/cpp-1.1.6.tgz','cpp-1.1.6.tgz','fddd9038e5e2628d43357bd4243b8cca2f8266f68b6114497b884ecdd48084bf')
grammar=scratch/'grammar';grammar.mkdir(exist_ok=True)
with tarfile.open(generator) as t:t.extractall(scratch)
with tarfile.open(cpp) as t:t.extractall(grammar)
cs=download('https://raw.githubusercontent.com/ashmind/lezer-csharp-simple/23b0bda779e091f31d6581d7f4c46125fd084df6/src/csharp.grammar','csharp.grammar','883587992a7bc07e392a510eedb861e30494250d9a5670f7c1db0a07836502b7')
g=scratch/'rezel-generator-0.0.0'
with (g/'Cargo.toml').open('a') as f:f.write('\n[workspace]\n')
shutil.copy2(scripts/'generator.lock',g/'Cargo.lock')
env=dict(os.environ,TMPDIR=str(root/'.tmp'),CARGO_TARGET_DIR=str(root/'.target'))
cargo=str(Path.home()/'.cargo/bin/cargo')
env['PATH']=str(Path.home()/'.cargo/bin')+os.pathsep+env['PATH']
def build(manifest,*extra):
    with (root/'.tmp'/f'{manifest.parent.name}-build.log').open('w') as log:
        subprocess.run([cargo,'+1.98.1','build','--locked','--manifest-path',str(manifest),'--release',*extra],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
build(g/'Cargo.toml','--bin','rezel')
for name,input_path in [('lezer-c',grammar/'package/src/cpp.grammar'),('lezer-cs',cs)]:
    out=scratch/name;src=out/'src';src.mkdir(parents=True,exist_ok=True)
    (out/'Cargo.toml').write_text(f'''[package]
name="{name}"
version="0.0.0"
edition="2021"
[workspace]
[lib]
crate-type=["cdylib","rlib"]
[dependencies]
rezel-lr="=0.0.0"
rezel-common="=0.0.0"
zerocopy={{version="0.8",features=["derive"]}}
serde_json="1"
''')
    subprocess.run([str(root/'.target/release/rezel'),'generate',str(input_path),'--output',str(src/'generated.rs'),'--terms',str(src/'terms.rs')],env=env,check=True)
    # Highlight properties have no role in syntax/rule extraction. Tokenizers
    # remain exact ports; no grammar productions are removed or added.
    p=src/'generated.rs';p.write_text(p.read_text().replace('.extend(&[crate::highlight::cppHighlighting()])',''))
    source=(scripts/'probe.rs').read_text()
    if name=='lezer-c':shutil.copy2(scripts/'tokens.rs',src/'tokens.rs')
    else:source=source.replace('// CPP_ONLY\nmod tokens;','')
    (src/'lib.rs').write_text(source)
    shutil.copy2(scripts/f'{name}.lock',out/'Cargo.lock')
    build(out/'Cargo.toml','--target','wasm32-unknown-unknown')
    (scratch/'artifacts').mkdir(exist_ok=True)
    shutil.copy2(root/f'.target/wasm32-unknown-unknown/release/{name.replace("-","_")}.wasm',scratch/'artifacts')
print('Rezel C/C++ and community C# probes built')
