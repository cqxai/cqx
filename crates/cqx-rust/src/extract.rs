//! Scans a cargo workspace and emits facts.

use std::collections::HashMap;
use std::io::Write;

use cqx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Writer};
use cqx_vfs::Vfs;

use crate::manifest;
use crate::prepass::{self, PackageFacts, ParsedFile};
use crate::visit::FileVisitor;

pub struct Stats {
    pub packages: usize,
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unparsed: Vec<String>,
}

#[derive(Debug)]
pub enum ExtractError {
    Io(std::io::Error),
    Metadata(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Io(e) => write!(f, "{e}"),
            ExtractError::Metadata(m) => write!(f, "cargo metadata: {m}"),
        }
    }
}

impl From<std::io::Error> for ExtractError {
    fn from(e: std::io::Error) -> Self {
        ExtractError::Io(e)
    }
}

pub fn run(vfs: &Vfs, out: impl Write) -> Result<Stats, ExtractError> {
    let metadata = manifest::read(vfs).map_err(ExtractError::Metadata)?;
    let mut w = Writer::new(out);
    w.fact(&Fact::header("rust", &vfs.label))?;

    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();

    // Package name (underscored, as it appears in `use`) -> node id, so an
    // import of a sibling crate resolves to that package rather than to an
    // external.
    let mut known: HashMap<String, Id> = HashMap::new();
    for pkg in &packages {
        if let Some(name) = pkg["name"].as_str() {
            known.insert(name.replace('-', "_"), Id::package(name));
        }
    }

    let mut stats = Stats {
        packages: 0,
        files: 0,
        nodes: 0,
        edges: 0,
        unparsed: Vec::new(),
    };

    // Parse the whole workspace before visiting any of it: a value's
    // provenance routinely crosses a crate boundary.
    // Paths are relative to the snapshot throughout: there is no filesystem to
    // be absolute against.
    let pkg_dirs: Vec<String> = packages
        .iter()
        .filter_map(|p| p["manifest_path"].as_str().map(parent_of))
        .collect();
    let mut per_package: Vec<(Id, Vec<ParsedFile>)> = Vec::new();

    for pkg in &packages {
        let Some(name) = pkg["name"].as_str() else {
            continue;
        };
        let pkg_dir = parent_of(pkg["manifest_path"].as_str().unwrap_or_default());
        // A package owns its directory minus any package nested inside it:
        // deno has eleven such pairs, and dropping the parent outright lost 442
        // files while keeping it stole them from their real owner.
        let nested: Vec<String> = pkg_dirs
            .iter()
            .filter(|o| **o != pkg_dir && is_inside(o, &pkg_dir))
            .cloned()
            .collect();
        let (parsed, failures) =
            prepass::parse_package(vfs, &source_roots(pkg, &pkg_dir), &nested);
        stats.unparsed.extend(failures);
        per_package.push((Id::package(name), parsed));
    }
    let all_files: Vec<&ParsedFile> = per_package.iter().flat_map(|(_, f)| f.iter()).collect();
    let facts = PackageFacts::collect(&all_files);
    drop(all_files);

    for pkg in &packages {
        let Some(name) = pkg["name"].as_str() else {
            continue;
        };
        let manifest_path = pkg["manifest_path"].as_str().unwrap_or_default().to_string();
        let pkg_dir = parent_of(&manifest_path);
        let pkg_id = Id::package(name);
        stats.packages += 1;

        w.node(
            Node::new(pkg_id.clone(), NodeKind::Package)
                .attr("name", name)
                .attr("version", pkg["version"].as_str().unwrap_or("")),
        )?;

        // Binary targets are the roots of the whole graph: reachability is
        // measured from them, which is how real dead code gets found.
        for target in pkg["targets"].as_array().into_iter().flatten() {
            let kinds = target["kind"].as_array().cloned().unwrap_or_default();
            let is_bin = kinds.iter().any(|k| k.as_str() == Some("bin"));
            if !is_bin {
                continue;
            }
            let bin_name = target["name"].as_str().unwrap_or(name);
            let bin_id = Id(format!("bin:{bin_name}"));
            w.node(Node::new(bin_id.clone(), NodeKind::Binary).attr("name", bin_name))?;
            w.edge(Edge::new(EdgeKind::Contains, pkg_id.clone(), bin_id))?;
        }

        emit_dependencies(&mut w, &pkg_id, pkg, &manifest_path, vfs, &known)?;

        let dir_id = Id::directory(&pkg_dir);
        w.node(Node::new(dir_id.clone(), NodeKind::Directory).attr("path", pkg_dir.as_str()))?;
        w.edge(Edge::new(EdgeKind::Contains, pkg_id.clone(), dir_id))?;

        let Some((_, parsed)) = per_package.iter().find(|(id, _)| *id == pkg_id) else {
            continue;
        };
        for file in parsed {
            visit_file(&mut w, file, vfs, &pkg_id, &known, &facts);
            stats.files += 1;
        }
    }

    stats.nodes = w.nodes;
    stats.edges = w.edges;
    Ok(stats)
}

