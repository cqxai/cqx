#!/usr/bin/env python3
"""Release gate revert proofs; valid WASM makes hash validation indispensable."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
proofs = [
    ('scripts/wasm-compat.mjs', "if (latest.tag_name !== tag) return false;", "if (latest.tag_name !== tag) return true;", 'legacy-cutoff'),
    ('scripts/package-wasm.mjs', 'if (actual !== entry.sha256)', 'if (false)', 'release-integrity'),
]
for path, before, after, name in proofs:
    source = ROOT / path
    original = source.read_text()
    try:
        assert original.count(before) == 1
        source.write_text(original.replace(before, after))
        result = subprocess.run(['node', 'scripts/test-wasm-package.mjs', sys.argv[1]], cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / f'{name}-revert.log').write_text(result.stdout + result.stderr)
        assert result.returncode != 0 and 'AssertionError' in result.stderr, result.stderr
        print(f'{name}: actual regression fails, exit {result.returncode}', flush=True)
    finally:
        source.write_text(original)
subprocess.run(['node', 'scripts/test-wasm-package.mjs', sys.argv[1]], cwd=ROOT, check=True)
