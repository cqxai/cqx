// Parse and walk only: same grammars as the held PRs, no rule emission.
#[no_mangle]
pub extern "C" fn cqx_facts()->*mut u8 {
 let mut c=tree_sitter::Parser::new();c.set_language(&tree_sitter_c::LANGUAGE.into()).unwrap();
 let mut cpp=tree_sitter::Parser::new();cpp.set_language(&tree_sitter_cpp::LANGUAGE.into()).unwrap();
 let mut cs=tree_sitter::Parser::new();cs.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).unwrap();
 let values=FILES.with(|f|{
  let files=f.borrow();let has_cpp=files.iter().any(|(p,_)|p.ends_with(".cc")||p.ends_with(".cpp")||p.ends_with(".cxx"));
  files.iter().filter(|(p,_)|p!="__experiment_header_dialect.cpp").map(|(path,source)|{
   let is_cs=path.ends_with(".cs")||path.ends_with(".csx");
   let is_cpp=!(path.ends_with(".c")||(path.ends_with(".h")&&!has_cpp));
   let parser=if is_cs{&mut cs}else if is_cpp{&mut cpp}else{&mut c};
   let normalized=if is_cs&&!source.ends_with('\n'){format!("{source}\n")}else{source.clone()};
   let tree=parser.parse(&normalized,None).unwrap();let mut cursor=tree.walk();let mut errors=usize::from(tree.root_node().has_error());
   loop {if cursor.node().is_error()||cursor.node().is_missing(){errors+=1;}
    if cursor.goto_first_child(){continue;}
    while !cursor.goto_next_sibling(){if !cursor.goto_parent(){return serde_json::json!({"path":path,"errors":errors});}}
   }
  }).collect::<Vec<_>>()
 });output(serde_json::to_string(&values).unwrap())
}
