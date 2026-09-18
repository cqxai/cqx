use engine::Pipeline;
use std::process::Command;
use util::report;

mod config;

fn main() {
    let mode = std::env::var("APP_MODE").unwrap_or_else(|_| "dev".to_string());
    let pipeline = Pipeline::new(&mode);
    pipeline.run();
    compile_with_dsc("main.ds");
    report("done");
}

/// Planted: the one process boundary in this sample.
fn compile_with_dsc(entry: &str) {
    let _ = Command::new("dsc").arg("build").arg(entry).status();
}
