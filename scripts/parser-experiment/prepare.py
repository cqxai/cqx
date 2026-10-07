#!/usr/bin/env python3
"""Assemble held frontend copies; never checkout or edit either held branch."""
import io
import pathlib
import subprocess
import tarfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
BASE = "519c1a303bfb41f2d22e5dcf2c7d967bb62728c4"
C = "dd7801fa8b7cb611e659a5c54ffa8060c049e3d0"
CS = "ee16c2cc498f6b05a18c2179c9a760436135eea6"
OUT = ROOT / ".experiment/combined"

def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)

def copy(ref, path):
    names = git("ls-tree", "-r", "--name-only", ref, path).decode().splitlines()
    for name in names:
        dest = OUT / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(git("show", f"{ref}:{name}"))

def edit(path, transform):
    p = OUT / path
    p.write_text(transform(p.read_text()))

OUT.mkdir(parents=True, exist_ok=True)
with tarfile.open(fileobj=io.BytesIO(git("archive", BASE))) as archive:
    archive.extractall(OUT)
for ref, paths in [(C, ["crates/cqx-c", "crates/cqx-score", "fixtures/c-cpp", "scripts/test-c-wasm.mjs"]),
                   (CS, ["crates/cqx-csharp", "crates/cqx-syntax", "fixtures/csharp", "scripts/test-csharp-wasm.mjs"])]:
    for path in paths:
        copy(ref, path)
