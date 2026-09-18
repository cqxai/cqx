//! Reading a fact stream back into memory.

use std::collections::BTreeMap;
use std::path::Path;

use cqx_schema::Fact;

#[derive(Default)]
pub struct Stream {
    pub nodes: Vec<cqx_schema::Node>,
    pub edges: Vec<cqx_schema::Edge>,
    pub schema: String,
    pub root: String,
}

impl Stream {
    pub fn load(path: &Path) -> std::io::Result<Stream> {
        Ok(Stream::from_ndjson(&std::fs::read_to_string(path)?))
    }

    /// Parses a fact stream from text.
    ///
    /// The only form that exists in a browser, where there is no path to read
    /// from — and the form `load` is written in terms of, so there is one
    /// parser rather than two.
    pub fn from_ndjson(text: &str) -> Stream {
        let mut stream = Stream::default();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Fact>(line) {
                Ok(Fact::Header { schema, root, .. }) => {
                    stream.schema = schema;
                    stream.root = root;
                }
                Ok(Fact::Node(node)) => stream.nodes.push(node),
                Ok(Fact::Edge(edge)) => stream.edges.push(edge),
                // A stream from a newer extractor may carry facts this build
                // does not know. Skipping them beats refusing to load.
                Err(_) => continue,
            }
        }
        stream
    }

    /// Drops what more than one reader said.
    ///
    /// Readers given different parts of a workspace all describe the packages,
    /// the binaries and the dependencies, because each of them read every
    /// manifest. The same node arriving twice says nothing new, and a
    /// containment edge arriving twice multiplies every path through it — one
    /// query returned 7,999 rows where 212 was right, and that was this.
    ///
    /// Edges are compared whole, evidence included, so two facts are dropped
    /// only when they are the same fact. Two readers cannot produce the same
    /// evidence for different things: no file is given to more than one.
    pub fn dedupe(&mut self) {
        let mut nodes = std::collections::HashSet::new();
        self.nodes.retain(|n| nodes.insert(n.id.clone()));
        let mut edges = std::collections::HashSet::new();
        self.edges
            .retain(|e| edges.insert(serde_json::to_string(e).unwrap_or_default()));
    }

    pub fn report(&self) {
        let mut node_kinds: BTreeMap<String, usize> = BTreeMap::new();
        for node in &self.nodes {
            *node_kinds
                .entry(format!("{:?}", node.kind).to_lowercase())
                .or_default() += 1;
        }
        let mut edge_kinds: BTreeMap<String, usize> = BTreeMap::new();
        for edge in &self.edges {
            *edge_kinds
                .entry(format!("{:?}", edge.kind).to_lowercase())
                .or_default() += 1;
        }
        println!("schema {} · {}", self.schema, self.root);
        println!("{} nodes, {} edges", self.nodes.len(), self.edges.len());
        for (kind, count) in node_kinds {
            println!("  node {kind:<12} {count:>7}");
        }
        for (kind, count) in edge_kinds {
            println!("  edge {kind:<12} {count:>7}");
        }
    }
}
