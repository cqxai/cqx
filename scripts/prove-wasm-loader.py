#!/usr/bin/env python3
"""Revert proofs through the actual loader/WASM, always restoring production."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
source = ROOT / 'scripts/cqx-loader.mjs'
original = source.read_text()
directory = sys.argv[1]
mutations = {
    'drop-language-facts': (
        "      for (const [id, host] of hosts) checked(core.call('cqx_fold_add', checked(host.call('cqx_emit', shared), `emit ${id}`)), `fold ${id}`);",
        '      // Reverted facts handoff.',
    ),
    'accept-stale-binary': (
        'if (info.module !== id || info.version !== manifest.version || info.abi_version !== ABI_VERSION)',
        'if (false)',
    ),
}
try:
    for name, (before, after) in mutations.items():
        assert original.count(before) == 1, name
        source.write_text(original.replace(before, after))
        result = subprocess.run(['node', 'scripts/test-loader.mjs', directory], cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / f'{name}.log').write_text(result.stdout + result.stderr)
        assert result.returncode != 0 and 'AssertionError' in result.stderr, f'{name}: unexpected outcome: {result.stderr}'
        print(f'{name}: actual regression failed, exit {result.returncode}')
finally:
    source.write_text(original)
subprocess.run(['node', 'scripts/test-loader.mjs', directory], cwd=ROOT, check=True)
