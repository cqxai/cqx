#!/usr/bin/env python3
"""Exercise actual compiler/target adaptation and cache corruption refusal."""
import importlib.util
import os
import pathlib
import subprocess
import tempfile
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('zig_toolchain', ROOT / 'scripts/zig-toolchain.py')
toolchain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(toolchain)
for system, machine, expected in [
    ('Darwin', 'x86_64', 'x86_64-macos'), ('Darwin', 'arm64', 'aarch64-macos'),
    ('Linux', 'x86_64', 'x86_64-linux'), ('Linux', 'aarch64', 'aarch64-linux'),
    ('Windows', 'AMD64', 'x86_64-windows'), ('Windows', 'ARM64', 'aarch64-windows'),
]:
    with patch.object(toolchain.platform, 'system', return_value=system), patch.object(toolchain.platform, 'machine', return_value=machine):
        assert toolchain.host() == expected
zig = toolchain.install(ROOT / '.target/zig')
env = os.environ.copy()
env['CQX_BUILD_ZIG'] = str(zig)
env['ZIG_GLOBAL_CACHE_DIR'] = str(ROOT / '.target/zig/global')
env['ZIG_LOCAL_CACHE_DIR'] = str(ROOT / '.target/zig/local')
(ROOT / '.tmp').mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(dir=ROOT / '.tmp') as temporary:
    directory = pathlib.Path(temporary)
    source = directory / 'answer.c'
    source.write_text('int answer(void) { return 42; }\n')
    output = directory / 'answer.wasm'
    args = ['--target=wasm32-unknown-unknown', str(source), '-Wl,--no-entry', '-Wl,--export=answer', '-o', str(output)]
    subprocess.run([str(zig), 'cc', *toolchain.adapt(args)], env=env, check=True)
    subprocess.run(['node', '--input-type=module', '-e', 'import{readFile}from"node:fs/promises";const{instance}=await WebAssembly.instantiate(await readFile(process.argv[1]));if(instance.exports.answer()!==42)throw Error("bad compiler output");', str(output)], check=True)
    # Revert proof: cc-rs spelling without our adapter fails the real compiler.
    reverted = subprocess.run([str(zig), 'cc', *args], env=env, capture_output=True, text=True)
    assert reverted.returncode != 0 and 'UnknownOperatingSystem' in reverted.stderr, reverted.stderr
    corrupt = directory / 'corrupt.tar.xz'
    corrupt.write_bytes(b'corrupt cached download')
    try:
        toolchain.verify(corrupt, toolchain.PIN['hosts'][toolchain.host()]['sha256'])
    except RuntimeError as error:
        assert 'SHA-256 mismatch' in str(error)
    else:
        raise AssertionError('corrupt Zig archive accepted')
print('Actual Zig WASM answer=42; reverted target adapter fails; corrupt cache refused')
