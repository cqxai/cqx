//! Go syntax facts, with file-scoped import resolution and lexical bindings.
//! Unknown types/provenance stay unknown; analysis never invokes a Go compiler.
use std::collections::BTreeMap;
use std::io::{self, Write};

use cqx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Writer};
use cqx_vfs::Vfs;
use gosyn::ast::*;
use gosyn::token::{LitKind, Operator, Token};

mod walk;

#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub packages: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unparsed: Vec<String>,
}

pub fn is_source(path: &str) -> bool {
    path.ends_with(".go")
}

/// Module roots travel with coordinator metadata, so a shard keeps its package
/// identity even when it does not contain go.mod itself.
pub fn modules(vfs: &Vfs) -> BTreeMap<String, String> {
    vfs.paths()
        .filter(|p| *p == "go.mod" || p.ends_with("/go.mod"))
        .filter_map(|p| {
            let name = vfs.read(p)?.lines().find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some("module"))
                    .then(|| words.next())
                    .flatten()
            })?;
            Some((
                p.strip_suffix("go.mod")?.into(),
                name.trim_matches('"').into(),
            ))
        })
        .collect()
}

pub fn run(vfs: &Vfs, out: impl Write) -> io::Result<Stats> {
    prepare(vfs, modules(vfs), &|| {})?.emit(vfs, out)
}

pub struct Prepared {
    files: Vec<ParsedFile>,
    skipped: Vec<(String, String)>,
    modules: BTreeMap<String, String>,
    layout: cqx_layout::Layout,
}

struct ParsedFile {
    path: String,
    ast: File,
    tokens: Vec<gosyn::LexicalToken>,
}

pub fn prepare(
    vfs: &Vfs,
    modules: BTreeMap<String, String>,
    tick: &dyn Fn(),
) -> io::Result<Prepared> {
    prepare_with_layout(
        vfs,
        modules,
        tick,
        cqx_layout::Layout::from_paths(vfs.paths()),
    )
}
pub fn prepare_with_layout(
    vfs: &Vfs,
    modules: BTreeMap<String, String>,
    tick: &dyn Fn(),
    layout: cqx_layout::Layout,
) -> io::Result<Prepared> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for path in vfs.paths().filter(|p| is_source(p)) {
        let source = vfs.read(path).unwrap_or_default();
        // Validate both stages before emitting any facts for this file. One
        // unsupported file must not prevent scoring the rest of the snapshot.
        let parsed = gosyn::parse_source(source).and_then(|ast| {
            gosyn::tokenize_source(source).map(|tokens| ParsedFile {
                path: path.into(),
                ast,
                tokens,
            })
        });
        tick();
        match parsed {
            Ok(file) => files.push(file),
            Err(error) => skipped.push((path.into(), error.to_string())),
        }
    }
    Ok(Prepared {
        files,
        skipped,
        modules,
        layout,
    })
}

