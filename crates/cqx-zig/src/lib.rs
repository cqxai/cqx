//! Zig syntax facts from the pure-Rust zigsyn parser's structured AST.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Lexed, Stats};
use cqx_vfs::Vfs;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::{self, Write},
    ops::Range,
};
use zigsyn::{scanner::Scanner, token::Token};
pub fn is_source(p: &str) -> bool {
    p.ends_with(".zig")
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    run_with_layout(vfs, out, tick, &cqx_layout::Layout::from_paths(vfs.paths()))
}
pub fn run_with_layout(
    vfs: &Vfs,
    out: impl Write,
    tick: &dyn Fn(),
    layout: &cqx_layout::Layout,
) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "zig", "Zig (zigsyn 0.1.0)")?;
    for path in vfs.paths().filter(|p| is_source(p)) {
        tick();
        let source = vfs.read(path).unwrap_or_default();
        if layout.excluded("zig", path, source) {
            continue;
        }
        let ast = match zigsyn::parse_source(source) {
            Ok(t) => t,
            Err(e) => {
                emitter.skip(path, &e.to_string())?;
                continue;
            }
        };
        let value = serde_json::to_value(ast).map_err(io::Error::other)?;
        let mut nodes = Vec::new();
        let mut tests = Vec::new();
        walk(&value, &mut nodes, &mut tests);
        let mut scanner = Scanner::from(source);
        let mut tokens = Vec::new();
        let mut comments = Vec::new();
        while let Some(t) = scanner.next_token().map_err(io::Error::other)? {
            let r = t.pos..scanner.position();
            if matches!(t.token, Token::Comment(_, _)) {
                comments.push(r)
            } else {
                tokens.push((r, format!("{:?}", t.token.kind())));
            }
        }
        let mut ignored = Vec::new();
        for t in tests {
            ignored.push(span(t, &tokens, source.len()));
        }
        let mut std_aliases = BTreeSet::new();
        let mut shadowed = BTreeSet::new();
        for (kind, n) in &nodes {
            if *kind == "Var" {
                for name in var_names(n) {
                    if import_std(&n["init"]) {
                        std_aliases.insert(name);
                    } else {
                        shadowed.insert(name);
                    }
                }
            }
            if *kind == "Fn" {
                for p in n["params"].as_array().into_iter().flatten() {
                    if let Some(name) = p["name"]["name"].as_str() {
                        shadowed.insert(name.into());
                    }
                }
            }
        }
        let mut entries = cqx_layout::Entries::default();
        let build = path.rsplit('/').next() == Some("build.zig");
        for (kind, n) in &nodes {
            if *kind == "Fn" {
                entries.declaration(
                    span(n, &tokens, source.len()),
                    n["visibility"] == "Pub"
                        && (n["name"]["name"] == "main" || build && n["name"]["name"] == "build"),
                );
            }
        }
        let test = layout.is_test("zig", path);
        let comptime: Vec<_> = nodes
            .iter()
            .filter(|(k, _)| *k == "Comptime")
            .map(|(_, n)| expression_span(n, &tokens, source.len()))
            .collect();
        let mut sink = emitter.file_tokens(
            path,
            source,
            test,
            Lexed {
                tokens: tokens.clone(),
                comments,
            },
            &ignored,
        )?;
        for (kind, n) in &nodes {
            if *kind == "Fn" && !n["body"].is_null() {
                sink.symbol(
                    n["name"]["name"].as_str().unwrap_or("<anonymous>"),
                    span(n, &tokens, source.len()),
                    Some(span(&n["body"], &tokens, source.len())),
                )?;
            }
        }
        for (kind, n) in &nodes {
            let kind = *kind;
            if kind == "Catch" {
                let rhs = &n["rhs"];
                let empty = rhs
                    .get("Block")
                    .is_some_and(|b| b["statements"].as_array().is_some_and(Vec::is_empty));
                let r = expression_span(rhs, &tokens, source.len());
                let unreachable = rhs.get("Unreachable").is_some();
                let catch_range = expression_span(n, &tokens, source.len());
                let known = empty_error_set(&n["lhs"], &nodes)
                    || comptime
                        .iter()
                        .any(|c| c.start <= catch_range.start && c.end >= catch_range.end);
                if !test && (unreachable && !known || empty && !sink.documented(&r)) {
                    sink.rule(
                        "swallowed-errors",
                        EdgeKind::Discards,
                        expression_span(n, &tokens, source.len()),
                        "catch unreachable or unexplained empty catch replaces error handling",
                    )?;
                }
            }
            if kind == "Call" {
                let callee = expression_name(&n["callee"]);
                sink.call(&callee, expression_span(n, &tokens, source.len()))?;
                let standard = |suffix: &str| {
                    std_aliases
                        .iter()
                        .any(|s| !shadowed.contains(s) && callee == format!("{s}.{suffix}"))
                };
                if !entries.contains(&expression_span(n, &tokens, source.len()))
                    && standard("process.exit")
                {
                    sink.rule(
                        "exit-in-library",
                        EdgeKind::Crosses,
                        expression_span(n, &tokens, source.len()),
                        "std.process.exit outside a public main entry unit",
                    )?;
                }
                if standard("process.Child.init")
                    && n["args"]
                        .as_array()
                        .and_then(|a| a.first())
                        .is_some_and(|a| !literal_argv(a))
                {
                    sink.rule(
                        "nonliteral-process",
                        EdgeKind::Spawns,
                        expression_span(n, &tokens, source.len()),
                        "std.process.Child.init argv executable is non-literal; review its source",
                    )?;
                }
            }
        }
    }
    Ok(emitter.finish())
}
// The parser's serde representation retains enum variants and every AST child.
// Traverse that representation, never source substrings or debug text; skip tokens
// (which repeat syntax) and test declarations, including nested test containers.
fn walk<'a>(v: &'a Value, nodes: &mut Vec<(&'a str, &'a Value)>, tests: &mut Vec<&'a Value>) {
    match v {
        Value::Object(o) => {
            if let Some(t) = o.get("Test") {
                tests.push(t);
                return;
            }
            for (k, v) in o {
                if k == "tokens"
                    || k == "comments"
                    || k == "docs"
                    || k == "fields" && o.contains_key("entries")
                {
                    continue;
                }
                if matches!(k.as_str(), "Fn" | "Var" | "Call" | "Catch" | "Comptime") {
                    nodes.push((k, v));
                }
                walk(v, nodes, tests)
            }
        }
        Value::Array(a) => {
            for v in a {
                walk(v, nodes, tests)
            }
        }
        _ => {}
    }
}
fn pos(v: &Value) -> usize {
    v["pos"].as_u64().unwrap_or(0) as usize
}
fn span(v: &Value, tokens: &[(Range<usize>, String)], len: usize) -> Range<usize> {
    let start = pos(v);
    let count = v["tokens"].as_array().map_or(0, Vec::len);
    let mut selected = tokens.iter().filter(|(r, _)| r.start >= start);
    let end = selected
        .nth(count.saturating_sub(1))
        .map_or(start, |(r, _)| r.end);
    start.min(len)..end.min(len)
}
fn expression_span(v: &Value, tokens: &[(Range<usize>, String)], len: usize) -> Range<usize> {
    if v["tokens"].is_array() {
        return span(v, tokens, len);
    }
    if let Some(o) = v.as_object() {
        if o.len() == 1 {
            let (_, inner) = o.iter().next().unwrap();
            if let Some(p) = inner.as_u64() {
                let p = p as usize;
                return p..tokens
                    .iter()
                    .find(|(r, _)| r.start == p)
                    .map_or(p, |(r, _)| r.end);
            }
            return expression_span(inner, tokens, len);
        }
    }
    let start = pos(v).min(len);
    let mut end = start;
    fn positions(v: &Value, end: &mut usize) {
        match v {
            Value::Object(o) => {
                if let Some(p) = o.get("pos").and_then(Value::as_u64) {
                    *end = (*end).max(p as usize)
                }
                for (k, v) in o {
                    if k != "tokens" {
                        positions(v, end)
                    }
                }
            }
            Value::Array(a) => {
                for v in a {
                    positions(v, end)
                }
            }
            _ => {}
        }
    }
    positions(v, &mut end);
    let end = tokens
        .iter()
        .find(|(r, _)| r.start >= end)
        .map_or(end, |(r, _)| r.end);
    start..end.min(len)
}
fn expression_name(v: &Value) -> String {
    if let Some(i) = v.get("Ident") {
        return i["name"].as_str().unwrap_or("").into();
    }
    if let Some(f) = v.get("FieldAccess") {
        return format!(
            "{}.{}",
            expression_name(&f["expr"]),
            f["field"]["name"].as_str().unwrap_or("")
        );
    }
    String::new()
}
fn var_names(v: &Value) -> Vec<String> {
    v["lhs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v["VarProto"]["name"]["name"]
                .as_str()
                .or_else(|| v["Expr"]["Ident"]["name"].as_str())
        })
        .map(str::to_owned)
        .collect()
}
fn import_std(v: &Value) -> bool {
    v["BuiltinCall"]["name"] == "@import"
        && v["BuiltinCall"]["args"][0]["BasicLit"]["text"] == "\"std\""
}
fn literal_string(v: &Value) -> bool {
    v["BasicLit"]["kind"] == "String"
}
fn literal_argv(v: &Value) -> bool {
    let v = v.get("Unary").map_or(v, |u| &u["expr"]);
    let init = v.get("AnonInit").or_else(|| v.get("InitList"));
    init.and_then(|i| i["entries"].as_array())
        .and_then(|a| a.first())
        .is_some_and(|e| e.get("Expr").is_some_and(literal_string))
}

/// A direct call to a uniquely named local function with explicit `error{}!T`
/// has no possible error. Unknown/inferred/external error sets still fire.
fn empty_error_set(lhs: &Value, nodes: &[(&str, &Value)]) -> bool {
    let lhs = lhs.get("Grouped").unwrap_or(lhs);
    let Some(call) = lhs.get("Call") else {
        return false;
    };
    let name = expression_name(&call["callee"]);
    let funcs: Vec<_> = nodes
        .iter()
        .filter(|(k, n)| *k == "Fn" && n["name"]["name"].as_str() == Some(name.as_str()))
        .collect();
    funcs.len() == 1
        && funcs[0].1["ret"]["ErrorUnion"]["error"]["ErrorSetDecl"]["names"]
            .as_array()
            .is_some_and(Vec::is_empty)
}
