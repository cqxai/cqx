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

pub fn run(
    stream: &Stream,
    named: Option<&str>,
    zql: Option<&str>,
    json: bool,
) -> Result<(), String> {
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

    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::Value::Object(
                    row.fields
                        .iter()
                        .map(|(k, v)| (k.clone(), as_json(v)))
                        .collect(),
                )
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }

    table(query, &rows);
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

/// How many rows a terminal can be shown before it stops being a terminal.
const ROWS: usize = 40;

/// The widest a single cell is allowed to get before it is cut.
const CELL: usize = 44;

/// What a query asked for, in the order it asked.
///
/// `Row.fields` is a `HashMap`, so iterating it gives a different column order
/// every run — which is why the first version of this printed
/// `Row { fields: {...} }` and left the reader to find the field they wanted.
/// The RETURN clause already names the columns in order, and its text is
/// exactly what the engine keyed the map by, so it is read from there.
fn columns(query: &str, rows: &[zega_core::Row]) -> Vec<String> {
    let lower = query.to_lowercase();
    if let Some(at) = lower.rfind("return ") {
        let tail = &query[at + "return ".len()..];
        // Stop at the clauses that may follow a RETURN.
        let end = ["order by", "limit", "skip"]
            .iter()
            .filter_map(|k| tail.to_lowercase().find(k))
            .min()
            .unwrap_or(tail.len());
        let named: Vec<String> = tail[..end]
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
        // Only trust it when the names are the keys the engine actually used;
        // an aliased or computed projection is not worth guessing at.
        if !named.is_empty()
            && rows
                .first()
                .map(|r| named.iter().all(|n| r.fields.contains_key(n)))
                .unwrap_or(true)
        {
            return named;
        }
    }
    let mut keys: Vec<String> = rows
        .first()
        .map(|r| r.fields.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

fn show(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(bits) => format!("{}", f64::from_bits(*bits)),
        Value::Bool(b) => b.to_string(),
        Value::Null => "—".to_string(),
        Value::List(items) => items.iter().map(show).collect::<Vec<_>>().join(", "),
        Value::Map(_) => "{…}".to_string(),
    }
}

fn as_json(value: &Value) -> serde_json::Value {
    match value {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::Float(bits) => serde_json::Value::from(f64::from_bits(*bits)),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Null => serde_json::Value::Null,
        Value::List(items) => serde_json::Value::Array(items.iter().map(as_json).collect()),
        Value::Map(m) => {
            serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), as_json(v))).collect())
        }
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= CELL {
        return text.to_string();
    }
    text.chars().take(CELL - 1).collect::<String>() + "…"
}

/// Rows as a table, because a person asked.
fn table(query: &str, rows: &[zega_core::Row]) {
    if rows.is_empty() {
        println!("  no rows");
        return;
    }
    let cols = columns(query, rows);
    let shown: Vec<Vec<String>> = rows
        .iter()
        .take(ROWS)
        .map(|row| {
            cols.iter()
                .map(|c| clip(&row.fields.get(c).map(show).unwrap_or_default()))
                .collect()
        })
        .collect();

    let width: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            shown
                .iter()
                .map(|r| r[i].chars().count())
                .chain(std::iter::once(c.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let pad = |text: &str, w: usize| {
        let mut out = text.to_string();
        out.push_str(&" ".repeat(w.saturating_sub(text.chars().count())));
        out
    };

    println!(
        "  {}",
        cols.iter()
            .enumerate()
            .map(|(i, c)| pad(c, width[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
    );
    println!("  {}", width.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("  "));
    for row in &shown {
        println!(
            "  {}",
            row.iter()
                .enumerate()
                .map(|(i, cell)| pad(cell, width[i]))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
        );
    }
    if rows.len() > ROWS {
        println!("  … {} more", rows.len() - ROWS);
    }
}
