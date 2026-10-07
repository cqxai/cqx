//! Pure Rust TS/TSX and JS/JSX extraction. No filesystem or compiler subprocess.
use std::collections::BTreeSet;
use std::io::Write;

use cqx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Writer};
use cqx_vfs::Vfs;
use oxc_allocator::Allocator;
use oxc_ast::{ast::*, AstKind};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{IsGlobalReference, Semantic, SemanticBuilder};
use oxc_span::{GetSpan, SourceType, Span};

#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unparsed: Vec<String>,
}

pub use cqx_vfs::is_typescript_source as is_source;

fn is_test(path: &str) -> bool {
    path.split('/')
        .any(|s| matches!(s, "test" | "tests" | "__tests__"))
        || [".test.", ".spec."].iter().any(|s| path.contains(s))
}

/// Package entries include declared bins and scripts that directly run a source
/// file with Node, tsx or Bun. `index` by itself is often a library.
pub fn entry_files(vfs: &Vfs) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for path in vfs.paths().filter(|p| p.ends_with("package.json")) {
        let Some(value) = vfs
            .read(path)
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        else {
            continue;
        };
        let dir = path.strip_suffix("package.json").unwrap_or("");
        let paths: Vec<&str> = match &value["bin"] {
            serde_json::Value::String(s) => vec![s],
            serde_json::Value::Object(m) => m.values().filter_map(|v| v.as_str()).collect(),
            _ => Vec::new(),
        };
        let scripts = value["scripts"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(|v| v.as_str())
            .filter_map(|command| {
                let mut words = command.split_whitespace();
                let runner = words.next()?;
                let file = words.next()?;
                (matches!(runner, "node" | "tsx" | "bun") && is_source(file)).then_some(file)
            });
        for bin in paths.into_iter().chain(scripts) {
            result.insert(format!("{dir}{}", bin.trim_start_matches("./")));
        }
    }
    result
}

pub fn run(vfs: &Vfs, out: impl Write) -> Result<Stats, std::io::Error> {
    run_watched(vfs, out, &|| {})
}

pub fn run_watched(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> Result<Stats, std::io::Error> {
    run_with_entries(vfs, out, &entry_files(vfs), tick)
}

/// Entry metadata can come from the coordinator's full snapshot, so sharding
/// cannot change whether a process exit belongs to a declared binary.
pub fn run_with_entries(
    vfs: &Vfs,
    out: impl Write,
    bins: &BTreeSet<String>,
    tick: &dyn Fn(),
) -> Result<Stats, std::io::Error> {
    let mut writer = Writer::new(out);
    writer.fact(&Fact::header("typescript", &vfs.label))?;
    let package = Id::package("typescript:root");
    let mut stats = Stats::default();
    for path in vfs.paths().filter(|p| is_source(p)) {
        let source = vfs.read(path).unwrap_or_default();
        if excluded_source(path, source) {
            tick();
            continue;
        }
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            source,
            SourceType::from_path(path).expect("source extension checked"),
        )
        .with_config(oxc_parser::config::TokensParserConfig)
        .parse();
        tick();
        // A bad file cannot supply trustworthy facts, but must not prevent
        // scoring the rest of a mixed repository. Carry the diagnostic through
        // the fact stream so CLI, WASM and sharded reports all expose it.
        if let Some(diagnostic) = parsed.diagnostics.first() {
            skipped_file(&mut writer, &mut stats, path, &diagnostic.to_string())?;
            continue;
        }
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .with_check_syntax_error(true)
            .build(&parsed.program);
        if let Some(diagnostic) = semantic.diagnostics.first() {
            skipped_file(&mut writer, &mut stats, path, &diagnostic.to_string())?;
            continue;
        }
        writer.node(
            Node::new(package.clone(), NodeKind::Package)
                .attr("name", "TypeScript / JavaScript")
                .attr("language", "typescript"),
        )?;
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let directory = Id(format!("dir:typescript:{dir}"));
        writer.node(Node::new(directory.clone(), NodeKind::Directory).attr("path", dir))?;
        writer.edge(Edge::new(
            EdgeKind::Contains,
            package.clone(),
            directory.clone(),
        ))?;
        let file = Id::file(path);
        let test = is_test(path);
        writer.node(
            Node::new(file.clone(), NodeKind::File)
                .attr("path", path)
                .attr("lines", source.lines().count() as u64)
                .attr("language", "typescript")
                .attr("role", if test { "test" } else { "product" }),
        )?;
        writer.edge(Edge::new(EdgeKind::Contains, directory, file.clone()))?;
        let stem = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .split('.')
            .next()
            .unwrap_or("");
        let entry = bins.contains(path)
            || source.starts_with("#!")
            || path.split('/').any(|s| matches!(s, "bin" | "scripts"))
            || is_config(path)
            || (matches!(stem, "main" | "cli") && matches!(dir, "" | "src"));
        let mut extractor = Extractor {
            writer: &mut writer,
            source,
            path,
            file,
            semantic: &semantic.semantic,
            with_depth: 0,
            tokens: &parsed.tokens,
            symbols: Vec::new(),
            error: None,
            test,
            entry,
        };
        for comment in &parsed.program.comments {
            let text = comment.content_span().source_text(source).trim();
            let undocumented = text
                .strip_prefix("@ts-ignore")
                .is_some_and(|s| s.trim().trim_start_matches([':', '-']).trim().is_empty())
                || ([
                    "eslint-disable-next-line",
                    "eslint-disable-line",
                    "eslint-disable",
                ]
                .iter()
                .any(|p| {
                    text.strip_prefix(p)
                        .is_some_and(|s| s.is_empty() || s.starts_with(char::is_whitespace))
                }) && !text
                    .split_once("--")
                    .is_some_and(|(_, reason)| !reason.trim().is_empty()));
            if undocumented {
                extractor.rule(
                    "undocumented-suppressions",
                    EdgeKind::Silences,
                    comment.span,
                    "suppression has no reason",
                );
            }
        }
        extractor.visit_program(&parsed.program);
        if let Some(e) = extractor.error {
            return Err(e);
        }
        stats.files += 1;
    }
    stats.nodes = writer.nodes;
    stats.edges = writer.edges;
    Ok(stats)
}

