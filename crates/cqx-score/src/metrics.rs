//! Turning a fact stream into the numbers the rules are scored against.

use std::collections::{HashMap, HashSet};

use cqx_schema::{Edge, EdgeKind, NodeKind};
use cqx_store::facts::Stream;

/// One rule's input: a value, and the findings that produced it.
pub struct Measure {
    pub value: f64,
    pub findings: Vec<Finding>,
}

pub struct Finding {
    pub what: String,
    pub file: String,
    pub line: u32,
}

/// Closures cannot express the lifetime tie between an edge and a string
/// borrowed out of it, so these stay plain functions.
fn package_of(id: &str) -> Option<&str> {
    id.strip_prefix("sym:").and_then(|r| r.split("::").next())
}

fn is_product(e: &Edge) -> bool {
    e.attrs.get("role").and_then(|v| v.as_str()) != Some("test")
}

fn attr<'a>(e: &'a Edge, k: &str) -> &'a str {
    e.attrs.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

fn finding(what: impl Into<String>, edge: &Edge) -> Finding {
    let (file, line) = edge
        .ev
        .first()
        .map(|e| (e.file.clone(), e.line[0]))
        .unwrap_or_default();
    Finding {
        what: what.into(),
        file,
        line,
    }
}

pub struct Metrics {
    /// Product lines, in units of ten thousand — the denominator for densities.
    pub scale: f64,
    pub lines: u64,
    values: HashMap<String, Measure>,
}

impl Metrics {
    pub fn get(&self, id: &str) -> Option<&Measure> {
        self.values.get(id)
    }

    pub fn compute(stream: &Stream, exclude: &[String]) -> Metrics {
        let excluded = |path: &str| exclude.iter().any(|p| path.starts_with(p.as_str()));

        // Test code is tagged rather than dropped, but it is not the product.
        let test_files: HashSet<&str> = stream
            .nodes
            .iter()
            .filter(|n| {
                n.kind == NodeKind::File
                    && n.attrs.get("role").and_then(|v| v.as_str()) == Some("test")
            })
            .map(|n| n.id.0.as_str())
            .collect();

        let mut lines = 0u64;
        for node in &stream.nodes {
            if node.kind != NodeKind::File || test_files.contains(node.id.0.as_str()) {
                continue;
            }
            let path = node.attrs.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if excluded(path) {
                continue;
            }
            lines += node.attrs.get("lines").and_then(|v| v.as_u64()).unwrap_or(0);
        }
        // A tiny codebase would otherwise divide its way to an infinite density.
        let scale = (lines as f64 / 10_000.0).max(0.05);

        let binary_packages: HashSet<String> = stream
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains && e.to.0.starts_with("bin:"))
            .filter_map(|e| e.from.0.strip_prefix("pkg:").map(str::to_string))
            .collect();


        // Only effect edges carry a test role; a declaration edge has to be
        // placed by the file that holds its symbol, or test helpers count as
        // product code.
        let mut symbol_file: HashMap<&str, &str> = HashMap::new();
        for edge in &stream.edges {
            if edge.kind == EdgeKind::Contains
                && edge.from.0.starts_with("file:")
                && edge.to.0.starts_with("sym:")
            {
                symbol_file.insert(edge.to.0.as_str(), edge.from.0.as_str());
            }
        }
        let in_product = |sym: &str| -> bool {
            match symbol_file.get(sym) {
                Some(file) => !test_files.contains(*file),
                None => true,
            }
        };

        let mut values: HashMap<String, Measure> = HashMap::new();
        let mut density = |id: &str, findings: Vec<Finding>| {
            values.insert(
                id.to_string(),
                Measure {
                    value: findings.len() as f64 / scale,
                    findings,
                },
            );
        };

        density(
            "result-string-density",
            stream
                .edges
                .iter()
                .filter(|e| {
                    e.kind == EdgeKind::Returns
                        && e.to.0.replace(' ', "").contains(",String>")
                        && in_product(&e.from.0)
                })
                .map(|e| finding("returns Result<_, String>", e))
                .collect(),
        );

