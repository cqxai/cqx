//! A cheap pass over a package, collecting the few facts that let the main
//! visitor resolve a name it would otherwise report as `<dynamic>`.
//!
//! Everything here is still syntax. None of it needs types, and all of it runs
//! in the same order of magnitude as the main pass — which is the point: most
//! unresolved names in real code are one alias, one constant or one hop away,
//! not a question about the type system.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

#[derive(Default)]
pub struct PackageFacts {
    /// Per file: a name introduced by `use … as X` or `type X = …`, mapped to
    /// the path it stands for. File-scoped because that is the real scope of
    /// both forms.
    pub aliases: HashMap<String, HashMap<String, String>>,
    /// `const`/`static` name -> its string literal. Package-scoped, which is an
    /// approximation: two constants of the same name in different modules
    /// collide. Recorded as inferred, never as proven.
    pub constants: HashMap<String, String>,
    /// A function whose entire body is one string literal -> that literal.
    pub literal_fns: HashMap<String, String>,
    /// Function -> the functions it calls.
    ///
    /// Workspace-wide and keyed by BARE name, because resolving a call across
    /// a crate boundary is exactly the problem that needs a compiler. Two
    /// functions sharing a name are therefore conflated, so anything derived
    /// from this map is reported as inferred with the function named, never as
    /// a proven fact.
    pub calls: HashMap<String, Vec<String>>,
    /// Function -> environment variables read anywhere in its body, then
    /// propagated through `calls`. This is what turns "unresolved" into
    /// "environment-controlled", which is the more useful answer.
    pub reads_env: HashMap<String, Vec<String>>,
}

impl PackageFacts {
    /// Collected across the whole workspace, because a value's provenance
    /// routinely crosses a crate boundary: deka's spawn target is three named
    /// hops away in two different crates.
    pub fn collect(files: &[&ParsedFile]) -> PackageFacts {
        let mut facts = PackageFacts::default();
        for ParsedFile { rel_path, parsed, .. } in files.iter().copied() {
            let mut file_aliases = HashMap::new();
            let mut collector = Collector {
                aliases: &mut file_aliases,
                constants: &mut facts.constants,
                literal_fns: &mut facts.literal_fns,
                calls: &mut facts.calls,
                reads_env: &mut facts.reads_env,
                current_fn: Vec::new(),
            };
            collector.visit_file(parsed);
            facts.aliases.insert(rel_path.clone(), file_aliases);
        }
        facts.propagate_env();
        facts
    }

    /// Walks `calls` to a fixpoint so a function that calls a function that
    /// reads the environment is itself marked environment-derived.
    fn propagate_env(&mut self) {
        for _ in 0..8 {
            let mut changed = false;
            let snapshot = self.reads_env.clone();
            for (caller, callees) in &self.calls {
                let mut gained: Vec<String> = Vec::new();
                for callee in callees {
                    if let Some(vars) = snapshot.get(callee) {
                        for var in vars {
                            let known = snapshot.get(caller).is_some_and(|v| v.contains(var));
                            if !known && !gained.contains(var) {
                                gained.push(var.clone());
                            }
                        }
                    }
                }
                if !gained.is_empty() {
                    changed = true;
                    self.reads_env.entry(caller.clone()).or_default().extend(gained);
                }
            }
            if !changed {
                break;
            }
        }
    }

    pub fn aliases_for(&self, rel_path: &str) -> HashMap<String, String> {
        self.aliases.get(rel_path).cloned().unwrap_or_default()
    }
}

struct Collector<'a> {
    aliases: &'a mut HashMap<String, String>,
    constants: &'a mut HashMap<String, String>,
    literal_fns: &'a mut HashMap<String, String>,
    calls: &'a mut HashMap<String, Vec<String>>,
    reads_env: &'a mut HashMap<String, Vec<String>>,
    current_fn: Vec<String>,
}

/// The dotted path of a call's callee, when it has one.
fn callee_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

impl<'a> Collector<'a> {
    fn walk_use(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.walk_use(&p.tree, prefix);
                prefix.pop();
            }
            // `use std::process::Command as Proc` — the only form that renames.
            syn::UseTree::Rename(r) => {
                let mut full = prefix.clone();
                full.push(r.ident.to_string());
                self.aliases.insert(r.rename.to_string(), full.join("::"));
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.walk_use(item, prefix);
                }
            }
            syn::UseTree::Name(_) | syn::UseTree::Glob(_) => {}
        }
    }
}

