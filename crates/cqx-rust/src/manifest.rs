//! Reading a cargo workspace from a snapshot instead of from cargo.
//!
//! `cargo metadata` is a subprocess, and a subprocess is the one thing a browser
//! cannot do. Everything it reports that matters here — members, versions,
//! targets, dependencies — is written in the manifests themselves, so this
//! parses them directly and produces the same shape the rest of the extractor
//! already consumes.
//!
//! What is deliberately not reproduced is feature resolution. Nothing here
//! depends on it, and guessing at it would be worse than not having it.

use std::collections::BTreeMap;

use cqx_vfs::Vfs;
use serde_json::{json, Value};

/// The workspace as the extractor expects to receive it.
pub fn read(vfs: &Vfs) -> Result<Value, String> {
    let root_manifest = vfs
        .read("Cargo.toml")
        .ok_or_else(|| "no Cargo.toml at the root of the snapshot".to_string())?;
    let root: toml::Value =
        toml::from_str(root_manifest).map_err(|e| format!("Cargo.toml: {e}"))?;

    let inherited = root
        .get("workspace")
        .and_then(|w| w.get("package"))
        .cloned();

    let mut member_dirs: Vec<String> = Vec::new();
    // A manifest with a [package] of its own is a member, workspace root or not.
    if root.get("package").is_some() {
        member_dirs.push(String::new());
    }
    if let Some(workspace) = root.get("workspace") {
        let patterns = workspace
            .get("members")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let excluded = workspace
            .get("exclude")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for pattern in patterns {
            for dir in expand(vfs, &pattern) {
                if excluded.iter().any(|e| dir == *e || dir.starts_with(&format!("{e}/"))) {
                    continue;
                }
                if !member_dirs.contains(&dir) {
                    member_dirs.push(dir);
                }
            }
        }
        // A path declared in [workspace.dependencies] makes a member too, even
        // if no member has drawn on it yet. deno has sixty-three of these, and
        // three of its extension crates appear nowhere else.
        if let Some(t) = workspace.get("dependencies").and_then(|v| v.as_table()) {
            for spec in t.values() {
                let Some(rel) = spec.get("path").and_then(|v| v.as_str()) else {
                    continue;
                };
                let target = resolve("", rel);
                if vfs.contains(&join(&target, "Cargo.toml")) && !member_dirs.contains(&target) {
                    member_dirs.push(target);
                }
            }
        }
        // Cargo also treats a member's path dependencies as members, even when
        // they are not listed.
        let mut i = 0;
        while i < member_dirs.len() {
            let dir = member_dirs[i].clone();
            i += 1;
            let Some(text) = vfs.read(&join(&dir, "Cargo.toml")) else {
                continue;
            };
            let Ok(manifest) = toml::from_str::<toml::Value>(text) else {
                continue;
            };
            for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
                let Some(t) = manifest.get(table).and_then(|v| v.as_table()) else {
                    continue;
                };
                for spec in t.values() {
                    let Some(rel) = spec.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let target = resolve(&dir, rel);
                    if excluded
                        .iter()
                        .any(|e| target == *e || target.starts_with(&format!("{e}/")))
                    {
                        continue;
                    }
                    if vfs.contains(&join(&target, "Cargo.toml")) && !member_dirs.contains(&target)
                    {
                        member_dirs.push(target);
                    }
                }
            }
        }
    }

    let mut packages = Vec::new();
    for dir in &member_dirs {
        let manifest_path = join(dir, "Cargo.toml");
        let Some(text) = vfs.read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<toml::Value>(text) else {
            continue;
        };
        let Some(package) = manifest.get("package") else {
            continue; // a virtual manifest declares members, not a package
        };
        let Some(name) = package.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        packages.push(json!({
            "name": name,
            "version": field(package, "version", inherited.as_ref()).unwrap_or_default(),
            "manifest_path": manifest_path,
            "targets": targets(vfs, dir, &manifest, name),
            "dependencies": dependencies(&manifest),
        }));
    }

    Ok(json!({
        "packages": packages,
        // The resolved tree, which the manifests alone do not give: versions of
        // everything, including the crates nothing in this repository names.
        "lock": lockfile(vfs),
    }))
}

/// `version.workspace = true` means look it up in `[workspace.package]`.
fn field(package: &toml::Value, key: &str, inherited: Option<&toml::Value>) -> Option<String> {
    match package.get(key) {
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(toml::Value::Table(t)) if t.get("workspace").and_then(|w| w.as_bool()) == Some(true) => {
            inherited?.get(key)?.as_str().map(str::to_string)
        }
        _ => None,
    }
}

