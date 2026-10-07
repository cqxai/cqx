#!/usr/bin/env python3
"""Release gate revert proofs; valid WASM makes hash validation indispensable."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
proofs = [
    ('scripts/wasm-compat.mjs', 'return prior && compare(prior, currentVersion) < 0 && modular(release);', 'return false;', 'legacy-cutoff'),
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
# Revert to the immutable pre-review implementation to exercise its actual
# /releases/latest call against the same recording gh stub.
source = ROOT / 'scripts/wasm-compat.mjs'
original = source.read_text()
try:
    baseline = subprocess.check_output(['git', 'show', '7aea66b:scripts/wasm-compat.mjs'], cwd=ROOT, text=True)
    source.write_text(baseline)
    result = subprocess.run(['node', 'scripts/test-wasm-compat.mjs'], cwd=ROOT, capture_output=True, text=True)
    (ROOT / '.tmp' / 'latest-inventory-revert.log').write_text(result.stdout + result.stderr)
    assert result.returncode != 0 and 'AssertionError' in result.stderr and 'out-of-order older hotfix retains monolith' in result.stderr, result.stderr
    print(f'latest-inventory: actual CLI compatibility regression fails, exit {result.returncode}', flush=True)
finally:
    source.write_text(original)
subprocess.run(['node', 'scripts/test-wasm-compat.mjs'], cwd=ROOT, check=True)
subprocess.run(['node', 'scripts/test-wasm-package.mjs', sys.argv[1]], cwd=ROOT, check=True)
