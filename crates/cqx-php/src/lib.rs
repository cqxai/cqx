//! PHP 8.5 syntax/effect evidence, with optional declaration-based API binding.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Stats};
use cqx_vfs::Vfs;
use rezel_common::{IterMode, SyntaxNode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    ops::Range,
};
#[derive(Default, Serialize, Deserialize)]
pub struct Metadata {
    pub entries: BTreeSet<String>,
    pub legacy_roots: BTreeMap<String, bool>,
}
pub fn metadata(vfs: &Vfs) -> Metadata {
    let mut m = Metadata {
        entries: cqx_vfs::composer_bins(vfs),
        ..Default::default()
    };
    for path in vfs
        .paths()
        .filter(|p| p.rsplit('/').next() == Some("composer.json"))
    {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(vfs.read(path).unwrap_or_default())
        {
            let root = path.rsplit_once('/').map_or("", |(p, _)| p);
            m.legacy_roots.insert(
                root.into(),
                v["require"]["php"].as_str().is_some_and(legacy_constraint),
            );
        }
    }
    m
}
fn legacy_constraint(s: &str) -> bool {
    s.split('|').filter(|s| !s.trim().is_empty()).any(|s| {
        let normalized = s.split_whitespace().collect::<Vec<_>>().join(",");
        let Ok(req) = semver::VersionReq::parse(&normalized) else {
            return false;
        };
        let mut versions = Vec::new();
        for major in [5, 7] {
            for minor in 0..=6 {
                for patch in [0, 1, 99999] {
                    versions.push(semver::Version::new(major, minor, patch));
                }
            }
        }
        for c in &req.comparators {
            if c.major < 8 {
                let patch = c.patch.unwrap_or(0);
                for p in [patch, patch.saturating_sub(1), patch.saturating_add(1)] {
                    versions.push(semver::Version::new(c.major, c.minor.unwrap_or(0), p));
                }
            }
        }
        versions.iter().any(|v| req.matches(v))
    })
}
pub fn is_source(p: &str) -> bool {
    p.ends_with(".php")
}
pub fn is_source_in(vfs: &Vfs, p: &str, m: &Metadata) -> bool {
    is_source(p)
        || m.entries.contains(p)
            && vfs.read(p).is_some_and(|s| {
                s.get(..s.len().min(512))
                    .unwrap_or(s)
                    .to_ascii_lowercase()
                    .contains("<?php")
            })
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    run_with_metadata(vfs, out, tick, &metadata(vfs))
}
pub fn run_with_metadata(
    vfs: &Vfs,
    out: impl Write,
    tick: &dyn Fn(),
    meta: &Metadata,
) -> io::Result<Stats> {
    run_with_layout(
        vfs,
        out,
        tick,
        meta,
        &cqx_layout::Layout::from_paths(vfs.paths()),
    )
}
pub fn run_with_layout(
    vfs: &Vfs,
    out: impl Write,
    tick: &dyn Fn(),
    meta: &Metadata,
    layout: &cqx_layout::Layout,
) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "php", "PHP 8.5")?;
    for path in vfs.paths().filter(|p| is_source_in(vfs, p, meta)) {
        tick();
        let source = vfs.read(path).unwrap_or_default();
        if layout.excluded("php", path, source) {
            continue;
        }
        let tree = match rezel_lang_php::parser().with_strict(true).parse(source) {
            Ok(t) => t,
            Err(e) => {
                emitter.skip(path, &e.to_string())?;
                continue;
            }
        };
        let all = nodes(&tree);
        let namespace = all
            .iter()
            .any(|n| n.name().as_ref() == "NamespaceDefinition");
        let mut functions = BTreeMap::new();
        let mut classes = BTreeMap::new();
        let mut shadows = BTreeSet::new();
        for n in &all {
            if n.name().as_ref() == "FunctionDefinition" || n.name().as_ref() == "ClassDeclaration"
            {
                if let Some(name) = n.children().find(|n| n.name().as_ref() == "Name") {
                    shadows.insert(text(source, &name).to_ascii_lowercase());
                }
            }
            if n.name().as_ref() == "NamespaceUseDeclaration" {
                let declaration = text(source, n)
                    .trim()
                    .trim_end_matches(';')
                    .trim_start_matches("use ");
                let (function, decl) = if let Some(s) = declaration.strip_prefix("function ") {
                    (true, s)
                } else {
                    (false, declaration)
                };
                for part in decl.split(',') {
                    let parts: Vec<_> = part.split_whitespace().collect();
                    if let Some(name) = parts.first() {
                        let name = name.trim_start_matches('\\').to_ascii_lowercase();
                        let alias = parts
                            .get(2)
                            .map(|s| s.to_ascii_lowercase())
                            .unwrap_or_else(|| name.rsplit('\\').next().unwrap_or("").into());
                        if function {
                            functions.insert(alias, name);
                        } else {
                            classes.insert(alias, name);
                        }
                    }
                }
            }
        }
        let canonical = |n: &SyntaxNode| {
            let raw = global_name(source, n);
            if raw.starts_with('\\') {
                return raw.trim_start_matches('\\').to_ascii_lowercase();
            }
            let name = raw.to_ascii_lowercase();
            if shadows.contains(&name) {
                String::new()
            } else {
                functions.get(&name).cloned().unwrap_or(name)
            }
        };
        let class = |n: &SyntaxNode| {
            let raw = text(source, n).trim().trim_start_matches('?');
            let name = raw.trim_start_matches('\\').to_ascii_lowercase();
            if raw.starts_with('\\') {
                return name;
            }
            if shadows.contains(&name) {
                return String::new();
            }
            classes
                .get(&name)
                .cloned()
                .unwrap_or_else(|| if namespace { String::new() } else { name })
        };
        let mut sql_bindings: BTreeMap<(usize, String), Vec<String>> = BTreeMap::new();
        let mut inputs = BTreeSet::new();
        let mut string_inputs = BTreeSet::new();
        let mut numeric_inputs = BTreeSet::new();
        for n in &all {
            if matches!(
                n.name().as_ref(),
                "Parameter" | "PropertyParameter" | "VariadicParameter"
            ) {
                if let Some(v) = n.children().find(|n| n.name().as_ref() == "VariableName") {
                    inputs.insert((scope(n), text(source, &v).to_owned()));
                    if let Some(t) = n
                        .children()
                        .find(|n| matches!(n.name().as_ref(), "NamedType" | "OptionalType"))
                    {
                        let ty = class(&t);
                        if matches!(
                            text(source, &t).to_ascii_lowercase().as_str(),
                            "int" | "float" | "bool"
                        ) {
                            numeric_inputs.insert((scope(n), text(source, &v).to_owned()));
                        }
                        if text(source, &t).eq_ignore_ascii_case("string") {
                            string_inputs.insert((scope(n), text(source, &v).to_owned()));
                        }
                        if matches!(ty.as_str(), "pdo" | "mysqli") {
                            sql_bindings
                                .entry((scope(n), text(source, &v).into()))
                                .or_default()
                                .push(ty);
                        }
                    }
                }
            }
            if n.name().as_ref() == "AssignmentExpression" {
                let children: Vec<_> = n.children().collect();
                if let (Some(v), Some(new)) = (children.first(), children.last()) {
                    if v.name().as_ref() == "VariableName" {
                        if new.name().as_ref() == "NewExpression" {
                            if let Some(ty) = new
                                .children()
                                .find(|n| matches!(n.name().as_ref(), "Name" | "QualifiedName"))
                            {
                                let ty = class(&ty);
                                sql_bindings
                                    .entry((scope(n), text(source, v).into()))
                                    .or_default()
                                    .push(ty);
                            }
                        } else {
                            sql_bindings
                                .entry((scope(n), text(source, v).into()))
                                .or_default()
                                .push(String::new());
                        }
                    }
                }
            }
        }
        let mut entries = cqx_layout::Entries::default();
        entries.script(meta.entries.contains(path) || layout.front_controller(path));
        for n in &all {
            if matches!(
                n.name().as_ref(),
                "FunctionDefinition"
                    | "MethodDeclaration"
                    | "FunctionExpression"
                    | "ArrowFunction"
                    | "ClassDeclaration"
            ) {
                entries.declaration(range(n), false);
            }
        }
        let legacy = meta
            .legacy_roots
            .iter()
            .filter(|(root, _)| root.is_empty() || path.starts_with(&format!("{root}/")))
            .max_by_key(|(root, _)| root.len())
            .is_some_and(|(_, legacy)| *legacy);
        let strict = all.iter().any(|n| {
            n.name().as_ref() == "DeclareStatement"
                && text(source, n)
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
                    .contains("strict_types=1")
        });
        let test = layout.is_test("php", path);
        let mut sink = emitter.file(path, source, test, &tree)?;
        for n in &all {
            if matches!(
                n.name().as_ref(),
                "FunctionDefinition" | "MethodDeclaration"
            ) {
                if let Some(body) = n.children().find(|n| n.name().as_ref() == "Block") {
                    let name = n
                        .children()
                        .find(|n| n.name().as_ref() == "Name")
                        .map(|n| text(source, &n).to_owned())
                        .unwrap_or_default();
                    let params = n
                        .children()
                        .find(|n| n.name().as_ref() == "ParamList")
                        .map(|n| text(source, &n).to_owned())
                        .unwrap_or_default();
                    let ret = n
                        .children()
                        .filter(|n| {
                            matches!(
                                n.name().as_ref(),
                                "NamedType" | "UnionType" | "IntersectionType" | "OptionalType"
                            )
                        })
                        .map(|n| text(source, &n))
                        .collect::<String>();
                    sink.symbol_with_attrs(
                        &name,
                        range(n),
                        Some(range(&body)),
                        &[
                            ("php:parameters".into(), params),
                            ("php:return".into(), ret),
                            ("php:strict_types".into(), strict.to_string()),
                        ],
                    )?;
                }
            }
        }
        let mut exit_sites = BTreeSet::new();
        for n in &all {
            if n.name().as_ref() == "TryStatement" {
                let children: Vec<_> = n.children().collect();
                for (i, c) in children.iter().enumerate() {
                    if c.name().as_ref() == "CatchDeclarator" {
                        if let Some(b) = children
                            .iter()
                            .skip(i + 1)
                            .find(|n| n.name().as_ref() == "Block")
                        {
                            if !b.children().any(|n| {
                                n.name().ends_with("Statement")
                                    || matches!(n.name().as_ref(), "Block" | "FunctionDefinition")
                            }) && !sink.documented(&range(b))
                            {
                                sink.rule(
                                    "swallowed-errors",
                                    EdgeKind::Discards,
                                    range(b),
                                    "empty catch discards an error without an explanation",
                                )?;
                            }
                        }
                    }
                }
            }
            if n.name().as_ref() == "UnaryExpression"
                && n.children()
                    .next()
                    .is_some_and(|c| c.name().as_ref() == "ControlOp" && text(source, &c) == "@")
            {
                sink.rule(
                    "error-suppression",
                    EdgeKind::Silences,
                    range(n),
                    "@ expression suppresses diagnostic errors",
                )?;
            }
            if n.name().as_ref().contains("Comment")
                && cqx_layout::broad_directive("php", text(source, n))
                && !reason(text(source, n))
                && !all
                    .iter()
                    .filter(|n| n.name().as_ref().contains("Comment"))
                    .any(|c| {
                        range(c).end <= range(n).start
                            && {
                                let gap = &source[range(c).end..range(n).start];
                                gap.contains('\n')
                                    && gap.rsplit('\n').skip(1).all(|s| s.trim().is_empty())
                            }
                            && !suppression(text(source, c))
                            && text(source, c).trim_matches(['/', '*', ' ']).len() > 3
                    })
            {
                sink.rule(
                    "undocumented-suppressions",
                    EdgeKind::Silences,
                    range(n),
                    "phpcs/phpstan suppression has no reason",
                )?;
            }
            if n.name().as_ref() == "ShellExpression" && shell_dynamic(text(source, n)) {
                sink.rule(
                    "nonliteral-process",
                    EdgeKind::Spawns,
                    range(n),
                    "backtick shell command interpolates non-literal input",
                )?;
            }
            if n.name().as_ref() == "ExpressionStatement" && !entries.contains(&range(n)) {
                if let Some(c) = n.children().next() {
                    if c.name().as_ref() != "CallExpression"
                        && matches!(canonical(&c).as_str(), "exit" | "die")
                        && exit_sites.insert(range(&c).start)
                    {
                        sink.rule(
                            "exit-in-library",
                            EdgeKind::Crosses,
                            range(n),
                            "process termination outside an entry script",
                        )?;
                    }
                }
            }
            if n.name().as_ref() == "CallExpression" {
                let children: Vec<_> = n.children().collect();
                let Some(function) = children.first() else {
                    continue;
                };
                let name = canonical(function);
                sink.call(&name, range(n))?;
                let args = children
                    .iter()
                    .find(|n| n.name().as_ref() == "ArgList")
                    .map(|n| arguments(source, n))
                    .unwrap_or_default();
                let first = args
                    .iter()
                    .find(|(k, _)| k.is_none())
                    .map(|(_, n)| n)
                    .or_else(|| {
                        args.iter()
                            .find(|(k, _)| k.as_deref() == Some("command"))
                            .map(|(_, n)| n)
                    });
                if !entries.contains(&range(n))
                    && matches!(name.as_str(), "exit" | "die")
                    && exit_sites.insert(range(n).start)
                {
                    sink.rule(
                        "exit-in-library",
                        EdgeKind::Crosses,
                        range(n),
                        "process termination outside an entry script",
                    )?;
                }
                if matches!(
                    name.as_str(),
                    "shell_exec" | "exec" | "system" | "passthru" | "popen" | "proc_open"
                ) && first.is_some_and(|n| !command_literal(n))
                {
                    sink.rule(
                        "nonliteral-process",
                        EdgeKind::Spawns,
                        range(n),
                        "shell/process command executable is non-literal",
                    )?;
                }
                if name == "eval"
                    || legacy && name == "create_function"
                    || legacy
                        && name == "assert"
                        && first.is_some_and(|n| {
                            literal(n)
                                || n.name().as_ref() == "VariableName"
                                    && string_inputs.contains(&(scope(n), text(source, n).into()))
                        })
                {
                    sink.rule(
                        "dynamic-code",
                        EdgeKind::Spawns,
                        range(n),
                        "eval or a legacy PHP dynamic-code API",
                    )?;
                }
                if name == "unserialize"
                    && first.is_some_and(|n| external_input(source, n, &inputs))
                    && !safe_unserialize(source, &args)
                {
                    sink.rule("unsafe-deserialization",EdgeKind::Spawns,range(n),"unserialize input comes from a parameter/superglobal-looking source without allowed_classes=false")?;
                }
                let method = if function.name().as_ref() == "MemberExpression" {
                    function
                        .children()
                        .last()
                        .map(|n| text(source, &n).to_ascii_lowercase())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let receiver = function.children().next();
                let bound = receiver
                    .as_ref()
                    .and_then(|n| sql_bindings.get(&(scope(n), text(source, n).into())))
                    .is_some_and(|types| {
                        types.len() == 1 && matches!(types[0].as_str(), "pdo" | "mysqli")
                    });
                let query = method == "query" && bound || name == "mysqli_query";
                let sql = if name == "mysqli_query" {
                    args.iter()
                        .filter(|(k, _)| k.is_none())
                        .nth(1)
                        .map(|(_, n)| n)
                } else {
                    first
                };
                if query
                    && sql.is_some_and(|n| {
                        dynamic_concat(n)
                            && !safe_sql_term(source, n, &numeric_inputs, &sql_bindings)
                    })
                {
                    sink.rule(
                        "concatenated-sql",
                        EdgeKind::Spawns,
                        range(n),
                        "SQL concatenates dynamic data at a bound mysqli/PDO query API",
                    )?;
                }
            }
        }
    }
    Ok(emitter.finish())
}
fn nodes(t: &rezel_common::Tree) -> Vec<SyntaxNode> {
    let mut c = t.cursor(IterMode::NONE);
    let mut all = Vec::new();
    loop {
        all.push(c.node());
        if !c.next(true) {
            break;
        }
    }
    all
}
fn range(n: &SyntaxNode) -> Range<usize> {
    usize::from(n.from())..usize::from(n.to())
}
fn text<'s>(s: &'s str, n: &SyntaxNode) -> &'s str {
    &s[range(n)]
}
fn descendants(n: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut result = Vec::new();
    let mut todo = vec![n.clone()];
    while let Some(n) = todo.pop() {
        todo.extend(n.children());
        result.push(n)
    }
    result
}
fn scope(n: &SyntaxNode) -> usize {
    let mut p = n.parent();
    while let Some(n) = p {
        if matches!(
            n.name().as_ref(),
            "FunctionDefinition" | "MethodDeclaration" | "FunctionExpression" | "ArrowFunction"
        ) {
            return range(&n).start;
        }
        p = n.parent()
    }
    usize::MAX
}
fn global_name(s: &str, n: &SyntaxNode) -> String {
    if matches!(n.name().as_ref(), "Name" | "QualifiedName") {
        text(s, n).into()
    } else {
        String::new()
    }
}
fn arguments(s: &str, n: &SyntaxNode) -> Vec<(Option<String>, SyntaxNode)> {
    let c: Vec<_> = n.children().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].name().as_ref() == "NamedArgument" {
            let parts: Vec<_> = c[i].children().collect();
            if let (Some(k), Some(v)) = (parts.first(), parts.last()) {
                args.push((Some(text(s, k).into()), v.clone()))
            }
            i += 1
        } else {
            if !matches!(c[i].name().as_ref(), "(" | ")" | ",") {
                args.push((None, c[i].clone()));
            }
            i += 1
        }
    }
    args
}
fn literal(n: &SyntaxNode) -> bool {
    matches!(n.name().as_ref(), "String" | "HeredocString")
        && !descendants(n).iter().any(|n| {
            matches!(
                n.name().as_ref(),
                "VariableName" | "Interpolation" | "MemberExpression" | "SubscriptExpression"
            )
        })
}
fn command_literal(n: &SyntaxNode) -> bool {
    literal(n)
        || n.name().as_ref() == "ArrayExpression"
            && n.children()
                .find(|n| !matches!(n.name().as_ref(), "[" | "]" | "array" | "ValueList"))
                .is_some_and(|n| literal(&n))
}
fn external_input(s: &str, n: &SyntaxNode, inputs: &BTreeSet<(usize, String)>) -> bool {
    descendants(n).iter().any(|n| {
        n.name().as_ref() == "VariableName"
            && (inputs.contains(&(scope(n), text(s, n).into()))
                || matches!(
                    text(s, n),
                    "$_GET" | "$_POST" | "$_REQUEST" | "$_COOKIE" | "$_FILES"
                )
                || matches!(
                    text(s, n),
                    "$input" | "$data" | "$payload" | "$request" | "$serialized"
                ))
    })
}
fn safe_unserialize(s: &str, args: &[(Option<String>, SyntaxNode)]) -> bool {
    args.iter().any(|(_, n)| {
        n.name().as_ref() == "ArrayExpression"
            && descendants(n).iter().any(|p| {
                p.name().as_ref() == "Pair" && {
                    let c: Vec<_> = p.children().collect();
                    c.first()
                        .is_some_and(|n| text(s, n).trim_matches(['\'', '"']) == "allowed_classes")
                        && c.last()
                            .is_some_and(|n| text(s, n).eq_ignore_ascii_case("false"))
                }
            })
    })
}
fn dynamic_concat(n: &SyntaxNode) -> bool {
    let all = descendants(n);
    all.iter().any(|n| n.name().as_ref() == "ConcatOp")
        && all.iter().any(|n| {
            matches!(
                n.name().as_ref(),
                "VariableName" | "CallExpression" | "MemberExpression" | "SubscriptExpression"
            )
        })
}
fn safe_sql_term(
    s: &str,
    n: &SyntaxNode,
    numeric: &BTreeSet<(usize, String)>,
    bindings: &BTreeMap<(usize, String), Vec<String>>,
) -> bool {
    if literal(n) || matches!(n.name().as_ref(), "Integer" | "Float" | "Boolean") {
        return true;
    }
    if n.name().as_ref() == "VariableName" && numeric.contains(&(scope(n), text(s, n).into())) {
        return true;
    }
    if n.name().as_ref() == "CastExpression"
        && n.children().any(|n| {
            n.name().as_ref() == "NamedType"
                && matches!(
                    text(s, &n).to_ascii_lowercase().as_str(),
                    "int" | "integer" | "float" | "double" | "bool" | "boolean"
                )
        })
    {
        return true;
    }
    if n.name().as_ref() == "BinaryExpression"
        && n.children().any(|n| n.name().as_ref() == "ConcatOp")
    {
        return n
            .children()
            .filter(|n| n.name().as_ref() != "ConcatOp")
            .all(|n| safe_sql_term(s, &n, numeric, bindings));
    }
    if n.name().as_ref() == "CallExpression" {
        if let Some(f) = n
            .children()
            .next()
            .filter(|n| n.name().as_ref() == "MemberExpression")
        {
            let c: Vec<_> = f.children().collect();
            if c.last()
                .is_some_and(|n| text(s, n).eq_ignore_ascii_case("quote"))
                && c.first()
                    .and_then(|n| bindings.get(&(scope(n), text(s, n).into())))
                    .is_some_and(|v| v.len() == 1 && v[0] == "pdo")
            {
                return true;
            }
        }
    }
    false
}
fn shell_dynamic(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
            continue;
        }
        if c == '$'
            && chars
                .peek()
                .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '{'))
        {
            return true;
        }
    }
    false
}
fn suppression(s: &str) -> bool {
    s.contains("phpcs:ignore") || s.contains("phpcs:disable") || s.contains("@phpstan-ignore")
}
fn reason(s: &str) -> bool {
    s.split_once("--")
        .or_else(|| s.split_once(" // "))
        .is_some_and(|(_, s)| s.chars().any(char::is_alphabetic))
        || s.split_once("(").is_some_and(|(_, s)| {
            s.split_once(')')
                .is_some_and(|(r, _)| r.chars().any(char::is_alphabetic))
        })
}
