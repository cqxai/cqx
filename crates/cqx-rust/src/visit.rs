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
        Evidence::at(
            &self.rel_path,
            span.start().line as u32,
            span.end().line as u32,
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
    fn type_node(&mut self, ty: &syn::Type) -> Id {
        let text = type_text(ty);
        let id = Id::type_ref(&text);
        let _ = self
            .out
            .node(Node::new(id.clone(), NodeKind::Type).attr("text", text.as_str()));
        id
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

    fn effect_via(&mut self, kind: EdgeKind, to: Id, op: &str, span: Span, via: &str) {
        self.effect_provenance(kind, to, op, span, via, None, None);
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
        via: &str,
        source_fn: Option<String>,
        env_var: Option<String>,
    ) {
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
    fn spawn_target(&self, resolved: Resolved) -> (String, &'static str, Option<String>, Option<String>) {
        match resolved {
            Resolved::Value(v, via) => (v, via, None, None),
            Resolved::From(f) => {
                // A target derived from the environment is the interesting
                // case: it means the program launched is chosen at runtime by
                // whoever sets that variable.
                let env = self.facts.reads_env.get(&f).map(|vars| {
                    let mut names: Vec<String> = vars.clone();
                    names.sort();
                    names.dedup();
                    names.join(",")
                });
                match env {
                    Some(vars) if !vars.is_empty() => {
                        ("<dynamic>".to_string(), "env", Some(f), Some(vars))
                    }
                    _ => ("<dynamic>".to_string(), "fn", Some(f), None),
                }
            }
            Resolved::Unresolved => ("<dynamic>".to_string(), "unresolved", None, None),
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
            // A process boundary. At the binary zoom level these are most of
            // the edges that exist, and every one of them matters.
            if path.ends_with("Command::new") {
                let resolved = match node.args.first() {
                    Some(arg) => self.resolve_string_arg(arg),
                    None => Resolved::Unresolved,
                };
                let (name, via, source_fn, env_var) = self.spawn_target(resolved);
                let to = Id::process(&name);
                let _ = self
                    .out
                    .node(Node::new(to.clone(), NodeKind::Process).attr("name", name.as_str()));
                self.effect_provenance(
                    EdgeKind::Spawns,
                    to,
                    &path,
                    span,
                    via,
                    source_fn,
                    env_var,
                );
            } else if path.ends_with("env::var")
                || path.ends_with("env::var_os")
                || path.ends_with("env::set_var")
                || path.ends_with("env::remove_var")
            {
                let resolved = match node.args.first() {
                    Some(arg) => self.resolve_string_arg(arg),
                    None => Resolved::Unresolved,
                };
                let (name, via, _, _) = self.spawn_target(resolved);
                let to = Id::env_var(&name);
                let _ = self
                    .out
                    .node(Node::new(to.clone(), NodeKind::EnvVar).attr("name", name.as_str()));
                self.effect_via(EdgeKind::ReadsEnv, to, &path, span, via);
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
