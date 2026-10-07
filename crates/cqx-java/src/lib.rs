//! Java SE 26 analysis using a strict pure-Rust parser and decoded AST names.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Stats};
use cqx_vfs::Vfs;
use rezel_lang_java::ast::{
    AstNodeId as Id, JavaAst as Ast, JavaAstField as F, JavaAstKind as K, JavaAstProperty as P,
    JavaModifier as M, JavaPrimitiveKind,
};
use std::{
    collections::BTreeSet,
    io::{self, Write},
    ops::Range,
};
pub fn is_source(path: &str) -> bool {
    path.ends_with(".java")
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "java", "Java SE 26")?;
    for path in vfs.paths().filter(|p| is_source(p)) {
        let source = vfs.read(path).unwrap_or_default();
        tick();
        if excluded(path, source) {
            continue;
        }
        let tree = match rezel_lang_java::parser().with_strict(true).parse(source) {
            Ok(t) => t,
            Err(e) => {
                emitter.skip(path, &e.to_string())?;
                continue;
            }
        };
        let ast =
            match Ast::lower_with_file_name(&tree, source, path.rsplit('/').next().unwrap_or(path))
            {
                Ok(a) => a,
                Err(e) => {
                    emitter.skip(path, &e.to_string())?;
                    continue;
                }
            };
        let ids: Vec<_> = (0..ast.nodes().len()).filter_map(Id::from_index).collect();
        let entry = ids.iter().any(|&id| is_entry(&ast, id));
        let mut shadows = BTreeSet::new();
        for &id in &ids {
            match kind(&ast, id) {
                K::Class | K::Interface | K::Enum | K::Record | K::AnnotationType | K::Variable => {
                    shadows.insert(name(&ast, id));
                }
                K::Import => {
                    if let Some(q) = child(&ast, id, F::QualifiedIdentifier) {
                        let q = expr(&ast, q);
                        if !q.starts_with("java.lang.") {
                            shadows.insert(q.rsplit('.').next().unwrap_or("").to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        let mut sink = emitter.file(path, source, is_test(path), &tree)?;
        for &id in &ids {
            if kind(&ast, id) == K::Method {
                if let Some(r) = span(&ast, id) {
                    sink.symbol(
                        &name(&ast, id),
                        r,
                        child(&ast, id, F::Body).and_then(|b| span(&ast, b)),
                    )?;
                }
            }
        }
        for id in ids {
            let Some(range) = span(&ast, id) else {
                continue;
            };
            match kind(&ast, id) {
                K::Catch => {
                    if let Some(b) = child(&ast, id, F::Block) {
                        if children(&ast, b, F::Statements).is_empty()
                            && span(&ast, b).is_some_and(|r| !sink.documented(&r))
                        {
                            sink.rule(
                                "swallowed-errors",
                                EdgeKind::Discards,
                                range,
                                "empty catch discards an error without an explanation",
                            )?;
                        }
                    }
                }
                K::Annotation | K::TypeAnnotation => {
                    if child(&ast, id, F::AnnotationType).is_some_and(|n| {
                        matches!(
                            expr(&ast, n).as_str(),
                            "SuppressWarnings" | "java.lang.SuppressWarnings"
                        )
                    }) && !shadows.contains("SuppressWarnings")
                        && !sink.has_reason(&range)
                    {
                        sink.rule(
                            "undocumented-suppressions",
                            EdgeKind::Silences,
                            range,
                            "SuppressWarnings annotation has no explanation",
                        )?;
                    }
                }
                K::NewClass => {
                    if child(&ast, id, F::Identifier).is_some_and(|n| {
                        matches!(
                            expr(&ast, n).as_str(),
                            "ProcessBuilder" | "java.lang.ProcessBuilder"
                        )
                    }) && !shadows.contains("ProcessBuilder")
                        && child(&ast, id, F::Arguments).is_some_and(|a| !literal(&ast, a))
                    {
                        sink.rule(
                            "nonliteral-process",
                            EdgeKind::Spawns,
                            range,
                            "ProcessBuilder executable is non-literal; review its source",
                        )?;
                    }
                }
                K::MethodInvocation => {
                    let call = child(&ast, id, F::MethodSelect)
                        .map(|n| expr(&ast, n))
                        .unwrap_or_default();
                    sink.call(&call, range.clone())?;
                    if matches!(call.as_str(), "System.exit" | "java.lang.System.exit")
                        && !shadows.contains("System")
                        && !entry
                    {
                        sink.rule(
                            "exit-in-library",
                            EdgeKind::Crosses,
                            range.clone(),
                            "System.exit outside a main entry translation unit",
                        )?;
                    }
                    if matches!(
                        call.as_str(),
                        "Runtime.getRuntime().exec" | "java.lang.Runtime.getRuntime().exec"
                    ) && !shadows.contains("Runtime")
                        && child(&ast, id, F::Arguments).is_some_and(|a| !literal(&ast, a))
                    {
                        sink.rule(
                            "nonliteral-process",
                            EdgeKind::Spawns,
                            range,
                            "Runtime.exec command is non-literal; review its source",
                        )?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(emitter.finish())
}
fn kind(a: &Ast, id: Id) -> K {
    a.node(id).unwrap().kind()
}
fn span(a: &Ast, id: Id) -> Option<Range<usize>> {
    let r = a.node(id)?.source_range().byte_range()?;
    Some(usize::from(r.start())..usize::from(r.end()))
}
fn children(a: &Ast, id: Id, f: F) -> Vec<Id> {
    a.edges(id)
        .unwrap_or_default()
        .iter()
        .filter(|e| e.field() == f)
        .map(|e| e.node())
        .collect()
}
fn child(a: &Ast, id: Id, f: F) -> Option<Id> {
    children(a, id, f).first().copied()
}
fn name(a: &Ast, id: Id) -> String {
    a.properties(id)
        .unwrap_or_default()
        .iter()
        .find_map(|p| {
            if let P::Name(s) = p {
                a.string(*s).map(str::to_owned)
            } else {
                None
            }
        })
        .unwrap_or_default()
}
fn expr(a: &Ast, id: Id) -> String {
    match kind(a, id) {
        K::MemberSelect => format!(
            "{}.{}",
            child(a, id, F::Expression)
                .map(|n| expr(a, n))
                .unwrap_or_default(),
            name(a, id)
        ),
        K::MethodInvocation => format!(
            "{}()",
            child(a, id, F::MethodSelect)
                .map(|n| expr(a, n))
                .unwrap_or_default()
        ),
        K::ArrayType => format!(
            "{}[]",
            child(a, id, F::Type)
                .map(|n| expr(a, n))
                .unwrap_or_default()
        ),
        _ => name(a, id),
    }
}
fn literal(a: &Ast, id: Id) -> bool {
    match kind(a, id) {
        K::StringLiteral => true,
        K::Parenthesized => child(a, id, F::Expression).is_some_and(|n| literal(a, n)),
        K::Plus => {
            child(a, id, F::LeftOperand).is_some_and(|n| literal(a, n))
                && child(a, id, F::RightOperand).is_some_and(|n| literal(a, n))
        }
        K::NewArray => child(a, id, F::Initializers).is_some_and(|n| literal(a, n)),
        _ => false,
    }
}
fn is_entry(a: &Ast, id: Id) -> bool {
    if kind(a, id) != K::Method || name(a, id) != "main" {
        return false;
    }
    if child(a, id, F::Modifiers).is_some_and(|m| {
        a.properties(m)
            .unwrap_or_default()
            .contains(&P::Modifier(M::Private))
    }) {
        return false;
    }
    if !child(a, id, F::ReturnType).is_some_and(|r| {
        a.properties(r)
            .unwrap_or_default()
            .contains(&P::PrimitiveKind(JavaPrimitiveKind::Void))
    }) {
        return false;
    }
    let params = children(a, id, F::Parameters);
    params.is_empty()
        || (params.len() == 1
            && child(a, params[0], F::Type)
                .is_some_and(|t| matches!(expr(a, t).as_str(), "String[]" | "java.lang.String[]")))
}
fn excluded(path: &str, source: &str) -> bool {
    path.split('/').any(|s| {
        matches!(
            s,
            "build" | "target" | "generated-sources" | "generated" | "vendor"
        )
    }) || source.lines().any(|s| {
        [
            "@Generated",
            "@javax.annotation.Generated",
            "@javax.annotation.processing.Generated",
            "@jakarta.annotation.Generated",
        ]
        .iter()
        .any(|a| {
            s.trim_start()
                .strip_prefix(a)
                .is_some_and(|r| r.trim_start().starts_with('('))
        })
    }) || source.lines().take(12).any(|s| {
        let s = s.trim().to_ascii_lowercase();
        (s.starts_with("//") || s.starts_with("/*") || s.starts_with('*'))
            && (s.contains("@generated") || s.contains("generated by") || s.contains("do not edit"))
    })
}
fn is_test(path: &str) -> bool {
    path.split('/')
        .any(|s| matches!(s, "test" | "tests" | "testFixtures" | "androidTest"))
        || path.rsplit('/').next().is_some_and(|s| {
            s.ends_with("Test.java") || s.ends_with("Tests.java") || s.starts_with("Test")
        })
}
