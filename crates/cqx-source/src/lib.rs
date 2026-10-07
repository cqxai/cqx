//! Source locations, symbols and rule facts for pure-Rust parser frontends.
use cqx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Source, Writer};
use rezel_common::{IterMode, Tree};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    io::{self, Write},
    ops::Range,
};
#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unparsed: Vec<String>,
}
pub struct Emitter<W: Write> {
    writer: Writer<W>,
    language: &'static str,
    stats: Stats,
}
impl<W: Write> Emitter<W> {
    pub fn new(out: W, language: &'static str, label: &str) -> io::Result<Self> {
        let mut writer = Writer::new(out);
        writer.fact(&Fact::header(language, label))?;
        Ok(Self {
            writer,
            language,
            stats: Stats::default(),
        })
    }
    pub fn skip(&mut self, path: &str, reason: &str) -> io::Result<()> {
        self.stats.unparsed.push(format!("{path}: {reason}"));
        self.writer.node(
            Node::new(Id::file(path), NodeKind::File)
                .attr("path", path)
                .attr("language", self.language)
                .attr("skipped", reason),
        )
    }
    pub fn file<'s, 'w>(
        &'w mut self,
        path: &'s str,
        source: &'s str,
        test: bool,
        tree: &Tree,
    ) -> io::Result<File<'s, 'w, W>> {
        let lang = self.language;
        let file = Id::file(path);
        let package = Id::package(&format!("{lang}:root"));
        self.writer.node(
            Node::new(package.clone(), NodeKind::Package)
                .attr("name", lang)
                .attr("language", lang),
        )?;
        self.writer.node(
            Node::new(file.clone(), NodeKind::File)
                .attr("path", path)
                .attr("language", lang)
                .attr("role", if test { "test" } else { "product" })
                .attr("lines", source.lines().count() as u64),
        )?;
        self.writer.edge(
            Edge::new(EdgeKind::Contains, package, file.clone()).evidence(evidence(
                path,
                lang,
                source,
                0..source.len(),
            )),
        )?;
        self.stats.files += 1;
        let mut tokens = Vec::new();
        let mut comments = Vec::new();
        let mut cursor = tree.cursor(IterMode::INCLUDE_ANONYMOUS);
        loop {
            let range = usize::from(cursor.from())..usize::from(cursor.to());
            let name = cursor.name().to_string();
            if name.to_ascii_lowercase().contains("comment") {
                comments.push(range);
                if !cursor.next(false) {
                    break;
                }
                continue;
            }
            if cursor.first_child() {
                continue;
            }
            tokens.push((range, name));
            if !cursor.next(false) {
                break;
            }
        }
        Ok(File {
            writer: &mut self.writer,
            language: lang,
            path,
            source,
            test,
            file,
            symbols: BTreeMap::new(),
            tokens,
            comments,
        })
    }
    pub fn finish(self) -> Stats {
        Stats {
            nodes: self.writer.nodes,
            edges: self.writer.edges,
            ..self.stats
        }
    }
}
fn evidence(path: &str, lang: &str, source: &str, range: Range<usize>) -> Evidence {
    let loc = |i: usize| {
        let s = &source[..i];
        (
            s.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
            s.rsplit('\n').next().unwrap_or("").chars().count() as u32,
        )
    };
    let (a, b) = (loc(range.start), loc(range.end));
    Evidence {
        file: path.into(),
        line: [a.0, b.0],
        col: [a.1, b.1],
        extractor: lang.into(),
        source: Source::Static,
    }
}
pub struct File<'s, 'w, W: Write> {
    writer: &'w mut Writer<W>,
    pub language: &'static str,
    pub path: &'s str,
    pub source: &'s str,
    pub test: bool,
    file: Id,
    symbols: BTreeMap<usize, (Range<usize>, Id)>,
    tokens: Vec<(Range<usize>, String)>,
    comments: Vec<Range<usize>>,
}
impl<W: Write> File<'_, '_, W> {
    pub fn symbol(
        &mut self,
        name: &str,
        range: Range<usize>,
        body: Option<Range<usize>>,
    ) -> io::Result<()> {
        let id = Id::symbol(&format!(
            "{}:{}::{name}@{}",
            self.language, self.path, range.start
        ));
        let mut node = Node::new(id.clone(), NodeKind::Symbol)
            .attr("name", name)
            .attr("language", self.language)
            .attr("lang:kind", "function");
        if !self.test {
            if let Some(body) = body {
                let tokens: Vec<_> = self
                    .tokens
                    .iter()
                    .filter(|(r, _)| r.start >= body.start && r.end <= body.end)
                    .map(|(r, k)| (k, &self.source[r.clone()]))
                    .collect();
                if tokens.len() >= 40 {
                    let mut hash = std::collections::hash_map::DefaultHasher::new();
                    tokens.hash(&mut hash);
                    node = node.attr("body", format!("{:016x}", hash.finish()));
                }
            }
        }
        self.writer.node(node)?;
        self.writer.edge(
            Edge::new(EdgeKind::Contains, self.file.clone(), id.clone()).evidence(evidence(
                self.path,
                self.language,
                self.source,
                range.clone(),
            )),
        )?;
        self.symbols.insert(range.start, (range, id));
        Ok(())
    }
    fn container(&self, r: &Range<usize>) -> Id {
        self.symbols
            .range(..=r.start)
            .rev()
            .find(|(_, (s, _))| s.end >= r.end)
            .map(|(_, (_, id))| id.clone())
            .unwrap_or_else(|| self.file.clone())
    }
    pub fn call(&mut self, name: &str, range: Range<usize>) -> io::Result<()> {
        let target = Id::external(&format!("{}:{name}", self.language));
        self.writer.node(
            Node::new(target.clone(), NodeKind::External)
                .attr("name", name)
                .attr("language", self.language),
        )?;
        self.writer.edge(
            Edge::new(EdgeKind::Calls, self.container(&range), target).evidence(evidence(
                self.path,
                self.language,
                self.source,
                range,
            )),
        )
    }
    pub fn rule(
        &mut self,
        rule: &str,
        kind: EdgeKind,
        range: Range<usize>,
        what: &str,
    ) -> io::Result<()> {
        let target = Id::capability(&format!("{}/{rule}", self.language));
        self.writer.node(
            Node::new(target.clone(), NodeKind::Capability)
                .attr("name", rule)
                .attr("language", self.language),
        )?;
        self.writer.edge(
            Edge::new(kind, self.container(&range), target)
                .evidence(evidence(self.path, self.language, self.source, range))
                .attr(&format!("{}:rule", self.language), rule)
                .attr("what", what)
                .attr("role", if self.test { "test" } else { "product" }),
        )
    }
    pub fn documented(&self, range: &Range<usize>) -> bool {
        self.comments.iter().any(|r| {
            r.start >= range.start
                && r.end <= range.end
                && !self.source[r.clone()]
                    .trim_start_matches(['/', '*', '#'])
                    .trim_end_matches(['/', '*'])
                    .trim()
                    .is_empty()
        })
    }
    pub fn has_reason(&self, range: &Range<usize>) -> bool {
        self.comments.iter().any(|r| {
            let adjacent = (r.end <= range.start
                && self.source[r.end..range.start].trim().is_empty())
                || (r.start >= range.end && {
                    let gap = &self.source[range.end..r.start];
                    !gap.contains('\n') && gap.trim().is_empty()
                });
            adjacent
                && !self.source[r.clone()]
                    .trim_start_matches(['/', '*', '#'])
                    .trim_end_matches(['/', '*'])
                    .trim()
                    .is_empty()
        })
    }
}