impl Prepared {
    pub fn emit(&self, vfs: &Vfs, out: impl Write) -> io::Result<Stats> {
        let mut writer = Writer::new(out);
        writer.fact(&Fact::header("go", &vfs.label))?;
        for (path, reason) in &self.skipped {
            // The same file-node diagnostic used by TypeScript travels through
            // CLI, WASM and reader shards, without entering the denominator.
            writer.node(
                Node::new(Id::file(path), NodeKind::File)
                    .attr("path", path.clone())
                    .attr("language", "go")
                    .attr("skipped", reason.clone()),
            )?;
        }
        let mut packages = std::collections::BTreeSet::new();
        for ParsedFile { path, ast, tokens } in &self.files {
            let source = vfs.read(path).unwrap_or_default();
            let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let module = self
                .modules
                .iter()
                .filter(|(root, _)| path.starts_with(root.as_str()))
                .max_by_key(|(root, _)| root.len());
            let (root, name) = module
                .map(|(r, n)| (r.as_str(), n.as_str()))
                .unwrap_or(("", "snapshot"));
            let package = Id::package(&format!("go:{name}:{dir}:{}", ast.pkg_name.name));
            packages.insert(package.clone());
            writer.node(
                Node::new(package.clone(), NodeKind::Package)
                    .attr(
                        "name",
                        format!("{name}/{}", dir.strip_prefix(root).unwrap_or(dir)),
                    )
                    .attr("language", "go")
                    .attr("go:package", ast.pkg_name.name.clone()),
            )?;
            let directory = Id(format!("dir:go:{dir}"));
            writer.node(
                Node::new(directory.clone(), NodeKind::Directory)
                    .attr("path", dir)
                    .attr("language", "go"),
            )?;
            writer.edge(Edge::new(EdgeKind::Contains, package, directory.clone()))?;
            let file = Id::file(path);
            // cmd/ contains shipped binaries, internal/ contains product libraries.
            // Tests, third-party and generated code stay in the graph but are
            // excluded from scoring. A package named tool/tools is product code.
            let test = ast.comments.iter().any(|c| {
                c.pos < ast.pkg_name.pos
                    && matches!(
                        c.text.trim().trim_start_matches('/').trim(),
                        "go:build tools" | "go:build ignore" | "+build tools" | "+build ignore"
                    )
            }) || self.layout.is_test("go", path)
                || self.layout.excluded("go", path, source);
            writer.node(
                Node::new(file.clone(), NodeKind::File)
                    .attr("path", path.clone())
                    .attr("lines", source.lines().count() as u64)
                    .attr("language", "go")
                    .attr("role", if test { "test" } else { "product" }),
            )?;
            writer.edge(Edge::new(EdgeKind::Contains, directory, file.clone()))?;
            let offsets: Vec<usize> = source
                .char_indices()
                .map(|(i, _)| i)
                .chain([source.len()])
                .collect();
            let lines: Vec<usize> = std::iter::once(0)
                .chain(
                    source
                        .chars()
                        .enumerate()
                        .filter(|(_, c)| *c == '\n')
                        .map(|(i, _)| i + 1),
                )
                .collect();
            let mut extractor = Extractor {
                offsets: &offsets,
                lines: &lines,
                writer: &mut writer,
                source,
                path,
                file,
                current: None,
                ast,
                tokens,
                scopes: vec![BTreeMap::new()],
                test,
                main: ast.pkg_name.name == "main",
            };
            for import in &ast.imports {
                let package = unquote(&import.path.value);
                let alias = import
                    .name
                    .as_ref()
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| package.rsplit('/').next().unwrap_or("").into());
                extractor.bind(&alias, Binding::Import(package));
            }
            // Same-file functions are provable without type checking. Calls to
            // external/custom methods remain unknown rather than guessed by name.
            let custom_error = ast.decl.iter().any(|d| matches!(d, Declaration::Type(d) if d.specs.iter().any(|s| s.name.name == "error")));
            for decl in &ast.decl {
                match decl {
                    Declaration::Function(f) if f.recv.is_none() => extractor.bind(
                        &f.name.name,
                        Binding::Function(
                            results(&f.typ)
                                .into_iter()
                                .map(|error| {
                                    error
                                        && !custom_error
                                        && !f
                                            .typ
                                            .typ_params
                                            .list
                                            .iter()
                                            .any(|p| p.name.iter().any(|n| n.name == "error"))
                                })
                                .collect(),
                        ),
                    ),
                    Declaration::Variable(d) => {
                        for s in &d.specs {
                            for n in &s.name {
                                extractor.bind(&n.name, Binding::Unknown);
                            }
                        }
                    }
                    Declaration::Const(d) => {
                        for s in &d.specs {
                            for n in &s.name {
                                extractor.bind(&n.name, Binding::Unknown);
                            }
                        }
                    }
                    Declaration::Type(d) => {
                        for s in &d.specs {
                            extractor.bind(&s.name.name, Binding::Unknown);
                        }
                    }
                    _ => {}
                }
            }
            for comment in &ast.comments {
                let text = comment.text.trim().trim_start_matches('/').trim();
                if text.strip_prefix("nolint").is_some_and(|s| {
                    s.is_empty() || s.starts_with(':') || s.starts_with(char::is_whitespace)
                }) && !text
                    .split_once("//")
                    .is_some_and(|(_, reason)| !reason.trim().is_empty())
                {
                    extractor.rule(
                        "undocumented-suppressions",
                        EdgeKind::Silences,
                        comment.pos,
                        "nolint suppression has no reason",
                    )?;
                }
            }
            let globals = extractor.scopes.clone();
            for decl in &ast.decl {
                extractor.scopes = globals.clone();
                extractor.decl(decl)?;
            }
        }
        Ok(Stats {
            files: self.files.len(),
            packages: packages.len(),
            nodes: writer.nodes,
            edges: writer.edges,
            unparsed: self
                .skipped
                .iter()
                .map(|(path, reason)| format!("{path}: {reason}"))
                .collect(),
        })
    }
}

