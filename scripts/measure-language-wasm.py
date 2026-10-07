#!/usr/bin/env python3
"""Measure release linkage ablations; restore source/manifests/lock even on failure.

No reduced parser is shipped. The Python no-names case is diagnostic only: it
would reject valid named Unicode escapes, and therefore is not an optimization.
"""
import json
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
paths = [root / p for p in ["crates/cqx-analysis/src/lib.rs", "crates/cqx-analysis/Cargo.toml", "Cargo.lock", "vendor/rezel-lang-python/src/unicode_names.rs"]]
saved = {p: p.read_bytes() for p in paths}
original = saved[paths[0]].decode()
tail = original[original.index("pub fn run(vfs:"):]
languages = ["java", "kotlin", "swift", "zig", "python", "php"]
def dispatch(enabled):
    # Use exactly the production frontend calls with a subset of dependencies.
    fields = ""; metadata = ""; prepare = ""; values = ""
    if "python" in enabled:
        fields += "python_entries: std::collections::BTreeMap<String,std::collections::BTreeSet<String>>,"
        metadata += 'metadata["python_entries"]=serde_json::json!(cqx_python::entry_files(vfs));'
        prepare += 'let python_entries=serde_json::from_value(metadata["python_entries"].clone()).unwrap_or_default();'
        values += "python_entries,"
    if "php" in enabled:
        fields += "php:cqx_php::Metadata,"
        metadata += 'metadata["php"]=serde_json::json!(cqx_php::metadata(vfs));'
        prepare += 'let php=serde_json::from_value(metadata["php"].clone()).unwrap_or_default();'
        values += "php,"
    predicates = ["cqx_php::is_source_in(vfs,p,&php)" if l == "php" else f"cqx_{l}::is_source(p)" for l in enabled]
    count = "vfs.paths().filter(|p| " + " || ".join(predicates) + ").count() as u32" if enabled else "0"
    calls = []
    for l in enabled:
        if l == "java": calls.append("cqx_java::run_with_layout(vfs,&mut out,&self.layout,tick)?")
        elif l == "python": calls.append("cqx_python::run_with_layout(vfs,&mut out,tick,&self.python_entries,&self.layout)?")
        elif l == "php": calls.append("cqx_php::run_with_layout(vfs,&mut out,tick,&self.php,&self.layout)?")
        else: calls.append(f"cqx_{l}::run_with_layout(vfs,&mut out,tick,&self.layout)?")
    emit = "for extra in [" + ",".join(calls) + "] {stats.packages+=usize::from(extra.files>0);stats.files+=extra.files;stats.nodes+=extra.nodes;stats.edges+=extra.edges;stats.unparsed.extend(extra.unparsed);}" if calls else ""
    return f'''use cqx_vfs::Vfs; use serde_json::Value;
pub fn manifests(vfs:&Vfs)->Result<Value,String>{{
 let mut metadata=if vfs.paths().any(|p|p=="Cargo.toml"||p.ends_with("/Cargo.toml")){{cqx_rust::manifest::read(vfs)?}}else{{serde_json::json!({{"packages":[]}})}};
 metadata["typescript_bins"]=serde_json::json!(cqx_ts::entry_files(vfs));metadata["go_modules"]=serde_json::json!(cqx_go::modules(vfs));metadata["layout"]=serde_json::json!(cqx_layout::Layout::from_paths(vfs.paths()));{metadata}Ok(metadata)
}}
pub struct Prepared{{rust:cqx_rust::extract::Prepared,bins:std::collections::BTreeSet<String>,go:cqx_go::Prepared,layout:cqx_layout::Layout,{fields}}}
pub fn prepare_reporting(vfs:&Vfs,metadata:Value,total:&dyn Fn(u32),tick:&dyn Fn())->Result<Prepared,ExtractError>{{
 let bins=serde_json::from_value(metadata["typescript_bins"].clone()).unwrap_or_default();let layout=serde_json::from_value(metadata["layout"].clone()).unwrap_or_else(|_|cqx_layout::Layout::from_paths(vfs.paths()));{prepare}
 let go_modules=serde_json::from_value(metadata["go_modules"].clone()).unwrap_or_default();let go_files=vfs.paths().filter(|p|cqx_go::is_source(p)).count() as u32;let ts_files=vfs.paths().filter(|p|cqx_ts::is_source(p)).count() as u32;let extra_files={count};
 let rust=cqx_rust::extract::prepare_reporting(vfs,metadata,&|n|total(n+ts_files+go_files+extra_files),tick)?;let go=cqx_go::prepare_with_layout(vfs,go_modules,tick,layout.clone())?;Ok(Prepared{{rust,bins,go,layout,{values}}})
}}
impl Prepared{{
 pub fn gathered(&self)->cqx_rust::prepass::PackageFacts{{self.rust.gathered()}}
 pub fn emit(&self,vfs:&Vfs,facts:&cqx_rust::prepass::PackageFacts,out:impl std::io::Write)->Result<Stats,ExtractError>{{self.emit_watched(vfs,facts,out,&||{{}})}}
 pub fn emit_watched(&self,vfs:&Vfs,facts:&cqx_rust::prepass::PackageFacts,mut out:impl std::io::Write,tick:&dyn Fn())->Result<Stats,ExtractError>{{
 let mut stats=self.rust.emit(vfs,facts,&mut out)?;let ts=cqx_ts::run_with_layout(vfs,&mut out,&self.bins,tick,&self.layout)?;stats.packages+=usize::from(ts.files>0);stats.files+=ts.files;stats.nodes+=ts.nodes;stats.edges+=ts.edges;
 let go=self.go.emit(vfs,&mut out)?;stats.packages+=go.packages;stats.files+=go.files;stats.nodes+=go.nodes;stats.edges+=go.edges;{emit}stats.unparsed.extend(ts.unparsed);stats.unparsed.extend(go.unparsed);Ok(stats)
 }}
}}
''' + tail