cs_config = git("show", f"{CS}:crates/cqx-score/src/config.rs").decode()
extra = cs_config[cs_config.index('    for id in ["duplicated-bodies"', cs_config.index('"go/{id}"')):cs_config.index('    rules\n}', cs_config.index('"go/{id}"'))]
edit("crates/cqx-score/src/config.rs", lambda s: s.replace('    rules\n}', extra + '    rules\n}'))
edit("Cargo.toml", lambda s: s.replace('members = [', 'members = ["crates/cqx-c", "crates/cqx-csharp", "crates/cqx-syntax", '))
edit("crates/cqx-vfs/src/lib.rs", lambda s: s.replace('    path.ends_with(".rs")', '    matches!(path.rsplit(\'.\').next(), Some("c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "C" | "H" | "cs" | "csx")) || path.ends_with(".rs")'))
edit("crates/cqx-analysis/Cargo.toml", lambda s: s.replace('cqx-go =', 'cqx-go =').replace('[dependencies]', '''[features]
default = ["core", "c", "cs"]
core = []
c = ["dep:cqx-c"]
cs = ["dep:cqx-csharp"]
[dependencies]
cqx-c = { path = "../cqx-c", optional = true }
cqx-csharp = { path = "../cqx-csharp", optional = true }'''))
edit("crates/cqx-wasm/Cargo.toml", lambda s: s.replace('[dependencies]', '''[features]
default = ["core", "c", "cs"]
core = ["cqx-analysis/core"]
c = ["cqx-analysis/c"]
cs = ["cqx-analysis/cs"]
[dependencies]''').replace('cqx-analysis = { path = "../cqx-analysis" }', 'cqx-analysis = { path = "../cqx-analysis", default-features = false }'))
analysis = git("show", f"{BASE}:crates/cqx-analysis/src/lib.rs").decode()
tail = analysis[analysis.index('pub fn run(vfs:'):]
dispatch = '''use cqx_vfs::Vfs;
use serde_json::Value;
pub fn manifests(vfs: &Vfs) -> Result<Value,String> {
 let mut m = if vfs.paths().any(|p| p=="Cargo.toml" || p.ends_with("/Cargo.toml")) { cqx_rust::manifest::read(vfs)? } else { serde_json::json!({"packages":[]}) };
 m["typescript_bins"] = serde_json::json!(cqx_ts::entry_files(vfs));
 m["go_modules"] = serde_json::json!(cqx_go::modules(vfs));
 #[cfg(feature="c")] {m["cpp_headers"] = serde_json::json!(cqx_c::has_cpp(vfs));}
 Ok(m)
}
pub struct Prepared {
 #[cfg(feature="core")] rust: cqx_rust::extract::Prepared,
 #[cfg(feature="core")] bins: std::collections::BTreeSet<String>,
 #[cfg(feature="core")] go: cqx_go::Prepared,
 #[cfg(feature="c")] cpp_headers: bool,
}
pub fn prepare_reporting(vfs:&Vfs, metadata:Value,total:&dyn Fn(u32),tick:&dyn Fn())->Result<Prepared,ExtractError> {
 #[cfg(feature="core")] let bins=serde_json::from_value(metadata["typescript_bins"].clone()).unwrap_or_default();
 #[cfg(feature="core")] let modules=serde_json::from_value(metadata["go_modules"].clone()).unwrap_or_default();
 #[cfg(feature="c")] let cpp_headers=metadata["cpp_headers"].as_bool().unwrap_or(false);
 let mut extra=0;
 #[cfg(feature="c")] {extra+=vfs.paths().filter(|p|cqx_c::is_source(p)).count() as u32;}
 #[cfg(feature="cs")] {extra+=vfs.paths().filter(|p|cqx_csharp::is_source(p)).count() as u32;}
 #[cfg(feature="core")] {extra+=vfs.paths().filter(|p|cqx_ts::is_source(p)||cqx_go::is_source(p)).count() as u32;}
 #[cfg(feature="core")] let rust=cqx_rust::extract::prepare_reporting(vfs,metadata,&|n|total(n+extra),tick)?;
 #[cfg(not(feature="core"))] {let _=(metadata,tick);total(extra);}
 #[cfg(feature="core")] let go=cqx_go::prepare(vfs,modules,tick)?;
 Ok(Prepared { #[cfg(feature="core")] rust, #[cfg(feature="core")] bins, #[cfg(feature="core")] go, #[cfg(feature="c")] cpp_headers })
}
impl Prepared {
 pub fn gathered(&self)->cqx_rust::prepass::PackageFacts {
  #[cfg(feature="core")] {self.rust.gathered()}
  #[cfg(not(feature="core"))] {cqx_rust::prepass::PackageFacts::default()}
 }
 pub fn emit(&self,vfs:&Vfs,facts:&cqx_rust::prepass::PackageFacts,out:impl std::io::Write)->Result<Stats,ExtractError>{self.emit_watched(vfs,facts,out,&||{})}
 pub fn emit_watched(&self,vfs:&Vfs,facts:&cqx_rust::prepass::PackageFacts,mut out:impl std::io::Write,tick:&dyn Fn())->Result<Stats,ExtractError>{
  #[cfg(feature="core")] let mut stats=self.rust.emit(vfs,facts,&mut out)?;
  #[cfg(not(feature="core"))] let mut stats={let _=facts;Stats { packages:0,files:0,nodes:0,edges:0,unparsed:Vec::new() }};
  #[cfg(feature="core")] {let ts=cqx_ts::run_with_entries(vfs,&mut out,&self.bins,tick)?;stats.packages+=usize::from(ts.files>0);stats.files+=ts.files;stats.nodes+=ts.nodes;stats.edges+=ts.edges;stats.unparsed.extend(ts.unparsed);
  let go=self.go.emit(vfs,&mut out)?;stats.packages+=go.packages;stats.files+=go.files;stats.nodes+=go.nodes;stats.edges+=go.edges;stats.unparsed.extend(go.unparsed);}
  #[cfg(feature="c")] {let c=cqx_c::run_with_headers(vfs,&mut out,self.cpp_headers,tick)?;stats.packages+=usize::from(c.files>0);stats.files+=c.files;stats.nodes+=c.nodes;stats.edges+=c.edges;stats.unparsed.extend(c.unparsed);}
  #[cfg(feature="cs")] {let cs=cqx_csharp::run(vfs,&mut out,tick)?;stats.packages+=usize::from(cs.files>0);stats.files+=cs.files;stats.nodes+=cs.nodes;stats.edges+=cs.edges;stats.unparsed.extend(cs.unparsed);}
  Ok(stats)
 }
}
'''
(OUT / "crates/cqx-analysis/src/lib.rs").write_text(dispatch + tail)
shutil_lock = ROOT / 'scripts/parser-experiment/combined.lock'
if shutil_lock.exists():
    (OUT/'Cargo.lock').write_bytes(shutil_lock.read_bytes())
(ROOT/'.experiment/artifacts').mkdir(exist_ok=True)
(ROOT/'.tmp').mkdir(exist_ok=True)
print(OUT)