/// The literal a body evaluates to, when the body is just that literal.
fn sole_string_literal(block: &syn::Block) -> Option<String> {
    if block.stmts.len() != 1 {
        return None;
    }
    let syn::Stmt::Expr(expr, None) = &block.stmts[0] else {
        return None;
    };
    string_literal(expr)
}

fn string_literal(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        // `"ffmpeg".to_string()` and friends still name the same program.
        syn::Expr::MethodCall(call) => match call.method.to_string().as_str() {
            "to_string" | "to_owned" | "into" => string_literal(&call.receiver),
            _ => None,
        },
        syn::Expr::Reference(r) => string_literal(&r.expr),
        _ => None,
    }
}

impl<'a> Collector<'a> {
    fn enter_fn(&mut self, name: String, block: &syn::Block) {
        if let Some(value) = sole_string_literal(block) {
            self.literal_fns.insert(name.clone(), value);
        }
        self.current_fn.push(name);
    }
}

impl<'ast, 'a> Visit<'ast> for Collector<'a> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let (Some(caller), Some(callee)) = (self.current_fn.last(), callee_name(&node.func)) {
            let caller = caller.clone();
            // `env::var("X")` inside a body makes that body environment-derived.
            if matches!(callee.as_str(), "var" | "var_os") {
                if let Some(syn::Expr::Lit(lit)) = node.args.first() {
                    if let syn::Lit::Str(name) = &lit.lit {
                        self.reads_env
                            .entry(caller.clone())
                            .or_default()
                            .push(name.value());
                    }
                }
            }
            self.calls.entry(caller).or_default().push(callee);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.walk_use(&node.tree, &mut Vec::new());
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        // `type Cmd = std::process::Command;`
        if let syn::Type::Path(path) = &*node.ty {
            let full = path
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            self.aliases.insert(node.ident.to_string(), full);
        }
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        if let Some(value) = string_literal(&node.expr) {
            self.constants.insert(node.ident.to_string(), value);
        }
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        if let Some(value) = string_literal(&node.expr) {
            self.constants.insert(node.ident.to_string(), value);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.enter_fn(node.sig.ident.to_string(), &node.block);
        syn::visit::visit_item_fn(self, node);
        self.current_fn.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.enter_fn(node.sig.ident.to_string(), &node.block);
        syn::visit::visit_impl_item_fn(self, node);
        self.current_fn.pop();
    }
}

pub struct ParsedFile {
    pub rel_path: String,
    pub parsed: syn::File,
    /// Under `tests/` or `benches/` rather than `src/`.
    pub test_role: bool,
    /// Module path of the file itself, e.g. `foo::bar` for `src/foo/bar.rs`.
    pub module_prefix: String,
}

/// Parses every `.rs` file under a package's source roots once, so both passes
/// read the same trees instead of parsing twice.
///
/// `roots` comes from the manifest's own targets rather than an assumed `src/`.
/// Not every workspace uses that layout — deno declares `ext/fs/lib.rs` — and
/// assuming it silently reported 81 packages holding 93 files.
pub fn parse_package(
    pkg_dir: &Path,
    root: &Path,
    roots: &[(PathBuf, bool)],
    nested: &[PathBuf],
) -> (Vec<ParsedFile>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut failures = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let _ = pkg_dir;
    for (dir, is_test) in roots {
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if nested.iter().any(|n| path.starts_with(n)) {
                continue; // belongs to a package nested inside this one
            }
            if !seen.insert(path.to_path_buf()) {
                continue; // two roots can overlap; a file belongs to one package once
            }
            let rel = crate::extract::rel(root, path);
            match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(
                |source| syn::parse_file(&source).map_err(|e| e.to_string()),
            ) {
                Ok(file) => parsed.push(ParsedFile {
                    module_prefix: crate::extract::module_prefix(dir, path),
                    rel_path: rel,
                    parsed: file,
                    test_role: *is_test,
                }),
                Err(e) => failures.push(format!("{rel}: {e}")),
            }
        }
    }
    (parsed, failures)
}
