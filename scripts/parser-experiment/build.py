#!/usr/bin/env python3
"""Pinned-toolchain builds in scratch space. PATH excludes system LLVM for Zig."""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser()
p.add_argument('compiler', choices=['clang','zig'])
p.add_argument('--features', default='core,c,cs')
p.add_argument('--name', default=None)
p.add_argument('--source', default='.experiment/combined')
args = p.parse_args()
env = dict(os.environ)
env.update(TMPDIR=str(ROOT/'.tmp'), CARGO_TARGET_DIR=str(ROOT/'.target'))
cargo = str(pathlib.Path.home()/'.cargo/bin/cargo')
env['PATH'] = str(pathlib.Path.home()/'.cargo/bin')+os.pathsep+env['PATH']
manifest = ROOT / args.source / 'Cargo.toml'
metadata = json.loads(subprocess.check_output([cargo,'+1.98.1','metadata','--format-version','1','--manifest-path',str(manifest)],env=env))
package = next(p for p in metadata['packages'] if p['name']=='tree-sitter-language')
headers = pathlib.Path(package['manifest_path']).parent/'wasm/include'
env['CFLAGS_wasm32_unknown_unknown'] = f'-I{headers} -include wchar.h -Dstatic_assert=_Static_assert'
if args.compiler=='zig':
    bindir=ROOT/'.experiment/no-llvm-bin'
    bindir.mkdir(exist_ok=True)
    for executable in ['python3','sccache']:
        link=bindir/executable
        if not link.exists(): link.symlink_to(shutil.which(executable))
    env['PATH']=str(bindir)+os.pathsep+str(pathlib.Path.home()/'.cargo/bin')
    zig_platform='macos' if sys.platform=='darwin' else 'linux'
    env['CQX_EXPERIMENT_ZIG']=str(ROOT/f'.experiment/zig-x86_64-{zig_platform}-0.15.2/zig')
    env['ZIG_GLOBAL_CACHE_DIR']=str(ROOT/'.experiment/zig-cache')
    env['CC_wasm32_unknown_unknown']=str(ROOT/'scripts/parser-experiment/zig-cc.py')
    env['AR_wasm32_unknown_unknown']=str(ROOT/'scripts/parser-experiment/zig-ar.py')
    assert not shutil.which('clang',path=env['PATH'])
    assert not shutil.which('llvm-ar',path=env['PATH'])
    # Rust host build-script linking still needs Apple's SDK/linker. This is
    # explicit, never used to compile any wasm C, and absent from PATH.
    if sys.platform=='darwin':
        env['CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER']='/usr/bin/cc'
    else:
        env['CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER']='/usr/bin/gcc'
else:
    env['CC_wasm32_unknown_unknown']='/usr/local/opt/llvm/bin/clang'
    env['AR_wasm32_unknown_unknown']='/usr/local/opt/llvm/bin/llvm-ar'
name=args.name or args.compiler
start=time.monotonic()
with (ROOT/f'.tmp/{name}-build.log').open('w') as log:
    subprocess.run([cargo,'+1.98.1','build','--locked','--manifest-path',str(manifest),'--release','--target','wasm32-unknown-unknown','-p','cqx-wasm','--no-default-features','--features',args.features],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
artifact=ROOT/f'.experiment/artifacts/{name}.wasm'
shutil.copy2(ROOT/'.target/wasm32-unknown-unknown/release/cqx_wasm.wasm',artifact)
print(json.dumps({'name':name,'bytes':artifact.stat().st_size,'build_seconds':time.monotonic()-start}))
