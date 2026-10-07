//! C/C++ syntax rules. No preprocessor, compiler, or inferred taint contracts.
use cqx_schema::EdgeKind;
use cqx_syntax::{nodes, text, Context, Frontend, Language, Node, Stats};
use cqx_vfs::Vfs;
use std::io::{self, Write};

pub fn language(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()? {
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "C" | "H" => Some("cpp"),
        _ => None,
    }
}
pub fn is_source(path: &str) -> bool {
    language(path).is_some()
}

pub fn run(vfs: &Vfs, mut out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    run_with_layout(
        vfs,
        &mut out,
        has_cpp(vfs),
        tick,
        &cqx_layout::Layout::from_paths(vfs.paths()),
    )
}
pub fn has_cpp(vfs: &Vfs) -> bool {
    vfs.paths().any(|p| language(p) == Some("cpp"))
}
pub fn run_with_headers(
    vfs: &Vfs,
    mut out: impl Write,
    cpp_headers: bool,
    tick: &dyn Fn(),
) -> io::Result<Stats> {
    run_with_layout(
        vfs,
        &mut out,
        cpp_headers,
        tick,
        &cqx_layout::Layout::from_paths(vfs.paths()),
    )
}
pub fn run_with_layout(
    vfs: &Vfs,
    mut out: impl Write,
    cpp_headers: bool,
    tick: &dyn Fn(),
    layout: &cqx_layout::Layout,
) -> io::Result<Stats> {
    let mut stats = Stats::default();
    for cpp in [false, true] {
        let part = cqx_syntax::run(
            vfs,
            &mut out,
            &C {
                cpp,
                cpp_headers,
                layout,
            },
            tick,
        )?;
        stats.files += part.files;
        stats.nodes += part.nodes;
        stats.edges += part.edges;
        stats.unparsed.extend(part.unparsed);
    }
    Ok(stats)
}

