//! Scans a cargo workspace and emits facts.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dcx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Writer};

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

pub fn run(root: &Path, out: impl Write) -> Result<Stats, ExtractError> {
    let root = root
        .canonicalize()
        .map_err(|e| ExtractError::Metadata(format!("{}: {e}", root.display())))?;
    let metadata = cargo_metadata(&root)?;
    let mut w = Writer::new(out);
    w.fact(&Fact::header("rust", &root.display().to_string()))?;

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
    let pkg_dirs: Vec<PathBuf> = packages
        .iter()
        .filter_map(|p| {
            Path::new(p["manifest_path"].as_str()?)
                .parent()
                .map(Path::to_path_buf)
        })
        .collect();
    let mut per_package: Vec<(Id, Vec<ParsedFile>)> = Vec::new();

    for pkg in &packages {
        let Some(name) = pkg["name"].as_str() else {
            continue;
        };
        let manifest = PathBuf::from(pkg["manifest_path"].as_str().unwrap_or_default());
        let Some(pkg_dir) = manifest.parent() else {
            continue;
        };
        // A package owns its directory minus any package nested inside it:
        // deno has eleven such pairs, and dropping the parent outright lost 442
        // files while keeping it stole them from their real owner.
        let nested: Vec<PathBuf> = pkg_dirs
            .iter()
            .filter(|o| o.as_path() != pkg_dir && o.starts_with(pkg_dir))
            .cloned()
            .collect();
        let (parsed, failures) = prepass::parse_package(
            pkg_dir,
            &root,
            &source_roots(pkg, pkg_dir),
            &nested,
        );
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
        let manifest = PathBuf::from(pkg["manifest_path"].as_str().unwrap_or_default());
        let Some(pkg_dir) = manifest.parent() else {
            continue;
        };
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

        emit_dependencies(&mut w, &pkg_id, pkg, &manifest, &root, &known)?;

        let rel_dir = rel(&root, pkg_dir);
        let dir_id = Id::directory(&rel_dir);
        w.node(Node::new(dir_id.clone(), NodeKind::Directory).attr("path", rel_dir.as_str()))?;
        w.edge(Edge::new(EdgeKind::Contains, pkg_id.clone(), dir_id))?;

        let Some((_, parsed)) = per_package.iter().find(|(id, _)| *id == pkg_id) else {
            continue;
        };
        for file in parsed {
            visit_file(&mut w, file, &root, &pkg_id, &known, &facts);
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
    root: &Path,
    pkg_id: &Id,
    known: &HashMap<String, Id>,
    facts: &PackageFacts,
) {
    let rel_path = file.rel_path.clone();
    let test_role = file.test_role;
    let abs = root.join(&rel_path);
    let line_count = std::fs::read_to_string(&abs)
        .map(|s| s.lines().count() as u64)
        .unwrap_or(0);

    let dir_id = Id::directory(&rel(root, abs.parent().unwrap_or(root)));
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
fn source_roots(pkg: &serde_json::Value, pkg_dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    for target in pkg["targets"].as_array().into_iter().flatten() {
        let Some(src) = target["src_path"].as_str() else {
            continue;
        };
        let Some(dir) = Path::new(src).parent() else {
            continue;
        };
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
        let entry = (dir.to_path_buf(), is_test);
        if !roots.iter().any(|(d, _)| d == &entry.0) {
            roots.push(entry);
        }
    }
    for extra in ["tests", "benches"] {
        let dir = pkg_dir.join(extra);
        if dir.is_dir() && !roots.iter().any(|(d, _)| d == &dir) {
            roots.push((dir, true));
        }
    }
    if roots.is_empty() {
        roots.push((pkg_dir.join("src"), false));
    }
    roots
}

/// `src/foo/bar.rs` -> `foo::bar`; `src/lib.rs`, `src/main.rs` and
/// `src/foo/mod.rs` name the module they live in, not a child of it.
pub(crate) fn module_prefix(src_root: &Path, path: &Path) -> String {
    let Ok(rel) = path.strip_prefix(src_root) else {
        return String::new();
    };
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
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
    manifest: &Path,
    root: &Path,
    known: &HashMap<String, Id>,
) -> Result<(), ExtractError> {
    let manifest_rel = rel(root, manifest);
    let manifest_text = std::fs::read_to_string(manifest).unwrap_or_default();

    for dep in pkg["dependencies"].as_array().into_iter().flatten() {
        let Some(dep_name) = dep["name"].as_str() else {
            continue;
        };
        let target = known
            .get(&dep_name.replace('-', "_"))
            .cloned()
            .unwrap_or_else(|| Id::external(dep_name));
        if matches!(target.0.strip_prefix("ext:"), Some(_)) {
            w.node(Node::new(target.clone(), NodeKind::External).attr("name", dep_name))?;
        }
        let line = manifest_line(&manifest_text, dep_name).unwrap_or(1);
        let kind = dep["kind"].as_str().unwrap_or("normal");
        w.edge(
            Edge::new(EdgeKind::DependsOn, pkg_id.clone(), target)
                .attr("kind", kind)
                .evidence(Evidence::at(&manifest_rel, line, line)),
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

pub(crate) fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn cargo_metadata(root: &Path) -> Result<serde_json::Value, ExtractError> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| ExtractError::Metadata(format!("could not run cargo: {e}")))?;
    if !output.status.success() {
        return Err(ExtractError::Metadata(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| ExtractError::Metadata(format!("unreadable output: {e}")))
}
