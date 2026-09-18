//! A cheap pass over a package, collecting the few facts that let the main
//! visitor resolve a name it would otherwise report as `<dynamic>`.
//!
//! Everything here is still syntax. None of it needs types, and all of it runs
//! in the same order of magnitude as the main pass — which is the point: most
//! unresolved names in real code are one alias, one constant or one hop away,
//! not a question about the type system.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use cqx_vfs::Vfs;
use syn::visit::Visit;

/// What one reader must tell the others.
///
/// Four of the five maps are keyed by a bare name and mean something across the
/// whole workspace, so a reader holding one slice of it cannot resolve against
/// them alone. The fifth — aliases — is keyed by file and only ever read back
/// for that same file, so it stays where it was found and never crosses.
///
/// That distinction is most of the cost: makepad has six thousand files, and
/// sending every file's aliases to every reader would be a large multiple of
/// what actually needs sharing.
/// How many environment variables are worth naming against one function.
///
/// The question a score asks of this map is whether a function is environment
/// derived at all, and a truncated set answers it exactly as a whole one does.
/// The names are only ever rendered into a finding, and a finding that names
/// two hundred variables has told a reader nothing — while the closure that
/// produced them is the largest thing cqx ever holds: makepad's ran to four
/// and a half million pairs and ninety-five megabytes, against three hundred
/// and ninety variables that actually exist in the workspace.
///
/// A function that reads this many directly keeps every one of them. The bound
/// only refuses what propagation would pile on top.
const MOST_VARS: usize = 8;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Shared {
    pub constants: HashMap<String, String>,
    pub literal_fns: HashMap<String, String>,
    pub calls: HashMap<String, Vec<String>>,
    pub reads_env: HashMap<String, Vec<String>>,
    /// The functions whose list stopped at [`MOST_VARS`], so a finding can say
    /// that it is showing some of them rather than all of them.
    #[serde(default)]
    pub truncated: BTreeSet<String>,
    /// Every function name this workspace declares.
    ///
    /// What makes a call worth writing down. A call to `to_string` reaches the
    /// standard library, which this extractor does not read and has nothing to
    /// say about; a call to `enforce_read` may reach a function three crates
    /// away, and following that is the whole point of recording calls at all.
    /// Recording both put three hundred megabytes of `clone` and `push` into
    /// makepad's fact stream to carry the few thousand edges anyone queries.
    #[serde(default)]
    pub defined: BTreeSet<String>,
}

impl Shared {
    /// Takes on everything another reader said.
    ///
    /// Later wins, exactly as a later file wins within one pass, so merging in
    /// reading order reproduces reading them together. The two that accumulate
    /// rather than replace are extended, and repeats dropped, because a name
    /// seen twice says nothing new.
    pub fn merge(&mut self, other: Shared) {
        self.constants.extend(other.constants);
        self.literal_fns.extend(other.literal_fns);
        for (name, called) in other.calls {
            let held = self.calls.entry(name).or_default();
            for one in called {
                if !held.contains(&one) {
                    held.push(one);
                }
            }
        }
        for (name, vars) in other.reads_env {
            let held = self.reads_env.entry(name).or_default();
            for var in vars {
                if !held.contains(&var) {
                    held.push(var);
                }
            }
        }
        self.truncated.extend(other.truncated);
        self.defined.extend(other.defined);
    }

    /// Walks `calls` to a fixpoint so a function that calls a function that
    /// reads the environment is itself marked environment-derived.
    ///
    /// Done once, over everything, because a chain of calls routinely crosses
    /// a crate boundary — deka's spawn target is three named hops away in two
    /// different crates — and following it through part of a workspace would
    /// find fewer of them.
    pub fn resolve(&mut self) {
        propagate_env(&mut self.calls, &mut self.reads_env, &mut self.truncated);
    }
}

