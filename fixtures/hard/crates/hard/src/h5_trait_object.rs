//! H5 — dispatch through a trait object.
//! Expected base: PARTIAL — the spawn is found inside the impl, but nothing
//! connects `dispatch` to it, so reachability from the caller is lost.
use std::process::Command;

pub trait Runner {
    fn go(&self);
}

pub struct Git;

impl Runner for Git {
    fn go(&self) {
        let _ = Command::new("git").arg("status").status();
    }
}

pub fn dispatch(runner: &dyn Runner) {
    runner.go();
}
