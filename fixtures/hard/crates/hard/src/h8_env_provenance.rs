//! H8 — the shape that actually dominates real code, added after the first
//! seven turned out to be unrepresentative.
//!
//! The target is not a literal anywhere. It is bound from a function that picks
//! it up from the environment, so the useful answer is not a program name but
//! the provenance: this spawn is controlled by whoever sets $HARD_TOOL.
//!
//! Expected base: spawn found, target `<dynamic>`, via `env`, naming both the
//! function and the variable.
use std::path::PathBuf;
use std::process::Command;

fn tool_path() -> Option<PathBuf> {
    match std::env::var("HARD_TOOL") {
        Ok(path) => Some(PathBuf::from(path)),
        Err(_) => None,
    }
}

pub fn run_tool(arg: &str) -> bool {
    let tool = match tool_path() {
        Some(tool) => tool,
        None => return false,
    };
    Command::new(tool).arg(arg).status().is_ok()
}
