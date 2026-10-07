mod generated;
#[allow(dead_code, non_upper_case_globals)] // The generated bindings include every grammar term.
mod terms;
// CPP_ONLY
mod tokens;
use std::cell::RefCell;
use rezel_common::IterMode;
thread_local! {static FILES:RefCell<Vec<(String,String)>>=const{RefCell::new(Vec::new())};}
#[no_mangle]pub extern "C" fn cqx_alloc(n:usize)->*mut u8{let mut v=Vec::<u8>::with_capacity(n);let p=v.as_mut_ptr();std::mem::forget(v);p}
#[no_mangle]pub unsafe extern "C" fn cqx_free(p:*mut u8,n:usize){drop(Vec::from_raw_parts(p,0,n));}
unsafe fn input(p:*const u8,n:usize)->String{String::from_utf8_lossy(std::slice::from_raw_parts(p,n)).into_owned()}
fn output(text:String)->*mut u8{let mut v=Vec::with_capacity(text.len()+4);v.extend_from_slice(&(text.len() as u32).to_le_bytes());v.extend_from_slice(text.as_bytes());let p=v.as_mut_ptr();std::mem::forget(v);p}
#[no_mangle]pub extern "C" fn cqx_reset(_:usize,_:usize){FILES.with(|f|f.borrow_mut().clear());}
#[no_mangle]pub unsafe extern "C" fn cqx_add_file(p:*const u8,n:usize,s:*const u8,l:usize){FILES.with(|f|f.borrow_mut().push((input(p,n),input(s,l))));}
#[no_mangle]pub extern "C" fn cqx_facts()->*mut u8{
 let parser=rezel_lr::LRParser::from_language(&generated::LANGUAGE);
 let values=FILES.with(|f|f.borrow().iter().map(|(path,source)|{
   match parser.parse(source){Err(e)=>serde_json::json!({"path":path,"fatal":e.to_string(),"errors":1}),Ok(tree)=>{
    let mut cursor=tree.cursor(IterMode::NONE);let mut errors=0;let mut kinds=std::collections::BTreeMap::<String,usize>::new();
    let mut spans=Vec::new();
    loop{let name=cursor.node_type().name().to_string();if cursor.node_type().is_error(){errors+=1;}
      *kinds.entry(name.clone()).or_default()+=1;
      if source.len()<1000 {spans.push(serde_json::json!({"kind":name,"from":u32::from(cursor.from()),"to":u32::from(cursor.to())}));}
      if cursor.first_child(){continue;}while !cursor.next_sibling(){if !cursor.parent(){return serde_json::json!({"path":path,"errors":errors,"kinds":kinds,"spans":spans});}}
    }
   }}
 }).collect::<Vec<_>>());output(serde_json::to_string(&values).unwrap())
}
#[no_mangle]pub extern "C" fn cqx_summary()->*mut u8{
 let parser=rezel_lr::LRParser::from_language(&generated::LANGUAGE);
 let values=FILES.with(|f|f.borrow().iter().map(|(path,source)|{
  match parser.parse(source){Err(e)=>serde_json::json!({"path":path,"fatal":e.to_string(),"errors":1}),Ok(tree)=>{
   let mut cursor=tree.cursor(IterMode::NONE);let mut errors=0;
   loop {if cursor.node_type().is_error(){errors+=1;}
    if cursor.first_child(){continue;}while !cursor.next_sibling(){if !cursor.parent(){return serde_json::json!({"path":path,"errors":errors});}}
   }
  }}
 }).collect::<Vec<_>>());output(serde_json::to_string(&values).unwrap())
}
