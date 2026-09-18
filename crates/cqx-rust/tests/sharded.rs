//! Reading a workspace in pieces says what reading it whole says.
//!
//! The reading is the expensive part — of makepad's hundred and thirty
//! seconds in a browser, all but a second is this — and it divides, because a
//! file parses on its own. What does not divide is what the parsing is
//! resolved against: a value's provenance routinely crosses a crate boundary,
//! so a reader holding one slice would follow fewer of them and the scores
//! would depend on how the work had been split. Three phases avoid that, and
//! this is what proves it.

use cqx_rust::extract;
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;

/// A workspace whose provenance crosses a crate boundary, so that slicing it
/// wrongly is visible: `runner` spawns whatever `tool::name()` returns, and
/// that function is in another crate.
fn workspace() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"tool\", \"runner\", \"extra\"]\n",
        ),
        (
            "tool/Cargo.toml",
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n",
        ),
        (
            "tool/src/lib.rs",
            r#"
pub const HELPER: &str = "dsc";
pub fn name() -> &'static str { "dsc" }
"#,
        ),
        (
            "runner/Cargo.toml",
            "[package]\nname = \"runner\"\nversion = \"0.1.0\"\n\n[dependencies]\ntool = { path = \"../tool\" }\n",
        ),
        (
            "runner/src/lib.rs",
            r#"
use std::process::Command;
pub fn launch() {
    let _ = Command::new(tool::name()).status();
}
pub fn from_env() {
    let _ = Command::new(std::env::var("EDITOR").unwrap()).status();
}
"#,
        ),
        (
            "extra/Cargo.toml",
            "[package]\nname = \"extra\"\nversion = \"0.1.0\"\n",
        ),
        (
            "extra/src/lib.rs",
            "pub fn plain(a: &str, b: &str, c: &str) { let _ = (a, b, c); }\n",
        ),
    ]
}

fn vfs_of(files: &[(&str, &str)]) -> Vfs {
    let mut vfs = Vfs::new("test");
    for (path, content) in files {
        vfs.insert(*path, *content);
    }
    vfs
}

/// Everything, read at once.
fn whole(files: &[(&str, &str)]) -> Stream {
    let vfs = vfs_of(files);
    let mut out = Vec::new();
    extract::run(&vfs, &mut out).expect("extract");
    Stream::from_ndjson(&String::from_utf8_lossy(&out))
}

/// The same, read by `shards` readers that share their manifests and split
/// the sources between them — the three phases, in order.
fn in_pieces(files: &[(&str, &str)], shards: usize) -> Stream {
    let manifests: Vec<_> = files
        .iter()
        .filter(|(p, _)| p.ends_with("Cargo.toml") || p.ends_with("Cargo.lock"))
        .copied()
        .collect();
    let sources: Vec<_> = files
        .iter()
        .filter(|(p, _)| p.ends_with(".rs"))
        .copied()
        .collect();

    // Every reader gets every manifest, so each describes the same packages,
    // and a contiguous run of the sources, so merging in order reproduces
    // reading them in order.
    let mut slices: Vec<Vfs> = Vec::new();
    for shard in 0..shards {
        let mut held: Vec<(&str, &str)> = manifests.clone();
        held.extend(
            sources
                .iter()
                .enumerate()
                .filter(|(i, _)| i % shards == shard)
                .map(|(_, f)| *f),
        );
        slices.push(vfs_of(&held));
    }

    // Nought: read the manifests once, against everything. A reader holding
    // part of the sources would discover fewer targets, and a file found under
    // a different target is given a different module path.
    let all = vfs_of(files);
    let metadata = cqx_rust::manifest::read(&all).expect("manifests");

    // One: parse, and say what was found.
    let prepared: Vec<_> = slices
        .iter()
        .map(|vfs| extract::prepare_with(vfs, metadata.clone()).expect("prepare"))
        .collect();

    // Two: merge what they found — only the part that means something beyond
    // the file it came from — then follow it, once, over all of it.
    let mut shared = cqx_rust::prepass::Shared::default();
    let own: Vec<_> = prepared.iter().map(|one| one.gathered()).collect();
    for facts in &own {
        shared.merge(facts.shared());
    }
    shared.resolve();

    // Three: each reader writes down what it holds, resolved against
    // everything every reader found, keeping its own file-scoped aliases.
    let mut merged = Stream::default();
    for ((vfs, one), mut facts) in slices.iter().zip(&prepared).zip(own) {
        facts.adopt(shared.clone());
        let mut out = Vec::new();
        one.emit(vfs, &facts, &mut out).expect("emit");
        let part = Stream::from_ndjson(&String::from_utf8_lossy(&out));
        merged.nodes.extend(part.nodes);
        merged.edges.extend(part.edges);
    }
    merged.dedupe();
    merged
}