#[derive(Clone)]
enum Binding {
    Import(String),
    Function(Vec<bool>),
    Error,
    Env,
    Literal(String),
    Unknown,
}

struct Extractor<'a, W: Write> {
    writer: &'a mut Writer<W>,
    source: &'a str,
    path: &'a str,
    file: Id,
    current: Option<Id>,
    ast: &'a File,
    tokens: &'a [gosyn::LexicalToken],
    offsets: &'a [usize],
    lines: &'a [usize],
    scopes: Vec<BTreeMap<String, Binding>>,
    test: bool,
    main: bool,
}

fn unquote(s: &str) -> String {
    if s.starts_with('`') {
        s.trim_matches('`').replace('\r', "")
    } else {
        serde_json::from_str(s).unwrap_or_else(|_| s.trim_matches('"').into())
    }
}
fn ident(e: &Expression) -> Option<&str> {
    if let Expression::Ident(n) = inner(e) {
        Some(&n.name)
    } else {
        None
    }
}
fn inner(e: &Expression) -> &Expression {
    if let Expression::Paren(p) = e {
        inner(&p.expr)
    } else {
        e
    }
}
fn callee(e: &Expression) -> &Expression {
    match inner(e) {
        Expression::Index(x) => callee(&x.left),
        Expression::IndexList(x) => callee(&x.left),
        e => e,
    }
}
fn results(typ: &FuncType) -> Vec<bool> {
    typ.result
        .list
        .iter()
        .flat_map(|f| std::iter::repeat_n(ident(&f.typ) == Some("error"), f.name.len().max(1)))
        .collect()
}

