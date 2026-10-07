//! AST-based extraction shared by tree-sitter frontends. No filesystem access.
use cqx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node as FactNode, NodeKind, Source, Writer};
use cqx_vfs::Vfs;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    io::{self, Write},
};
pub use tree_sitter::{Language, Node};

#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unparsed: Vec<String>,
}

pub trait Frontend {
    fn needs_final_newline(&self) -> bool {
        false
    }
    fn language(&self) -> &'static str;
    fn grammar(&self) -> Language;
    fn is_source(&self, path: &str) -> bool;
    fn excluded(&self, path: &str, source: &str) -> bool;
    fn is_test(&self, path: &str) -> bool;
    fn is_entry(&self, path: &str, source: &str, root: Node<'_>) -> bool;
    fn test_ranges(&self, _source: &str, _root: Node<'_>) -> Vec<std::ops::Range<usize>> {
        Vec::new()
    }
    fn function<'a>(&self, source: &'a str, node: Node<'a>) -> Option<(String, Node<'a>)>;
    fn inspect(&self, context: &mut Context<'_, '_, '_>, node: Node<'_>) -> io::Result<()>;
}

pub fn nodes(root: Node<'_>) -> Vec<Node<'_>> {
    let mut result = Vec::new();
    let mut cursor = root.walk();
    loop {
        result.push(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return result;
            }
        }
    }
}

pub fn text<'a>(source: &'a str, node: Node<'_>) -> &'a str {
    &source[node.start_byte().min(source.len())..node.end_byte().min(source.len())]
}

fn evidence(path: &str, lang: &str, node: Node<'_>, source: &str) -> Evidence {
    let column = |offset: usize| {
        source[..offset.min(source.len())]
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .chars()
            .count() as u32
    };
    Evidence {
        file: path.into(),
        line: [
            node.start_position().row as u32 + 1,
            if node.end_byte() > source.len() {
                source.bytes().filter(|b| *b == b'\n').count() as u32 + 1
            } else {
                node.end_position().row as u32 + 1
            },
        ],
        col: [column(node.start_byte()), column(node.end_byte())],
        extractor: lang.into(),
        source: Source::Static,
    }
}

pub struct Context<'s, 'w, 'out> {
    pub source: &'s str,
    pub path: &'s str,
    pub language: &'static str,
    pub entry: bool,
    pub test: bool,
    pub root: Node<'s>,
    pub cache: BTreeMap<String, std::collections::BTreeSet<String>>,
    file: Id,
    symbols: BTreeMap<usize, Id>,
    writer: &'w mut Writer<&'out mut dyn Write>,
}

