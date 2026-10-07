#!/usr/bin/env python3
"""Prove the heavy-language review regressions using the actual CLI/scorer."""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
if '--core-gate' in sys.argv:
    path = ROOT / 'crates/cqx-wasm/src/lib.rs'
    original = path.read_text()
    before = '#[cfg(feature = "core")]\nfn score_report('
    command = ['cargo', 'check', '--locked', '-p', 'cqx-wasm', '--no-default-features', '--features', 'c']
    try:
        assert original.count(before) == 1
        path.write_text(original.replace(before, 'fn score_report('))
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / 'core-helper-gate.log').write_text(result.stdout + result.stderr)
        assert result.returncode == 101 and 'E0433' in result.stderr, result.stderr
        print('core helper gate: C-only build fails, exit 101', flush=True)
    finally:
        path.write_text(original)
    subprocess.run(command, cwd=ROOT, check=True)
    sys.exit(0)

source = ROOT / 'crates/cqx-layout/src/lib.rs'
saved = source.read_text()
start = saved.index('            if let Some(tail) = comment\n                .strip_prefix("pragma ")')
end = saved.index('            false\n', start)
old_directive = '''            if comment.contains("diagnostic ignored") {
                let codes: Vec<_> = comment.split('"').nth(1).into_iter().collect();
                return broad_suppression(SuppressionScope::Member, &codes);
            }
'''
proofs = [
    ('c-output-anchors', 'heavy_c_output_names_are_excluded_only_at_project_roots',
     '    pub fn excluded_dir(&self, language: &str, path: &str) -> bool {',
     '    pub fn excluded_dir(&self, language: &str, path: &str) -> bool {\n        if path.split(\'/\').any(|p| excluded_name(language, p)) { return true; }'),
    ('diagnostic-prose', 'heavy_diagnostic_prose_without_a_quoted_code_is_not_a_suppression', saved[start:end], old_directive),
    ('dot-tests-project', 'heavy_dot_tests_roots_require_a_project_marker_and_exact_suffix',
     'name.ends_with(".Tests") || name.ends_with(".Test")', 'false'),
]
try:
    for name, test, before, after in proofs:
        assert saved.count(before) == 1, name
        source.write_text(saved.replace(before, after))
        result = subprocess.run(['cargo', 'test', '--locked', '-p', 'cqx', '--test', 'heavy_layout', test, '--', '--exact'], cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / f'{name}.log').write_text(result.stdout + result.stderr)
        assert result.returncode == 101 and 'FAILED' in result.stdout and 'assertion' in result.stdout, result.stdout + result.stderr
        print(f'{name}: regression fails, exit 101', flush=True)
finally:
    source.write_text(saved)
# Exact extraction assertions also detect one missing rule while other rules
# still fire: the previous contains-only tests would accept these reversions.
for name, filename, rule, suite in [
    ('c-extract-rule', 'crates/cqx-c/src/lib.rs', 'shell-argument-unchecked', 'c'),
    ('csharp-extract-rule', 'crates/cqx-csharp/src/lib.rs', 'nonliteral-process', 'csharp'),
]:
    path = ROOT / filename
    original = path.read_text()
    try:
        before = f'"{rule}"'
        assert original.count(before) == 1
        path.write_text(original.replace(before, '"unexpected-review-rule"'))
        result = subprocess.run(['cargo', 'test', '--locked', '-p', 'cqx', '--test', suite, f'cli_extracts_exact_{"c_and_cpp" if suite == "c" else "csharp"}_rule_counts_and_roles'], cwd=ROOT, capture_output=True, text=True)
        (ROOT / '.tmp' / f'{name}.log').write_text(result.stdout + result.stderr)
        assert result.returncode == 101 and 'assertion' in result.stdout and 'FAILED' in result.stdout, result.stdout + result.stderr
        print(f'{name}: exact extraction regression fails, exit 101', flush=True)
    finally:
        path.write_text(original)
for suite in ['heavy_layout', 'c', 'csharp']:
    subprocess.run(['cargo', 'test', '--locked', '-p', 'cqx', '--test', suite], cwd=ROOT, check=True)
