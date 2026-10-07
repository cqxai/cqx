#!/usr/bin/env python3
"""Revert each shared policy independently; require an assertion failure, restore."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
source = root / "crates/cqx-layout/src/lib.rs"
saved = source.read_text()
proofs = {
    "anchored_exclusions": ("shared_anchored_exclusions", "    pub fn excluded_dir(&self, language: &str, path: &str) -> bool {", "        if path.split('/').any(|p| excluded_name(language,p)) { return true; }"),
    "test_layouts": ("shared_test_layouts", "    pub fn is_test(&self, language: &str, path: &str) -> bool {", '        if path.split(\'/\').any(|p| matches!(p,"test" | "tests" | "Tests")) { return true; }'),
    "scoped_entries": ("shared_scoped_entries", "    pub fn contains(&self, range: &Range<usize>) -> bool {", "        if self.script || !self.entries.is_empty() { return true; }"),
    "idiomatic_suppressions": ("shared_idiomatic_suppressions", "pub fn broad_suppression(scope: SuppressionScope, codes: &[&str]) -> bool {", "    if !codes.is_empty() { return true; }"),
}
suite = "heavy_layout" if "--heavy" in sys.argv or "--discovery" in sys.argv else "project_layout"
if suite == "heavy_layout":
    tests = ["heavy_anchored_exclusions_and_real_generated_markers", "heavy_test_paths_are_anchored_to_projects", "heavy_entries_bound_main_and_nested_helpers", "heavy_specific_codes_stay_quiet_and_broad_suppressions_fire"]
    proofs = {name: (test, signature, mutation) for (name, (_, signature, mutation)), test in zip(proofs.items(), tests)}
if "--discovery" in sys.argv:
    proofs = {}
# rustfmt may wrap signatures, so match function start through its opening brace.
import re
try:
    for name, (test, signature, mutation) in proofs.items():
        function = re.search(r"fn (\w+)\(", signature).group(1)
        pattern = r"((?:pub )?fn " + function + r"\([^{}]+\)\s*->\s*bool\s*\{)"
        mutated, n = re.subn(pattern, lambda m: m[1]+"\n"+mutation, saved, count=1)
        assert n == 1, function
        source.write_text(mutated)
        result = subprocess.run(["cargo","test","--locked","-p","cqx","--test",suite,test,"--","--exact"],cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
        (root / ".tmp" / f"layout-revert-{name}.log").write_text(result.stdout)
        assert result.returncode == 101 and "assertion" in result.stdout and "FAILED" in result.stdout, f"{name}: expected assertion failure, got {result.returncode}\n{result.stdout[-2000:]}"
        print(f"{name}: assertion failure, exit 101",flush=True)
        source.write_text(saved)
    if suite == "heavy_layout":
        mutated, n = re.subn(r'("" => \[[^\]]*)\]', r'\1"c", "csharp",]', saved, count=1)
        assert n == 1
        source.write_text(mutated)
        result = subprocess.run(["cargo", "test", "--locked", "-p", "cqx", "--test", suite, "directory_discovery_preserves_sibling_sources_in_heavy_output_names", "--", "--exact"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        (root / ".tmp" / "layout-revert-discovery.log").write_text(result.stdout)
        assert result.returncode == 101 and "assertion" in result.stdout and "FAILED" in result.stdout, result.stdout[-2000:]
        print("discovery sibling scope: assertion failure, exit 101", flush=True)
finally:
    source.write_text(saved)
result = subprocess.run(["cargo","test","--locked","-p","cqx","--test",suite],cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
(root / ".tmp" / "layout-restored.log").write_text(result.stdout)
assert result.returncode == 0, result.stdout
print("restored shared suite: passed")
