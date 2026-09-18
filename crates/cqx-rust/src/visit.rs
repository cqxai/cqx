//! The syn visitor: turns one parsed file into nodes and edges.
//!
//! No name resolution. Everything here is provable from the syntax of a single
//! file, which is deliberate — every high-value edge kind (spawns, reads_env,
//! the effect family, unsafe) is syntactically visible, and skipping resolution
//! keeps a whole workspace scannable in seconds with no build.

use std::collections::HashMap;

use cqx_schema::{Edge, EdgeKind, Evidence, Id, Node, NodeKind, Writer};

use crate::prepass::PackageFacts;
use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;

pub struct FileVisitor<'a, W: std::io::Write> {
    pub out: &'a mut Writer<W>,
    /// Package this file belongs to, e.g. `pkg:engine`.
    pub package: Id,
    /// Path relative to the scan root, used for evidence and the file id.
    pub rel_path: String,
    pub file_id: Id,
    /// Module path of the file itself, e.g. `foo::bar` for `src/foo/bar.rs`.
    pub module_prefix: String,
    /// `mod x { .. }` blocks entered so far.
    module_stack: Vec<String>,
    /// Enclosing symbols; the last one owns any effect found.
    symbol_stack: Vec<Id>,
    /// Enclosing `impl`/`trait` names, which qualify a symbol's path.
    path_stack: Vec<String>,
    /// True for a file under `tests/` or `benches/`.
    pub test_role: bool,
    /// Depth of enclosing `#[cfg(test)]` modules.
    cfg_test_depth: usize,
    /// Package name (underscored) -> node id, for resolving `use` targets.
    pub known_packages: &'a HashMap<String, Id>,
    /// Names this file renamed via `use … as X` or `type X = …`.
    aliases: HashMap<String, String>,
    /// `let` bindings of the function currently being walked.
    bindings: HashMap<String, Binding>,
    /// Constants and literal-returning functions collected from the package.
    facts: &'a PackageFacts,
}

impl<'a, W: std::io::Write> FileVisitor<'a, W> {
    pub fn new(
        out: &'a mut Writer<W>,
        package: Id,
        rel_path: String,
        module_prefix: String,
        test_role: bool,
        known_packages: &'a HashMap<String, Id>,
        facts: &'a PackageFacts,
    ) -> Self {
        let file_id = Id::file(&rel_path);
        let aliases = facts.aliases_for(&rel_path);
        FileVisitor {
            out,
            package,
            file_id,
            rel_path,
            module_prefix,
            module_stack: Vec::new(),
            symbol_stack: Vec::new(),
            path_stack: Vec::new(),
            test_role,
            cfg_test_depth: 0,
            known_packages,
            aliases,
            bindings: HashMap::new(),
            facts,
        }
    }

    fn evidence(&self, span: Span) -> Evidence {
        let (start, end) = (span.start(), span.end());
        Evidence::spanning(
            &self.rel_path,
            [start.line as u32, end.line as u32],
            [start.column as u32, end.column as u32],
        )
    }

    /// The node an effect or a definition belongs to: the innermost enclosing
    /// symbol, or the file when we are at top level.
    fn container(&self) -> Id {
        self.symbol_stack
            .last()
            .cloned()
            .unwrap_or_else(|| self.file_id.clone())
    }

    fn module_path(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if !self.module_prefix.is_empty() {
            parts.push(&self.module_prefix);
        }
        for m in &self.module_stack {
            parts.push(m);
        }
        parts.join("::")
    }

    /// `sym:<pkg>::<module>::<impl>::<name>` — path-addressed, never
    /// line-addressed, and unique: two methods of different types in one module
    /// must not collide.
    fn symbol_id(&self, name: &str) -> Id {
        let pkg = self.package.0.trim_start_matches("pkg:");
        let mut parts: Vec<&str> = vec![pkg];
        let module = self.module_path();
        if !module.is_empty() {
            parts.push(&module);
        }
        for component in &self.path_stack {
            parts.push(component);
        }
        parts.push(name);
        Id::symbol(&parts.join("::"))
    }

    fn emit_symbol(&mut self, name: &str, lang_kind: &str, span: Span) -> Id {
        self.emit_symbol_with(name, lang_kind, span, None)
    }

    fn emit_symbol_with(
        &mut self,
        name: &str,
        lang_kind: &str,
        span: Span,
        body: Option<&syn::Block>,
    ) -> Id {
        let id = self.symbol_id(name);
        let start = span.start().line as u32;
        let end = span.end().line as u32;
        let mut node = Node::new(id.clone(), NodeKind::Symbol)
            .attr("name", name)
            .attr("lang:kind", lang_kind)
            .attr("lines", end.saturating_sub(start) + 1);
        if let Some(block) = body {
            if !self.is_test_context() {
                if let Some(hash) = body_fingerprint(block) {
                    node = node.attr("body", hash);
                }
            }
        }
        if self.is_test_context() {
            node = node.attr("role", "test");
        }
        let node = node;
        let container = self.container();
        let ev = self.evidence(span);
        let _ = self.out.node(node);
        let _ = self
            .out
            .edge(Edge::new(EdgeKind::Contains, container, id.clone()).evidence(ev));
        id
    }