impl<W: Write> Extractor<'_, W> {
    fn bind(&mut self, name: &str, binding: Binding) {
        if name != "_" {
            self.scopes
                .last_mut()
                .expect("scope")
                .insert(name.into(), binding);
        }
    }
    fn lookup(&self, name: &str) -> Binding {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name).cloned())
            .unwrap_or(Binding::Unknown)
    }
    fn builtin_error(&self, typ: &Expression) -> bool {
        ident(typ) == Some("error") && !self.scopes.iter().any(|s| s.contains_key("error"))
    }
    fn set(&mut self, name: &str, binding: Binding, define: bool) {
        if !define {
            for scope in self.scopes.iter_mut().rev() {
                if scope.contains_key(name) {
                    scope.insert(name.into(), binding);
                    return;
                }
            }
        }
        self.bind(name, binding);
    }
    fn api(&self, e: &Expression) -> Option<String> {
        let Expression::Selector(s) = inner(e) else {
            return None;
        };
        let Binding::Import(package) = self.lookup(ident(&s.x)?) else {
            return None;
        };
        Some(format!("{package}.{}", s.sel.name))
    }
    fn value(&self, e: &Expression) -> Binding {
        match inner(e) {
            Expression::BasicLit(l) if l.kind == LitKind::String => {
                Binding::Literal(unquote(&l.value))
            }
            Expression::Ident(n) => self.lookup(&n.name),
            Expression::Call(c) if self.api(&c.func).as_deref() == Some("os.Getenv") => {
                Binding::Env
            }
            Expression::Call(c) if self.api(&c.func).as_deref() == Some("os.ExpandEnv") => {
                Binding::Env
            }
            Expression::Operation(o) => {
                let left = self.value(&o.x);
                let right =
                    o.y.as_ref()
                        .map(|e| self.value(e))
                        .unwrap_or(Binding::Unknown);
                match (left, right) {
                    (Binding::Env, _) | (_, Binding::Env) => Binding::Env,
                    (Binding::Literal(left), Binding::Literal(right)) if o.op == Operator::Add => {
                        Binding::Literal(left + &right)
                    }
                    _ => Binding::Unknown,
                }
            }
            _ => Binding::Unknown,
        }
    }
    fn call_results(&self, e: &Expression) -> Vec<bool> {
        let Expression::Call(c) = inner(e) else {
            return Vec::new();
        };
        if let Some(name) = ident(callee(&c.func)) {
            if let Binding::Function(result) = self.lookup(name) {
                return result;
            }
        }
        match self.api(&c.func).as_deref() {
            Some(
                "os.Chdir"
                | "os.Remove"
                | "os.RemoveAll"
                | "os.Mkdir"
                | "os.MkdirAll"
                | "os.Rename"
                | "os.WriteFile"
                | "os.Setenv"
                | "os.Unsetenv"
                | "encoding/json.Unmarshal",
            ) => vec![true],
            Some(
                "os.Open" | "os.Create" | "os.ReadFile" | "encoding/json.Marshal" | "strconv.Atoi",
            ) => vec![false, true],
            _ => Vec::new(),
        }
    }
    fn evidence(&self, pos: usize) -> Evidence {
        // gosyn positions count Unicode scalars, not UTF-8 bytes.
        let line = self.lines.partition_point(|start| *start <= pos).max(1);
        let col = pos.saturating_sub(self.lines[line - 1]) as u32;
        Evidence {
            file: self.path.into(),
            line: [line as u32, line as u32],
            col: [col, col + 1],
            extractor: "go".into(),
            source: cqx_schema::Source::Static,
        }
    }
    fn rule(&mut self, rule: &str, kind: EdgeKind, pos: usize, what: &str) -> io::Result<()> {
        let target = Id::capability(&format!("go/{rule}"));
        self.writer.node(
            Node::new(target.clone(), NodeKind::Capability)
                .attr("name", rule)
                .attr("language", "go"),
        )?;
        self.writer.edge(
            Edge::new(
                kind,
                self.current.as_ref().unwrap_or(&self.file).clone(),
                target,
            )
            .evidence(self.evidence(pos))
            .attr("go:rule", rule)
            .attr("what", what)
            .attr("role", if self.test { "test" } else { "product" }),
        )
    }
    fn call(&mut self, call: &Call) -> io::Result<()> {
        let api = self.api(&call.func).unwrap_or_default();
        if !self.main
            && matches!(
                api.as_str(),
                "os.Exit" | "log.Fatal" | "log.Fatalf" | "log.Fatalln"
            )
        {
            self.rule(
                "exit-in-library",
                EdgeKind::Crosses,
                call.func.pos(),
                "process exit outside package main",
            )?;
        }
        let offset = usize::from(api == "os/exec.CommandContext");
        if matches!(api.as_str(), "os/exec.Command" | "os/exec.CommandContext") {
            if let Some(program) = call.args.get(offset) {
                if matches!(self.value(program), Binding::Env) {
                    self.rule(
                        "env-controlled-spawn",
                        EdgeKind::Spawns,
                        program.pos(),
                        "environment chooses the executable",
                    )?;
                }
                if let Binding::Literal(program) = self.value(program) {
                    let name = program.rsplit(['/', '\\']).next().unwrap_or(&program);
                    let shell = matches!(
                        name,
                        "sh" | "bash"
                            | "zsh"
                            | "dash"
                            | "ksh"
                            | "cmd"
                            | "cmd.exe"
                            | "powershell"
                            | "pwsh"
                    );
                    if shell && call.args.get(offset + 1).is_some_and(|arg| matches!(self.value(arg), Binding::Literal(flag) if matches!(flag.as_str(), "-c" | "-lc" | "/C" | "/c" | "-Command"))) {
                        self.rule("shell-invocation", EdgeKind::Spawns, call.func.pos(), "shell interprets a command string")?;
                        if call.args.get(offset + 2).is_some_and(|arg| !matches!(self.value(arg), Binding::Literal(_))) { self.rule("shell-argument-unchecked", EdgeKind::Spawns, call.func.pos(), "shell command is not a resolved string literal")?; }
                    }
                }
            }
        }
        if api == "fmt.Sprintf" && call.args.len() > 1 {
            if let Binding::Literal(format) = self.value(&call.args[0]) {
                if json_template(&format) {
                    self.rule(
                        "hand-built-json",
                        EdgeKind::Interpolates,
                        call.func.pos(),
                        "JSON object assembled with fmt.Sprintf",
                    )?;
                }
            }
        }
        let target = Id::external(&format!(
            "go:{}",
            if api.is_empty() {
                type_name(&call.func)
            } else {
                api.clone()
            }
        ));
        self.writer.node(
            Node::new(target.clone(), NodeKind::External)
                .attr(
                    "name",
                    if api.is_empty() {
                        type_name(&call.func)
                    } else {
                        api
                    },
                )
                .attr("language", "go"),
        )?;
        self.writer.edge(
            Edge::new(
                EdgeKind::Calls,
                self.current.as_ref().unwrap_or(&self.file).clone(),
                target,
            )
            .evidence(self.evidence(call.func.pos())),
        )?;
        self.expr(&call.func)?;
        for arg in &call.args {
            self.expr(arg)?;
        }
        Ok(())
    }
    fn function(
        &mut self,
        typ: &FuncType,
        recv: Option<&FieldList>,
        body: Option<&BlockStmt>,
    ) -> io::Result<()> {
        self.scopes.push(BTreeMap::new());
        for field in recv
            .into_iter()
            .chain([&typ.typ_params, &typ.params, &typ.result])
            .flat_map(|list| &list.list)
        {
            for name in &field.name {
                self.bind(
                    &name.name,
                    if self.builtin_error(&field.typ) {
                        Binding::Error
                    } else if let Expression::TypeFunction(f) = &field.typ {
                        Binding::Function(results(f))
                    } else {
                        Binding::Unknown
                    },
                );
            }
        }
        if let Some(body) = body {
            self.block(body)?;
        }
        self.scopes.pop();
        Ok(())
    }
    fn decl(&mut self, decl: &Declaration) -> io::Result<()> {
        match decl {
            Declaration::Function(f) => {
                let name = f
                    .recv
                    .as_ref()
                    .map(|r| {
                        format!(
                            "{}.{}",
                            r.list
                                .first()
                                .map(|f| type_name(&f.typ))
                                .unwrap_or_default(),
                            f.name.name
                        )
                    })
                    .unwrap_or_else(|| f.name.name.clone());
                let id = Id::symbol(&format!("go:{}::{name}", self.path));
                let mut node = Node::new(id.clone(), NodeKind::Symbol)
                    .attr("name", f.name.name.clone())
                    .attr("lang:kind", "func")
                    .attr("language", "go");
                if let Some(body) = &f.body {
                    let tokens: Vec<_> = self
                        .tokens
                        .iter()
                        .filter(|t| {
                            t.start >= body.pos.0
                                && t.end <= body.pos.1 + 1
                                && !matches!(
                                    t.token,
                                    Token::Comment(_) | Token::Operator(Operator::SemiColon)
                                )
                        })
                        .map(|t| {
                            format!(
                                "{:?}:{}",
                                t.token.kind(),
                                &self.source[self.offsets[t.start]..self.offsets[t.end]]
                            )
                        })
                        .collect();
                    if tokens.len() >= 40 {
                        use std::hash::{Hash, Hasher};
                        let mut hash = std::collections::hash_map::DefaultHasher::new();
                        tokens.hash(&mut hash);
                        node = node.attr("body", format!("{:016x}", hash.finish()));
                    }
                }
                self.writer.node(node)?;
                let mut ev = self.evidence(f.typ.pos);
                if let Some(body) = &f.body {
                    ev.line[1] = self.evidence(body.pos.1).line[0];
                }
                self.writer.edge(
                    Edge::new(EdgeKind::Contains, self.file.clone(), id.clone()).evidence(ev),
                )?;
                let old = self.current.replace(id.clone());
                for (fields, kind) in [
                    (&f.typ.params, EdgeKind::Param),
                    (&f.typ.result, EdgeKind::Returns),
                ] {
                    for field in &fields.list {
                        let text = type_name(&field.typ);
                        let ty = Id::type_ref(&format!("go:{text}"));
                        self.writer.node(
                            Node::new(ty.clone(), NodeKind::Type)
                                .attr("name", text)
                                .attr("language", "go"),
                        )?;
                        self.writer.edge(
                            Edge::new(kind, id.clone(), ty)
                                .evidence(self.evidence(field.typ.pos())),
                        )?;
                    }
                }
                self.function(&f.typ, f.recv.as_ref(), f.body.as_ref())?;
                self.current = old;
            }
            Declaration::Variable(d) => {
                for spec in &d.specs {
                    self.variables(&spec.name, spec.typ.as_ref(), &spec.values)?;
                }
            }
            Declaration::Const(d) => {
                for spec in &d.specs {
                    self.variables(&spec.name, spec.typ.as_ref(), &spec.values)?;
                }
            }
            Declaration::Type(d) => {
                for spec in &d.specs {
                    self.expr(&spec.typ)?;
                }
            }
        }
        Ok(())
    }
    fn variables(
        &mut self,
        names: &[Ident],
        typ: Option<&Expression>,
        values: &[Expression],
    ) -> io::Result<()> {
        let errors = if values.len() == 1 {
            self.call_results(&values[0])
        } else {
            Vec::new()
        };
        let bindings: Vec<Binding> = names
            .iter()
            .enumerate()
            .map(|(i, _)| {
                if typ.is_some_and(|e| self.builtin_error(e)) || errors.get(i) == Some(&true) {
                    Binding::Error
                } else {
                    values
                        .get(i)
                        .map(|e| self.value(e))
                        .unwrap_or(Binding::Unknown)
                }
            })
            .collect();
        if values.len() == 1 {
            let left: Vec<Expression> = names.iter().cloned().map(Expression::Ident).collect();
            self.discarded(&values[0], Some(&left))?;
        } else {
            for (name, value) in names.iter().zip(values) {
                if name.name == "_" {
                    self.discarded(value, None)?;
                }
            }
        }
        for value in values {
            self.expr(value)?;
        }
        for (name, binding) in names.iter().zip(bindings) {
            self.bind(&name.name, binding);
        }

        Ok(())
    }
}

