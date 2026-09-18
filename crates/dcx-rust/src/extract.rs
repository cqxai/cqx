//! Scans a cargo workspace and emits facts.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dcx_schema::{Edge, EdgeKind, Evidence, Fact, Id, Node, NodeKind, Writer};

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

        for src_root in ["src", "tests", "benches"] {
            let dir = pkg_dir.join(src_root);
            if !dir.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let test_role = src_root != "src";
                match visit_file(&mut w, path, &root, &dir, &pkg_id, test_role, &known) {
                    Ok(()) => stats.files += 1,
                    Err(msg) => stats.unparsed.push(msg),
                }
            }
        }
    }

    stats.nodes = w.nodes;
    stats.edges = w.edges;
    Ok(stats)
}

fn visit_file<W: Write>(
    w: &mut Writer<W>,
    path: &Path,
    root: &Path,
    src_root: &Path,
    pkg_id: &Id,
    test_role: bool,
    known: &HashMap<String, Id>,
) -> Result<(), String> {
    let rel_path = rel(root, path);
    let source = std::fs::read_to_string(path).map_err(|e| format!("{rel_path}: {e}"))?;
    let line_count = source.lines().count() as u64;

    let parsed = syn::parse_file(&source).map_err(|e| format!("{rel_path}: {e}"))?;

    let dir_id = Id::directory(&rel(root, path.parent().unwrap_or(root)));
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

    let prefix = module_prefix(src_root, path);
    let mut visitor = FileVisitor::new(w, pkg_id.clone(), rel_path, prefix, test_role, known);
    syn::visit::Visit::visit_file(&mut visitor, &parsed);
    Ok(())
}

/// `src/foo/bar.rs` -> `foo::bar`; `src/lib.rs`, `src/main.rs` and
/// `src/foo/mod.rs` name the module they live in, not a child of it.
fn module_prefix(src_root: &Path, path: &Path) -> String {
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

fn rel(root: &Path, path: &Path) -> String {
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
