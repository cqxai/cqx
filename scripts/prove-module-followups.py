#!/usr/bin/env python3
"""Review regressions must fail with each policy reverted; restore every file."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
directory = sys.argv[1]

def proof(name, replacements, command):
    originals = {}
    try:
        for filename, before, after in replacements:
            path = ROOT / filename
            originals.setdefault(path, path.read_text())
            text = path.read_text()
            assert text.count(before) == 1, (name, filename, before)
            path.write_text(text.replace(before, after))
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / f'{name}.log').write_text(result.stdout + result.stderr)
        assert result.returncode != 0, f'{name}: regression passed with fix reverted'
        assert 'FAILED' in result.stdout or 'AssertionError' in result.stderr or 'unsafe path' in result.stderr, result.stderr
        print(f'{name}: reverted regression fails, exit {result.returncode}')
    finally:
        for path, text in originals.items():
            path.write_text(text)

proof('unscored-floor', [
    ('crates/cqx-score/src/lib.rs', 'if !strict {\n            for warning', 'if false {\n            for warning'),
    ('crates/cqx-scan/src/lib.rs', 'if !strict {\n            for warning', 'if false {\n            for warning'),
    ('crates/cqx-score/src/config.rs', '        if strict {\n            failures.extend', '        if strict && false {\n            failures.extend'),
], ['cargo', 'test', '--locked', '-p', 'cqx', '--test', 'multilang', 'scan_and_score_enforce_language_floors_and_numeric_headline_override'])

source = (ROOT / 'crates/cqx-score/src/lib.rs').read_text()
start = source.index('    // Zero deductions need no bucket.')
end = source.index('    let scores = if languages.len()', start)
proof('unbucketed-deduction', [('crates/cqx-score/src/lib.rs', source[start:end], '')],
      ['cargo', 'test', '--locked', '-p', 'cqx-score', 'deduction_cannot_disappear_from_weighted_headline'])
proof('zip-staging', [('scripts/zig-toolchain.py', '.is_relative_to(pathlib.Path(staging).resolve())', '.is_relative_to(staging)')],
      ['python3', 'scripts/test-zig-toolchain.py'])
proof('manifest-trust', [('scripts/cqx-loader.mjs', '      await verified();', '      // Reverted manifest authentication.')],
      ['node', 'scripts/test-loader.mjs', directory])
subprocess.run(['cargo', 'test', '--locked', '-p', 'cqx', '--test', 'multilang'], cwd=ROOT, check=True)
subprocess.run(['cargo', 'test', '--locked', '-p', 'cqx-score', 'deduction_cannot_disappear_from_weighted_headline'], cwd=ROOT, check=True)
subprocess.run(['node', 'scripts/test-loader.mjs', directory], cwd=ROOT, check=True)
