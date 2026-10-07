//! Swift 6.3 syntax facts. Standard API bindings are deliberately conservative.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Stats};
use cqx_vfs::Vfs;
use rezel_common::{IterMode, SyntaxNode, TypedNode};
use rezel_lang_swift::SwiftFunctionDeclaration;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    ops::Range,
};
pub fn is_source(p: &str) -> bool {
    p.ends_with(".swift")
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "swift", "Swift 6.3")?;
    for path in vfs.paths().filter(|p| is_source(p)) {
        tick();
        let source = vfs.read(path).unwrap_or_default();
        if excluded(path, source) {
            continue;
        }
        let tree = match rezel_lang_swift::parser().with_strict(true).parse(source) {
            Ok(t) => t,
            Err(e) => {
                emitter.skip(path, &e.to_string())?;
                continue;
            }
        };
        let mut cursor = tree.cursor(IterMode::NONE);
        let mut all = Vec::new();
        loop {
            all.push(cursor.node());
            if !cursor.next(true) {
                break;
            }
        }
        let mut shadows = BTreeSet::new();
        for n in &all {
            if matches!(
                n.name().as_ref(),
                "FunctionName" | "TypeName" | "ParameterNames" | "IdentifierPattern"
            ) {
                for i in descendants(n)
                    .iter()
                    .filter(|i| i.name().as_ref() == "Identifier")
                {
                    shadows.insert(text(source, i).trim_matches('`').to_owned());
                }
            }
            if n.name().as_ref() == "ImportDeclaration" {
                let name = text(source, n).trim_start_matches("import ").trim();
                if !matches!(name, "Foundation" | "Darwin" | "Glibc" | "Swift") {
                    // selective imports can shadow the standard names
                    if let Some(last) = name.rsplit('.').next() {
                        shadows.insert(last.to_owned());
                    }
                }
            }
        }
        let entry = path.rsplit('/').next() == Some("main.swift")
            || all.iter().any(|n| {
                (n.name().as_ref() == "AttributeName" && text(source, n) == "@main")
                    || (n.name().as_ref() == "ExpressionStatement"
                        && n.parent()
                            .and_then(|p| p.parent())
                            .is_some_and(|p| p.name().as_ref() == "SourceFile"))
            });
        let mut sink = emitter.file(
            path,
            source,
            is_test(path)
                || all.iter().any(|n| {
                    n.name().as_ref() == "ImportDeclaration"
                        && matches!(text(source, n).trim(), "import XCTest" | "import Testing")
                }),
            &tree,
        )?;
        for n in &all {
            if let Ok(f) = SwiftFunctionDeclaration::downcast_from(n.clone()) {
                if let Some(body) = f.body() {
                    sink.symbol(
                        &f.name()
                            .map(|n| text(source, n.syntax()).to_owned())
                            .unwrap_or_default(),
                        range(n),
                        Some(range(body.syntax())),
                    )?;
                }
            }
        }
        // A locally constructed Foundation.Process, in the same function, is enough to bind run/launch.
        // Ambiguous redeclarations are declined rather than guessed.
        let mut processes: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
        if !shadows.contains("Process")
            && all.iter().any(|n| {
                n.name().as_ref() == "ImportDeclaration"
                    && text(source, n).trim() == "import Foundation"
            })
        {
            for n in &all {
                if n.name().as_ref() == "PatternBinding" {
                    let parts: Vec<_> = n.children().collect();
                    if let (Some(id), Some(init)) = (parts.first(), parts.get(1)) {
                        if id.name().as_ref() == "IdentifierPattern"
                            && descendants(init).iter().any(|c| {
                                c.name().as_ref() == "FunctionCallExpression"
                                    && callee(source, c) == "Process"
                            })
                        {
                            processes
                                .entry((scope(n), text(source, id).to_owned()))
                                .or_default()
                                .push(range(n).start);
                        }
                    }
                }
            }
        }
        for n in &all {
            match n.name().as_ref() {
                "CatchClause" => {
                    if let Some(b) = n.children().find(|n| n.name().as_ref() == "CodeBlock") {
                        if !b.children().any(|n| n.name().as_ref() == "CodeBlockItem")
                            && !sink.documented(&range(&b))
                        {
                            sink.rule(
                                "swallowed-errors",
                                EdgeKind::Discards,
                                range(n),
                                "empty catch discards an error without an explanation",
                            )?;
                        }
                    }
                }
                "ForceUnwrapExpression" => sink.rule(
                    "forced-operations",
                    EdgeKind::Discards,
                    range(n),
                    "force unwrap can trap instead of handling nil",
                )?,
                "TryExpression" => {
                    if n.children().any(|c| {
                        c.name().as_ref() == "TrySuffix" && text(source, &c) == "!"
                            || c.name().as_ref() == "PrefixOperatorExpression"
                                && c.children().next().is_some_and(|p| text(source, &p) == "!")
                    }) {
                        sink.rule(
                            "forced-operations",
                            EdgeKind::Discards,
                            range(n),
                            "try! can trap instead of handling a thrown error",
                        )?;
                    }
                }
                "FunctionCallExpression" => {
                    let name = callee(source, n);
                    sink.call(&name, range(n))?;
                    if !entry
                        && (matches!(
                            name.as_str(),
                            "Swift.fatalError" | "Darwin.exit" | "Glibc.exit"
                        ) || matches!(name.as_str(), "exit" | "fatalError")
                            && !shadows.contains(&name))
                    {
                        sink.rule(
                            "exit-in-library",
                            EdgeKind::Crosses,
                            range(n),
                            "process termination outside an entry translation unit",
                        )?;
                    }
                    if let Some(receiver) = name
                        .strip_suffix(".run")
                        .or_else(|| name.strip_suffix(".launch"))
                    {
                        let key = (scope(n), receiver.to_owned());
                        if let Some(defs) = processes.get(&key).filter(|v| v.len() == 1) {
                            let dynamic = all
                                .iter()
                                .filter(|a| {
                                    a.name().as_ref() == "SequenceExpression"
                                        && scope(a) == key.0
                                        && range(a).start > defs[0]
                                        && range(a).end < range(n).start
                                })
                                .filter_map(|a| {
                                    dynamic_path(source, a, receiver).map(|d| (range(a).start, d))
                                })
                                .max_by_key(|(start, _)| *start)
                                .is_some_and(|(_, dynamic)| dynamic);
                            if dynamic {
                                sink.rule("nonliteral-process",EdgeKind::Spawns,range(n),"locally constructed Foundation.Process has a non-literal executable path")?;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(emitter.finish())
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
        if n.name().as_ref() == "FunctionDeclaration" {
            return range(&n).start;
        }
        p = n.parent()
    }
    usize::MAX
}
fn name(s: &str, n: &SyntaxNode) -> String {
    match n.name().as_ref() {
        "DeclReferenceExpression" => text(s, n).trim_matches('`').to_owned(),
        "MemberAccessExpression" => n
            .children()
            .filter(|n| {
                matches!(
                    n.name().as_ref(),
                    "DeclReferenceExpression" | "MemberAccessExpression"
                )
            })
            .map(|n| name(s, &n))
            .collect::<Vec<_>>()
            .join("."),
        _ => String::new(),
    }
}
fn callee(s: &str, n: &SyntaxNode) -> String {
    n.children().next().map(|n| name(s, &n)).unwrap_or_default()
}
fn literal(n: &SyntaxNode) -> bool {
    n.name().as_ref() == "StringLiteralExpression"
        && !descendants(n)
            .iter()
            .any(|n| n.name().contains("Interpolation"))
}
fn dynamic_path(s: &str, n: &SyntaxNode, receiver: &str) -> Option<bool> {
    let parts: Vec<_> = n.children().collect();
    if parts.len() != 3 || text(s, &parts[1]) != "=" {
        return None;
    }
    let member = name(s, &parts[0]);
    let rhs = &parts[2];
    if member == format!("{receiver}.launchPath") {
        return Some(!literal(rhs));
    }
    if member != format!("{receiver}.executableURL") {
        return None;
    }
    if rhs.name().as_ref() == "FunctionCallExpression" && callee(s, rhs) == "URL" {
        if let Some(arg) = descendants(rhs)
            .into_iter()
            .find(|n| n.name().as_ref() == "LabeledExpression")
        {
            return Some(arg.children().last().is_some_and(|n| !literal(&n)));
        }
    }
    Some(true)
}
fn excluded(p: &str, s: &str) -> bool {
    p.split('/').any(|p| {
        matches!(
            p,
            ".build" | "Pods" | "DerivedData" | "Carthage" | "vendor" | "generated"
        )
    }) || s.lines().take(15).any(|s| {
        let s = s.trim().to_ascii_lowercase();
        (s.starts_with("//") || s.starts_with("/*") || s.starts_with('*'))
            && (s.contains("generated") || s.contains("do not edit"))
    })
}
fn is_test(p: &str) -> bool {
    p.split('/').any(|p| {
        matches!(p, "Tests" | "tests" | "Benchmarks")
            || p.ends_with("Tests")
            || p.ends_with("TestHelpers")
    }) || p.ends_with("Tests.swift")
}
