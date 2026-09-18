use std::env;

pub struct Config {
    pub verbose: bool,
}

impl Config {
    pub fn load() -> Config {
        Config {
            verbose: env::var("APP_VERBOSE").is_ok(),
        }
    }
}