    /// Records an effect against the innermost enclosing symbol.
    ///
    /// Test code is tagged rather than dropped: a spawn in a test is a real
    /// fact about the repository, it is just not a fact about the product, and
    /// a query that cannot tell them apart reports numbers nobody can act on.
    /// Declares a referenced type and returns its id.
    ///
    /// The node carries the type exactly as written, so a signature can be
    /// rendered back verbatim, plus the outermost name on its own — `Result`
    /// out of `Result<Output, Diag>` — because attribution works on names, not
    /// on spellings.
    fn type_node(&mut self, ty: &syn::Type) -> Id {
        let text = type_text(ty);
        let id = Id::type_ref(&text);
        let mut node = Node::new(id.clone(), NodeKind::Type).attr("text", text.as_str());
        if let Some(base) = base_name(ty) {
            node = node.attr("base", base);
        }
        let fresh = self.out.node(node).is_ok();
        let _ = fresh;
        self.emit_type_args(&id, ty);
        id
    }

    /// Records each generic argument as a type in its own right.
    fn emit_type_args(&mut self, owner: &Id, ty: &syn::Type) {
        match ty {
            syn::Type::Reference(r) => self.emit_type_args(owner, &r.elem),
            syn::Type::Paren(p) => self.emit_type_args(owner, &p.elem),
            syn::Type::Slice(s) => {
                let child = self.type_node(&s.elem);
                self.type_arg_edge(owner, child, 0);
            }
            syn::Type::Array(a) => {
                let child = self.type_node(&a.elem);
                self.type_arg_edge(owner, child, 0);
            }
            syn::Type::Tuple(t) => {
                for (position, elem) in t.elems.iter().enumerate() {
                    let child = self.type_node(elem);
                    self.type_arg_edge(owner, child, position);
                }
            }
            syn::Type::Path(p) => {
                let Some(segment) = p.path.segments.last() else {
                    return;
                };
                let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
                    return;
                };
                let mut position = 0;
                for arg in &args.args {
                    // Lifetimes and const generics say nothing about ownership.
                    if let syn::GenericArgument::Type(inner) = arg {
                        let child = self.type_node(inner);
                        self.type_arg_edge(owner, child, position);
                        position += 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn type_arg_edge(&mut self, owner: &Id, child: Id, position: usize) {
        if owner == &child {
            return; // a type is not its own argument
        }
        let _ = self.out.edge(
            Edge::new(EdgeKind::TypeArg, owner.clone(), child)
                .attr("position", position as u64),
        );
    }

    /// Records a function's declared parameters and return type.
    ///
    /// A signature is a promise the compiler already checked, so reading it
    /// needs no inference — which is why `fn f(a: &str, b: &str)` can be called
    /// out as a swap hazard without knowing anything about `a` or `b`.
    fn emit_signature(&mut self, symbol: &Id, sig: &syn::Signature) {
        for (position, input) in sig.inputs.iter().enumerate() {
            let syn::FnArg::Typed(typed) = input else {
                continue; // `self` carries no declared type of interest
            };
            let name = match &*typed.pat {
                syn::Pat::Ident(ident) => ident.ident.to_string(),
                _ => "_".to_string(),
            };
            let ty = self.type_node(&typed.ty);
            let ev = self.evidence(typed.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::Param, symbol.clone(), ty)
                    .attr("name", name)
                    .attr("position", position as u64)
                    .evidence(ev),
            );
        }
        if let syn::ReturnType::Type(_, ty) = &sig.output {
            let id = self.type_node(ty);
            let ev = self.evidence(ty.span());
            let _ = self
                .out
                .edge(Edge::new(EdgeKind::Returns, symbol.clone(), id).evidence(ev));
        }
    }

    /// Records a type's declared fields, so two types can be compared by shape
    /// and one concept's many spellings can be counted.
    fn emit_fields(&mut self, symbol: &Id, name: &str, fields: &syn::Fields) {
        let owner = Id::type_ref(name);
        let _ = self
            .out
            .node(Node::new(owner.clone(), NodeKind::Type).attr("text", name));
        let _ = self
            .out
            .edge(Edge::new(EdgeKind::Defines, symbol.clone(), owner.clone()));
        for (position, field) in fields.iter().enumerate() {
            let field_name = field
                .ident
                .as_ref()
                .map(|i| i.to_string())
                .unwrap_or_else(|| position.to_string());
            let ty = self.type_node(&field.ty);
            let ev = self.evidence(field.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::HasField, owner.clone(), ty)
                    .attr("name", field_name)
                    .evidence(ev),
            );
        }
    }

    fn effect(&mut self, kind: EdgeKind, to: Id, op: &str, span: Span) {
        self.effect_via(kind, to, op, span, "literal");
    }

    fn effect_via(&mut self, kind: EdgeKind, to: Id, op: &str, span: Span, via: &'static str) {
        self.effect_provenance(kind, to, op, span, Provenance::new(via));
    }

    /// Declares a capability node before an edge points at it. An edge whose
    /// endpoint has no node is dropped silently by anything that loads the
    /// stream, so the extractor never emits one.
    fn declare_capability(&mut self, id: &Id) {
        if let Some(name) = id.0.strip_prefix("cap:") {
            let name = name.to_string();
            let _ = self
                .out
                .node(Node::new(id.clone(), NodeKind::Capability).attr("name", name));
        }
    }

    fn effect_provenance(
        &mut self,
        kind: EdgeKind,
        to: Id,
        op: &str,
        span: Span,
        from_where: Provenance,
    ) {
        let Provenance {
            via,
            source_fn,
            env_var,
        } = from_where;
        self.declare_capability(&to);
        let from = self.container();
        let ev = self.evidence(span);
        let mut edge = Edge::new(kind, from, to).attr("op", op).evidence(ev);
        // Anything that took a step beyond the source as written says so.
        if matches!(via, "const" | "fn" | "binding") {
            edge = edge.inferred(via);
        } else if matches!(via, "unresolved" | "env") {
            edge = edge.attr("via", via);
        }
        if let Some(f) = source_fn {
            edge = edge.attr("source_fn", f);
        }
        if let Some(var) = env_var {
            edge = edge.attr("env_var", var);
        }
        if self.is_test_context() {
            edge = edge.attr("role", "test");
        }
        let _ = self.out.edge(edge);
    }

    /// Who calls what, by name.
    ///
    /// The name and not the definition: resolving a call to the function it
    /// reaches needs a compiler, and this extractor has `syn`. What it can say
    /// is that this function calls something called `enforce_read`, which is
    /// enough for the question a policy actually asks — does every operation
    /// on this boundary reach a check. A workspace with two `enforce_read`s
    /// conflates them, so the edge is recorded as inferred and carries the
    /// path as written for anyone who needs to tell them apart.
    ///
    /// Method calls are recorded too, with no receiver type to go on. That
    /// costs precision and buys the majority of calls in idiomatic Rust:
    /// `self.enforce_read(path)` was previously invisible.
    fn calls(&mut self, name: &str, path: &str, _span: Span, form: &'static str) {
        // Only a name this workspace declares somewhere. A call into the
        // standard library reaches code this extractor never reads, so the
        // edge would lead nowhere — and `clone`, `push` and `to_string` are
        // more than half of every call written.
        if name.is_empty() || !self.facts.defined.contains(name) {
            return;
        }
        let to = Id::external(name);
        let _ = self
            .out
            .node(Node::new(to.clone(), NodeKind::External).attr("name", name));
        let from = self.container();
        // No evidence, deliberately. A call edge answers "does this function
        // reach something named X", and the finding that follows points at the
        // function, not at the call. Carrying a file, a line and a column for
        // every call site instead of one edge per pair is what made makepad's
        // facts three hundred megabytes; the Writer folds the repeats away on
        // its own once there is nothing to tell them apart.
        let mut edge = Edge::new(EdgeKind::Calls, from, to)
            .attr("form", form)
            .inferred("name");
        if path != name {
            edge = edge.attr("path", path);
        }
        if self.is_test_context() {
            edge = edge.attr("role", "test");
        }
        let _ = self.out.edge(edge);
    }

    /// What a macro was handed.
    ///
    /// `syn` parses a macro invocation into an opaque token stream and the
    /// default walk stops there, so everything inside one is invisible — deka
    /// writes 7,536 macro invocations, sixteen of which call `env::var`,
    /// `process::exit` or `Command::new` in their arguments. None of those
    /// effects were recorded.
    ///
    /// The arguments are re-parsed as expressions and walked like any others.
    /// A macro whose body is not a list of expressions — `matches!(x, Some(_))`,
    /// a `quote!` block — simply fails to parse and is left alone, which is
    /// the right outcome: this reads what was written, and still expands
    /// nothing.
    fn walk_macro(&mut self, mac: &syn::Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        // Generated code is not this function's behaviour, and a token stream
        // meant for a parser is not a list of expressions.
        if matches!(name.as_str(), "quote" | "quote_spanned" | "matches" | "cfg_if" | "include_str" | "include_bytes") {
            return;
        }
        type Args = syn::punctuated::Punctuated<syn::Expr, syn::Token![,]>;
        let Ok(args) = mac.parse_body_with(Args::parse_terminated) else {
            return;
        };
        // A structured format assembled by interpolation. The values decide
        // the structure, which is the injection family in one line.
        if matches!(name.as_str(), "format" | "write" | "writeln" | "print" | "println" | "eprint" | "eprintln") {
            if let Some(syn::Expr::Lit(lit)) = args.first() {
                if let syn::Lit::Str(text) = &lit.lit {
                    let body = text.value();
                    if args.len() > 1 && looks_structured(&body) {
                        let to = Id::capability("json");
                        self.declare_capability(&to);
                        let from = self.container();
                        let ev = self.evidence(mac.span());
                        let mut edge = Edge::new(EdgeKind::Interpolates, from, to)
                            .attr("via", name.as_str())
                            .evidence(ev);
                        if self.is_test_context() {
                            edge = edge.attr("role", "test");
                        }
                        let _ = self.out.edge(edge);
                    }
                }
            }
        }
        for arg in &args {
            syn::visit::visit_expr(self, arg);
        }
    }

    /// Rewrites a callee path through this file's aliases, so `Proc::new`
    /// reads as `std::process::Command::new` when `Proc` was renamed.
    fn canonical_path(&self, path: &str) -> String {
        let Some((head, rest)) = path.split_once("::") else {
            return self.aliases.get(path).cloned().unwrap_or_else(|| path.to_string());
        };
        match self.aliases.get(head) {
            Some(full) => format!("{full}::{rest}"),
            None => path.to_string(),
        }
    }

    /// Resolves a call argument to the string it will hold, following at most
    /// one constant or one literal-returning function.
    ///
    /// Returns the value and how it was reached; `None` means the extractor
    /// genuinely cannot say, which is itself worth reporting.
    fn resolve_string_arg(&self, expr: &syn::Expr) -> Resolved {
        match expr {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Str(s) => Resolved::Value(s.value(), "literal"),
                _ => Resolved::Unresolved,
            },
            syn::Expr::Reference(r) => self.resolve_string_arg(&r.expr),
            syn::Expr::Try(t) => self.resolve_string_arg(&t.expr),
            syn::Expr::Path(p) => {
                let Some(name) = p.path.segments.last().map(|s| s.ident.to_string()) else {
                    return Resolved::Unresolved;
                };
                // A local binding first: `let dsc = dsc_bin()?;` then
                // `Command::new(dsc)` is the shape real code actually uses.
                match self.bindings.get(&name) {
                    Some(Binding::Literal(v)) => return Resolved::Value(v.clone(), "binding"),
                    Some(Binding::Call(f)) => {
                        return match self.facts.literal_fns.get(f) {
                            Some(v) => Resolved::Value(v.clone(), "fn"),
                            None => Resolved::From(f.clone()),
                        }
                    }
                    None => {}
                }
                match self.facts.constants.get(&name) {
                    Some(v) => Resolved::Value(v.clone(), "const"),
                    None => Resolved::Unresolved,
                }
            }
            syn::Expr::Call(call) => {
                let Some(path) = callee_path(&call.func) else {
                    return Resolved::Unresolved;
                };
                let name = path.rsplit("::").next().unwrap_or(&path).to_string();
                match self.facts.literal_fns.get(&name) {
                    Some(v) => Resolved::Value(v.clone(), "fn"),
                    None => Resolved::From(name),
                }
            }
            syn::Expr::MethodCall(call) => match call.method.to_string().as_str() {
                "to_string" | "to_owned" | "into" | "as_str" | "clone" | "as_ref"
                | "to_path_buf" | "display" => self.resolve_string_arg(&call.receiver),
                _ => Resolved::Unresolved,
            },
            _ => Resolved::Unresolved,
        }
    }

    /// Turns a resolution outcome into the node id and the attributes that
    /// explain how it was reached.
    fn spawn_target(&self, resolved: Resolved) -> (String, Provenance) {
        match resolved {
            Resolved::Value(v, via) => (v, Provenance::new(via)),
            Resolved::From(f) => {
                // A target derived from the environment is the interesting
                // case: it means the program launched is chosen at runtime by
                // whoever sets that variable.
                let env = self.facts.reads_env.get(&f).map(|vars| {
                    let mut names: Vec<String> = vars.clone();
                    names.sort();
                    names.dedup();
                    let mut listed = names.join(",");
                    // Propagation stops naming variables once a function is
                    // plainly environment derived, so say that these are some
                    // of them rather than letting the list read as all of them.
                    if self.facts.truncated.contains(&f) {
                        listed.push_str(",…");
                    }
                    listed
                });
                match env {
                    Some(vars) if !vars.is_empty() => (
                        "<dynamic>".to_string(),
                        Provenance {
                            via: "env",
                            source_fn: Some(f),
                            env_var: Some(vars),
                        },
                    ),
                    _ => (
                        "<dynamic>".to_string(),
                        Provenance {
                            via: "fn",
                            source_fn: Some(f),
                            env_var: None,
                        },
                    ),
                }
            }
            Resolved::Unresolved => {
                ("<dynamic>".to_string(), Provenance::new("unresolved"))
            }
        }
    }

    /// Records a lint being switched off. `scope` separates the two cases that
    /// matter: an item-level allow is a local decision, a crate-wide one hides
    /// every future occurrence including code nobody has written yet.
    fn record_silencing(&mut self, owner: Id, attrs: &[syn::Attribute], scope: &'static str) {
        for attr in attrs {
            let kind = if attr.path().is_ident("allow") {
                "allow"
            } else if attr.path().is_ident("expect") {
                "expect"
            } else {
                continue;
            };
            let mut lints: Vec<String> = Vec::new();
            let mut documented = false;
            let _ = attr.parse_nested_meta(|meta| {
                let path = meta
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                // `reason = "..."` documents the suppression; it is not a lint,
                // and counting it as one inflated deno's total by 99.
                if path == "reason" {
                    documented = true;
                } else {
                    lints.push(path);
                }
                Ok(())
            });
            let ev = self.evidence(attr.span());
            self.declare_capability(&Id::capability("lint"));
            for lint in lints {
                // How broadly the lint is drawn matters far more than how many
                // there are: naming one lint is a decision, switching off
                // `clippy::all` hides every rule including future ones.
                let breadth = if matches!(lint.as_str(), "all" | "clippy::all" | "warnings") {
                    "broad"
                } else {
                    "named"
                };
                let mut edge = Edge::new(EdgeKind::Silences, owner.clone(), Id::capability("lint"))
                    .attr("lint", lint)
                    .attr("scope", scope)
                    .attr("breadth", breadth)
                    .attr("form", kind)
                    .evidence(ev.clone());
                if documented {
                    edge = edge.attr("documented", true);
                }
                let _ = self.out.edge(edge);
            }
        }
    }

    fn is_test_context(&self) -> bool {
        self.test_role || self.cfg_test_depth > 0
    }

    /// Flattens a `use` tree into the leaf paths it brings into scope.
    fn use_leaves(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                Self::use_leaves(&p.tree, prefix, out);
                prefix.pop();
            }
            syn::UseTree::Name(n) => {
                let mut full = prefix.clone();
                full.push(n.ident.to_string());
                out.push(full);
            }
            syn::UseTree::Rename(r) => {
                let mut full = prefix.clone();
                full.push(r.ident.to_string());
                out.push(full);
            }
            syn::UseTree::Glob(_) => {
                let mut full = prefix.clone();
                full.push("*".to_string());
                out.push(full);
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    Self::use_leaves(item, prefix, out);
                }
            }
        }
    }
}