struct C<'a> {
    layout: &'a cqx_layout::Layout,
    cpp: bool,
    cpp_headers: bool,
}
impl Frontend for C<'_> {
    fn language(&self) -> &'static str {
        if self.cpp {
            "cpp"
        } else {
            "c"
        }
    }
    fn grammar(&self) -> Language {
        if self.cpp {
            tree_sitter_cpp::LANGUAGE.into()
        } else {
            tree_sitter_c::LANGUAGE.into()
        }
    }
    fn is_source(&self, path: &str) -> bool {
        if path.ends_with(".h") && self.cpp_headers {
            self.cpp
        } else {
            language(path) == Some(self.language())
        }
    }
    fn excluded(&self, path: &str, source: &str) -> bool {
        self.layout.excluded(self.language(), path, source) || path.contains(".generated.")
    }
    fn is_test(&self, path: &str) -> bool {
        self.layout.is_test(self.language(), path)
    }
    fn entries(&self, _path: &str, source: &str, root: Node<'_>) -> cqx_layout::Entries {
        let mut entries = cqx_layout::Entries::default();
        for n in nodes(root) {
            if n.kind() == "function_definition" {
                let main = n
                    .child_by_field_name("declarator")
                    .is_some_and(|n| function_name(source, n) == "main");
                entries.declaration(n.byte_range(), main);
            } else if n.kind() == "lambda_expression" {
                entries.declaration(n.byte_range(), false);
            }
        }
        entries
    }
    fn test_ranges(&self, source: &str, root: Node<'_>) -> Vec<std::ops::Range<usize>> {
        nodes(root)
            .into_iter()
            .filter_map(|n| {
                let condition = if n.kind() == "preproc_ifdef"
                    && text(source, n).trim_start().starts_with("#ifdef")
                {
                    n.child_by_field_name("name")
                        .map(|n| text(source, n).trim().to_string())
                } else if n.kind() == "preproc_if" {
                    n.child_by_field_name("condition")
                        .map(|n| text(source, n).replace(' ', ""))
                        .and_then(|s| {
                            s.strip_prefix("defined(")
                                .and_then(|s| s.strip_suffix(')'))
                                .map(str::to_string)
                        })
                } else {
                    None
                };
                condition.filter(|s| {
                    matches!(
                        s.as_str(),
                        "REDIS_TEST" | "UNIT_TEST" | "UNIT_TESTS" | "UNIT_TESTING"
                    )
                })?;
                Some(
                    n.start_byte()
                        ..n.child_by_field_name("alternative")
                            .map(|n| n.start_byte())
                            .unwrap_or(n.end_byte()),
                )
            })
            .collect()
    }
    fn function<'a>(&self, source: &'a str, node: Node<'a>) -> Option<(String, Node<'a>)> {
        if node.kind() != "function_definition" {
            return None;
        }
        Some((
            function_name(source, node.child_by_field_name("declarator")?),
            node.child_by_field_name("body")?,
        ))
    }
    fn inspect(&self, c: &mut Context<'_, '_, '_>, node: Node<'_>) -> io::Result<()> {
        if node == c.root {
            prepare_file(c);
        }
        if node.kind() == "comment" || node.kind() == "preproc_call" {
            let s = c.text(node);
            let suppression = cqx_layout::broad_directive(self.language(), s);
            if suppression && !reason(s) && !preceding_reason(c.source, node) {
                c.rule(
                    "undocumented-suppressions",
                    EdgeKind::Silences,
                    node,
                    "warning suppression has no explanation",
                )?;
            }
        }
        if node.kind() != "call_expression" {
            return Ok(());
        }
        let Some(function) = node.child_by_field_name("function") else {
            return Ok(());
        };
        let name = c.text(function).trim();
        c.call(node, name)?;
        // Member calls and project-defined names are ordinary API calls.
        if !matches!(function.kind(), "identifier" | "qualified_identifier") {
            return Ok(());
        }
        let name = name
            .strip_prefix("std::")
            .or_else(|| name.strip_prefix("::"))
            .unwrap_or(name);
        if name.contains("::") {
            return Ok(());
        }
        let header = |c: &Context<'_, '_, '_>, names: &[&str]| {
            names.iter().any(|n| c.cache["headers"].contains(*n))
        };
        let standard = match name {
            "exit" | "abort" | "system" => header(c, &["<stdlib.h>", "<cstdlib>"]),
            "popen" | "sprintf" | "vsprintf" | "gets" => header(c, &["<stdio.h>", "<cstdio>"]),
            "strcpy" | "strcat" => header(c, &["<string.h>", "<cstring>"]),
            _ => false,
        };
        let standard = standard && !shadowed(c, name);
        let args: Vec<_> = node
            .child_by_field_name("arguments")
            .map(|n| {
                let mut cursor = n.walk();
                n.named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                    .collect()
            })
            .unwrap_or_default();
        if standard && matches!(name, "exit" | "abort") && !c.entry {
            c.rule(
                "exit-in-library",
                EdgeKind::Crosses,
                node,
                "standard process termination outside a main translation unit",
            )?;
        }
        if standard
            && matches!(name, "system" | "popen")
            && args.first().is_some_and(|n| !literal(*n))
        {
            c.rule(
                "shell-argument-unchecked",
                EdgeKind::Spawns,
                node,
                "shell command is non-literal; review how input reaches it",
            )?;
        }
        if standard && matches!(name, "sprintf" | "vsprintf" | "strcpy" | "strcat" | "gets") {
            c.rule(
                "unsafe-buffer-calls",
                EdgeKind::UnsafeAt,
                node,
                "unbounded C buffer API cannot check destination capacity",
            )?;
        }
        if matches!(name, "snprintf" | "sprintf")
            && standard_or_snprintf(c, name)
            && !shadowed(c, name)
        {
            let index = if name == "snprintf" { 2 } else { 1 };
            if args.get(index).is_some_and(|n| json_format(c.text(*n))) {
                c.rule(
                    "hand-built-json",
                    EdgeKind::Interpolates,
                    node,
                    "JSON object assembled with a printf format instead of a serializer",
                )?;
            }
        }
        if node
            .parent()
            .is_some_and(|n| n.kind() == "expression_statement")
            && must_check(c, name)
        {
            c.rule(
                "discarded-check",
                EdgeKind::Discards,
                node,
                "return value of a locally declared must-check function is discarded",
            )?;
        }
        Ok(())
    }
}

