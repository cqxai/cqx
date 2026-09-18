//! Turning a fact stream into the numbers the rules are scored against.

use std::collections::{HashMap, HashSet};

use cqx_schema::{Edge, EdgeKind, NodeKind};
use cqx_store::facts::Stream;

use crate::config::Config;

/// One rule's input: a value, and the findings that produced it.
pub struct Measure {
    pub value: f64,
    pub findings: Vec<Finding>,
}

pub struct Finding {
    pub what: String,
    pub file: String,
    pub line: u32,
    /// Which part of the line, `[start, end)`, as the parser counts columns.
    ///
    /// `[0, 0]` where there is no such part: a file that is too long, a crate
    /// drawing on too many others. Those are true of the whole thing, and
    /// inventing a column to satisfy a format would point at code that is not
    /// the problem.
    pub col: [u32; 2],
    /// The line itself, so a reader is shown the code rather than told to go
    /// and find it. Filled in after scoring, by whoever still holds the
    /// source — the scorer sees a graph, not a repository.
    pub text: String,
}

/// Closures cannot express the lifetime tie between an edge and a string
/// borrowed out of it, so these stay plain functions.
/// Types that say nothing about where logic belongs.
const PRIMITIVES: &[&str] = &[
    "String", "&str", "bool", "usize", "u8", "u32", "u64", "i32", "i64", "f64", "char", "()",
    "Self", "&self", "&mut self", "PathBuf", "&Path", "Vec<String>", "serde_json::Value",
    "&[u8]", "Vec<u8>", "&mut Self",
];

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
    let (file, line, col) = edge
        .ev
        .first()
        .map(|e| (e.file.clone(), e.line[0], e.col))
        .unwrap_or_default();
    Finding {
        what: what.into(),
        file,
        line,
        col,
        text: String::new(),
    }
}

/// A finding about a whole file or crate, which has no part of a line to point
/// at.
pub fn about(what: impl Into<String>, file: impl Into<String>, line: u32) -> Finding {
    Finding {
        what: what.into(),
        file: file.into(),
        line,
        col: [0, 0],
        text: String::new(),
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

    pub fn compute(stream: &Stream, config: &Config) -> Metrics {
        let exclude = &config.exclude;
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
        let mut files: Vec<(String, u64)> = Vec::new();
        for node in &stream.nodes {
            if node.kind != NodeKind::File || test_files.contains(node.id.0.as_str()) {
                continue;
            }
            let path = node.attrs.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if excluded(path) {
                continue;
            }
            let n = node.attrs.get("lines").and_then(|v| v.as_u64()).unwrap_or(0);
            lines += n;
            files.push((path.to_string(), n));
        }
        files.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
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
                v.iter().skip(1).map(|id| {
                    about(
                        format!("copy of {}", v[0].trim_start_matches("sym:")),
                        id.trim_start_matches("sym:"),
                        0,
                    )
                })
            })
            .collect();
        density("duplicated-bodies", dup);

        // A crate whose signatures are mostly built from other crates' types,
        // pulled from three or more of them, is a junk drawer rather than a
        // component. One strong pull is an adapter and perfectly fine, which is
        // why the owner count matters as much as the share.
        let mut type_owner: HashMap<&str, HashSet<&str>> = HashMap::new();
        for edge in &stream.edges {
            if edge.kind == EdgeKind::Defines {
                if let Some(p) = package_of(&edge.from.0) {
                    type_owner
                        .entry(edge.to.0.trim_start_matches("type:"))
                        .or_default()
                        .insert(p);
                }
            }
        }
        let mut own: HashMap<&str, u32> = HashMap::new();
        let mut foreign: HashMap<&str, u32> = HashMap::new();
        let mut sources: HashMap<&str, HashSet<&str>> = HashMap::new();
        for edge in &stream.edges {
            if !matches!(edge.kind, EdgeKind::Param | EdgeKind::Returns) {
                continue;
            }
            let Some(p) = package_of(&edge.from.0) else { continue };
            let written = edge.to.0.trim_start_matches("type:");
            if PRIMITIVES.contains(&written) {
                continue;
            }
            let base = written
                .trim_start_matches('&')
                .split('<')
                .next()
                .unwrap_or(written)
                .rsplit("::")
                .next()
                .unwrap_or(written);
            let Some(owners) = type_owner.get(base) else { continue };
            // A name defined in more than one crate cannot be attributed
            // without resolution, and guessing is not free: picking one owner
            // arbitrarily out of a HashSet made this metric change between runs
            // on identical input, because Rust seeds its hasher per process.
            // An ambiguous name is skipped instead.
            if owners.len() != 1 {
                continue;
            }
            let owner = owners.iter().next().copied().unwrap_or_default();
            if owner == p {
                *own.entry(p).or_default() += 1;
            } else {
                *foreign.entry(p).or_default() += 1;
                sources.entry(p).or_default().insert(owner);
            }
        }
        let mut scatter: Vec<Finding> = Vec::new();
        let mut by_package: Vec<(&&str, &u32)> = foreign.iter().collect();
        by_package.sort_by_key(|(p, _)| **p);
        for (pkg, f) in by_package {
            let total = own.get(*pkg).copied().unwrap_or(0) + f;
            let owners = sources.get(*pkg).map(HashSet::len).unwrap_or(0);
            if total >= 12 && (*f as f64 / total as f64) > 0.30 && owners >= 3 {
                scatter.push(about(
                    format!(
                        "{:.0}% of its signature types belong to {owners} other crates",
                        *f as f64 / total as f64 * 100.0
                    ),
                    pkg.to_string(),
                    0,
                ));
            }
        }

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

        // --- modularity: a house standard, so the threshold is a parameter ---
        let max_lines = config
            .rules
            .get("oversized-files")
            .map(|r| r.param("max_lines", 1000.0))
            .unwrap_or(1000.0) as u64;
        let oversized: Vec<&(String, u64)> =
            files.iter().filter(|(_, n)| *n > max_lines).collect();
        values.insert(
            "oversized-files".into(),
            Measure {
                value: oversized.len() as f64 / scale,
                findings: oversized
                    .iter()
                    .map(|(path, n)| {
                        about(
                            format!("{n} lines, over the {max_lines}-line standard"),
                            path.clone(),
                            0,
                        )
                    })
                    .collect(),
            },
        );
        let share_max = config
            .rules
            .get("oversized-line-share")
            .map(|r| r.param("max_lines", 1000.0))
            .unwrap_or(1000.0) as u64;
        let in_big: u64 = files.iter().filter(|(_, n)| *n > share_max).map(|(_, n)| *n).sum();
        values.insert(
            "oversized-line-share".into(),
            Measure {
                value: if lines == 0 {
                    0.0
                } else {
                    in_big as f64 / lines as f64
                },
                findings: Vec::new(),
            },
        );
        values.insert(
            "crate-type-scatter".into(),
            Measure {
                value: scatter.len() as f64 / scale,
                findings: scatter,
            },
        );

        Metrics {
            scale,
            lines,
            values,
        }
    }
}
