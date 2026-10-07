#!/usr/bin/env python3
"""Diagnostic only: held frontends in today's engine, always restoring source.

This changes neither held branch. The adapter supplies the new dispatch signature
without changing the original frontends' extraction behavior.
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PINS = {
    'c': 'dd7801fa8b7cb611e659a5c54ffa8060c049e3d0',
    'csharp': 'ee16c2cc498f6b05a18c2179c9a760436135eea6',
    'syntax': 'ee16c2cc498f6b05a18c2179c9a760436135eea6',
}
saved = {}
try:
    for name, pin in PINS.items():
        relative = f'crates/cqx-{name}/src/lib.rs'
        source = ROOT / relative
        saved[source] = source.read_bytes()
        held = subprocess.check_output(['git', 'show', f'{pin}:{relative}'], cwd=ROOT).decode()
        if name == 'c':
            held += '\npub fn run_with_layout(vfs: &Vfs, out: impl Write, cpp_headers: bool, tick: &dyn Fn(), _layout: &cqx_layout::Layout) -> io::Result<Stats> { run_with_headers(vfs, out, cpp_headers, tick) }\n'
        elif name == 'csharp':
            held += '\npub fn run_with_layout(vfs: &Vfs, out: impl Write, tick: &dyn Fn(), _layout: &cqx_layout::Layout) -> io::Result<Stats> { run(vfs, out, tick) }\n'
        source.write_text(held)
    subprocess.run([sys.executable, 'scripts/build-tree-wasm.py', '--features', 'core,c,csharp', '--output', '.target/wasm-dist/before.wasm'], cwd=ROOT, check=True)
finally:
    for source, content in saved.items():
        source.write_bytes(content)
print('Held frontend reference built; production source restored')
