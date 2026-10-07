//! A snapshot of the files being analysed, held in memory.
//!
//! Analysis never touches a filesystem. Whatever is doing the reading — a
//! directory walk, git objects, an HTTP API — fills one of these first, and
//! everything downstream is pure and synchronous.
//!
//! That is the whole point. In a browser, reading a file is asynchronous, and a
//! trait with a `read()` method would force every caller up the stack to become
//! asynchronous with it — the syn visitors, the metrics, the scoring. Gathering
//! the bytes first and then analysing them keeps the async confined to the part
//! that genuinely does I/O.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub content: String,
    /// The git object id of this content, when it came from git.
    ///
    /// A file's facts depend only on its bytes, so this is the cache key that
    /// makes scoring a second commit cost about a percent of the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

/// The files, and what they were a snapshot of.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Vfs {
    /// What this is a snapshot of — a path, a commit, a repository.
    pub label: String,
    /// Paths are relative to the snapshot root and use forward slashes, so a
    /// snapshot taken on one platform reads the same on another.
    pub files: BTreeMap<String, FileEntry>,
}

impl Vfs {
    pub fn new(label: impl Into<String>) -> Vfs {
        Vfs {
            label: label.into(),
            files: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.files.insert(
            normalise(&path.into()),
            FileEntry {
                content: content.into(),
                blob: None,
            },
        );
    }

    pub fn insert_blob(
        &mut self,
        path: impl Into<String>,
        content: impl Into<String>,
        blob: impl Into<String>,
    ) {
        self.files.insert(
            normalise(&path.into()),
            FileEntry {
                content: content.into(),
                blob: Some(blob.into()),
            },
        );
    }

    pub fn read(&self, path: &str) -> Option<&str> {
        self.files.get(&normalise(path)).map(|f| f.content.as_str())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(&normalise(path))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every path, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Every path inside a directory, at any depth.
    pub fn under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = normalise(prefix);
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        self.files.keys().filter_map(move |p| {
            if prefix.is_empty() || p.starts_with(&prefix) {
                Some(p.as_str())
            } else {
                None
            }
        })
    }

    pub fn total_bytes(&self) -> usize {
        self.files.values().map(|f| f.content.len()).sum()
    }
}

/// Windows separators and leading `./` are spelling, not structure.
fn normalise(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./");
    p.trim_start_matches('/').to_string()
}

/// Shared by snapshot discovery and frontend dispatch.
pub fn is_typescript_source(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

/// Source and manifest discovery uses the same project-root policy as readers.
pub fn is_interesting(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        path.rsplit('.').next(),
        Some("rs" | "go" | "java" | "kt" | "kts" | "swift" | "zig" | "py" | "php")
    ) || is_typescript_source(path)
        || cqx_layout::is_manifest(name)
        || matches!(name, "cqx.json" | "Cargo.lock" | "pyvenv.cfg")
}
pub fn is_ignored_dir(name: &str) -> bool {
    cqx_layout::excluded_name("", name)
}
fn project_dir(path: &Path, root: &Path) -> bool {
    path == root
        || std::fs::read_dir(path).is_ok_and(|mut entries| {
            entries
                .any(|e| e.is_ok_and(|e| cqx_layout::is_manifest(&e.file_name().to_string_lossy())))
        })
}
pub fn from_dir(root: &Path) -> std::io::Result<Vfs> {
    let mut vfs = Vfs::new(root.display().to_string());
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            if !e.file_type().is_dir() || e.depth() == 0 {
                return true;
            }
            if e.path().join("pyvenv.cfg").is_file() {
                return false;
            }
            !(is_ignored_dir(&e.file_name().to_string_lossy())
                && e.path().parent().is_some_and(|p| project_dir(p, root)))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        if is_interesting(&rel) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                vfs.insert(rel, content);
            }
        }
    }
    let layout = cqx_layout::Layout::from_paths(vfs.paths());
    for rel in composer_bins(&vfs) {
        if layout.excluded_dir("php", &rel) {
            continue;
        }
        let candidate = root.join(&rel);
        if vfs.read(&rel).is_none()
            && std::fs::canonicalize(root)
                .ok()
                .zip(std::fs::canonicalize(&candidate).ok())
                .is_some_and(|(root, target)| target.starts_with(root))
        {
            if let Ok(content) = std::fs::read_to_string(candidate) {
                vfs.insert(rel, content);
            }
        }
    }
    Ok(vfs)
}

/// Literal Composer binaries, resolved relative to each nested manifest.
pub fn composer_bins(vfs: &Vfs) -> std::collections::BTreeSet<String> {
    let mut paths = std::collections::BTreeSet::new();
    for path in vfs
        .paths()
        .filter(|p| p.rsplit('/').next() == Some("composer.json"))
    {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(vfs.read(path).unwrap_or_default())
        else {
            continue;
        };
        let bins: Vec<_> = if let Some(s) = v["bin"].as_str() {
            vec![s]
        } else {
            v["bin"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect()
        };
        for bin in bins {
            if bin.starts_with('/') || bin.contains('\\') {
                continue;
            }
            let base = path.rsplit_once('/').map_or("", |(p, _)| p);
            let joined = format!("{base}/{bin}");
            let mut parts = Vec::new();
            let mut valid = true;
            for p in joined.split('/') {
                match p {
                    "" | "." => {}
                    ".." => {
                        if parts.pop().is_none() {
                            valid = false;
                            break;
                        }
                    }
                    _ => parts.push(p),
                }
            }
            if valid && !parts.is_empty() {
                paths.insert(parts.join("/"));
            }
        }
    }
    paths
}
