//! The zega backend: load facts as a graph, then ask it questions.
//!
//! Node kinds become labels and edge kinds become relationship types, because
//! neither can be parameterised in a query — and it is the relationship type
//! that makes `-[:depends_on*]->` expressible, which is the whole reason for
//! using a graph engine rather than a table.

use std::collections::HashMap;

use zega_core::Zega;
use zega_parser::value::Value;

use crate::facts::Stream;

/// The relationship type as the schema spells it.
///
/// `Debug` lower-cased turns `DependsOn` into `dependson`, which silently made
/// every multi-word edge kind unqueryable while single-word ones worked.
fn edge_type(kind: cqx_schema::EdgeKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{kind:?}"))
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::from_f64(n.as_f64().unwrap_or(0.0))),
        other => Value::String(other.to_string()),
    }
}

/// `sym:deka_host::fs::op_read` -> `deka_host`, so a query can group by crate
/// without a join the extractor never emitted.
fn package_of(id: &str) -> Option<String> {
    id.strip_prefix("sym:")
        .and_then(|rest| rest.split("::").next())
        .map(str::to_string)
}

pub fn load(stream: &Stream) -> Result<Zega, String> {
    let zega = Zega::in_memory().build().map_err(|e| format!("{e:?}"))?;

    // One statement per fact. UNWIND in ZQL only follows a MATCH, and an
    // in-memory engine turns out to be quick enough that batching would be
    // complexity without a reason.
    for node in &stream.nodes {
        let label = format!("{:?}", node.kind);
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("id".into(), Value::String(node.id.0.clone()));
        params.insert(
            "pkg".into(),
            Value::String(package_of(&node.id.0).unwrap_or_default()),
        );
        for key in ["name", "lang:kind", "path", "text", "lint", "body"] {
            let v = node
                .attrs
                .get(key)
                .map(json_to_value)
                .unwrap_or_else(|| Value::String(String::new()));
            params.insert(key.replace(':', "_"), v);
        }
        params.insert(
            "lines".into(),
            node.attrs
                .get("lines")
                .map(json_to_value)
                .unwrap_or(Value::Int(0)),
        );
        zega
            .query(
                &format!(
                    "CREATE (n:{label} {{id: $id, name: $name, pkg: $pkg, kind: $lang_kind, \
                     path: $path, text: $text, lines: $lines}})"
                ),
                params,
            )
            .map_err(|e| format!("node {}: {e:?}", node.id.0))?;
    }

    let known: std::collections::HashSet<&str> =
        stream.nodes.iter().map(|n| n.id.0.as_str()).collect();
    let mut dangling = 0usize;
    for edge in &stream.edges {
        // `MATCH ... CREATE` quietly creates nothing when an endpoint is
        // missing, so a dangling edge would vanish without a word.
        if !known.contains(edge.from.0.as_str()) || !known.contains(edge.to.0.as_str()) {
            dangling += 1;
            continue;
        }
        let kind = edge_type(edge.kind);
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("from".into(), Value::String(edge.from.0.clone()));
        params.insert("to".into(), Value::String(edge.to.0.clone()));
        for key in ["via", "lint", "breadth", "scope", "role", "op", "form", "name"] {
            params.insert(
                key.to_string(),
                edge.attrs
                    .get(key)
                    .map(json_to_value)
                    .unwrap_or_else(|| Value::String(String::new())),
            );
        }
        let (file, line) = edge
            .ev
            .first()
            .map(|e| (e.file.clone(), e.line[0] as i64))
            .unwrap_or_default();
        params.insert("file".into(), Value::String(file));
        params.insert("line".into(), Value::Int(line));
        zega
            .query(
                &format!(
                    "MATCH (a {{id: $from}}), (b {{id: $to}}) \
                     CREATE (a)-[:{kind} {{via: $via, lint: $lint, breadth: $breadth, \
                     role: $role, file: $file, line: $line}}]->(b)"
                ),
                params,
            )
            .map_err(|e| format!("edge {kind} {} -> {}: {e:?}", edge.from.0, edge.to.0))?;
    }
    if dangling > 0 {
        eprintln!("warning: {dangling} edges reference a node the stream never declared");
    }
    Ok(zega)
}

/// The questions the audit actually asked, as queries rather than scripts.
pub const NAMED: &[(&str, &str, &str)] = &[
    (
        "exit-in-library",
        "library crates that end the process",
        "MATCH (s:Symbol)-[e:effect_exec]->(:Capability) RETURN s.pkg, s.name, e.file, e.line",
    ),
    (
        "spawn-reach",
        "packages that contain a process spawn, however deeply nested",
        "MATCH (p:Package)-[:contains*1..6]->(s:Symbol)-[:spawns]->(t:Process) \
         RETURN p.name, t.name, s.name",
    ),
    (
        "depends-on-host",
        "every crate that transitively depends on deka_host",
        "MATCH (a:Package)-[:depends_on*1..8]->(b:Package {name: 'deka_host'}) RETURN a.name",
    ),
    (
        "env-controlled",
        "spawn targets chosen by the environment",
        "MATCH (s:Symbol)-[e:spawns]->(t:Process) WHERE e.via = 'env' \
         RETURN s.pkg, s.name, e.file, e.line",
    ),
    (
        "silenced",
        "lints switched off crate-wide",
        "MATCH (f)-[e:silences]->(:Capability) WHERE e.breadth = 'broad' \
         RETURN e.lint, e.file",
    ),
];

pub fn run(stream: &Stream, named: Option<&str>, zql: Option<&str>) -> Result<(), String> {
    if named == Some("list") {
        println!("named queries:");
        for (name, about, _) in NAMED {
            println!("  {name:<16} {about}");
        }
        return Ok(());
    }

    let query = match (named, zql) {
        (Some(name), _) => NAMED
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, _, q)| *q)
            .ok_or_else(|| format!("no named query '{name}'; try --named list"))?,
        (None, Some(q)) => q,
        (None, None) => return Err("give a query, or --named <name>".into()),
    };

    let started = std::time::Instant::now();
    let zega = load(stream)?;
    let loaded = started.elapsed();

    let began = std::time::Instant::now();
    let rows = zega
        .query(query, HashMap::new())
        .map_err(|e| format!("{e:?}"))?;
    let took = began.elapsed();

    for row in rows.iter().take(40) {
        println!("  {row:?}");
    }
    println!(
        "\n{} rows · loaded {} nodes and {} edges in {:.2}s · query {:.1}ms",
        rows.len(),
        stream.nodes.len(),
        stream.edges.len(),
        loaded.as_secs_f64(),
        took.as_secs_f64() * 1000.0
    );
    Ok(())
}