/// Source-independent spelling keeps type and method IDs stable when a file
/// moves lines. Debug AST output includes positions and cannot be an identity.
fn type_name(e: &Expression) -> String {
    let list = |items: &[Expression]| items.iter().map(type_name).collect::<Vec<_>>().join(", ");
    match e {
        Expression::Ident(n) => n.name.clone(),
        Expression::BasicLit(l) => l.value.clone(),
        Expression::Selector(s) => format!("{}.{}", type_name(&s.x), s.sel.name),
        Expression::TypePointer(p) => format!("*{}", type_name(&p.typ)),
        Expression::Star(p) => format!("*{}", type_name(&p.right)),
        Expression::TypeSlice(s) => format!("[]{}", type_name(&s.typ)),
        Expression::TypeArray(a) => format!("[{}]{}", type_name(&a.len), type_name(&a.typ)),
        Expression::TypeMap(m) => format!("map[{}]{}", type_name(&m.key), type_name(&m.val)),
        Expression::TypeChannel(c) => format!(
            "{}{}",
            match c.dir {
                Some(ChanMode::Send) => "chan<- ",
                Some(ChanMode::Recv) => "<-chan ",
                None => "chan ",
            },
            type_name(&c.typ)
        ),
        Expression::TypeFunction(f) => function_name(f),
        Expression::TypeStruct(s) => format!(
            "struct {{ {} }}",
            s.fields
                .iter()
                .map(field_name)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        Expression::TypeInterface(i) => format!(
            "interface {{ {} }}",
            i.methods
                .list
                .iter()
                .map(field_name)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        Expression::Index(i) => format!("{}[{}]", type_name(&i.left), type_name(&i.index)),
        Expression::IndexList(i) => format!("{}[{}]", type_name(&i.left), list(&i.indices)),
        Expression::Ellipsis(e) => format!(
            "...{}",
            e.elt.as_ref().map(|e| type_name(e)).unwrap_or_default()
        ),
        Expression::Paren(p) => format!("({})", type_name(&p.expr)),
        Expression::Operation(o) => {
            let op: &'static str = o.op.into();
            if let Some(y) = &o.y {
                format!("{} {op} {}", type_name(&o.x), type_name(y))
            } else {
                format!("{op}{}", type_name(&o.x))
            }
        }
        Expression::List(items) => list(items),
        Expression::Call(c) => format!(
            "{}({}{})",
            type_name(&c.func),
            list(&c.args),
            if c.dots.is_some() { "..." } else { "" }
        ),
        Expression::Slice(s) => {
            let indices: Vec<_> = s
                .index
                .iter()
                .map(|e| e.as_ref().map(|e| type_name(e)).unwrap_or_default())
                .collect();
            format!(
                "{}[{}]",
                type_name(&s.left),
                indices[..if s.index[2].is_some() { 3 } else { 2 }].join(":")
            )
        }
        Expression::TypeAssert(a) => format!(
            "{}.({})",
            type_name(&a.left),
            a.right
                .as_ref()
                .map(|e| type_name(e))
                .unwrap_or_else(|| "type".into())
        ),
        Expression::Range(r) => format!("range {}", type_name(&r.right)),
        Expression::FuncLit(f) => function_name(&f.typ),
        Expression::CompositeLit(c) => format!("{}{}", type_name(&c.typ), literal_name(&c.val)),
    }
}
fn field_name(f: &Field) -> String {
    let names = f
        .name
        .iter()
        .map(|n| n.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}{}{}{}",
        names,
        if names.is_empty() { "" } else { " " },
        type_name(&f.typ),
        f.tag
            .as_ref()
            .map(|t| format!(" {}", t.value))
            .unwrap_or_default()
    )
}
fn function_name(f: &FuncType) -> String {
    let fields = |v: &FieldList| v.list.iter().map(field_name).collect::<Vec<_>>().join(", ");
    format!(
        "func{}({}){}",
        if f.typ_params.list.is_empty() {
            String::new()
        } else {
            format!("[{}]", fields(&f.typ_params))
        },
        fields(&f.params),
        if f.result.list.is_empty() {
            String::new()
        } else {
            format!(" ({})", fields(&f.result))
        }
    )
}
fn literal_name(v: &LiteralValue) -> String {
    fn element(e: &Element) -> String {
        match e {
            Element::Expr(e) => type_name(e),
            Element::LitValue(v) => literal_name(v),
        }
    }
    format!(
        "{{{}}}",
        v.values
            .iter()
            .map(|e| format!(
                "{}{}",
                e.key
                    .as_ref()
                    .map(|k| format!("{}: ", element(k)))
                    .unwrap_or_default(),
                element(&e.val)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Validate the surrounding JSON grammar, not just a leading brace: shell
/// completion scripts routinely start with `{` and contain format directives.
fn json_template(format: &str) -> bool {
    let mut candidate = String::new();
    let mut chars = format.chars().peekable();
    let (mut quoted, mut escaped, mut directives) = (false, false, 0);
    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.peek() == Some(&'%') {
                chars.next();
                candidate.push('%');
                continue;
            }
            let mut verb = None;
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    verb = Some(c);
                    break;
                }
                if !matches!(c, '[' | ']' | '.' | '+' | '-' | '#' | ' ' | '*')
                    && !c.is_ascii_digit()
                {
                    return false;
                }
            }
            if !verb.is_some_and(|c| "vTtbcdoOqxXUeEfFgGsp".contains(c)) {
                return false;
            }
            directives += 1;
            candidate.push_str(if quoted { "value" } else { "0" });
            continue;
        }
        candidate.push(c);
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        }
    }
    directives > 0
        && matches!(
            serde_json::from_str::<serde_json::Value>(&candidate),
            Ok(serde_json::Value::Object(_))
        )
}