/// The bare name of whatever this expression calls, when it calls something.
fn called_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Call(call) => {
            let path = callee_path(&call.func)?;
            Some(path.rsplit("::").next().unwrap_or(&path).to_string())
        }
        syn::Expr::MethodCall(call) => Some(call.method.to_string()),
        // `let _ = check(..)?;` and `let _ = &check(..);` still called it.
        syn::Expr::Try(t) => called_name(&t.expr),
        syn::Expr::Reference(r) => called_name(&r.expr),
        syn::Expr::Await(a) => called_name(&a.base),
        _ => None,
    }
}

/// A format string that is building JSON rather than a message.
fn looks_structured(text: &str) -> bool {
    // Braces are how a format string names its holes, so a doubled brace is
    // the only way one can contain a literal `{` — which is what building an
    // object by hand looks like.
    (text.contains("{{") && text.contains("}}"))
        || text.contains("\":")
        || text.contains("\"{}\"")
}

/// An arm body that does nothing at all: `{}`, `()`, or `{ () }`.
fn does_nothing(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Tuple(t) => t.elems.is_empty(),
        syn::Expr::Block(b) => {
            b.block.stmts.is_empty()
                || (b.block.stmts.len() == 1
                    && matches!(&b.block.stmts[0], syn::Stmt::Expr(e, None) if does_nothing(e)))
        }
        _ => false,
    }
}

