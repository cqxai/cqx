//! H7 — the program name comes from a named function in the same crate.
//! Expected base: PARTIAL — spawn seen, target <dynamic>. This is the shape
//! that dominates the real dynamic sites in deka.
use std::process::Command;

fn encoder_binary() -> &'static str {
    "ffmpeg"
}

pub fn encode(input: &str) {
    let _ = Command::new(encoder_binary()).arg(input).status();
}