struct Extractor<'s, 'a, 'w, W: Write> {
    writer: &'w mut Writer<W>,
    source: &'s str,
    path: &'s str,
    file: Id,
    semantic: &'s Semantic<'a>,
    with_depth: usize,
    tokens: &'s [oxc_parser::Token],
    symbols: Vec<Id>,
    error: Option<std::io::Error>,
    test: bool,
    entry: bool,
}

impl<W: Write> Extractor<'_, '_, '_, W> {
    fn record(&mut self, result: std::io::Result<()>) {
        if self.error.is_none() {
            self.error = result.err();
        }
    }
    fn evidence(&self, span: Span) -> Evidence {
        let start = &self.source[..span.start as usize];
        let end = &self.source[..span.end as usize];
        Evidence {
            file: self.path.into(),
            line: [
                start.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
                end.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
            ],
            col: [
                start.rsplit('\n').next().unwrap_or("").chars().count() as u32,
                end.rsplit('\n').next().unwrap_or("").chars().count() as u32,
            ],
            extractor: "typescript".into(),
            source: cqx_schema::Source::Static,
        }
    }
    fn container(&self) -> Id {
        self.symbols.last().unwrap_or(&self.file).clone()
    }
    fn rule(&mut self, rule: &str, kind: EdgeKind, span: Span, what: &str) {
        let target = Id::capability(&format!("typescript/{rule}"));
        let result = self
            .writer
            .node(Node::new(target.clone(), NodeKind::Capability).attr("name", rule));
        self.record(result);
        let edge = Edge::new(kind, self.container(), target)
            .evidence(self.evidence(span))
            .attr("typescript:rule", rule)
            .attr("what", what)
            .attr("role", if self.test { "test" } else { "product" });
        let result = self.writer.edge(edge);
        self.record(result);
    }
    fn global(&self, expr: &Expression<'_>, name: &str) -> bool {
        self.with_depth == 0
            && matches!(expr.get_inner_expression(), Expression::Identifier(id) if id.name == name && id.is_global_reference(self.semantic.scoping()))
    }
    fn node_process(&self, expr: &Expression<'_>) -> bool {
        if self.global(expr, "process") {
            return true;
        }
        let Expression::Identifier(id) = expr.get_inner_expression() else {
            return false;
        };
        let Some(reference) = id.reference_id.get() else {
            return false;
        };
        let Some(symbol) = self.semantic.scoping().get_reference(reference).symbol_id() else {
            return false;
        };
        self.semantic.nodes().iter().any(|n| {
            let AstKind::ImportDeclaration(import) = n.kind() else {
                return false;
            };
            matches!(import.source.value.as_str(), "process" | "node:process")
                && import.specifiers.iter().flatten().any(|specifier| {
                    let local = match specifier {
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => &s.local,
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => &s.local,
                        _ => return false,
                    };
                    local.symbol_id.get() == Some(symbol)
                })
        })
    }
    fn signature(&mut self, kind: EdgeKind, annotation: &TSTypeAnnotation<'_>) {
        let span = annotation.type_annotation.span();
        let text = span.source_text(self.source);
        let ty = Id::type_ref(text);
        let result = self
            .writer
            .node(Node::new(ty.clone(), NodeKind::Type).attr("text", text));
        self.record(result);
        let result = self
            .writer
            .edge(Edge::new(kind, self.container(), ty).evidence(self.evidence(span)));
        self.record(result);
    }
    fn call(&mut self, callee: &Expression<'_>, arguments: &[Argument<'_>], span: Span) {
        if self.global(callee, "eval")
            || (self.global(callee, "Function") && !constant_arguments(arguments))
        {
            self.rule(
                "dynamic-code",
                EdgeKind::UnsafeAt,
                span,
                "executes dynamically constructed code",
            );
        }
        if (self.global(callee, "setTimeout") || self.global(callee, "setInterval"))
            && arguments
                .first()
                .and_then(Argument::as_expression)
                .is_some_and(is_literal_string)
        {
            self.rule(
                "dynamic-code",
                EdgeKind::UnsafeAt,
                span,
                "timer executes a string as code",
            );
        }
        if let Expression::StaticMemberExpression(member) = callee.get_inner_expression() {
            if member.property.name == "exit" && self.node_process(&member.object) && !self.entry {
                self.rule(
                    "exit-in-library",
                    EdgeKind::EffectExec,
                    span,
                    "library ends the process",
                );
            }
        }
        let target = self.external(callee.span().source_text(self.source));
        let edge =
            Edge::new(EdgeKind::Calls, self.container(), target).evidence(self.evidence(span));
        let result = self.writer.edge(edge);
        self.record(result);
    }
    fn external(&mut self, name: &str) -> Id {
        let id = Id::external(name);
        let result = self
            .writer
            .node(Node::new(id.clone(), NodeKind::External).attr("name", name));
        self.record(result);
        id
    }
    fn symbol(&mut self, name: &str, span: Span, body: Option<Span>) {
        let id = Id(format!(
            "sym:typescript::{}::{name}@{}",
            self.path, span.start
        ));
        let mut node = Node::new(id.clone(), NodeKind::Symbol)
            .attr("name", name)
            .attr("lang:kind", "function")
            .attr("language", "typescript");
        if let Some(body) = body.filter(|_| !self.test) {
            if let Some(hash) = fingerprint(self.source, self.tokens, body) {
                node = node.attr("body", hash);
            }
        }
        let result = self.writer.node(node);
        self.record(result);
        // File containment is explicit so shared metrics can place declarations.
        let result = self.writer.edge(
            Edge::new(EdgeKind::Contains, self.file.clone(), id.clone())
                .evidence(self.evidence(span)),
        );
        self.record(result);
        self.symbols.push(id);
    }
}

