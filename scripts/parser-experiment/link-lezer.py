#!/usr/bin/env python3
"""Link the generated parser into core wasm, retaining a callable parse export.

This is a parser-only size ablation, not a scoring frontend.
"""
import pathlib
import shutil

root=pathlib.Path(__file__).resolve().parents[2]
out=root/'.experiment/lezer-linked'
shutil.copytree(root/'.experiment/combined',out,dirs_exist_ok=True)
src=out/'crates/cqx-wasm/src'
for name in ['generated.rs','generated.le.bin','generated.be.bin','terms.rs','tokens.rs']:
    shutil.copy2(root/'.experiment/lezer-c/src'/name,src/name)
p=out/'crates/cqx-wasm/Cargo.toml'
p.write_text(p.read_text()+ '\nrezel-lr="=0.0.0"\nrezel-common="=0.0.0"\nzerocopy={version="0.8",features=["derive"]}\n')
lock=root/'scripts/parser-experiment/lezer-linked.lock'
if lock.exists():shutil.copy2(lock,out/'Cargo.lock')
p=src/'lib.rs'
p.write_text(p.read_text()+'''
mod generated;
#[allow(dead_code,non_upper_case_globals)] // The generated bindings include every grammar term.
mod terms;
mod tokens;
/// Experiment-only parse probe, retained to measure actual linked code/tables.
/// # Safety
/// The pointer must identify len bytes of UTF-8 in this module's memory.
#[no_mangle]
pub unsafe extern "C" fn cqx_cpp_probe(ptr:*const u8,len:usize)->*mut u8 {
    let parser=rezel_lr::LRParser::from_language(&generated::LANGUAGE);
    let source=borrow(ptr,len);
    respond(match parser.parse(&source) {
        Err(e)=>serde_json::json!({"fatal":e.to_string()}).to_string(),
        Ok(tree)=>{
            let mut cursor=tree.cursor(rezel_common::IterMode::NONE);
            let mut errors=0;
            loop {
                if cursor.node_type().is_error(){errors+=1;}
                if cursor.first_child(){continue;}
                while !cursor.next_sibling(){if !cursor.parent(){return respond(serde_json::json!({"errors":errors}).to_string());}}
            }
        }
    })
}
''')
print(out)
