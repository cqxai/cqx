//! Python 3.14 syntax/effect evidence; no unannotated type/domain claims.
use cqx_schema::EdgeKind;
use cqx_source::{Emitter, Stats};
use cqx_vfs::Vfs;
use rezel_common::{IterMode, SyntaxNode, TypedNode};
use rezel_lang_python::PythonFunctionDefinition;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    ops::Range,
};
use unicode_normalization::UnicodeNormalization;
pub fn is_source(p: &str) -> bool {
    p.ends_with(".py")
}
pub fn entry_files(vfs: &Vfs) -> BTreeMap<String, BTreeSet<String>> {
    let mut entries: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in vfs.paths() {
        let base = path.rsplit_once('/').map_or("", |(p, _)| p);
        let source = vfs.read(path).unwrap_or_default();
        let mut targets = Vec::new();
        if path.rsplit('/').next() == Some("pyproject.toml") {
            if let Ok(v) = source.parse::<toml::Value>() {
                for key in ["scripts", "gui-scripts"] {
                    if let Some(t) = v
                        .get("project")
                        .and_then(|p| p.get(key))
                        .and_then(toml::Value::as_table)
                    {
                        targets.extend(
                            t.values()
                                .filter_map(toml::Value::as_str)
                                .map(str::to_owned),
                        );
                    }
                }
            }
        }
        if path.rsplit('/').next() == Some("setup.cfg") {
            let mut section = false;
            let mut console = false;
            for line in source.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('[') {
                    section = trimmed == "[options.entry_points]";
                    console = false;
                    continue;
                }
                if !section {
                    continue;
                }
                if !line.starts_with(char::is_whitespace) {
                    console = trimmed.starts_with("console_scripts")
                        || trimmed.starts_with("gui_scripts");
                }
                if console {
                    if let Some((_, t)) = trimmed.split_once('=') {
                        if t.contains(':') {
                            targets.push(t.trim().into());
                        }
                    }
                }
            }
        }
        if path.rsplit('/').next() == Some("setup.py") {
            if let Ok(tree) = rezel_lang_python::parser().with_strict(true).parse(source) {
                for n in nodes(&tree) {
                    if n.name().as_ref() == "DictionaryExpression" {
                        let children: Vec<_> = n.children().collect();
                        for (i, c) in children.iter().enumerate() {
                            if matches!(
                                string(source, c).as_deref(),
                                Some("console_scripts" | "gui_scripts")
                            ) {
                                if let Some(list) = children.iter().skip(i + 1).find(|c| {
                                    matches!(
                                        c.name().as_ref(),
                                        "ArrayExpression" | "TupleExpression"
                                    )
                                }) {
                                    for s in list.children() {
                                        if let Some(s) = string(source, &s) {
                                            if let Some((_, t)) = s.split_once('=') {
                                                targets.push(t.trim().into());
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        for target in targets {
            let Some((_, function)) = target.split_once(':') else {
                continue;
            };
            let module = target
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .replace('.', "/");
            for prefix in ["", "src/"] {
                for suffix in [".py", "/__init__.py"] {
                    let p = format!("{prefix}{module}{suffix}");
                    let p = if base.is_empty() {
                        p
                    } else {
                        format!("{base}/{p}")
                    };
                    if vfs.read(&p).is_some() {
                        entries
                            .entry(p)
                            .or_default()
                            .insert(function.trim().to_owned());
                    }
                }
            }
        }
    }
    entries
}
pub fn run(vfs: &Vfs, out: impl Write, tick: &dyn Fn()) -> io::Result<Stats> {
    run_with_entries(vfs, out, tick, &entry_files(vfs))
}
pub fn run_with_entries(
    vfs: &Vfs,
    out: impl Write,
    tick: &dyn Fn(),
    entries: &BTreeMap<String, BTreeSet<String>>,
) -> io::Result<Stats> {
    run_with_layout(
        vfs,
        out,
        tick,
        entries,
        &cqx_layout::Layout::from_paths(vfs.paths()),
    )
}
pub fn run_with_layout(
    vfs: &Vfs,
    out: impl Write,
    tick: &dyn Fn(),
    entries: &BTreeMap<String, BTreeSet<String>>,
    layout: &cqx_layout::Layout,
) -> io::Result<Stats> {
    let mut emitter = Emitter::new(out, "python", "Python 3.14")?;
    for path in vfs.paths().filter(|p| is_source(p)) {
        tick();
        let source = vfs.read(path).unwrap_or_default();
        if layout.excluded("python", path, source) {
            continue;
        }
        let tree = match rezel_lang_python::parser().with_strict(true).parse(source) {
            Ok(t) => t,
            Err(e) => {
                emitter.skip(path, &e.to_string())?;
                continue;
            }
        };
        let all = nodes(&tree);
        let mut bindings = BTreeMap::new();
        let mut shadows = BTreeSet::new();
        let mut wildcard = false;
        for n in &all {
            match n.name().as_ref() {
                "ImportStatement" => {
                    let normalized = text(source, n).nfkc().collect::<String>();
                    let s = normalized.as_str();
                    if let Some(s) = s.strip_prefix("from ") {
                        if let Some((module, names)) = s.split_once(" import ") {
                            for part in names.trim().trim_matches(['(', ')']).split(',') {
                                let parts: Vec<_> = part.split_whitespace().collect();
                                if let Some(name) = parts.first() {
                                    if *name == "*" {
                                        wildcard = true;
                                        continue;
                                    }
                                    let alias = parts.get(2).unwrap_or(name);
                                    bindings.insert(
                                        (*alias).to_owned(),
                                        format!("{}.{}", module.trim(), name),
                                    );
                                }
                            }
                        }
                    } else if let Some(s) = s.strip_prefix("import ") {
                        for part in s.split(',') {
                            let parts: Vec<_> = part.split_whitespace().collect();
                            if let Some(name) = parts.first() {
                                let alias = parts
                                    .get(2)
                                    .copied()
                                    .unwrap_or_else(|| name.split('.').next().unwrap_or(name));
                                let bound = if parts.len() > 2 {
                                    *name
                                } else {
                                    name.split('.').next().unwrap_or(name)
                                };
                                bindings.insert(alias.to_owned(), bound.to_owned());
                            }
                        }
                    }
                }
                "FunctionDefinition" | "ClassDefinition" => {
                    if let Some(name) = n.children().find(|n| n.name().as_ref() == "VariableName") {
                        shadows.insert(text(source, &name).nfkc().collect::<String>());
                    }
                }
                "ParamList" => {
                    for p in n.children().filter(|p| p.name().as_ref() == "VariableName") {
                        shadows.insert(text(source, &p).nfkc().collect::<String>());
                    }
                }
                "AssignStatement" => {
                    for c in n.children().take_while(|c| c.name().as_ref() != "AssignOp") {
                        if c.name().as_ref() == "VariableName" {
                            shadows.insert(text(source, &c).nfkc().collect::<String>());
                        }
                    }
                }
                _ => {}
            }
        }
        let mut entry_scope = cqx_layout::Entries::default();
        entry_scope.script(matches!(
            path.rsplit('/').next(),
            Some("__main__.py" | "setup.py")
        ));
        for n in &all {
            if matches!(
                n.name().as_ref(),
                "FunctionDefinition" | "ClassDefinition" | "LambdaExpression"
            ) {
                let name = n
                    .children()
                    .find(|n| n.name().as_ref() == "VariableName")
                    .map(|n| text(source, &n))
                    .unwrap_or("");
                let entry = n.name().as_ref() == "FunctionDefinition"
                    && entries.get(path).is_some_and(|names| names.contains(name))
                    && n.parent().is_some_and(|p| p.name().as_ref() == "Module");
                entry_scope.declaration(range(n), entry);
            }
            if let Some(block) = main_guard(source, n) {
                entry_scope.block(block);
            }
        }
        let mut sink = emitter.file(path, source, layout.is_test("python", path), &tree)?;
        for n in &all {
            if let Ok(f) = PythonFunctionDefinition::downcast_from(n.clone()) {
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
        let canonical = |name: String| {
            let (first, rest) = name.split_once('.').unwrap_or((&name, ""));
            if shadows.contains(first) {
                return String::new();
            }
            if let Some(bound) = bindings.get(first) {
                if rest.is_empty() {
                    bound.clone()
                } else {
                    format!("{bound}.{rest}")
                }
            } else if !wildcard && matches!(name.as_str(), "eval" | "exec") {
                name
            } else {
                String::new()
            }
        };
        for n in &all {
            if n.name().as_ref() == "TryStatement" {
                let children: Vec<_> = n.children().collect();
                for (i, c) in children.iter().enumerate() {
                    if text(source, c) == "except" {
                        if let Some((j, body)) = children
                            .iter()
                            .enumerate()
                            .skip(i + 1)
                            .find(|(_, c)| c.name().as_ref() == "Body")
                        {
                            let ty = children[i + 1..j]
                                .iter()
                                .map(|n| text(source, n))
                                .collect::<String>();
                            let statements: Vec<_> = body
                                .children()
                                .filter(|n| n.name().ends_with("Statement"))
                                .collect();
                            let raises = statements
                                .iter()
                                .any(|n| n.name().as_ref() == "RaiseStatement");
                            let pass = statements.len() == 1
                                && statements[0].name().as_ref() == "PassStatement";
                            if (ty.is_empty() && !raises
                                || (ty == "Exception"
                                    || ty == "BaseException"
                                    || ty.starts_with("Exceptionas"))
                                    && pass
                                    && !shadows.contains("Exception")
                                    && !shadows.contains("BaseException"))
                                && !sink.documented(&range(body))
                            {
                                sink.rule("swallowed-errors",EdgeKind::Discards,range(body),"bare except or broad exception/pass hides failures without an explanation")?;
                            }
                        }
                    }
                }
            }
            if n.name().as_ref() == "Comment"
                && cqx_layout::broad_directive("python", text(source, n))
                && !suppression_reason(text(source, n))
                && !all
                    .iter()
                    .filter(|n| n.name().as_ref() == "Comment")
                    .any(|c| {
                        range(c).end <= range(n).start
                            && {
                                let gap = &source[range(c).end..range(n).start];
                                gap.contains('\n')
                                    && gap.rsplit('\n').skip(1).all(|s| s.trim().is_empty())
                            }
                            && !suppression(text(source, c))
                            && text(source, c).trim_start_matches('#').trim().len() > 3
                    })
            {
                sink.rule(
                    "undocumented-suppressions",
                    EdgeKind::Silences,
                    range(n),
                    "noqa/type: ignore has no reason",
                )?;
            }
            if n.name().as_ref() == "CallExpression" {
                let children: Vec<_> = n.children().collect();
                let Some(function) = children.first() else {
                    continue;
                };
                let name = canonical(expression_name(source, function));
                sink.call(&name, range(n))?;
                let args = children
                    .iter()
                    .find(|n| n.name().as_ref() == "ArgList")
                    .map(|n| arguments(source, n))
                    .unwrap_or_default();
                let first = args.iter().find(|(key, _)| key.is_none()).map(|(_, n)| n);
                let keyword = |key: &str| {
                    args.iter()
                        .find(|(k, _)| k.as_deref() == Some(key))
                        .map(|(_, n)| n)
                };
                if !entry_scope.contains(&range(n))
                    && matches!(name.as_str(), "sys.exit" | "os._exit")
                {
                    sink.rule(
                        "exit-in-library",
                        EdgeKind::Crosses,
                        range(n),
                        "process exit outside an entry module or __main__ guard",
                    )?;
                }
                if matches!(
                    name.as_str(),
                    "eval" | "exec" | "builtins.eval" | "builtins.exec"
                ) && first
                    .or_else(|| keyword("source"))
                    .is_some_and(|n| !literal(n))
                {
                    sink.rule(
                        "dynamic-code",
                        EdgeKind::Spawns,
                        range(n),
                        "builtin eval/exec runs dynamic Python code",
                    )?;
                }
                let shell_api = name == "os.system"
                    || name.starts_with("subprocess.")
                        && matches!(
                            name.rsplit('.').next(),
                            Some(
                                "run"
                                    | "Popen"
                                    | "call"
                                    | "check_call"
                                    | "check_output"
                                    | "getoutput"
                                    | "getstatusoutput"
                            )
                        );
                let shell = matches!(
                    name.as_str(),
                    "os.system" | "subprocess.getoutput" | "subprocess.getstatusoutput"
                ) || keyword("shell").is_some_and(|n| text(source, n) == "True");
                if shell_api
                    && shell
                    && first
                        .or_else(|| keyword("args"))
                        .is_some_and(|n| !literal(n))
                {
                    sink.rule(
                        "nonliteral-process",
                        EdgeKind::Spawns,
                        range(n),
                        "non-literal command passed to a shell-capable standard API",
                    )?;
                }
                if name == "pickle.load"
                    || name == "pickle.loads"
                        && first
                            .or_else(|| keyword("data"))
                            .is_some_and(|n| !constant(source, n, &all))
                    || name == "yaml.load"
                        && !keyword("Loader")
                            .or_else(|| {
                                args.iter()
                                    .filter(|(key, _)| key.is_none())
                                    .nth(1)
                                    .map(|(_, n)| n)
                            })
                            .is_some_and(|n| {
                                matches!(
                                    canonical(expression_name(source, n)).as_str(),
                                    "yaml.SafeLoader"
                                        | "yaml.CSafeLoader"
                                        | "yaml.loader.SafeLoader"
                                )
                            })
                {
                    sink.rule("unsafe-deserialization",EdgeKind::Spawns,range(n),"pickle or yaml.load without a bound SafeLoader can construct arbitrary objects")?;
                }
            }
        }
    }
    Ok(emitter.finish())
}
fn nodes(t: &rezel_common::Tree) -> Vec<SyntaxNode> {
    let mut cursor = t.cursor(IterMode::NONE);
    let mut all = Vec::new();
    loop {
        all.push(cursor.node());
        if !cursor.next(true) {
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
fn expression_name(s: &str, n: &SyntaxNode) -> String {
    match n.name().as_ref() {
        "VariableName" | "PropertyName" => text(s, n).nfkc().collect(),
        "MemberExpression" => n
            .children()
            .filter(|n| {
                matches!(
                    n.name().as_ref(),
                    "VariableName" | "PropertyName" | "MemberExpression"
                )
            })
            .map(|n| expression_name(s, &n))
            .collect::<Vec<_>>()
            .join("."),
        _ => String::new(),
    }
}
fn arguments(s: &str, n: &SyntaxNode) -> Vec<(Option<String>, SyntaxNode)> {
    let c: Vec<_> = n.children().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if i + 2 < c.len()
            && c[i].name().as_ref() == "VariableName"
            && c[i + 1].name().as_ref() == "AssignOp"
        {
            args.push((Some(text(s, &c[i]).to_owned()), c[i + 2].clone()));
            i += 3
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
    matches!(n.name().as_ref(), "String" | "ContinuedString")
        && !n
            .children()
            .any(|n| n.name().contains("Format") || n.name().contains("Interpolation"))
}
fn string(s: &str, n: &SyntaxNode) -> Option<String> {
    literal(n).then(|| text(s, n).trim_matches(['\'', '"']).to_owned())
}
fn main_guard(s: &str, n: &SyntaxNode) -> Option<Range<usize>> {
    if n.name().as_ref() != "IfStatement" {
        return None;
    }
    let c = n
        .children()
        .find(|n| n.name().as_ref() == "BinaryExpression")?;
    let c = text(s, &c)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace('\'', "\"");
    if !matches!(
        c.as_str(),
        "__name__==\"__main__\"" | "\"__main__\"==__name__"
    ) {
        return None;
    }
    n.children()
        .find(|n| n.name().as_ref() == "Body")
        .map(|b| range(&b))
}
// Literal/constant bytes do not supply an externally variable pickle payload.
// Only uniquely assigned, uppercase module constants initialized by a literal
// qualify; mutation, reassignment and dynamic expressions stay review sites.
fn constant(s: &str, n: &SyntaxNode, all: &[SyntaxNode]) -> bool {
    if literal(n) {
        return true;
    }
    if n.name().as_ref() != "VariableName" {
        return false;
    }
    let name = text(s, n);
    if !name
        .chars()
        .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return false;
    }
    if all.iter().any(|v| {
        v.name().as_ref() == "VariableName"
            && text(s, v) == name
            && v.parent().is_some_and(|p| p.name().as_ref() == "ParamList")
    }) {
        return false;
    }
    let assigns: Vec<_> = all
        .iter()
        .filter(|a| {
            a.name().as_ref() == "AssignStatement"
                && a.children().next().is_some_and(|v| text(s, &v) == name)
        })
        .collect();
    assigns.len() == 1
        && assigns[0]
            .parent()
            .is_some_and(|p| p.name().as_ref() == "Module")
        && assigns[0].children().last().is_some_and(|v| literal(&v))
}
fn suppression(s: &str) -> bool {
    let s = s.trim_start_matches('#').trim().to_ascii_lowercase();
    s.strip_prefix("noqa")
        .is_some_and(|tail| tail.is_empty() || tail.starts_with([':', ' ']))
        || s.starts_with("type: ignore")
}
fn suppression_reason(s: &str) -> bool {
    let s = s.trim_start_matches('#').trim().to_ascii_lowercase();
    let tail = s
        .find("noqa")
        .map(|p| &s[p + 4..])
        .or_else(|| s.find("type: ignore").map(|p| &s[p + 12..]))
        .unwrap_or("");
    tail.contains("--")
        && tail
            .split_once("--")
            .is_some_and(|(_, r)| r.chars().any(char::is_alphabetic))
        || tail.contains('#')
            && tail
                .split_once('#')
                .is_some_and(|(_, r)| r.chars().any(char::is_alphabetic))
}