impl<'a, W: Write> Visit<'a> for Extractor<'_, 'a, '_, W> {
    fn visit_with_statement(&mut self, it: &WithStatement<'a>) {
        // The object expression is evaluated before entering the dynamic scope.
        self.visit_expression(&it.object);
        self.with_depth += 1;
        self.visit_statement(&it.body);
        self.with_depth -= 1;
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        self.symbol(
            it.id
                .as_ref()
                .map(|i| i.name.as_str())
                .unwrap_or("anonymous"),
            it.span,
            it.body.as_ref().map(|b| b.span),
        );
        if let Some(annotation) = &it.return_type {
            self.signature(EdgeKind::Returns, annotation);
        }
        walk::walk_function(self, it, flags);
        self.symbols.pop();
    }
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.symbol(
            "arrow",
            it.span,
            match &it.body {
                ArrowFunctionBody::FunctionBody(b) => Some(b.span),
                _ => None,
            },
        );
        if let Some(annotation) = &it.return_type {
            self.signature(EdgeKind::Returns, annotation);
        }
        walk::walk_arrow_function_expression(self, it);
        self.symbols.pop();
    }
    fn visit_formal_parameter(&mut self, it: &FormalParameter<'a>) {
        if let Some(annotation) = &it.type_annotation {
            self.signature(EdgeKind::Param, annotation);
        }
        walk::walk_formal_parameter(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.call(&it.callee, &it.arguments, it.span);
        walk::walk_call_expression(self, it);
    }
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        if self.global(&it.callee, "Function") && !constant_arguments(&it.arguments) {
            self.rule(
                "dynamic-code",
                EdgeKind::UnsafeAt,
                it.span,
                "constructs code at runtime",
            );
        }
        walk::walk_new_expression(self, it);
    }
    fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
        if it.body.body.is_empty()
            && !self
                .semantic
                .comments_range(it.body.span.start..it.body.span.end)
                .any(|c| !c.content_span().source_text(self.source).trim().is_empty())
        {
            self.rule(
                "swallowed-errors",
                EdgeKind::Discards,
                it.span,
                "empty catch discards the error without an explanation",
            );
        }
        walk::walk_catch_clause(self, it);
    }
    fn visit_ts_any_keyword(&mut self, it: &TSAnyKeyword) {
        self.rule(
            "any-density",
            EdgeKind::Silences,
            it.span,
            "explicit any bypasses type checking",
        );
    }
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if matches!(&it.left, AssignmentTarget::StaticMemberExpression(m) if m.property.name == "innerHTML")
            && !is_literal_string(&it.right)
        {
            self.rule(
                "dynamic-html",
                EdgeKind::UnsafeAt,
                it.span,
                "innerHTML receives a non-literal; review its sanitization",
            );
        }
        walk::walk_assignment_expression(self, it);
    }
    fn visit_jsx_attribute(&mut self, it: &JSXAttribute<'a>) {
        if matches!(&it.name, JSXAttributeName::Identifier(id) if id.name == "dangerouslySetInnerHTML")
        {
            if let Some(JSXAttributeValue::ExpressionContainer(c)) = &it.value {
                if let Some(expr) = c.expression.as_expression() {
                    let safe = match expr.get_inner_expression() {
                        Expression::ObjectExpression(o) => {
                            o.properties.len() == 1
                                && matches!(&o.properties[0], ObjectPropertyKind::ObjectProperty(p) if p.key.static_name().is_some_and(|k| k == "__html") && is_literal_string(&p.value))
                        }
                        _ => false,
                    };
                    if !safe {
                        self.rule("dynamic-html", EdgeKind::UnsafeAt, it.span, "dangerouslySetInnerHTML receives non-literal HTML; review its sanitization");
                    }
                }
            }
        }
        walk::walk_jsx_attribute(self, it);
    }
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        let target = self.external(it.source.value.as_str());
        let edge = Edge::new(EdgeKind::Imports, self.file.clone(), target)
            .evidence(self.evidence(it.span));
        let result = self.writer.edge(edge);
        self.record(result);
        walk::walk_import_declaration(self, it);
    }
}

