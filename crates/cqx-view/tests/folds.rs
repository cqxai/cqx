//! What the fold has to get right for a reader to trust the page.
//!
//! Written against a snapshot built here rather than a fixture on disk: this
//! crate compiles for wasm, where there is no disk, and a test that needs one
//! would be testing a path the browser never takes.

use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
use serde_json::Value;

fn analyse(files: &[(&str, &str)]) -> Value {
    let mut vfs = Vfs::new("test");
    for (path, content) in files {
        vfs.insert(*path, *content);
    }
    let mut facts = Vec::new();
    cqx_rust::extract::run(&vfs, &mut facts).expect("extract");
    let stream = Stream::from_ndjson(&String::from_utf8_lossy(&facts));
    cqx_view::dataset(
        &stream,
        serde_json::json!({}),
        serde_json::json!([]),
        &cqx_view::Meta {
            repo: "acme/thing",
            branch: "main",
            remote: None,
            commits_url: None,
            analysed_ms: None,
            fetched_ms: None,
        },
    )
}

const MANIFEST: &str = r#"
[package]
name = "widget"
version = "0.1.0"
"#;

#[test]
fn a_file_belongs_to_the_crate_that_declares_it() {
    let data = analyse(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub fn nothing() {}\n"),
    ]);
    let file = &data["files"][0];
    assert_eq!(file["path"], "src/lib.rs");
    assert_eq!(file["pkg"], "pkg:widget");
    let package = &data["packages"][0];
    assert_eq!(package["name"], "widget");
    assert_eq!(package["files"], 1);
}

#[test]
fn a_function_that_starts_a_process_is_notable_and_says_so() {
    let data = analyse(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            r#"
use std::process::Command;
pub fn launch() {
    let _ = Command::new("dsc").status();
}
pub fn quiet() {}
"#,
        ),
    ]);
    let functions = data["functions"].as_array().expect("functions");
    assert_eq!(functions.len(), 1, "only the interesting one is listed");
    assert_eq!(functions[0]["name"], "launch");
    assert_eq!(functions[0]["eff"][0], "spawns");
    // Every function is still counted, or the ratio the page prints is a lie.
    assert_eq!(data["totals"]["functions"], 2);
    assert_eq!(data["totals"]["notable"], 1);

    let effect = &data["effects"][0];
    assert_eq!(effect["k"], "spawns");
    assert_eq!(effect["to"], "dsc");
    assert_eq!(effect["file"], "src/lib.rs");
    assert_eq!(effect["pkg"], "widget");
}

#[test]
fn a_signature_attributes_each_name_to_the_crate_that_defines_it() {
    let data = analyse(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            r#"
pub struct Widget { pub id: String }
pub fn find(id: &str) -> Result<Widget, String> { let _ = id; Err(String::new()) }
"#,
        ),
    ]);
    let found = data["functions"]
        .as_array()
        .expect("functions")
        .iter()
        .find(|f| f["name"] == "find")
        .expect("Result<_, String> makes a function worth showing");

    let returns = &found["r"];
    assert_eq!(returns[0], "Result<Widget,String>");
    let parts = returns[1].as_array().expect("parts");
    // `Result` and `String` come from outside this scan, so neither is claimed.
    assert_eq!(parts[0], serde_json::json!(["Result", Value::Null, 0]));
    assert!(parts
        .iter()
        .any(|p| p[0] == "Widget" && p[1] == "widget" && p[2] == 1));

    let widget = data["types"]
        .as_array()
        .expect("types")
        .iter()
        .find(|t| t["name"] == "Widget")
        .expect("a declared type is listed");
    assert_eq!(widget["k"], "struct");
    assert_eq!(widget["pkg"], "widget");
    assert_eq!(widget["fields"][0][0], "id");
}

#[test]
fn a_workspace_below_the_root_is_still_found() {
    // vercel-labs/agent-browser: a repository most of the way Rust, whose Rust
    // lives in cli/ beside a web application. Refusing to look below the root
    // reported it as having none.
    let data = analyse(&[
        ("package.json", "{}"),
        ("cli/Cargo.toml", MANIFEST),
        ("cli/src/lib.rs", "pub fn nothing() {}\n"),
    ]);
    assert_eq!(data["packages"][0]["name"], "widget");
    // Repo-relative, not rebased: a finding has to point at a path someone can
    // open in the repository it came from.
    assert_eq!(data["files"][0]["path"], "cli/src/lib.rs");
}

#[test]
fn the_shallowest_manifest_wins_and_a_workspace_beats_a_package() {
    let workspace = r#"
[workspace]
members = ["inner"]
"#;
    let data = analyse(&[
        ("tools/one/Cargo.toml", MANIFEST),
        ("tools/one/src/lib.rs", "pub fn a() {}\n"),
        ("rust/Cargo.toml", workspace),
        ("rust/inner/Cargo.toml", MANIFEST),
        ("rust/inner/src/lib.rs", "pub fn b() {}\n"),
    ]);
    // rust/ is shallower than tools/one, and names a member besides.
    assert_eq!(data["files"][0]["path"], "rust/inner/src/lib.rs");
    assert_eq!(data["files"].as_array().expect("files").len(), 1);
}
