//! Kotlin 2.4 syntax facts with conservative standard-API binding.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Stats};
use cqx_vfs::Vfs;
use rezel_common::{IterMode, SyntaxNode, TypedNode};
use rezel_lang_kotlin::{
    KotlinCallExpression, KotlinCatchClause, KotlinExpression, KotlinFunctionDeclaration,
    KotlinImportHeader,
};
use std::{
    collections::BTreeSet,
    io::{self, Write},
    ops::Range,
};
pub fn is_source(path: &str) -> bool {
    path.ends_with(".kt") || path.ends_with(".kts")
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "kotlin", "Kotlin 2.4")?;
    for path in vfs.paths().filter(|p| is_source(p)) {
        let source = vfs.read(path).unwrap_or_default();
        tick();
        if excluded(path, source) {
            continue;
        }
        let tree = match rezel_lang_kotlin::parser().with_strict(true).parse(source) {
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
        let mut exit_aliases = BTreeSet::new();
        let mut runtime_aliases = BTreeSet::from(["Runtime".to_owned()]);
        for n in &all {
            if n.name().as_ref() == "Definition"
                && !parents(n)
                    .iter()
                    .any(|p| p.name().as_ref() == "ImportAlias")
            {
                shadows.insert(ident(source, n));
            }
            if let Ok(i) = KotlinImportHeader::downcast_from(n.clone()) {
                if let Some(p) = i.path() {
                    let name = p
                        .segments()
                        .map(|n| ident(source, n.syntax()))
                        .collect::<Vec<_>>()
                        .join(".");
                    let local = i
                        .alias()
                        .and_then(|a| a.name())
                        .map(|n| ident(source, n.syntax()))
                        .unwrap_or_else(|| name.rsplit('.').next().unwrap_or("").into());
                    match name.as_str() {
                        "kotlin.system.exitProcess" => {
                            exit_aliases.insert(local);
                        }
                        "java.lang.Runtime" => {
                            runtime_aliases.insert(local);
                        }
                        _ => {
                            shadows.insert(local);
                        }
                    }
                }
            }
        }
        let entry = path.ends_with(".kts") || all.iter().any(|n| entry(source, n));
        let mut sink = emitter.file(path, source, is_test(path), &tree)?;
        for n in &all {
            if let Ok(f) = KotlinFunctionDeclaration::downcast_from(n.clone()) {
                if let Some(body) = f.body() {
                    let name = f
                        .direct_name()
                        .map(|n| ident(source, n.syntax()))
                        .unwrap_or_else(|| "<extension>".into());
                    sink.symbol(&name, range(n), Some(range(body.syntax())))?;
                }
            }
        }
        for n in &all {
            if let Ok(c) = KotlinCatchClause::downcast_from(n.clone()) {
                if let Some(b) = c.block() {
                    if b.statements().is_none_or(|s| s.items().next().is_none())
                        && !sink.documented(&range(b.syntax()))
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
            if matches!(n.name().as_ref(), "AnnotationEntry" | "FileAnnotation") {
                if let Some(a) = descendants(n)
                    .into_iter()
                    .find(|n| n.name().as_ref() == "UnescapedAnnotation")
                {
                    let name = text(source, &a).split('(').next().unwrap_or("").trim();
                    if matches!(name, "Suppress" | "kotlin.Suppress")
                        && !shadows.contains("Suppress")
                        && !sink.has_reason(&range(n))
                    {
                        sink.rule(
                            "undocumented-suppressions",
                            EdgeKind::Silences,
                            range(n),
                            "Suppress annotation has no explanation",
                        )?;
                    }
                }
            }
            if let Ok(c) = KotlinCallExpression::downcast_from(n.clone()) {
                let Some(receiver) = c.receiver() else {
                    continue;
                };
                let name = expression_name(source, &receiver);
                sink.call(&name, range(n))?;
                if !entry
                    && (name == "kotlin.system.exitProcess"
                        || (exit_aliases.contains(&name) && !shadows.contains(&name)))
                {
                    sink.rule(
                        "exit-in-library",
                        EdgeKind::Crosses,
                        range(n),
                        "exitProcess outside a main entry translation unit",
                    )?;
                }
                let runtime = name.strip_suffix(".getRuntime().exec");
                if runtime.is_some_and(|r| {
                    r == "java.lang.Runtime"
                        || (runtime_aliases.contains(r) && !shadows.contains(r))
                }) && first_arg(&c).is_some_and(|a| !literal(source, &a))
                {
                    sink.rule(
                        "nonliteral-process",
                        EdgeKind::Spawns,
                        range(n),
                        "Runtime.exec command is non-literal; review its source",
                    )?;
                }
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
fn ident(s: &str, n: &SyntaxNode) -> String {
    text(s, n).trim_matches('`').to_owned()
}
fn parents(n: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut p = n.parent();
    let mut result = Vec::new();
    while let Some(n) = p {
        p = n.parent();
        result.push(n);
    }
    result
}
fn descendants(n: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut result = Vec::new();
    let mut todo = vec![n.clone()];
    while let Some(n) = todo.pop() {
        todo.extend(n.children());
        result.push(n);
    }
    result
}
fn expression_name(s: &str, e: &KotlinExpression) -> String {
    match e {
        KotlinExpression::Name(n) => ident(s, n.syntax()),
        KotlinExpression::Member(m) => format!(
            "{}.{}",
            m.receiver()
                .map(|r| expression_name(s, &r))
                .unwrap_or_default(),
            m.member().map(|n| ident(s, n.syntax())).unwrap_or_default()
        ),
        KotlinExpression::Call(c) => format!(
            "{}()",
            c.receiver()
                .map(|r| expression_name(s, &r))
                .unwrap_or_default()
        ),
        _ => String::new(),
    }
}
fn first_arg(c: &KotlinCallExpression) -> Option<KotlinExpression> {
    c.suffix()?.arguments()?.arguments().next()?.expression()
}
fn literal(s: &str, e: &KotlinExpression) -> bool {
    match e {
        KotlinExpression::Literal(n) => {
            let nodes = descendants(n.syntax());
            nodes.iter().any(|n| n.name().as_ref() == "StringLiteral")
                && !nodes.iter().any(|n| n.name().starts_with("Interpolated"))
        }
        KotlinExpression::String(n) => !descendants(n.syntax())
            .iter()
            .any(|n| n.name().starts_with("Interpolated")),
        KotlinExpression::Parenthesized(n) => n.expression().is_some_and(|e| literal(s, &e)),
        KotlinExpression::Call(c) => {
            c.receiver()
                .is_some_and(|r| expression_name(s, &r) == "arrayOf")
                && first_arg(c).is_some_and(|a| literal(s, &a))
        }
        _ => false,
    }
}
fn entry(s: &str, n: &SyntaxNode) -> bool {
    let Ok(f) = KotlinFunctionDeclaration::downcast_from(n.clone()) else {
        return false;
    };
    if !f
        .direct_name()
        .is_some_and(|n| ident(s, n.syntax()) == "main")
        || f.qualified_receiver().is_some()
    {
        return false;
    }
    let ancestors = parents(n);
    if ancestors
        .iter()
        .any(|n| n.name().as_ref() == "FunctionDeclaration")
    {
        return false;
    }
    if ancestors
        .iter()
        .any(|n| matches!(n.name().as_ref(), "ClassDeclaration" | "ObjectDeclaration"))
        && !f.modifiers().is_some_and(|m| {
            descendants(m.syntax())
                .iter()
                .any(|n| n.name().as_ref() == "UnescapedAnnotation" && text(s, n) == "JvmStatic")
        })
    {
        return false;
    }
    if let Some(t) = f.return_type() {
        let ty = text(s, t.syntax()).trim().trim_start_matches(':').trim();
        if !matches!(ty, "Unit" | "kotlin.Unit") {
            return false;
        }
    }
    let params: Vec<_> = f
        .parameters()
        .map(|p| p.parameters().collect())
        .unwrap_or_default();
    params.is_empty()
        || (params.len() == 1 && {
            let nodes = descendants(params[0].syntax());
            let vararg = nodes.iter().any(|n| text(s, n) == "vararg");
            nodes.iter().any(|n| {
                n.name().as_ref() == "TypeAnnotation" && {
                    let ty = text(s, n)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>();
                    matches!(
                        ty.trim_start_matches(':'),
                        "Array<String>" | "Array<kotlin.String>"
                    ) || (vararg
                        && matches!(ty.trim_start_matches(':'), "String" | "kotlin.String"))
                }
            })
        })
}
fn excluded(path: &str, s: &str) -> bool {
    path.split('/').any(|p| {
        matches!(
            p,
            "build" | "target" | ".gradle" | "generated" | "generated-sources" | "vendor"
        )
    }) || s.lines().take(15).any(|s| {
        let s = s.trim().to_ascii_lowercase();
        (s.starts_with("//") || s.starts_with("/*") || s.starts_with('*'))
            && (s.contains("generated by") || s.contains("do not edit") || s.contains("@generated"))
    }) || s.lines().any(|s| {
        [
            "@Generated(",
            "@javax.annotation.Generated(",
            "@javax.annotation.processing.Generated(",
        ]
        .iter()
        .any(|m| s.trim_start().starts_with(m))
    })
}
fn is_test(path: &str) -> bool {
    path.split('/').any(|p| {
        matches!(p, "test" | "tests" | "jmh" | "benchmark" | "benchmarks") || p.ends_with("Test")
    }) || path.ends_with("Test.kt")
        || path.ends_with("Tests.kt")
}
