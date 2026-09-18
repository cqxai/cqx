//! H4 — the call only exists after macro expansion.
//! Expected base: MISS. The definition is a token stream and the call site is
//! a macro invocation, so neither is an expression the visitor inspects.
macro_rules! run_tool {
    ($tool:expr) => {
        std::process::Command::new($tool).status()
    };
}

pub fn fetch_url() {
    let _ = run_tool!("curl");
}
