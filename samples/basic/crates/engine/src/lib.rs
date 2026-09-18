use std::fs;
use util::report;

pub mod cache;

pub struct Pipeline {
    mode: String,
}

impl Pipeline {
    pub fn new(mode: &str) -> Pipeline {
        Pipeline {
            mode: mode.to_string(),
        }
    }

    pub fn run(&self) {
        let source = fs::read_to_string("input.txt").unwrap_or_default();
        fs::write("output.txt", source.to_uppercase()).ok();
        report(&self.mode);
    }
}
