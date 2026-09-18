//! Turning a fact stream into the dataset an explorer renders.
//!
//! The extractor emits facts; the scorer emits a verdict. Neither emits the
//! shape a reader needs, because a reader needs the facts folded up: which
//! files a crate owns, which crate owns a type, which functions are worth
//! looking at. That folding is here, once, rather than in each consumer — the
//! browser and the exporter must agree on it or the same commit renders two
//! different ways depending on where it was processed.
//!
//! Nothing in here touches the filesystem, so it compiles for wasm.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cqx_schema::{EdgeKind, Node, NodeKind};
use cqx_store::facts::Stream;
use serde_json::{json, Value};

/// What a dataset says about itself. The facts do not carry it: a fact stream
/// is about code, and this is about where the code came from.
pub struct Meta<'a> {
    pub repo: &'a str,
    pub branch: &'a str,
    pub remote: Option<&'a str>,
    pub commits_url: Option<&'a str>,
    /// How long this took, in milliseconds.
    ///
    /// Measured from source in memory to finished dataset: parsing, the fact
    /// stream, the metrics, the score and this fold. Not the checkout, not the
    /// upload — a browser has neither, and a number that means two different
    /// things depending on where it was produced is not worth printing.
    ///
    /// The caller measures it. A wasm build has no clock, so there the page
    /// times the call and fills this in.
    pub analysed_ms: Option<u64>,
    /// How long it took to get the source in hand, in milliseconds.
    ///
    /// Not the same work in both places — a checkout unpacks a commit and
    /// reads it off a local disk, a browser pulls every file across the
    /// internet — but it is the same question, and leaving it out of one of
    /// them makes that one look faster than it was.
    pub fetched_ms: Option<u64>,
}

/// The six effects worth a badge. `silences` is deliberately not among them:
/// it is a property of the code, not something the program does at runtime.
fn effect_name(kind: EdgeKind) -> Option<&'static str> {
    Some(match kind {
        EdgeKind::Spawns => "spawns",
        EdgeKind::ReadsEnv => "reads_env",
        EdgeKind::EffectFs => "effect_fs",
        EdgeKind::EffectNet => "effect_net",
        EdgeKind::EffectExec => "effect_exec",
        EdgeKind::UnsafeAt => "unsafe_at",
        _ => return None,
    })
}

fn attr<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.attrs.get(key)?.as_str()
}

fn lines_of(node: &Node) -> u64 {
    node.attrs.get("lines").and_then(Value::as_u64).unwrap_or(0)
}

/// Walks containment upward to the first id with this prefix.
///
/// Bounded rather than trusting the graph to be acyclic: a cycle here would
/// hang the browser tab, and a depth limit costs nothing.
fn climb<'a>(start: &'a str, parent: &HashMap<&'a str, &'a str>, prefix: &str) -> Option<&'a str> {
    let mut at = start;
    for _ in 0..64 {
        if at.starts_with(prefix) {
            return Some(at);
        }
        at = parent.get(at).copied()?;
    }
    None
}

/// The bare name of a type, with whatever a generic wraps stripped away.
fn base_of<'a>(id: &'a str, node: &HashMap<&'a str, &'a Node>) -> &'a str {
    node.get(id)
        .and_then(|n| attr(n, "base").or_else(|| attr(n, "text")))
        .unwrap_or_else(|| id.strip_prefix("type:").unwrap_or(id))
}

/// The type exactly as the author typed it, which is what a signature renders.
fn written<'a>(id: &'a str, node: &HashMap<&'a str, &'a Node>) -> &'a str {
    node.get(id)
        .and_then(|n| attr(n, "text"))
        .unwrap_or_else(|| id.strip_prefix("type:").unwrap_or(id))
}

/// A name that two crates both define cannot be attributed to either, so it is
/// attributed to neither. Guessing is what made an earlier metric report five
/// different values for one unchanged repository.
const AMBIGUOUS: &str = "~";

