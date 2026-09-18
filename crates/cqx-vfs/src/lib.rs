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

/// What a source snapshot is worth carrying: the manifests that describe the
/// workspace, the lockfile that pins it, and the code itself.
pub fn is_interesting(path: &str) -> bool {
    path.ends_with(".rs")
        || path.ends_with("/Cargo.toml")
        || path == "Cargo.toml"
        || path.ends_with("/Cargo.lock")
        || path == "Cargo.lock"
}

/// Directories that hold build output or history rather than source.
pub fn is_ignored_dir(name: &str) -> bool {
    matches!(name, "target" | ".git" | "node_modules") || name.starts_with(".target")
}

/// Fills a snapshot from a directory on disk.
///
/// The only part of analysis that touches a filesystem, and it is not part of
/// analysis — it is what happens before it.
pub fn from_dir(root: &Path) -> std::io::Result<Vfs> {
    let mut vfs = Vfs::new(root.display().to_string());
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || !is_ignored_dir(&e.file_name().to_string_lossy())
                || e.depth() == 0
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
        if !is_interesting(&rel) {
            continue;
        }
        // Source that is not valid UTF-8 is not source we can parse.
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            vfs.insert(rel, content);
        }
    }
    Ok(vfs)
}