impl<'s> Context<'s, '_, '_> {
    pub fn text(&self, node: Node<'_>) -> &'s str {
        text(self.source, node)
    }
    fn container(&self, node: Node<'_>) -> Id {
        let mut parent = Some(node);
        while let Some(p) = parent {
            if let Some(id) = self.symbols.get(&p.start_byte()) {
                return id.clone();
            }
            parent = p.parent();
        }
        self.file.clone()
    }
    pub fn call(&mut self, node: Node<'_>, name: &str) -> io::Result<()> {
        let target = Id::external(&format!("{}:{name}", self.language));
        self.writer.node(
            FactNode::new(target.clone(), NodeKind::External)
                .attr("name", name)
                .attr("language", self.language),
        )?;
        self.writer.edge(
            Edge::new(EdgeKind::Calls, self.container(node), target).evidence(evidence(
                self.path,
                self.language,
                node,
                self.source,
            )),
        )
    }
    pub fn rule(
        &mut self,
        rule: &str,
        kind: EdgeKind,
        node: Node<'_>,
        what: &str,
    ) -> io::Result<()> {
        let target = Id::capability(&format!("{}/{rule}", self.language));
        self.writer.node(
            FactNode::new(target.clone(), NodeKind::Capability)
                .attr("name", rule)
                .attr("language", self.language),
        )?;
        let container = self.container(node);
        self.writer.edge(
            Edge::new(kind, container, target)
                .evidence(evidence(self.path, self.language, node, self.source))
                .attr(&format!("{}:rule", self.language), rule)
                .attr("what", what)
                .attr("role", if self.test { "test" } else { "product" }),
        )
    }
}

pub fn run(
    vfs: &Vfs,
    mut out: impl Write,
    frontend: &dyn Frontend,
    tick: &dyn Fn(),
) -> io::Result<Stats> {
    let mut stats = Stats::default();
    let lang = frontend.language();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&frontend.grammar())
        .map_err(io::Error::other)?;
    let output: &mut dyn Write = &mut out;
    let mut writer = Writer::new(output);
    writer.fact(&Fact::header(lang, &vfs.label))?;
    for path in vfs.paths().filter(|p| frontend.is_source(p)) {
        let source = vfs.read(path).unwrap_or_default();
        if frontend.excluded(path, source) {
            tick();
            continue;
        }
        let input = if frontend.needs_final_newline() && !source.ends_with('\n') {
            std::borrow::Cow::Owned(format!("{source}\n"))
        } else {
            std::borrow::Cow::Borrowed(source)
        };
        let tree = parser.parse(input.as_ref(), None);
        tick();
        let reason = match &tree {
            None => Some("parser did not produce a syntax tree".to_string()),
            Some(tree) if tree.root_node().has_error() => {
                let n = nodes(tree.root_node())
                    .into_iter()
                    .find(|n| n.is_error() || n.is_missing())
                    .unwrap_or(tree.root_node());
                Some(format!(
                    "syntax error at {}:{} ({})",
                    n.start_position().row + 1,
                    n.start_position().column + 1,
                    n.kind()
                ))
            }
            _ => None,
        };
        let file = Id::file(path);
        if let Some(reason) = reason {
            stats.unparsed.push(format!("{path}: {reason}"));
            writer.node(
                FactNode::new(file, NodeKind::File)
                    .attr("path", path)
                    .attr("language", lang)
                    .attr("skipped", reason),
            )?;
            continue;
        }
        let tree = tree.expect("successful parse");
        let root = tree.root_node();
        let test_ranges = frontend.test_ranges(source, root);
        let in_test = |n: Node<'_>| test_ranges.iter().any(|r| r.contains(&n.start_byte()));
        let product_lines = {
            let mut offset = 0;
            source
                .split_inclusive('\n')
                .filter(|line| {
                    let is_test = test_ranges.iter().any(|r| r.contains(&offset));
                    offset += line.len();
                    !is_test
                })
                .count() as u64
        };
        let package = Id::package(&format!("{lang}:root"));
        let test = frontend.is_test(path);
        writer.node(
            FactNode::new(package.clone(), NodeKind::Package)
                .attr("name", lang)
                .attr("language", lang),
        )?;
        writer.node(
            FactNode::new(file.clone(), NodeKind::File)
                .attr("path", path)
                .attr("lines", product_lines)
                .attr("language", lang)
                .attr("role", if test { "test" } else { "product" }),
        )?;
        writer.edge(
            Edge::new(EdgeKind::Contains, package, file.clone())
                .evidence(evidence(path, lang, root, source)),
        )?;
        let mut symbols = BTreeMap::new();
        let all = nodes(root);
        for &node in &all {
            if let Some((name, body)) = frontend.function(source, node) {
                let id = Id::symbol(&format!("{lang}:{path}::{name}@{}", node.start_byte()));
                let mut symbol = FactNode::new(id.clone(), NodeKind::Symbol)
                    .attr("name", name)
                    .attr("language", lang)
                    .attr("lang:kind", "function");
                if !test && !in_test(node) {
                    if let Some(hash) = fingerprint(source, body) {
                        symbol = symbol.attr("body", hash);
                    }
                }
                writer.node(symbol)?;
                writer.edge(
                    Edge::new(EdgeKind::Contains, file.clone(), id.clone())
                        .evidence(evidence(path, lang, node, source)),
                )?;
                symbols.insert(node.start_byte(), id);
            }
        }
        // Each context only borrows the writer for this file.
        let mut context = Context {
            source,
            path,
            language: lang,
            entry: frontend.is_entry(path, source, root),
            test,
            root,
            cache: BTreeMap::new(),
            file,
            symbols,
            writer: &mut writer,
        };
        for node in all {
            context.test = test || in_test(node);
            frontend.inspect(&mut context, node)?;
        }
        stats.files += 1;
    }
    stats.nodes = writer.nodes;
    stats.edges = writer.edges;
    Ok(stats)
}

fn fingerprint(source: &str, body: Node<'_>) -> Option<String> {
    let tokens: Vec<_> = nodes(body)
        .into_iter()
        .filter(|n| n.child_count() == 0 && !n.kind().contains("comment"))
        .map(|n| (n.kind(), text(source, n)))
        .collect();
    if tokens.len() < 40 {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    tokens.hash(&mut hash);
    Some(format!("{:016x}", hash.finish()))
}