fn constant_arguments(arguments: &[Argument<'_>]) -> bool {
    arguments.iter().all(|a| {
        a.as_expression()
            .is_some_and(|e| matches!(e.get_inner_expression(), Expression::StringLiteral(_)))
    })
}

fn is_config(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(stem, ext)| {
        stem.ends_with(".config") && matches!(ext, "js" | "ts" | "mjs" | "cjs")
    })
}

fn excluded_source(path: &str, source: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let leading = source.trim_start_matches('\u{feff}').trim_start();
    name.ends_with(".d.ts")
        || name.ends_with(".min.js")
        || name.ends_with(".min.mjs")
        || name.contains(".generated.")
        || leading.strip_prefix("//").is_some_and(|comment| {
            comment
                .trim_start()
                .strip_prefix("@generated")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        })
        || leading
            .strip_prefix("/*")
            .and_then(|c| c.split_once("*/"))
            .is_some_and(|(banner, _)| banner.trim() == "eslint-disable")
}

fn skipped_file<W: Write>(
    writer: &mut Writer<W>,
    stats: &mut Stats,
    path: &str,
    reason: &str,
) -> std::io::Result<()> {
    stats.unparsed.push(format!("{path}: {reason}"));
    writer.node(
        Node::new(Id::file(path), NodeKind::File)
            .attr("path", path)
            .attr("language", "typescript")
            .attr("skipped", reason),
    )
}

fn is_literal_string(expr: &Expression<'_>) -> bool {
    matches!(expr.get_inner_expression(), Expression::StringLiteral(_))
        || matches!(expr.get_inner_expression(), Expression::TemplateLiteral(t) if t.expressions.is_empty())
}

/// Exact tokens, preserving spaces inside literals and ignoring formatting and
/// comments. Short wrappers are legitimate duplication.
fn fingerprint(source: &str, tokens: &[oxc_parser::Token], span: Span) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let start = tokens.partition_point(|t| t.span().start < span.start);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut length = 0;
    for token in tokens[start..]
        .iter()
        .take_while(|t| t.span().end <= span.end)
    {
        let text = token.span().source_text(source);
        length += text.len();
        text.hash(&mut hasher);
    }
    (length >= 220).then(|| format!("typescript:{:x}", hasher.finish()))
}