sizes = {}
env = dict(os.environ)
env["CARGO_TARGET_DIR"] = str(root / ".target")
env["TMPDIR"] = str(root / ".tmp")
env["PATH"] = str(Path.home() / ".cargo/bin") + os.pathsep + env["PATH"]
# rustup supplies the installed wasm target; the configured sccache is retained.
cargo = str(Path.home() / ".cargo/bin/cargo")
try:
    for name, enabled in [("baseline", []), *[(l, [l]) for l in languages], ("python_without_unicode_names", ["python"])]:
        for p,b in saved.items(): p.write_bytes(b)
        paths[0].write_text(dispatch(enabled))
        manifest = saved[paths[1]].decode()
        manifest = "\n".join(line for line in manifest.splitlines() if not any(line.startswith(f"cqx-{l} ") and l not in enabled for l in languages)) + "\n"
        paths[1].write_text(manifest)
        if name == "python_without_unicode_names":
            paths[3].write_text("pub(crate) fn character(_name: &str) -> Option<u32> { None }\n")
        with (root / ".tmp" / f"layout-size-{name}.log").open("w") as log:
            subprocess.run([cargo,"+1.98.1","build","--offline","--release","--target","wasm32-unknown-unknown","-p","cqx-wasm"],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        wasm = root / ".target/wasm32-unknown-unknown/release/cqx_wasm.wasm"
        sizes[name] = wasm.stat().st_size
        print(f"{name}: {sizes[name]} bytes",flush=True)
finally:
    for p,b in saved.items(): p.write_bytes(b)
with (root / ".tmp/layout-wasm.log").open("w") as log:
    subprocess.run([cargo,"+1.98.1","build","--locked","--release","--target","wasm32-unknown-unknown","-p","cqx-wasm"],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
sizes["all"] = (root / ".target/wasm32-unknown-unknown/release/cqx_wasm.wasm").stat().st_size
(root / ".tmp/layout-wasm-sizes.json").write_text(json.dumps(sizes,indent=2)+"\n")
print(f"all (restored production): {sizes['all']} bytes")
