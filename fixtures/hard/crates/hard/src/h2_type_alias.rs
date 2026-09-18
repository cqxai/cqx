//! H2 — a type alias hides the type entirely.
//! Expected base: MISS.
type Cmd = std::process::Command;

pub fn copy_remote() {
    let _ = Cmd::new("scp").arg("host:/f").status();
}