/// Cargo's target conventions, plus whatever the manifest states explicitly.
///
/// This is where the layouts that have already caught this extractor live: deno
/// declares `ext/fs/lib.rs` rather than `src/lib.rs`, and a build script at a
/// repository root would otherwise claim the whole repository.
fn targets(vfs: &Vfs, dir: &str, manifest: &toml::Value, package: &str) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let mut add = |name: &str, kind: &str, path: String| {
        // The same file declared and then discovered is still one target.
        if vfs.contains(&path) && !out.iter().any(|t| t["src_path"] == json!(path)) {
            out.push(json!({ "name": name, "kind": [kind], "src_path": path }));
        }
    };

    match manifest.get("lib") {
        Some(lib) => {
            let path = lib
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| join(dir, p))
                .unwrap_or_else(|| join(dir, "src/lib.rs"));
            let name = lib.get("name").and_then(|v| v.as_str()).unwrap_or(package);
            add(name, "lib", path);
        }
        None => add(package, "lib", join(dir, "src/lib.rs")),
    }

    // Declaring a target does not switch discovery off — cargo reports both.
    // tokio's tests-integration lists four [[test]] entries in a directory of
    // five files, and cargo finds all five. Only an explicit `autotests = false`
    // and its siblings suppress the rest.
    for bin in manifest.get("bin").and_then(|v| v.as_array()).into_iter().flatten() {
        let name = bin.get("name").and_then(|v| v.as_str()).unwrap_or(package);
        let path = bin
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| join(dir, p))
            .unwrap_or_else(|| join(dir, format!("src/bin/{name}.rs")));
        add(name, "bin", path);
    }
    if auto_enabled(manifest, "autobins") {
        add(package, "bin", join(dir, "src/main.rs"));
        for path in auto(vfs, &join(dir, "src/bin")) {
            let name = stem(&path);
            add(&name, "bin", path);
        }
    }

    // Declared targets win over discovered ones: tokio's examples crate lists
    // twenty [[example]] entries pointing at files in its own root, which no
    // amount of convention would find.
    for (folder, kind, table, flag) in [
        ("tests", "test", "test", "autotests"),
        ("benches", "bench", "bench", "autobenches"),
        ("examples", "example", "example", "autoexamples"),
    ] {
        for entry in manifest.get(table).and_then(|v| v.as_array()).into_iter().flatten() {
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or(package);
            let path = entry
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| join(dir, p))
                .unwrap_or_else(|| join(dir, format!("{folder}/{name}.rs")));
            add(name, kind, path);
        }
        if auto_enabled(manifest, flag) {
            for path in auto(vfs, &join(dir, folder)) {
                let name = stem(&path);
                add(&name, kind, path);
            }
        }
    }

    let build = manifest
        .get("package")
        .and_then(|p| p.get("build"))
        .and_then(|v| v.as_str())
        .map(|p| join(dir, p))
        .unwrap_or_else(|| join(dir, "build.rs"));
    add("build-script-build", "custom-build", build);

    json!(out)
}

/// `autotests = false` and friends turn discovery off for one target kind.
fn auto_enabled(manifest: &toml::Value, flag: &str) -> bool {
    manifest
        .get("package")
        .and_then(|p| p.get(flag))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Files cargo would pick up automatically: `foo.rs`, or `foo/main.rs`.
fn auto(vfs: &Vfs, folder: &str) -> Vec<String> {
    let mut found = Vec::new();
    for path in vfs.under(folder) {
        let rest = &path[folder.len() + 1..];
        let depth = rest.matches('/').count();
        // `foo.rs` directly in the folder, or `foo/main.rs` one level down.
        let discovered =
            (depth == 0 && rest.ends_with(".rs")) || (depth == 1 && rest.ends_with("/main.rs"));
        if discovered {
            found.push(path.to_string());
        }
    }
    found
}

fn dependencies(manifest: &toml::Value) -> Value {
    let mut out = Vec::new();
    for (table, kind) in [
        ("dependencies", "normal"),
        ("dev-dependencies", "dev"),
        ("build-dependencies", "build"),
    ] {
        if let Some(t) = manifest.get(table).and_then(|v| v.as_table()) {
            for name in t.keys() {
                out.push(json!({ "name": name, "kind": kind }));
            }
        }
    }
    json!(out)
}

/// Every crate in the resolved tree with its exact version.
///
/// This is the surface an advisory database is matched against, and it is
/// mostly invisible from the manifests: a workspace naming eighty dependencies
/// resolves to hundreds.
fn lockfile(vfs: &Vfs) -> Value {
    let Some(text) = vfs.read("Cargo.lock") else {
        return json!({});
    };
    let Ok(lock) = toml::from_str::<toml::Value>(text) else {
        return json!({});
    };
    let mut out = BTreeMap::new();
    for package in lock.get("package").and_then(|p| p.as_array()).into_iter().flatten() {
        if let (Some(name), Some(version)) = (
            package.get("name").and_then(|v| v.as_str()),
            package.get("version").and_then(|v| v.as_str()),
        ) {
            out.insert(name.to_string(), version.to_string());
        }
    }
    json!(out)
}

/// `crates/*` against the paths actually present.
fn expand(vfs: &Vfs, pattern: &str) -> Vec<String> {
    if !pattern.contains('*') {
        return vec![pattern.trim_end_matches('/').to_string()];
    }
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return Vec::new();
    };
    let prefix = prefix.trim_end_matches('/');
    let mut found: Vec<String> = Vec::new();
    for path in vfs.paths() {
        let Some(rest) = path.strip_prefix(&format!("{prefix}/")) else {
            continue;
        };
        let Some(segment) = rest.split('/').next() else {
            continue;
        };
        if !suffix.is_empty() && !segment.ends_with(suffix.trim_start_matches('/')) {
            continue;
        }
        let dir = format!("{prefix}/{segment}");
        if vfs.contains(&join(&dir, "Cargo.toml")) && !found.contains(&dir) {
            found.push(dir);
        }
    }
    found
}

/// Resolves a relative path against a directory, honouring `..`.
fn resolve(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    let rel = rel.replace('\\', "/");
    for segment in rel.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn join(dir: &str, rest: impl AsRef<str>) -> String {
    let rest = rest.as_ref().trim_start_matches("./");
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), rest)
    }
}

fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    if file == "main.rs" {
        return path
            .trim_end_matches("/main.rs")
            .rsplit('/')
            .next()
            .unwrap_or(file)
            .to_string();
    }
    file.trim_end_matches(".rs").to_string()
}
