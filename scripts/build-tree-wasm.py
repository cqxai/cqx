#!/usr/bin/env python3
"""Build release WASM with pinned Zig; never discovers a system LLVM."""
import argparse
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('zig_toolchain', ROOT / 'scripts/zig-toolchain.py')
toolchain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(toolchain)


def build(features, output, cache):
    zig = toolchain.install(cache)
    env = os.environ.copy()
    env['CQX_BUILD_ZIG'] = str(zig)
    wrappers = pathlib.Path(cache).resolve() / 'wrappers'
    wrappers.mkdir(exist_ok=True)
    for mode, variable in [('cc', 'CC_wasm32_unknown_unknown'), ('ar', 'AR_wasm32_unknown_unknown')]:
        wrapper = wrappers / (mode + ('.cmd' if os.name == 'nt' else ''))
        if os.name == 'nt':
            wrapper.write_text(f'@"{sys.executable}" "{ROOT / "scripts/zig-toolchain.py"}" {mode} %*\n')
        else:
            import shlex
            wrapper.write_text('#!/bin/sh\nexec ' + shlex.join([sys.executable, str(ROOT / 'scripts/zig-toolchain.py'), mode]) + ' "$@"\n')
            wrapper.chmod(0o755)
        env[variable] = str(wrapper)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], cwd=ROOT, env=env))
    language = next((p for p in metadata['packages'] if p['name'] == 'tree-sitter-language'), None)
    if language:
        headers = pathlib.Path(language['manifest_path']).parent / 'wasm/include'
        env['CFLAGS_wasm32_unknown_unknown'] = f'-I"{headers}" -include wchar.h -Dstatic_assert=_Static_assert'
        env['CC_SHELL_ESCAPED_FLAGS'] = '1'
    # Also confine Zig's build caches to this checkout.
    env['ZIG_GLOBAL_CACHE_DIR'] = str(pathlib.Path(cache).resolve() / 'global')
    env['ZIG_LOCAL_CACHE_DIR'] = str(pathlib.Path(cache).resolve() / 'local')
    subprocess.run(['cargo', 'build', '--locked', '--release', '--target', 'wasm32-unknown-unknown', '-p', 'cqx-wasm', '--no-default-features', '--features', features], cwd=ROOT, env=env, check=True)
    source = pathlib.Path(metadata['target_directory']) / 'wasm32-unknown-unknown/release/cqx_wasm.wasm'
    output = pathlib.Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--features', default='core')
    parser.add_argument('--output', default=str(ROOT / '.target/wasm-dist/core.wasm'))
    parser.add_argument('--cache', default=str(ROOT / '.target/zig'))
    args = parser.parse_args()
    build(args.features, args.output, args.cache)