fn visit_file<W: Write>(
    w: &mut Writer<W>,
    file: &ParsedFile,
    vfs: &Vfs,
    pkg_id: &Id,
    known: &HashMap<String, Id>,
    facts: &PackageFacts,
) {
    let rel_path = file.rel_path.clone();
    let test_role = file.test_role;
    let line_count = vfs
        .read(&rel_path)
        .map(|s| s.lines().count() as u64)
        .unwrap_or(0);

    let dir_id = Id::directory(&parent_of(&rel_path));
    let _ = w.node(
        Node::new(dir_id.clone(), NodeKind::Directory)
            .attr("path", dir_id.0.trim_start_matches("dir:")),
    );
    let _ = w.edge(Edge::new(
        EdgeKind::Contains,
        pkg_id.clone(),
        dir_id.clone(),
    ));

    let file_id = Id::file(&rel_path);
    let mut file_node = Node::new(file_id.clone(), NodeKind::File)
        .attr("path", rel_path.as_str())
        .attr("lines", line_count);
    if test_role {
        file_node = file_node.attr("role", "test");
    }
    let _ = w.node(file_node);
    let _ = w.edge(Edge::new(EdgeKind::Contains, dir_id, file_id));

    let mut visitor = FileVisitor::new(
        w,
        pkg_id.clone(),
        rel_path,
        file.module_prefix.clone(),
        test_role,
        known,
        facts,
    );
    syn::visit::Visit::visit_file(&mut visitor, &file.parsed);
}

/// The directories a package's sources actually live in, taken from its
/// declared targets. Falls back to `src/` only when a manifest declares nothing.
fn source_roots(pkg: &serde_json::Value, pkg_dir: &str) -> Vec<(String, bool)> {
    let mut roots: Vec<(String, bool)> = Vec::new();
    for target in pkg["targets"].as_array().into_iter().flatten() {
        let Some(src) = target["src_path"].as_str() else {
            continue;
        };
        let dir = parent_of(src);
        let kinds = target["kind"].as_array().cloned().unwrap_or_default();
        // A build script commonly sits at the repository root, which would make
        // the whole repository this package's source root and parse every other
        // crate a second time under the wrong name.
        if kinds.iter().any(|k| k.as_str() == Some("custom-build")) {
            continue;
        }
        let is_test = kinds
            .iter()
            .any(|k| matches!(k.as_str(), Some("test") | Some("bench") | Some("example")));
        if !roots.iter().any(|(d, _)| *d == dir) {
            roots.push((dir, is_test));
        }
    }
    if roots.is_empty() {
        roots.push((join(pkg_dir, "src"), false));
    }
    roots
}

/// The directory a path sits in, in snapshot space.
pub(crate) fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

pub(crate) fn is_inside(path: &str, dir: &str) -> bool {
    dir.is_empty() || path.starts_with(&format!("{dir}/"))
}

pub(crate) fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// `src/foo/bar.rs` -> `foo::bar`; `src/lib.rs`, `src/main.rs` and
/// `src/foo/mod.rs` name the module they live in, not a child of it.
pub(crate) fn module_prefix(src_root: &str, path: &str) -> String {
    let rest = match path.strip_prefix(&format!("{src_root}/")) {
        Some(r) => r,
        None => return String::new(),
    };
    let mut parts: Vec<String> = rest.split('/').map(str::to_string).collect();
    let Some(last) = parts.pop() else {
        return String::new();
    };
    let stem = last.trim_end_matches(".rs");
    if !matches!(stem, "lib" | "main" | "mod") {
        parts.push(stem.to_string());
    }
    parts.join("::")
}

/// Declared dependencies, with the manifest line that declares them as
/// evidence. Metadata alone would give an edge nobody can point at.
fn emit_dependencies<W: Write>(
    w: &mut Writer<W>,
    pkg_id: &Id,
    pkg: &serde_json::Value,
    manifest_path: &str,
    vfs: &Vfs,
    known: &HashMap<String, Id>,
) -> Result<(), ExtractError> {
    let manifest_text = vfs.read(manifest_path).unwrap_or_default();

    for dep in pkg["dependencies"].as_array().into_iter().flatten() {
        let Some(dep_name) = dep["name"].as_str() else {
            continue;
        };
        let target = known
            .get(&dep_name.replace('-', "_"))
            .cloned()
            .unwrap_or_else(|| Id::external(dep_name));
        if target.0.starts_with("ext:") {
            w.node(Node::new(target.clone(), NodeKind::External).attr("name", dep_name))?;
        }
        let line = manifest_line(manifest_text, dep_name).unwrap_or(1);
        let kind = dep["kind"].as_str().unwrap_or("normal");
        w.edge(
            Edge::new(EdgeKind::DependsOn, pkg_id.clone(), target)
                .attr("kind", kind)
                .evidence(Evidence::at(manifest_path, line, line)),
        )?;
    }
    Ok(())
}

fn manifest_line(text: &str, dep: &str) -> Option<u32> {
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(dep)
            && trimmed[dep.len()..]
                .trim_start()
                .starts_with(['=', '.', ' '])
        {
            return Some(i as u32 + 1);
        }
        if trimmed.starts_with('[') && trimmed.contains(dep) {
            return Some(i as u32 + 1);
        }
    }
    None
}

