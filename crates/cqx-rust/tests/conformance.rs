//! Checks the manifest parser against cargo itself.
//!
//! cqx reads a workspace by parsing manifests rather than by running
//! `cargo metadata`, because a subprocess is the one thing a browser cannot do.
//! That makes cargo the oracle: whatever it reports about packages, versions and
//! target roots is what the parser has to reproduce.
//!
//! The repositories are external on purpose. Two cargo rules were missing from
//! the parser and neither showed up against our own code — it took tokio and
//! deno to find them.
//!
//! Setting `CQX_REFERENCE_DIR` is a statement that the checkouts are there, so a
//! missing one is a failure. Leaving it unset is a laptop that has not cloned
//! 170MB of deno, and the test skips. Cargo captures output from a passing test,
//! so "skipped everything" would otherwise be indistinguishable from "checked
//! everything" — which is the failure this file exists to prevent.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the pinned checkouts live, and whether their absence is an error.
fn reference_dir() -> (PathBuf, bool) {
    match std::env::var_os("CQX_REFERENCE_DIR") {
        Some(dir) => (PathBuf::from(dir), true),
        None => (workspace_root().join(".reference"), false),
    }
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/cqx-rust.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn pinned_names() -> Vec<String> {
    let text = std::fs::read_to_string(workspace_root().join("reference-repos.toml"))
        .expect("reference-repos.toml");
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("name")?.trim_start().strip_prefix('=')?;
            Some(rest.trim().trim_matches('"').to_string())
        })
        .collect()
}

struct Truth {
    packages: BTreeSet<(String, String)>,
    /// Target source files, relative to the repository, build scripts excluded.
    roots: BTreeSet<String>,
}

/// What cargo says, which is the thing being reproduced.
fn cargo_truth(repo: &Path) -> Option<Truth> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let real = repo.canonicalize().ok()?;
    let mut packages = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for package in meta["packages"].as_array()?.iter() {
        packages.insert((
            package["name"].as_str()?.to_string(),
            package["version"].as_str().unwrap_or_default().to_string(),
        ));
        for target in package["targets"].as_array().into_iter().flatten() {
            let kinds = target["kind"].as_array().cloned().unwrap_or_default();
            // A build script is deliberately not a source root: one at a
            // repository root would otherwise claim the whole repository.
            if kinds.iter().any(|k| k.as_str() == Some("custom-build")) {
                continue;
            }
            let src = Path::new(target["src_path"].as_str()?);
            if let Ok(rel) = src.strip_prefix(&real) {
                roots.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Some(Truth { packages, roots })
}

#[test]
fn manifest_parser_agrees_with_cargo() {
    let (dir, required) = reference_dir();
    let mut checked = 0;
    let mut skipped = Vec::new();

    for name in pinned_names() {
        let repo = dir.join(&name);
        if !repo.join("Cargo.toml").is_file() {
            skipped.push(name);
            continue;
        }
        let Some(truth) = cargo_truth(&repo) else {
            skipped.push(format!("{name} (cargo metadata failed)"));
            continue;
        };

        let vfs = cqx_vfs::from_dir(&repo).expect("snapshot");
        let meta = cqx_rust::manifest::read(&vfs).expect("manifest parse");

        let mut packages = BTreeSet::new();
        let mut roots = BTreeSet::new();
        for package in meta["packages"].as_array().expect("packages") {
            packages.insert((
                package["name"].as_str().unwrap_or_default().to_string(),
                package["version"].as_str().unwrap_or_default().to_string(),
            ));
            for target in package["targets"].as_array().into_iter().flatten() {
                let kinds = target["kind"].as_array().cloned().unwrap_or_default();
                if kinds.iter().any(|k| k.as_str() == Some("custom-build")) {
                    continue;
                }
                roots.insert(target["src_path"].as_str().unwrap_or_default().to_string());
            }
        }

        let missing_packages: Vec<_> = truth.packages.difference(&packages).collect();
        let extra_packages: Vec<_> = packages.difference(&truth.packages).collect();
        let missing_roots: Vec<_> = truth.roots.difference(&roots).collect();

        assert!(
            missing_packages.is_empty() && extra_packages.is_empty(),
            "{name}: packages disagree with cargo\n  cargo has, cqx missed: {missing_packages:?}\n  cqx has, cargo did not: {extra_packages:?}"
        );
        assert!(
            missing_roots.is_empty(),
            "{name}: {} target roots cargo reports are missing, e.g. {:?}",
            missing_roots.len(),
            missing_roots.iter().take(5).collect::<Vec<_>>()
        );
        checked += 1;
    }

    assert!(
        !required || skipped.is_empty(),
        "CQX_REFERENCE_DIR is set to {}, so these were expected and are missing: {skipped:?}\n         See reference-repos.toml for the pinned revisions.",
        dir.display()
    );
    if !skipped.is_empty() {
        eprintln!(
            "conformance: checked {checked}, skipped {} — {skipped:?}\n\
             clone them into {} to include them (see reference-repos.toml)",
            skipped.len(),
            dir.display()
        );
    }
}
