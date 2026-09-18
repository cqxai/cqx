//! H1 — the import is renamed, so the call is spelled `Proc::new`.
//! Expected base: MISS. Nothing named `Command` appears at the call site.
use std::process::Command as Proc;

pub fn sync_files() {
    let _ = Proc::new("rsync").arg("-a").status();
}