        let crate_silences: Vec<&Edge> = stream
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Silences && attr(e, "scope") == "crate")
            .collect();
        density(
            "broad-lint-silencing",
            crate_silences
                .iter()
                .filter(|e| attr(e, "breadth") == "broad")
                .map(|e| finding(format!("crate-wide allow({})", attr(e, "lint")), e))
                .collect(),
        );
        density(
            "undocumented-suppressions",
            crate_silences
                .iter()
                .filter(|e| e.attrs.get("documented").is_none())
                .map(|e| finding(format!("allow({}) with no reason", attr(e, "lint")), e))
                .collect(),
        );

        density(
            "exit-in-library",
            stream
                .edges
                .iter()
                .filter(|e| e.kind == EdgeKind::EffectExec && is_product(e))
                .filter(|e| {
                    package_of(&e.from.0).is_some_and(|p| !binary_packages.contains(p))
                })
                .map(|e| finding("library ends the process", e))
                .collect(),
        );

        let spawns: Vec<&Edge> = stream
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Spawns && is_product(e))
            .collect();
        density(
            "env-controlled-spawn",
            spawns
                .iter()
                .filter(|e| attr(e, "via") == "env")
                .map(|e| {
                    finding(
                        format!("spawn target from ${}", attr(e, "env_var")),
                        e,
                    )
                })
                .collect(),
        );
        density(
            "shell-invocation",
            spawns
                .iter()
                .filter(|e| {
                    ["zsh", "bash", "/sh", "cmd", "powershell"]
                        .iter()
                        .any(|s| e.to.0.contains(s))
                })
                .map(|e| finding(format!("spawns {}", e.to.0.trim_start_matches("proc:")), e))
                .collect(),
        );

        // Duplication is counted as redundant copies, not groups: three copies
        // of one body is two things to delete, not one.
        let mut bodies: HashMap<&str, Vec<&str>> = HashMap::new();
        for node in &stream.nodes {
            if !in_product(&node.id.0) {
                continue;
            }
            if let Some(hash) = node.attrs.get("body").and_then(|v| v.as_str()) {
                bodies.entry(hash).or_default().push(node.id.0.as_str());
            }
        }
        let dup: Vec<Finding> = bodies
            .values()
            .filter(|v| v.len() > 1)
            .flat_map(|v| {
                v.iter().skip(1).map(|id| Finding {
                    what: format!("copy of {}", v[0].trim_start_matches("sym:")),
                    file: id.trim_start_matches("sym:").to_string(),
                    line: 0,
                })
            })
            .collect();
        density("duplicated-bodies", dup);

        // Ratios, which need no scale.
        let params: Vec<&Edge> = stream
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Param && in_product(&e.from.0))
            .collect();
        let bare = params
            .iter()
            .filter(|e| matches!(e.to.0.as_str(), "type:&str" | "type:String" | "type:&String"))
            .count();
        values.insert(
            "bare-string-params".into(),
            Measure {
                value: if params.is_empty() {
                    0.0
                } else {
                    bare as f64 / params.len() as f64
                },
                findings: Vec::new(),
            },
        );

        let targets: Vec<&Edge> = stream
            .edges
            .iter()
            .filter(|e| {
                matches!(e.kind, EdgeKind::Spawns | EdgeKind::ReadsEnv) && is_product(e)
            })
            .collect();
        let unproven: Vec<Finding> = targets
            .iter()
            .filter(|e| !matches!(attr(e, "via"), "" | "literal"))
            .map(|e| finding(format!("target unproven ({})", attr(e, "via")), e))
            .collect();
        values.insert(
            "unproven-spawn-targets".into(),
            Measure {
                value: if targets.is_empty() {
                    0.0
                } else {
                    unproven.len() as f64 / targets.len() as f64
                },
                findings: unproven,
            },
        );

        Metrics {
            scale,
            lines,
            values,
        }
    }
}