/// A fingerprint of a function body, for finding copies.
///
/// Exact token match rather than a structural one: two bodies with the same
/// tokens were copied, whereas two bodies with the same *shape* are often just
/// two functions doing similar work, which is not a defect. Short bodies are
/// skipped because getters and one-line delegations collide constantly and mean
/// nothing.
fn body_fingerprint(block: &syn::Block) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let text = block.to_token_stream().to_string();
    let normalised: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalised.len() < 220 {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalised.hash(&mut hasher);
    Some(format!("{:x}", hasher.finish()))
}

/// Reads a `let` initialiser as a binding, when it is one syntax can follow.
fn binding_of(expr: &syn::Expr) -> Option<Binding> {
    match expr {
        syn::Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Str(s) => Some(Binding::Literal(s.value())),
            _ => None,
        },
        syn::Expr::Try(t) => binding_of(&t.expr),
        syn::Expr::Reference(r) => binding_of(&r.expr),
        // `let tool = match tool_path() { … }` — the value still comes from
        // the scrutinee, whichever arm produced it.
        syn::Expr::Match(m) => binding_of(&m.expr),
        syn::Expr::Call(call) => {
            let path = callee_path(&call.func)?;
            Some(Binding::Call(
                path.rsplit("::").next().unwrap_or(&path).to_string(),
            ))
        }
        syn::Expr::MethodCall(call) => match call.method.to_string().as_str() {
            "ok" | "unwrap" | "clone" | "to_path_buf" | "to_owned" | "into" => {
                binding_of(&call.receiver)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Where a value came from, travelling together because they answer one
/// question: not "what is the target" but "who decided it".
#[derive(Default)]
struct Provenance {
    /// How the target was reached: a literal, a constant, a hop, or not at all.
    via: &'static str,
    /// The function the value came out of, when it came out of one.
    source_fn: Option<String>,
    /// The environment variables that function consults, when it consults any.
    env_var: Option<String>,
}

impl Provenance {
    fn new(via: &'static str) -> Provenance {
        Provenance {
            via,
            ..Provenance::default()
        }
    }
}

/// What a `let` bound a name to, as far as syntax can tell.
#[derive(Clone, Debug)]
enum Binding {
    Literal(String),
    /// Bound to the result of a named function, which is where provenance
    /// continues.
    Call(String),
}

/// The outcome of asking what string a call argument will hold.
enum Resolved {
    /// A value we can name, and how we got to it.
    Value(String, &'static str),
    /// Cannot be named, but it comes from this function — which is usually more
    /// informative than the value would be.
    From(String),
    Unresolved,
}

/// The outermost type name, with generics and references stripped:
/// `&ast::Expr<'a>` is an `Expr`. Attribution matches on this, because a crate
/// defines `Expr`, not `&ast::Expr<'a>`.
fn base_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => base_name(&r.elem),
        syn::Type::Paren(p) => base_name(&p.elem),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// Renders a declared type as normalised source text, so that two spellings of
/// the same type compare equal.
fn type_text(ty: &syn::Type) -> String {
    let raw = ty.to_token_stream().to_string();
    let mut out = String::with_capacity(raw.len());
    let mut prev = ' ';
    for ch in raw.chars() {
        if ch == ' ' {
            prev = ch;
            continue;
        }
        // keep a space only where removing it would join two identifiers
        if prev == ' ' && !out.is_empty() {
            let last = out.chars().last().unwrap_or(' ');
            if (last.is_alphanumeric() || last == '_') && (ch.is_alphanumeric() || ch == '_') {
                out.push(' ');
            }
        }
        out.push(ch);
        prev = ch;
    }
    out
}

/// Renders an expression's callee as a dotted path, e.g. `std::process::exit`.
/// Returns `None` for calls we cannot name syntactically (closures, fields).
fn callee_path(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(p) => Some(
            p.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::"),
        ),
        _ => None,
    }
}

impl<'ast, 'a, W: std::io::Write> Visit<'ast> for FileVisitor<'a, W> {
    fn visit_file(&mut self, node: &'ast syn::File) {
        // `#![allow(..)]` at the top of a crate root or module file.
        let file_id = self.file_id.clone();
        self.record_silencing(file_id, &node.attrs, "crate");
        syn::visit::visit_file(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let id = self.emit_symbol_with(
            &node.sig.ident.to_string(),
            "fn",
            node.span(),
            Some(&node.block),
        );
        self.record_silencing(id.clone(), &node.attrs, "item");
        self.emit_signature(&id, &node.sig);
        if node.sig.unsafety.is_some() {
            self.declare_capability(&Id::capability("unsafe"));
            let ev = self.evidence(node.sig.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::UnsafeAt, id.clone(), Id::capability("unsafe"))
                    .attr("form", "unsafe fn")
                    .evidence(ev),
            );
        }
        self.symbol_stack.push(id);
        let outer = std::mem::take(&mut self.bindings);
        syn::visit::visit_item_fn(self, node);
        self.bindings = outer;
        self.symbol_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let id = self.emit_symbol_with(
            &node.sig.ident.to_string(),
            "method",
            node.span(),
            Some(&node.block),
        );
        self.record_silencing(id.clone(), &node.attrs, "item");
        self.emit_signature(&id, &node.sig);
        if node.sig.unsafety.is_some() {
            self.declare_capability(&Id::capability("unsafe"));
            let ev = self.evidence(node.sig.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::UnsafeAt, id.clone(), Id::capability("unsafe"))
                    .attr("form", "unsafe fn")
                    .evidence(ev),
            );
        }
        self.symbol_stack.push(id);
        let outer = std::mem::take(&mut self.bindings);
        syn::visit::visit_impl_item_fn(self, node);
        self.bindings = outer;
        self.symbol_stack.pop();
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let id = self.emit_symbol(&node.ident.to_string(), "struct", node.span());
        let name = node.ident.to_string();
        self.emit_fields(&id, &name, &node.fields);
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        let id = self.emit_symbol(&node.ident.to_string(), "enum", node.span());
        let name = node.ident.to_string();
        let owner = Id::type_ref(&name);
        let _ = self
            .out
            .node(Node::new(owner.clone(), NodeKind::Type).attr("text", name.as_str()));
        let _ = self
            .out
            .edge(Edge::new(EdgeKind::Defines, id.clone(), owner));
        for variant in &node.variants {
            self.emit_fields(&id, &format!("{name}::{}", variant.ident), &variant.fields);
        }
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        let name = node.ident.to_string();
        let id = self.emit_symbol(&name, "trait", node.span());
        self.symbol_stack.push(id);
        self.path_stack.push(name);
        syn::visit::visit_item_trait(self, node);
        self.path_stack.pop();
        self.symbol_stack.pop();
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.emit_symbol(&node.ident.to_string(), "const", node.span());
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        let name = node.ident.to_string();
        let id = self.emit_symbol(&name, "static", node.span());
        // `static mut` is shared mutable state with no synchronisation: an
        // effect, not a definition detail.
        if matches!(node.mutability, syn::StaticMutability::Mut(_)) {
            self.declare_capability(&Id::capability("unsafe"));
            let ev = self.evidence(node.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::UnsafeAt, id, Id::capability("unsafe"))
                    .attr("form", "static mut")
                    .evidence(ev),
            );
        }
        syn::visit::visit_item_static(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let type_name = match &*node.self_ty {
            syn::Type::Path(p) => p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_else(|| "?".to_string()),
            _ => "?".to_string(),
        };
        let name = match &node.trait_ {
            Some((_, path, _)) => {
                let trait_name = path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_else(|| "?".to_string());
                format!("<{type_name} as {trait_name}>")
            }
            None => format!("impl {type_name}"),
        };
        let id = self.emit_symbol(&name, "impl", node.span());
        if node.unsafety.is_some() {
            self.declare_capability(&Id::capability("unsafe"));
            let ev = self.evidence(node.span());
            let _ = self.out.edge(
                Edge::new(EdgeKind::UnsafeAt, id.clone(), Id::capability("unsafe"))
                    .attr("form", "unsafe impl")
                    .evidence(ev),
            );
        }
        self.symbol_stack.push(id);
        self.path_stack.push(type_name);
        syn::visit::visit_item_impl(self, node);
        self.path_stack.pop();
        self.symbol_stack.pop();
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        // `let _ = check(..)` — called, and its answer thrown away. Only the
        // wildcard: `let _x = ..` keeps the value and may well be used later,
        // and a bare `check(..);` statement cannot be told from a function
        // that returns nothing without knowing its type.
        if matches!(node.pat, syn::Pat::Wild(_)) {
            if let Some(init) = &node.init {
                if let Some(name) = called_name(&init.expr) {
                    let to = Id::external(&name);
                    let _ = self
                        .out
                        .node(Node::new(to.clone(), NodeKind::External).attr("name", name.as_str()));
                    let from = self.container();
                    let ev = self.evidence(node.span());
                    let mut edge = Edge::new(EdgeKind::Discards, from, to).evidence(ev);
                    if self.is_test_context() {
                        edge = edge.attr("role", "test");
                    }
                    let _ = self.out.edge(edge);
                }
            }
        }
        if let syn::Pat::Ident(ident) = &node.pat {
            if let Some(init) = &node.init {
                let name = ident.ident.to_string();
                if let Some(binding) = binding_of(&init.expr) {
                    self.bindings.insert(name, binding);
                }
            }
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let is_cfg_test = node.attrs.iter().any(is_cfg_test_attr);
        if is_cfg_test {
            self.cfg_test_depth += 1;
        }
        self.module_stack.push(node.ident.to_string());
        syn::visit::visit_item_mod(self, node);
        self.module_stack.pop();
        if is_cfg_test {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut leaves = Vec::new();
        FileVisitor::<W>::use_leaves(&node.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            let Some(root) = leaf.first() else { continue };
            let target = match root.as_str() {
                // Intra-package: resolving these needs a module graph, which is
                // not v0's job. The containment spine already relates them.
                "crate" | "self" | "super" => continue,
                other => match self.known_packages.get(other) {
                    Some(id) => id.clone(),
                    None => Id::external(other),
                },
            };
            let ev = self.evidence(node.span());
            // Only declare the node when it is genuinely external. A sibling
            // package already has a Package node, and whichever emitter runs
            // first would otherwise decide its kind.
            if target.0.starts_with("ext:") {
                let _ = self.out.node(
                    Node::new(target.clone(), NodeKind::External).attr("name", root.as_str()),
                );
            }
            let _ = self.out.edge(
                Edge::new(EdgeKind::Imports, self.file_id.clone(), target)
                    .attr("path", leaf.join("::"))
                    .evidence(ev),
            );
        }
        syn::visit::visit_item_use(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        self.walk_macro(&node.mac);
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_stmt_macro(&mut self, node: &'ast syn::StmtMacro) {
        self.walk_macro(&node.mac);
        syn::visit::visit_stmt_macro(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        // A dispatch and its fallback. Whether an empty fallback is a hole
        // depends on what the other arms do, which is a question for a rule;
        // what the extractor can say is that the fallback is there and empty.
        if let Some(arm) = node.arms.iter().find(|a| matches!(a.pat, syn::Pat::Wild(_))) {
            let from = self.container();
            let ev = self.evidence(node.span());
            let mut edge = Edge::new(EdgeKind::DefaultArm, from, Id::capability("dispatch"))
                .attr("arms", node.arms.len() as u64)
                .attr("empty", does_nothing(&arm.body))
                .evidence(ev);
            if self.is_test_context() {
                edge = edge.attr("role", "test");
            }
            self.declare_capability(&Id::capability("dispatch"));
            let _ = self.out.edge(edge);
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        self.declare_capability(&Id::capability("unsafe"));
        let from = self.container();
        let ev = self.evidence(node.span());
        let _ = self.out.edge(
            Edge::new(EdgeKind::UnsafeAt, from, Id::capability("unsafe"))
                .attr("form", "unsafe block")
                .evidence(ev),
        );
        syn::visit::visit_expr_unsafe(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some(raw) = callee_path(&node.func) {
            let path = self.canonical_path(&raw);
            let span = node.span();
            let bare = path.rsplit("::").next().unwrap_or(&path).to_string();
            self.calls(&bare, &path, span, "path");
            // A process boundary. At the binary zoom level these are most of
            // the edges that exist, and every one of them matters.
            if path.ends_with("Command::new") {
                let resolved = match node.args.first() {
                    Some(arg) => self.resolve_string_arg(arg),
                    None => Resolved::Unresolved,
                };
                let (name, from_where) = self.spawn_target(resolved);
                let to = Id::process(&name);
                let _ = self
                    .out
                    .node(Node::new(to.clone(), NodeKind::Process).attr("name", name.as_str()));
                self.effect_provenance(EdgeKind::Spawns, to, &path, span, from_where);
            } else if path.ends_with("env::var")
                || path.ends_with("env::var_os")
                || path.ends_with("env::set_var")
                || path.ends_with("env::remove_var")
            {
                let resolved = match node.args.first() {
                    Some(arg) => self.resolve_string_arg(arg),
                    None => Resolved::Unresolved,
                };
                let (name, from_where) = self.spawn_target(resolved);
                let to = Id::env_var(&name);
                let _ = self
                    .out
                    .node(Node::new(to.clone(), NodeKind::EnvVar).attr("name", name.as_str()));
                self.effect_provenance(EdgeKind::ReadsEnv, to, &path, span, from_where);
            } else if path.ends_with("process::exit") || path.ends_with("process::abort") {
                self.effect(EdgeKind::EffectExec, Id::capability("exec"), &path, span);
            } else if path.contains("fs::") {
                self.effect(EdgeKind::EffectFs, Id::capability("fs"), &path, span);
            } else if is_net_path(&path) {
                self.effect(EdgeKind::EffectNet, Id::capability("net"), &path, span);
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // Receivers are chased by the default walk; only the method name is
        // new information here.
        let name = node.method.to_string();
        self.calls(&name, &name, node.span(), "method");
        // What was asked of the subprocess, not only which one. A shell is
        // not dangerous; a shell handed a string nobody checked is, and the
        // two are indistinguishable without this.
        //
        // Attributed to the enclosing function rather than to a particular
        // `Command`: following a builder through a local binding needs
        // dataflow, and the question a rule asks — does this function spawn a
        // shell and pass it something unresolved — is answered without it.
        if matches!(name.as_str(), "arg" | "args") {
            for arg in &node.args {
                let (value, from_where) = self.spawn_target(self.resolve_string_arg(arg));
                let to = Id::capability("exec");
                self.declare_capability(&to);
                let from = self.container();
                let ev = self.evidence(node.span());
                let mut edge = Edge::new(EdgeKind::SpawnArg, from, to)
                    .attr("via", from_where.via)
                    .evidence(ev);
                if from_where.via == "literal" {
                    edge = edge.attr("value", value.as_str());
                }
                if self.is_test_context() {
                    edge = edge.attr("role", "test");
                }
                let _ = self.out.edge(edge);
            }
        }
        if matches!(name.as_str(), "spawn" | "output" | "status") {
            if let syn::Expr::Call(inner) = &*node.receiver {
                if callee_path(&inner.func)
                    .map(|p| p.ends_with("Command::new"))
                    .unwrap_or(false)
                {
                    // Already recorded at Command::new; recording the launch
                    // again would double-count the same edge.
                    syn::visit::visit_expr_method_call(self, node);
                    return;
                }
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// `#[cfg(test)]`, the marker that a module is not part of the product.
fn is_cfg_test_attr(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("test") {
            found = true;
        }
        Ok(())
    });
    found
}

fn is_net_path(path: &str) -> bool {
    const NET: [&str; 6] = [
        "TcpStream",
        "TcpListener",
        "UdpSocket",
        "reqwest",
        "hyper",
        "ureq",
    ];
    NET.iter().any(|n| path.contains(n))
}