fn function_name(source: &str, mut n: Node<'_>) -> String {
    while let Some(next) = n.child_by_field_name("declarator").or_else(|| {
        (n.kind() == "parenthesized_declarator")
            .then(|| n.named_child(0))
            .flatten()
    }) {
        n = next;
    }
    text(source, n).to_string()
}
fn literal(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "string_literal" | "concatenated_string" | "raw_string_literal"
    ) || (node.kind() == "parenthesized_expression" && node.named_child(0).is_some_and(literal))
}
fn prepare_file(c: &mut Context<'_, '_, '_>) {
    let mut headers = std::collections::BTreeSet::new();
    let mut shadows = std::collections::BTreeSet::new();
    let mut checked = std::collections::BTreeSet::new();
    let mut parameters = std::collections::BTreeSet::new();
    for n in nodes(c.root) {
        if n.kind() == "preproc_include" {
            if let Some(p) = n.child_by_field_name("path") {
                headers.insert(c.text(p).to_string());
            }
        }
        if matches!(
            n.kind(),
            "function_definition" | "parameter_declaration" | "init_declarator"
        ) {
            if let Some(d) = n.child_by_field_name("declarator") {
                let name = function_name(c.source, d);
                if n.kind() == "parameter_declaration" {
                    parameters.insert(name.clone());
                }
                shadows.insert(name);
            }
        }
        if matches!(n.kind(), "declaration" | "function_definition") {
            let body_start = n
                .child_by_field_name("body")
                .map(|b| b.start_byte())
                .unwrap_or(n.end_byte());
            let children: Vec<_> = nodes(n)
                .into_iter()
                .filter(|n| n.end_byte() <= body_start)
                .collect();
            if children.iter().any(|a| {
                a.kind().contains("attribute")
                    && (c.text(*a).contains("nodiscard")
                        || c.text(*a).contains("warn_unused_result"))
            }) {
                for d in children
                    .iter()
                    .filter(|n| n.kind() == "function_declarator")
                {
                    checked.insert(function_name(c.source, *d));
                }
            }
        }
    }
    c.cache.insert("headers".into(), headers);
    c.cache.insert("shadows".into(), shadows);
    c.cache.insert("checked".into(), checked);
    c.cache.insert("parameters".into(), parameters);
}
fn shadowed(c: &Context<'_, '_, '_>, name: &str) -> bool {
    c.cache["shadows"].contains(name)
}
fn standard_or_snprintf(c: &Context<'_, '_, '_>, _name: &str) -> bool {
    c.cache["headers"].contains("<stdio.h>") || c.cache["headers"].contains("<cstdio>")
}
fn must_check(c: &Context<'_, '_, '_>, name: &str) -> bool {
    c.cache["checked"].contains(name) && !c.cache["parameters"].contains(name)
}
fn reason(s: &str) -> bool {
    let comment = s.trim_start_matches(['/', '*']).trim();
    if let Some(rest) = ["NOLINTNEXTLINE", "NOLINTBEGIN", "NOLINT"]
        .iter()
        .find_map(|tag| comment.strip_prefix(tag))
    {
        let rest = if rest.starts_with('(') {
            rest.split_once(')').map(|(_, rest)| rest).unwrap_or("")
        } else {
            rest
        };
        return !rest.trim().trim_end_matches("*/").trim().is_empty();
    }
    s.split_once("--")
        .or_else(|| s.split_once(" because "))
        .or_else(|| s.split_once("//"))
        .or_else(|| s.split_once("/*"))
        .is_some_and(|(_, r)| !r.trim().trim_end_matches("*/").trim().is_empty())
}
fn preceding_reason(source: &str, n: Node<'_>) -> bool {
    source[..n.start_byte()]
        .lines()
        .next_back()
        .is_some_and(|s| {
            let s = s.trim();
            s.starts_with("//")
                && s.trim_start_matches('/').trim().len() > 8
                && !s.contains("NOLINT")
        })
}
fn json_format(s: &str) -> bool {
    let s = s.trim().trim_start_matches('"').trim_end_matches('"');
    s.starts_with('{')
        && s.ends_with('}')
        && s.contains("\\\"")
        && s.contains(':')
        && s.contains('%')
}