pub fn dataset(stream: &Stream, score: Value, history: Value, meta: &Meta<'_>) -> Value {
    let node: HashMap<&str, &Node> = stream
        .nodes
        .iter()
        .map(|n| (n.id.0.as_str(), n))
        .collect();

    let mut parent: HashMap<&str, &str> = HashMap::new();
    for edge in &stream.edges {
        if edge.kind == EdgeKind::Contains {
            parent.insert(&edge.to.0, &edge.from.0);
        }
    }

    // --- where each symbol lives ---------------------------------------------
    let package_name = |id: &str| -> Option<&str> { node.get(id).and_then(|n| attr(n, "name")) };
    let file_path = |id: &str| -> Option<&str> { node.get(id).and_then(|n| attr(n, "path")) };

    // --- packages ------------------------------------------------------------
    let mut pkg_files: BTreeMap<&str, Vec<&Node>> = BTreeMap::new();
    let mut file_symbols: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &stream.nodes {
        match n.kind {
            NodeKind::File => {
                if let Some(pkg) = climb(&n.id.0, &parent, "pkg:") {
                    pkg_files.entry(pkg).or_default().push(n);
                }
            }
            NodeKind::Symbol => {
                if let Some(file) = climb(&n.id.0, &parent, "file:") {
                    *file_symbols.entry(file).or_default() += 1;
                }
            }
            _ => {}
        }
    }

    // --- effects, counted where they happen and listed once ------------------
    let mut pkg_eff: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    let mut file_eff: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    let mut sym_eff: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut effects: Vec<Value> = Vec::new();
    for edge in &stream.edges {
        let Some(kind) = effect_name(edge.kind) else {
            continue;
        };
        let from = edge.from.0.as_str();
        let pkg = climb(from, &parent, "pkg:");
        if let Some(pkg) = pkg {
            *pkg_eff.entry(pkg).or_default().entry(kind).or_default() += 1;
        }
        if let Some(file) = climb(from, &parent, "file:") {
            *file_eff.entry(file).or_default().entry(kind).or_default() += 1;
        }
        sym_eff.entry(from).or_default().insert(kind);

        // An effect without a site is not reportable, and the schema says it
        // cannot exist — but a stream from elsewhere might still carry one.
        let Some(ev) = edge.ev.first() else { continue };
        let target = node
            .get(edge.to.0.as_str())
            .and_then(|n| attr(n, "name"))
            .unwrap_or_else(|| {
                edge.to
                    .0
                    .split_once(':')
                    .map(|(_, rest)| rest)
                    .unwrap_or(&edge.to.0)
            });
        let mut row = json!({
            "k": kind,
            "pkg": pkg.and_then(package_name),
            "sym": node.get(from).and_then(|n| attr(n, "name")).unwrap_or(from),
            "to": target,
            "file": ev.file,
            "line": ev.line[0],
        });
        for (from_key, to_key) in [
            ("via", "via"),
            ("source_fn", "src"),
            ("env_var", "env"),
            ("form", "form"),
        ] {
            if let Some(v) = edge.attrs.get(from_key) {
                row[to_key] = v.clone();
            }
        }
        effects.push(row);
    }

    let mut deps_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &stream.edges {
        if edge.kind == EdgeKind::DependsOn {
            deps_of.entry(&edge.from.0).or_default().push(&edge.to.0);
        }
    }
    let mut bins_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &stream.edges {
        if edge.kind == EdgeKind::Contains {
            if let Some(n) = node.get(edge.to.0.as_str()) {
                if n.kind == NodeKind::Binary {
                    if let Some(name) = attr(n, "name") {
                        bins_of.entry(&edge.from.0).or_default().push(name);
                    }
                }
            }
        }
    }

    let packages: Vec<Value> = stream
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Package)
        .map(|n| {
            let id = n.id.0.as_str();
            let files = pkg_files.get(id).map(Vec::as_slice).unwrap_or(&[]);
            let deps = deps_of.get(id).map(Vec::as_slice).unwrap_or(&[]);
            json!({
                "id": id,
                "name": attr(n, "name").unwrap_or(id),
                "files": files.len(),
                "lines": files.iter().map(|f| lines_of(f)).sum::<u64>(),
                "eff": pkg_eff.get(id).cloned().unwrap_or_default(),
                "deps": deps.iter().map(|d| {
                    node.get(*d).and_then(|n| attr(n, "name")).unwrap_or(
                        d.split_once(':').map(|(_, r)| r).unwrap_or(d))
                }).collect::<Vec<_>>(),
                "ext": deps.iter().filter(|d| d.starts_with("ext:")).count(),
                "bins": bins_of.get(id).cloned().unwrap_or_default(),
            })
        })
        .collect();

    let files: Vec<Value> = stream
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| {
            let id = n.id.0.as_str();
            json!({
                "id": id,
                "path": attr(n, "path").unwrap_or(id),
                "lines": lines_of(n),
                "pkg": climb(id, &parent, "pkg:"),
                "n": file_symbols.get(id).copied().unwrap_or(0),
                "eff": file_eff.get(id).cloned().unwrap_or_default(),
            })
        })
        .collect();

    // --- who owns a type name ------------------------------------------------
    //
    // A type node is the type as written. What a reader wants underneath a
    // signature is which crate each name came from, which is answerable only
    // for names this scan saw declared.
    let mut defined_by: BTreeMap<&str, &str> = BTreeMap::new(); // type id → symbol id
    for edge in &stream.edges {
        if edge.kind == EdgeKind::Defines {
            defined_by.insert(&edge.to.0, &edge.from.0);
        }
    }
    let mut owner_of_base: HashMap<&str, Option<&str>> = HashMap::new();
    for (ty, sym) in &defined_by {
        let Some(base) = node
            .get(*ty)
            .and_then(|n| attr(n, "base").or_else(|| attr(n, "text")))
        else {
            continue;
        };
        let owner = climb(sym, &parent, "pkg:").and_then(package_name);
        match owner_of_base.get(base) {
            None => {
                owner_of_base.insert(base, owner);
            }
            Some(existing) if *existing != owner => {
                owner_of_base.insert(base, Some(AMBIGUOUS));
            }
            Some(_) => {}
        }
    }

    let mut type_args: BTreeMap<&str, Vec<(u64, &str)>> = BTreeMap::new();
    let mut used: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &stream.edges {
        match edge.kind {
            EdgeKind::TypeArg => {
                let position = edge
                    .attrs
                    .get("position")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                type_args
                    .entry(&edge.from.0)
                    .or_default()
                    .push((position, &edge.to.0));
                *used.entry(&edge.to.0).or_default() += 1;
            }
            EdgeKind::Param | EdgeKind::Returns | EdgeKind::HasField => {
                *used.entry(&edge.to.0).or_default() += 1;
            }
            _ => {}
        }
    }


    /// The written type and everything inside it, each attributed to a crate.
    fn parts(
        id: &str,
        depth: u64,
        out: &mut Vec<Value>,
        seen: &mut HashSet<String>,
        type_args: &BTreeMap<&str, Vec<(u64, &str)>>,
        node: &HashMap<&str, &Node>,
        owner_of_base: &HashMap<&str, Option<&str>>,
    ) {
        if depth > 3 || !seen.insert(id.to_string()) {
            return;
        }
        let base = base_of(id, node);
        out.push(json!([base, owner_of_base.get(base).copied().flatten(), depth]));
        let mut children = type_args.get(id).cloned().unwrap_or_default();
        children.sort();
        for (_, child) in children {
            parts(
                child,
                depth + 1,
                out,
                seen,
                type_args,
                node,
                owner_of_base,
            );
        }
    }
    let annotate = |id: &str| -> Vec<Value> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        parts(
            id,
            0,
            &mut out,
            &mut seen,
            &type_args,
            &node,
            &owner_of_base,
        );
        out
    };

    // --- types ---------------------------------------------------------------
    let mut fields_of: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for edge in &stream.edges {
        if edge.kind == EdgeKind::HasField {
            let name = edge
                .attrs
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            fields_of
                .entry(&edge.from.0)
                .or_default()
                .push((name, written(&edge.to.0, &node).to_string()));
        }
    }

    let types: Vec<Value> = defined_by
        .iter()
        .filter_map(|(ty, sym)| {
            let declaration = node.get(*sym)?;
            let pkg = climb(sym, &parent, "pkg:").and_then(package_name);
            json!({
                "id": ty,
                "name": base_of(ty, &node),
                "k": attr(declaration, "lang:kind").unwrap_or("type"),
                "pkg": pkg,
                "file": climb(sym, &parent, "file:").and_then(file_path),
                "lines": lines_of(declaration),
                "fields": fields_of.get(*ty).cloned().unwrap_or_default(),
                "uses": used.get(*ty).copied().unwrap_or(0),
            })
            .into()
        })
        .collect();

    // --- functions -----------------------------------------------------------
    let mut params_of: BTreeMap<&str, Vec<(u64, &str, &str)>> = BTreeMap::new();
    let mut returns_of: BTreeMap<&str, &str> = BTreeMap::new();
    for edge in &stream.edges {
        match edge.kind {
            EdgeKind::Param => params_of.entry(&edge.from.0).or_default().push((
                edge.attrs
                    .get("position")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                edge.attrs.get("name").and_then(Value::as_str).unwrap_or("_"),
                &edge.to.0,
            )),
            EdgeKind::Returns => {
                returns_of.insert(&edge.from.0, &edge.to.0);
            }
            _ => {}
        }
    }

    // `Result<_, String>` — an error that carries no structure, and the single
    // strongest signal this tool measures.
    let unstructured_error = |ty: &str| -> bool {
        base_of(ty, &node) == "Result"
            && type_args
                .get(ty)
                .map(|args| {
                    args.iter()
                        .any(|(p, child)| *p == 1 && base_of(child, &node) == "String")
                })
                .unwrap_or(false)
    };

    let is_function =
        |n: &Node| matches!(attr(n, "lang:kind"), Some("fn") | Some("method"));
    let total_functions = stream.nodes.iter().filter(|n| is_function(n)).count();

    let functions: Vec<Value> = stream
        .nodes
        .iter()
        .filter(|n| is_function(n))
        .filter_map(|n| {
            let id = n.id.0.as_str();
            let mut params = params_of.get(id).cloned().unwrap_or_default();
            params.sort();
            let returns = returns_of.get(id).copied();
            let effects: Vec<&str> = sym_eff
                .get(id)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();

            // Shown because something about it is worth a second look. A list
            // of every function is a file listing, not a finding.
            let notable = !effects.is_empty()
                || params.len() >= 3
                || returns.map(unstructured_error).unwrap_or(false);
            if !notable {
                return None;
            }

            Some(json!({
                "id": id,
                "name": attr(n, "name").unwrap_or(id),
                "k": attr(n, "lang:kind").unwrap_or("fn"),
                "pkg": climb(id, &parent, "pkg:").and_then(package_name),
                "file": climb(id, &parent, "file:").and_then(file_path),
                "lines": lines_of(n),
                "p": params.iter().map(|(_, name, ty)| {
                    json!([name, written(ty, &node), annotate(ty)])
                }).collect::<Vec<_>>(),
                "r": returns.map(|ty| json!([written(ty, &node), annotate(ty)])),
                "eff": effects,
            }))
        })
        .collect();

    let total_lines = stream
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(lines_of)
        .sum::<u64>();

    json!({
        "repo": meta.repo,
        "analysis": {
            "ms": meta.analysed_ms,
            "fetch": meta.fetched_ms,
            "cqx": env!("CARGO_PKG_VERSION"),
        },
        "branch": meta.branch,
        "remote": meta.remote,
        "commits_url": meta.commits_url,
        "score": score,
        "history": history,
        "packages": packages,
        "files": files,
        "types": types,
        "functions": functions,
        "effects": effects,
        "totals": {
            "types": types.len(),
            "functions": total_functions,
            "notable": functions.len(),
            "nodes": stream.nodes.len(),
            "edges": stream.edges.len(),
            "lines": total_lines,
        },
    })
}
