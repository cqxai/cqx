//! H3 — the program name is a constant in the same file.
//! Expected base: PARTIAL — spawn seen, target reported as <dynamic>.
use std::process::Command;

const SHELL: &str = "/bin/bash";

pub fn run_shell(script: &str) {
    let _ = Command::new(SHELL).arg("-c").arg(script).status();
}