/// One walk to a fixpoint, over whichever maps are handed to it, so the two
/// forms of the same facts do not carry two copies of it.
fn propagate_env(
    calls: &mut HashMap<String, Vec<String>>,
    reads_env: &mut HashMap<String, Vec<String>>,
    truncated: &mut BTreeSet<String>,
) {
    let calls = &*calls;
    // Who names whom, the other way round.
    //
    // A round only ever gives a caller what its callees already have, so the
    // only names that can gain anything are the callers of something that
    // reads the environment — a few hundred of them in makepad, against a
    // hundred thousand functions. Walking every function eight times to find
    // that out was eleven of the twenty-nine seconds the browser spent on it.
    let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (caller, callees) in calls.iter() {
        for callee in callees {
            callers_of
                .entry(callee.as_str())
                .or_default()
                .push(caller.as_str());
        }
    }

    for _ in 0..8 {
        let mut candidates: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for name in reads_env.keys() {
            for caller in callers_of.get(name.as_str()).into_iter().flatten() {
                if seen.insert(caller) {
                    candidates.push(caller);
                }
            }
        }

        // Everything gained lands at the end of the round, which is what the
        // copy of the map was for: what a caller is measured against is the
        // state the round began in. Holding the gains is the same rule without
        // eight deep copies of every name and variable in the workspace.
        let mut gains: Vec<(&str, Vec<String>)> = Vec::new();
        for caller in candidates {
            let Some(callees) = calls.get(caller) else {
                continue;
            };
            let known = reads_env.get(caller);
            let mut gained: Vec<String> = Vec::new();
            // In the order this caller names them, because that is the order
            // the variables are reported in.
            for callee in callees {
                let Some(vars) = reads_env.get(callee) else {
                    continue;
                };
                for var in vars {
                    if known.is_some_and(|v| v.contains(var)) {
                        continue;
                    }
                    if !gained.contains(var) {
                        gained.push(var.clone());
                    }
                }
            }
            if !gained.is_empty() {
                gains.push((caller, gained));
            }
        }
        if gains.is_empty() {
            break;
        }
        let owned: Vec<(String, Vec<String>)> = gains
            .into_iter()
            .map(|(caller, gained)| (caller.to_string(), gained))
            .collect();
        for (caller, gained) in owned {
            let held = reads_env.entry(caller.clone()).or_default();
            for var in gained {
                // Past the bound the answer is already "yes, and from more of
                // them than anyone wants listed". Refusing the rest is what
                // keeps the closure from squaring: the same functions come out
                // marked, carrying fewer names each.
                if held.len() >= MOST_VARS {
                    truncated.insert(caller);
                    break;
                }
                held.push(var);
            }
        }
    }
}

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
    /// Those whose list stopped at [`MOST_VARS`]. See it for why there is one.
    pub truncated: BTreeSet<String>,
    /// Every function name this workspace declares. See [`Shared::defined`].
    pub defined: BTreeSet<String>,
}

impl PackageFacts {
    /// Collected across the whole workspace, because a value's provenance
    /// routinely crosses a crate boundary: deka's spawn target is three named
    /// hops away in two different crates.
    pub fn collect(files: &[&ParsedFile]) -> PackageFacts {
        let mut facts = PackageFacts::gather(files);
        facts.resolve();
        facts
    }

