//! C# syntax facts. Standard API names are resolved conservatively within each file.
use cqx_schema::EdgeKind;
use cqx_syntax::{nodes, text, Context, Frontend, Language, Node, Stats};
use cqx_vfs::Vfs;
use std::io::{self, Write};
pub fn is_source(path: &str) -> bool {
    path.ends_with(".cs") || path.ends_with(".csx")
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
    cqx_syntax::run(vfs, out, &CSharp { layout }, tick)
}
struct CSharp<'a> {
    layout: &'a cqx_layout::Layout,
}
impl Frontend for CSharp<'_> {
    fn needs_final_newline(&self) -> bool {
        true
    }
    fn language(&self) -> &'static str {
        "csharp"
    }
    fn grammar(&self) -> Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }
    fn is_source(&self, path: &str) -> bool {
        is_source(path)
    }
    fn excluded(&self, path: &str, source: &str) -> bool {
        self.layout.excluded("csharp", path, source)
            || path.ends_with(".g.cs")
            || path.ends_with(".g.i.cs")
            || path.ends_with(".Designer.cs")
    }
    fn is_test(&self, path: &str) -> bool {
        self.layout.is_test("csharp", path)
    }
    fn entries(&self, path: &str, source: &str, root: Node<'_>) -> cqx_layout::Entries {
        let mut entries = cqx_layout::Entries::default();
        let all = nodes(root);
        entries
            .script(path.ends_with(".csx") || all.iter().any(|n| n.kind() == "global_statement"));
        for n in &all {
            let main = n.kind() == "method_declaration"
                && n.child_by_field_name("name")
                    .is_some_and(|n| text(source, n) == "Main")
                && n.child_by_field_name("returns").is_some_and(|n| {
                    matches!(
                        compact(text(source, n)).as_str(),
                        "void"
                            | "int"
                            | "Task"
                            | "Task<int>"
                            | "System.Threading.Tasks.Task"
                            | "System.Threading.Tasks.Task<int>"
                    )
                })
                && children(*n)
                    .iter()
                    .any(|&m| m.kind() == "modifier" && text(source, m) == "static")
                && n.child_by_field_name("parameters").is_some_and(|p| {
                    let mut c = p.walk();
                    let params: Vec<_> = p
                        .named_children(&mut c)
                        .filter(|n| n.kind() == "parameter")
                        .collect();
                    params.is_empty()
                        || (params.len() == 1
                            && params[0].child_by_field_name("type").is_some_and(|t| {
                                matches!(
                                    compact(text(source, t)).as_str(),
                                    "string[]" | "String[]" | "System.String[]"
                                )
                            }))
                });
            if matches!(
                n.kind(),
                "method_declaration"
                    | "constructor_declaration"
                    | "local_function_statement"
                    | "lambda_expression"
                    | "anonymous_method_expression"
            ) {
                entries.declaration(n.byte_range(), main);
            }
        }
        entries
    }
    fn test_ranges(&self, source: &str, root: Node<'_>) -> Vec<std::ops::Range<usize>> {
        nodes(root)
            .into_iter()
            .filter(|n| n.kind() == "attribute")
            .filter(|n| {
                n.child_by_field_name("name").is_some_and(|n| {
                    matches!(
                        text(source, n)
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .trim_end_matches("Attribute"),
                        "Fact"
                            | "Theory"
                            | "Test"
                            | "TestCase"
                            | "TestMethod"
                            | "DataTestMethod"
                            | "TestFixture"
                    )
                })
            })
            .filter_map(|n| {
                let mut p = n.parent();
                while let Some(n) = p {
                    if matches!(n.kind(), "method_declaration" | "class_declaration") {
                        return Some(n.byte_range());
                    }
                    p = n.parent();
                }
                None
            })
            .collect()
    }
    fn function<'a>(&self, source: &'a str, n: Node<'a>) -> Option<(String, Node<'a>)> {
        if !matches!(
            n.kind(),
            "method_declaration" | "constructor_declaration" | "local_function_statement"
        ) {
            return None;
        }
        Some((
            text(source, n.child_by_field_name("name")?).to_owned(),
            n.child_by_field_name("body")?,
        ))
    }
    fn inspect(&self, c: &mut Context<'_, '_, '_>, n: Node<'_>) -> io::Result<()> {
        if n == c.root {
            let mut shadows = std::collections::BTreeSet::new();
            for n in nodes(c.root) {
                if matches!(
                    n.kind(),
                    "class_declaration"
                        | "struct_declaration"
                        | "record_declaration"
                        | "parameter"
                        | "variable_declarator"
                        | "using_directive"
                ) {
                    if let Some(name) = n.child_by_field_name("name") {
                        shadows.insert(c.text(name).trim_start_matches('@').to_owned());
                    }
                }
            }
            c.cache.insert("shadows".into(), shadows);
        }
        if n.kind() == "catch_clause" {
            if let Some(b) = n.child_by_field_name("body") {
                let mut cursor = b.walk();
                if !b
                    .named_children(&mut cursor)
                    .any(|n| !n.kind().contains("comment"))
                    && !nodes(b)
                        .iter()
                        .any(|&n| n.kind().contains("comment") && !comment(c.text(n)).is_empty())
                {
                    c.rule(
                        "swallowed-errors",
                        EdgeKind::Discards,
                        n,
                        "empty catch discards an error without an explanation",
                    )?;
                }
            }
        }
        if n.kind() == "preproc_pragma" {
            let s = c.text(n);
            if cqx_layout::broad_directive("csharp", s) && !reason(c, n) {
                c.rule(
                    "undocumented-suppressions",
                    EdgeKind::Silences,
                    n,
                    "pragma warning disable has no explanation",
                )?;
            }
        }
        if n.kind() != "invocation_expression" {
            return Ok(());
        }
        let Some(f) = n.child_by_field_name("function") else {
            return Ok(());
        };
        let name = compact(c.text(f)).replace('@', "");
        c.call(n, &name)?;
        if matches!(
            name.as_str(),
            "Environment.Exit" | "System.Environment.Exit" | "global::System.Environment.Exit"
        ) && !c.cache["shadows"].contains("Environment")
            && !c.entry
        {
            c.rule(
                "exit-in-library",
                EdgeKind::Crosses,
                n,
                "Environment.Exit outside an entry translation unit",
            )?;
        }
        if matches!(
            name.as_str(),
            "Process.Start"
                | "System.Diagnostics.Process.Start"
                | "global::System.Diagnostics.Process.Start"
        ) && !c.cache["shadows"].contains("Process")
            && first_arg(n).is_some_and(|a| !literal(c.source, a))
        {
            c.rule(
                "nonliteral-process",
                EdgeKind::Spawns,
                n,
                "Process.Start executable is non-literal; review its source",
            )?;
        }
        Ok(())
    }
}
fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}
fn first_arg(n: Node<'_>) -> Option<Node<'_>> {
    let a = n.child_by_field_name("arguments")?;
    let mut c = a.walk();
    let first = a.named_children(&mut c).find(|n| n.kind() == "argument");
    first.and_then(|n| {
        let mut c = n.walk();
        let v = n
            .named_children(&mut c)
            .filter(|n| !n.kind().contains("comment"))
            .last();
        v
    })
}
fn literal(source: &str, n: Node<'_>) -> bool {
    if n.kind() == "object_creation_expression"
        && n.child_by_field_name("type").is_some_and(|t| {
            matches!(
                compact(text(source, t)).as_str(),
                "ProcessStartInfo" | "System.Diagnostics.ProcessStartInfo"
            )
        })
    {
        if let Some(a) = first_arg(n) {
            return literal(source, a);
        }
        return nodes(n).into_iter().any(|a| {
            a.kind() == "assignment_expression"
                && a.child_by_field_name("left")
                    .is_some_and(|v| text(source, v) == "FileName")
                && a.child_by_field_name("right")
                    .is_some_and(|v| literal(source, v))
        });
    }

    matches!(
        n.kind(),
        "string_literal" | "verbatim_string_literal" | "raw_string_literal"
    ) || (n.kind() == "parenthesized_expression"
        && n.named_child(0).is_some_and(|n| literal(source, n)))
        || (n.kind() == "binary_expression"
            && n.child_by_field_name("operator")
                .is_some_and(|n| text(source, n) == "+")
            && n.child_by_field_name("left")
                .is_some_and(|n| literal(source, n))
            && n.child_by_field_name("right")
                .is_some_and(|n| literal(source, n)))
}
fn comment(s: &str) -> &str {
    s.trim_start_matches(['/', '*'])
        .trim_end_matches(['/', '*'])
        .trim()
}
fn reason(c: &Context<'_, '_, '_>, n: Node<'_>) -> bool {
    nodes(c.root)
        .into_iter()
        .filter(|n| n.kind().contains("comment"))
        .any(|r| {
            let adjacent = (r.end_byte() <= n.start_byte()
                && c.source[r.end_byte()..n.start_byte()].trim().is_empty())
                || (r.start_byte() >= n.start_byte()
                    && r.start_position().row == n.start_position().row);
            adjacent && !comment(c.text(r)).is_empty()
        })
}

fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}
