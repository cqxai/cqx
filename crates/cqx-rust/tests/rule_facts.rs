//! The shapes a policy asks about, and the facts that answer.
//!
//! Each of these is a real finding from an audit of a real workspace, reduced
//! to the smallest code that has the same shape. They are here because the
//! question "could the tool have found this" turned out to be answerable, and
//! because a fact nobody tests is a fact that quietly stops being emitted.

use cqx_rust::extract;
use cqx_schema::EdgeKind;
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;

fn facts(source: &str) -> Stream {
    let mut vfs = Vfs::new("test");
    vfs.insert(
        "Cargo.toml",
        "[package]\nname = \"subject\"\nversion = \"0.1.0\"\n",
    );
    vfs.insert("src/lib.rs", source);
    let mut out = Vec::new();
    extract::run(&vfs, &mut out).expect("extract");
    Stream::from_ndjson(&String::from_utf8_lossy(&out))
}

/// Every edge of a kind, as `(from, to)`, so a test reads like the claim.
fn edges(stream: &Stream, kind: EdgeKind) -> Vec<(String, String)> {
    stream
        .edges
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| (e.from.0.clone(), e.to.0.clone()))
        .collect()
}

fn attrs_of(stream: &Stream, kind: EdgeKind) -> Vec<&serde_json::Map<String, serde_json::Value>> {
    stream
        .edges
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| &e.attrs)
        .collect()
}

/// Who calls what, by name — including through a method, which is how most
/// calls in Rust are written and none of which were recorded before.
#[test]
fn a_call_is_recorded_by_name_however_it_is_written() {
    let stream = facts(
        r#"
pub fn enforce_read(_p: &str) -> Result<(), String> { Ok(()) }

pub struct Guard;
impl Guard {
    pub fn check(&self) {}
}

pub fn checked(g: &Guard) {
    let _ = enforce_read("/tmp");
    g.check();
}
"#,
    );
    let called: Vec<String> = edges(&stream, EdgeKind::Calls)
        .into_iter()
        .map(|(_, to)| to)
        .collect();
    assert!(
        called.iter().any(|t| t == "ext:enforce_read"),
        "a plain call should be recorded: {called:?}"
    );
    assert!(
        called.iter().any(|t| t == "ext:check"),
        "a method call should be recorded too: {called:?}"
    );
}

/// `let _ = enforce(..)` reads as enforcement and performs none.
#[test]
fn a_check_whose_answer_is_thrown_away_is_recorded() {
    let stream = facts(
        r#"
pub fn enforce_read(_p: &str) -> Result<(), String> { Err("denied".into()) }

pub fn op_resolve(path: &str) -> String {
    let _ = enforce_read(path);
    path.to_string()
}
"#,
    );
    let discarded = edges(&stream, EdgeKind::Discards);
    assert!(
        discarded.iter().any(|(from, to)| from.contains("op_resolve") && to == "ext:enforce_read"),
        "the discarded check should be recorded: {discarded:?}"
    );
}

/// A dispatch whose fallback does nothing permits everything nobody listed.
#[test]
fn a_dispatch_with_an_empty_fallback_is_recorded() {
    let stream = facts(
        r#"
pub fn enforce(_a: &str) {}

pub fn dispatch(action: &str) {
    match action {
        "read" => enforce("read"),
        "write" => enforce("write"),
        _ => {}
    }
}
"#,
    );
    let arms = attrs_of(&stream, EdgeKind::DefaultArm);
    assert_eq!(arms.len(), 1, "one match, one catch-all");
    assert_eq!(arms[0].get("arms").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(
        arms[0].get("empty").and_then(|v| v.as_bool()),
        Some(true),
        "the fallback does nothing and should say so"
    );
}

/// A fallback that does something is not the same shape and must not be
/// reported as if it were.
#[test]
fn a_fallback_that_acts_is_not_reported_as_empty() {
    let stream = facts(
        r#"
pub fn enforce(_a: &str) {}
pub fn refuse() {}

pub fn dispatch(action: &str) {
    match action {
        "read" => enforce("read"),
        _ => refuse(),
    }
}
"#,
    );
    let arms = attrs_of(&stream, EdgeKind::DefaultArm);
    assert_eq!(arms.len(), 1);
    assert_eq!(arms[0].get("empty").and_then(|v| v.as_bool()), Some(false));
}

/// The program alone does not say what was asked of it.
#[test]
fn what_is_handed_to_a_subprocess_is_recorded() {
    let stream = facts(
        r#"
use std::process::Command;

pub fn run_shell(script: &str) {
    let _ = Command::new("/bin/zsh").arg("-lc").arg(script).status();
}

pub fn run_child() {
    let _ = Command::new("deka").arg("--allow-all").status();
}
"#,
    );
    let args = attrs_of(&stream, EdgeKind::SpawnArg);
    let literals: Vec<&str> = args
        .iter()
        .filter_map(|a| a.get("value").and_then(|v| v.as_str()))
        .collect();
    assert!(literals.contains(&"-lc"), "a literal argument: {literals:?}");
    assert!(
        literals.contains(&"--allow-all"),
        "a permission-granting literal is exactly what a rule looks for: {literals:?}"
    );
    assert!(
        args.iter()
            .any(|a| a.get("via").and_then(|v| v.as_str()) == Some("unresolved")),
        "the argument that came from a parameter cannot be resolved, and saying so is the point"
    );
}

/// A structured format assembled by interpolation: the values decide the
/// structure.
#[test]
fn json_built_by_interpolation_is_recorded() {
    let stream = facts(
        r#"
pub fn reply(shop: &str) -> String {
    format!("{{\"ok\":true,\"shop\":\"{}\"}}", shop)
}

pub fn greeting(name: &str) -> String {
    format!("hello {}", name)
}
"#,
    );
    let built = edges(&stream, EdgeKind::Interpolates);
    assert_eq!(
        built.len(),
        1,
        "the hand-built object, and not the greeting: {built:?}"
    );
    assert!(built[0].0.contains("reply"));
}

/// An effect written inside a macro is still an effect. `syn` hands back an
/// opaque token stream and the default walk stops there, so before this the
/// whole line was invisible.
#[test]
fn an_effect_inside_a_macro_is_seen() {
    let stream = facts(
        r#"
pub fn report() {
    println!("editor is {}", std::env::var("EDITOR").unwrap_or_default());
}
"#,
    );
    let read = edges(&stream, EdgeKind::ReadsEnv);
    assert!(
        read.iter().any(|(_, to)| to == "env:EDITOR"),
        "the variable read inside println! should be recorded: {read:?}"
    );
}

/// Code a macro *generates* is not what this function does, and guessing at it
/// would fill the graph with facts about a template.
#[test]
fn a_quoted_template_is_left_alone() {
    let stream = facts(
        r#"
pub fn generate() -> String {
    let body = quote::quote! { std::process::exit(1) };
    body.to_string()
}
"#,
    );
    let exits = edges(&stream, EdgeKind::EffectExec);
    assert!(
        exits.is_empty(),
        "a template is not a call: {exits:?}"
    );
}