    /// What one set of files says, before anything is followed across the
    /// workspace.
    ///
    /// Separate from [`collect`] so the reading can be divided. Every map here
    /// is keyed by a bare name and filled by insertion, so two sets gathered
    /// apart and merged in the order they were read hold exactly what one pass
    /// over the same files in the same order would hold — and the following,
    /// which is a fixpoint over all of it, happens once afterwards.
    pub fn gather(files: &[&ParsedFile]) -> PackageFacts {
        let mut facts = PackageFacts::default();
        for ParsedFile { rel_path, parsed, .. } in files.iter().copied() {
            let mut file_aliases = HashMap::new();
            let mut collector = Collector {
                defined: &mut facts.defined,
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
        facts
    }

    /// What this reader must tell the others.
    pub fn shared(&self) -> Shared {
        Shared {
            constants: self.constants.clone(),
            literal_fns: self.literal_fns.clone(),
            calls: self.calls.clone(),
            reads_env: self.reads_env.clone(),
            truncated: self.truncated.clone(),
            defined: self.defined.clone(),
        }
    }

    /// The same, for a caller with no further use for the facts.
    ///
    /// Copying four maps of a large workspace is not free: makepad's cost the
    /// module the last of the four gigabytes wasm32 can address, and it failed
    /// where it had previously finished. Moving them costs nothing.
    pub fn into_shared(self) -> Shared {
        Shared {
            constants: self.constants,
            literal_fns: self.literal_fns,
            calls: self.calls,
            reads_env: self.reads_env,
            truncated: self.truncated,
            defined: self.defined,
        }
    }

    /// Takes on what every reader found, keeping its own aliases — those are
    /// file-scoped and were never anyone else's to know.
    pub fn adopt(&mut self, shared: Shared) {
        self.constants = shared.constants;
        self.literal_fns = shared.literal_fns;
        self.calls = shared.calls;
        self.reads_env = shared.reads_env;
        self.truncated = shared.truncated;
        self.defined = shared.defined;
    }

    /// Follows what was gathered to its conclusion.
    ///
    /// In place. Going out through a copy and back doubled the four maps at
    /// the moment the trees were also held, which on a large workspace is the
    /// difference between finishing and running out of address space.
    pub fn resolve(&mut self) {
        propagate_env(&mut self.calls, &mut self.reads_env, &mut self.truncated);
    }

    pub fn aliases_for(&self, rel_path: &str) -> HashMap<String, String> {
        self.aliases.get(rel_path).cloned().unwrap_or_default()
    }
}

struct Collector<'a> {
    defined: &'a mut BTreeSet<String>,
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
        self.defined.insert(name.clone());
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
    vfs: &Vfs,
    roots: &[(String, bool)],
    nested: &[String],
) -> (Vec<ParsedFile>, Vec<String>) {
    parse_package_watched(vfs, roots, nested, &|| {})
}

/// The same, telling someone each time a file is done.
///
/// Parsing is where the time goes — seven of makepad's nine seconds — and it
/// is the one part that can say how far along it is, because it is a loop over
/// a known number of files. Everything after it is a single pass over what was
/// parsed.
pub fn parse_package_watched(
    vfs: &Vfs,
    roots: &[(String, bool)],
    nested: &[String],
    done: &dyn Fn(),
) -> (Vec<ParsedFile>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut failures = Vec::new();
    for (dir, is_test, path) in files_of(vfs, roots, nested) {
        {
            let path = path.as_str();
            let (dir, is_test) = (&dir, &is_test);
            let Some(source) = vfs.read(path) else {
                continue;
            };
            done();
            match syn::parse_file(source) {
                Ok(file) => parsed.push(ParsedFile {
                    module_prefix: crate::extract::module_prefix(dir, path),
                    rel_path: path.to_string(),
                    parsed: file,
                    test_role: *is_test,
                }),
                Err(e) => failures.push(format!("{path}: {e}")),
            }
        }
    }
    (parsed, failures)
}

/// Which files a package owns, before any of them is read.
///
/// Separate from the parsing so the number can be known in advance. A reader
/// waiting two minutes wants a fraction, and a fraction needs a denominator
/// that means something: makepad holds six thousand `.rs` files and fewer than
/// three thousand of them belong to a crate. Counting the rest made a finished
/// analysis look stalled at forty-five per cent.
pub fn files_of(
    vfs: &Vfs,
    roots: &[(String, bool)],
    nested: &[String],
) -> Vec<(String, bool, String)> {
    let mut found = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (dir, is_test) in roots {
        for path in vfs.under(dir) {
            if !path.ends_with(".rs") {
                continue;
            }
            if nested.iter().any(|n| crate::extract::is_inside(path, n)) {
                continue; // belongs to a package nested inside this one
            }
            if !seen.insert(path.to_string()) {
                continue; // two roots can overlap; a file belongs to one package once
            }
            found.push((dir.clone(), *is_test, path.to_string()));
        }
    }
    found
}
