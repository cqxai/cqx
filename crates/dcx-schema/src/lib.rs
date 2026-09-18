//! The dcx fact schema.
//!
//! An extractor emits a stream of [`Fact`]s as newline-delimited JSON. Nothing
//! in this crate knows about any particular language: language-specific
//! vocabulary belongs in `attrs` under a `lang:` prefix, never in a node or
//! edge kind.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Bumped whenever a consumer could misread an older stream. Emitted once, as
/// the first line of every fact stream.
pub const SCHEMA_VERSION: &str = "0.1.0";

/// A stable, path-addressed identifier.
///
/// Never line-addressed: ids have to survive a refactor so that annotations
/// stay attached and two graphs can be diffed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub String);

impl Id {
    pub fn package(name: &str) -> Id {
        Id(format!("pkg:{name}"))
    }
    pub fn directory(path: &str) -> Id {
        Id(format!("dir:{path}"))
    }
    pub fn file(path: &str) -> Id {
        Id(format!("file:{path}"))
    }
    pub fn symbol(path: &str) -> Id {
        Id(format!("sym:{path}"))
    }
    /// An external process this code can start.
    pub fn process(name: &str) -> Id {
        Id(format!("proc:{name}"))
    }
    pub fn env_var(name: &str) -> Id {
        Id(format!("env:{name}"))
    }
    /// A capability class (`fs`, `net`, `exec`).
    pub fn capability(name: &str) -> Id {
        Id(format!("cap:{name}"))
    }
    /// Something referenced but not owned by this scan — an upstream crate, an
    /// unresolved import.
    pub fn external(name: &str) -> Id {
        Id(format!("ext:{name}"))
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    System,
    Binary,
    Package,
    Directory,
    File,
    Symbol,
    Process,
    EnvVar,
    Capability,
    External,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// The zoom spine. The only edge every extractor must emit.
    Contains,
    DependsOn,
    Imports,
    Calls,
    // --- effect edges: few, dangerous, and the default view ---
    Spawns,
    ReadsEnv,
    Crosses,
    EffectFs,
    EffectNet,
    EffectExec,
    UnsafeAt,
}

/// Where a fact came from.
///
/// `Runtime` is unused today. It exists so that observing a running program
/// later is not a schema migration.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Static,
    Runtime,
}

/// Where a fact can be seen in the source. An edge without evidence does not
/// exist — that rule is what separates a map from a diagram.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Evidence {
    pub file: String,
    /// Inclusive `[start, end]`, 1-indexed.
    pub line: [u32; 2],
    pub extractor: String,
    #[serde(default, skip_serializing_if = "is_static")]
    pub source: Source,
}

fn is_static(s: &Source) -> bool {
    matches!(s, Source::Static)
}

impl Evidence {
    pub fn at(file: &str, start: u32, end: u32) -> Evidence {
        Evidence {
            file: file.to_string(),
            line: [start, end],
            extractor: "rust".to_string(),
            source: Source::Static,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Node {
    pub id: Id,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
}

impl Node {
    pub fn new(id: Id, kind: NodeKind) -> Node {
        Node {
            id,
            kind,
            attrs: Map::new(),
        }
    }
    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> Node {
        self.attrs.insert(key.to_string(), value.into());
        self
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: Id,
    pub to: Id,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ev: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
}

impl Edge {
    pub fn new(kind: EdgeKind, from: Id, to: Id) -> Edge {
        Edge {
            kind,
            from,
            to,
            ev: Vec::new(),
            attrs: Map::new(),
        }
    }
    pub fn evidence(mut self, ev: Evidence) -> Edge {
        self.ev.push(ev);
        self
    }
    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> Edge {
        self.attrs.insert(key.to_string(), value.into());
        self
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Fact {
    /// First line of every stream.
    Header {
        schema: String,
        extractor: String,
        root: String,
    },
    Node(Node),
    Edge(Edge),
}

impl Fact {
    pub fn header(extractor: &str, root: &str) -> Fact {
        Fact::Header {
            schema: SCHEMA_VERSION.to_string(),
            extractor: extractor.to_string(),
            root: root.to_string(),
        }
    }
}

/// Writes facts as newline-delimited JSON, de-duplicating nodes by id so an
/// extractor can emit a containing node every time it needs one.
pub struct Writer<W: std::io::Write> {
    out: W,
    seen_nodes: std::collections::HashSet<Id>,
    pub nodes: usize,
    pub edges: usize,
}

impl<W: std::io::Write> Writer<W> {
    pub fn new(out: W) -> Writer<W> {
        Writer {
            out,
            seen_nodes: std::collections::HashSet::new(),
            nodes: 0,
            edges: 0,
        }
    }

    pub fn fact(&mut self, fact: &Fact) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.out, fact)?;
        self.out.write_all(b"\n")
    }

    pub fn node(&mut self, node: Node) -> std::io::Result<()> {
        if !self.seen_nodes.insert(node.id.clone()) {
            return Ok(());
        }
        self.nodes += 1;
        self.fact(&Fact::Node(node))
    }

    pub fn edge(&mut self, edge: Edge) -> std::io::Result<()> {
        self.edges += 1;
        self.fact(&Fact::Edge(edge))
    }
}