fn shape(stream: &Stream) -> (usize, usize, Vec<String>) {
    let mut ids: Vec<String> = stream.nodes.iter().map(|n| n.id.0.clone()).collect();
    ids.sort();
    (stream.nodes.len(), stream.edges.len(), ids)
}

#[test]
fn a_workspace_read_in_pieces_holds_what_it_held_whole() {
    let files = workspace();
    let once = whole(&files);
    for shards in [2, 3, 5] {
        let split = in_pieces(&files, shards);
        let (n0, e0, ids0) = shape(&once);
        let (n1, e1, ids1) = shape(&split);
        assert_eq!(ids0, ids1, "{shards} readers found different nodes");
        assert_eq!(n0, n1, "{shards} readers: node count");
        assert_eq!(e0, e1, "{shards} readers: edge count");
    }
}

#[test]
fn provenance_across_a_crate_boundary_survives_the_split() {
    let files = workspace();
    let spawn_targets = |stream: &Stream| {
        let mut found: Vec<String> = stream
            .edges
            .iter()
            .filter(|e| e.kind == cqx_schema::EdgeKind::Spawns)
            .map(|e| {
                format!(
                    "{} via {}",
                    e.to.0,
                    e.attrs.get("via").and_then(|v| v.as_str()).unwrap_or("-")
                )
            })
            .collect();
        found.sort();
        found
    };
    // `runner` spawns what `tool::name()` returns, and reads EDITOR. A reader
    // holding only runner/ would resolve neither.
    let once = spawn_targets(&whole(&files));
    assert!(!once.is_empty(), "the fixture should spawn something");
    for shards in [2, 3, 5] {
        assert_eq!(once, spawn_targets(&in_pieces(&files, shards)), "{shards} readers");
    }
}

/// The same claim, against a repository on disk rather than a fixture.
///
/// Off by default because it needs one. Point `CQX_SHARD_REPO` at a checkout
/// and it reads it whole, then in pieces, and insists the two agree — which is
/// the only form of this test that says anything about deno's sixty-three
/// workspace dependencies or makepad's three hundred and seventy manifests.
#[test]
fn a_real_repository_read_in_pieces_agrees() {
    let Ok(root) = std::env::var("CQX_SHARD_REPO") else {
        eprintln!("CQX_SHARD_REPO unset; skipping");
        return;
    };
    let shards: usize = std::env::var("CQX_SHARDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let vfs = cqx_vfs::from_dir(std::path::Path::new(&root)).expect("read the checkout");
    let files: Vec<(String, String)> = vfs
        .paths()
        .map(|p| (p.to_string(), vfs.read(p).unwrap_or_default().to_string()))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();

    let mut once = whole(&borrowed);
    let before = once.edges.len();
    // Both sides are compared as sets of facts, because that is what a fact
    // stream means. A reader writing the same edge twice has said one thing.
    once.dedupe();
    if once.edges.len() != before {
        eprintln!("  whole held {} duplicate edges", before - once.edges.len());
    }
    let split = in_pieces(&borrowed, shards);
    let (n0, e0, ids0) = shape(&once);
    let (n1, e1, ids1) = shape(&split);
    eprintln!("  whole:  {n0} nodes, {e0} edges");
    eprintln!("  {shards} ways: {n1} nodes, {e1} edges");
    if ids0 != ids1 {
        let missing: Vec<_> = ids0.iter().filter(|i| !ids1.contains(i)).take(5).collect();
        let extra: Vec<_> = ids1.iter().filter(|i| !ids0.contains(i)).take(5).collect();
        panic!("nodes differ · missing {missing:?} · extra {extra:?}");
    }
    assert_eq!(e0, e1, "edge count");
}
