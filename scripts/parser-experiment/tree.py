#!/usr/bin/env python3
"""Build the matching tree-sitter parse/walk-only timing probe with Zig."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root=Path(__file__).resolve().parents[2];scripts=root/'scripts/parser-experiment'
out=root/'.experiment/tree-probe';(out/'src').mkdir(parents=True,exist_ok=True)
(out/'Cargo.toml').write_text('''[package]
name="tree-probe"
version="0.0.0"
edition="2021"
[workspace]
[lib]
crate-type=["cdylib"]
[dependencies]
tree-sitter="=0.27.0"
tree-sitter-c="=0.24.1"
tree-sitter-cpp="=0.23.4"
willbooster-tree-sitter-c-sharp="=2.0.2"
serde_json="1"
''')
source=(scripts/'probe.rs').read_text();source=source[source.index('use std::cell'):source.index('#[no_mangle]pub extern "C" fn cqx_facts()')].replace('use rezel_common::IterMode;\n','')
(out/'src/lib.rs').write_text(source+(scripts/'tree-probe.rs').read_text())
shutil.copy2(scripts/'tree-probe.lock',out/'Cargo.lock')
env=dict(os.environ,TMPDIR=str(root/'.tmp'),CARGO_TARGET_DIR=str(root/'.target'))
cargo=str(Path.home()/'.cargo/bin/cargo');env['PATH']=str(Path.home()/'.cargo/bin')+os.pathsep+env['PATH']
metadata=json.loads(subprocess.check_output([cargo,'+1.98.1','metadata','--locked','--manifest-path',str(out/'Cargo.toml'),'--format-version','1'],env=env))
headers=Path(next(p['manifest_path'] for p in metadata['packages'] if p['name']=='tree-sitter-language')).parent/'wasm/include'
platform='macos' if sys.platform=='darwin' else 'linux'
env.update(CQX_EXPERIMENT_ZIG=str(root/f'.experiment/zig-x86_64-{platform}-0.15.2/zig'),ZIG_GLOBAL_CACHE_DIR=str(root/'.experiment/zig-cache'),CC_wasm32_unknown_unknown=str(scripts/'zig-cc.py'),AR_wasm32_unknown_unknown=str(scripts/'zig-ar.py'),CFLAGS_wasm32_unknown_unknown=f'-I{headers} -include wchar.h -Dstatic_assert=_Static_assert')
with (root/'.tmp/tree-probe-build.log').open('w') as log:
    subprocess.run([cargo,'+1.98.1','build','--locked','--manifest-path',str(out/'Cargo.toml'),'--release','--target','wasm32-unknown-unknown'],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
shutil.copy2(root/'.target/wasm32-unknown-unknown/release/tree_probe.wasm',root/'.experiment/artifacts')
