//! Rule configuration: defaults, a JSON file, and environment overrides.
//!
//! Precedence is defaults < file < environment < flags. Every layer is partial:
//! a config that mentions one rule changes that rule and leaves the rest alone,
//! because a tool whose config file must restate every default is a tool people
//! copy once and never revisit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a setting came from, so `--explain` can show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Default,
    File,
    Env,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Origin::Default => "default",
            Origin::File => "config",
            Origin::Env => "env",
        })
    }
}

/// One scoring rule.
///
/// `free` and `full` bound a ramp rather than a step: a value at or below `free`
/// costs nothing, a value at or above `full` costs the whole weight, and
/// anything between is proportional. Thresholds are in the metric's own units,
/// which for most rules is "per ten thousand lines" so that a large codebase is
/// not penalised for being large.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub category: String,
    /// The most this rule can ever deduct.
    pub weight: f64,
    /// At or below this value, the rule deducts nothing.
    pub free: f64,
    /// At or above this value, the rule deducts its whole weight.
    pub full: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// The partial form: every field optional, so a file or an environment variable
/// can change one number without restating the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulePatch {
    pub weight: Option<f64>,
    pub free: Option<f64>,
    pub full: Option<f64>,
    pub enabled: Option<bool>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    /// Schema version of this file, so a future change can be detected rather
    /// than silently misread.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub rules: BTreeMap<String, RulePatch>,
    /// Fail the run when the lowest category score falls below this.
    #[serde(default)]
    pub min_score: Option<u32>,
    /// Paths excluded from every metric, as prefixes.
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub rules: BTreeMap<String, Rule>,
    pub origins: BTreeMap<String, Origin>,
    pub min_score: Option<u32>,
    pub exclude: Vec<String>,
    pub loaded_from: Option<PathBuf>,
}

/// The calibrated defaults.
///
/// Thresholds are anchored on ripgrep, tokio and deno rather than on intuition —
/// which is how two earlier rules were found to measure nothing. Densities are
/// per ten thousand lines of product code.
pub fn defaults() -> BTreeMap<String, Rule> {
    let mut rules = BTreeMap::new();
    let mut add = |id: &str, category: &str, weight: f64, free: f64, full: f64| {
        rules.insert(
            id.to_string(),
            Rule {
                category: category.to_string(),
                weight,
                free,
                full,
                enabled: true,
            },
        );
    };
    // Quality — reinvention and silencing.
    add("result-string-density", "quality", 30.0, 1.0, 20.0);
    add("broad-lint-silencing", "quality", 25.0, 0.3, 2.0);
    add("duplicated-bodies", "quality", 10.0, 0.5, 3.0);
    add("undocumented-suppressions", "quality", 10.0, 1.0, 6.0);
    // Containment — effects escaping the crate that should own them.
    add("exit-in-library", "containment", 30.0, 1.0, 15.0);
    // Legibility — what the tool, and the next author, can follow.
    add("bare-string-params", "legibility", 20.0, 0.12, 0.35);
    add("unproven-spawn-targets", "legibility", 30.0, 0.2, 0.8);
    // Security — reach that an attacker could steer.
    add("env-controlled-spawn", "security", 30.0, 0.0, 2.0);
    add("shell-invocation", "security", 20.0, 0.0, 1.0);
    rules
}

impl Config {
    /// Applies the layers in order and records where each rule ended up coming
    /// from.
    pub fn resolve(explicit: Option<&Path>, root: &Path) -> Result<Config, String> {
        let mut rules = defaults();
        let mut origins: BTreeMap<String, Origin> = rules
            .keys()
            .map(|id| (id.clone(), Origin::Default))
            .collect();
        let mut min_score = None;
        let mut exclude = Vec::new();

        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => std::env::var_os("CQX_CONFIG")
                .map(PathBuf::from)
                .or_else(|| discover(root)),
        };

        let mut loaded_from = None;
        if let Some(path) = path {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let file: ConfigFile = serde_json::from_str(&text)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if file.version > 1 {
                return Err(format!(
                    "{}: config version {} is newer than this build understands (1)",
                    path.display(),
                    file.version
                ));
            }
            for (id, patch) in &file.rules {
                let Some(rule) = rules.get_mut(id) else {
                    return Err(format!(
                        "{}: no rule named '{id}'. Run `cqx score --explain` for the list.",
                        path.display()
                    ));
                };
                apply(rule, patch);
                origins.insert(id.clone(), Origin::File);
            }
            min_score = file.min_score;
            exclude = file.exclude;
            loaded_from = Some(path);
        }

        // Environment last, because CI sets it per run.
        for (id, rule) in rules.iter_mut() {
            let key = id.to_uppercase().replace('-', "_");
            let mut touched = false;
            for (suffix, field) in [("WEIGHT", 0), ("FREE", 1), ("FULL", 2)] {
                if let Some(raw) = std::env::var_os(format!("CQX_RULE_{key}_{suffix}")) {
                    let v: f64 = raw
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| format!("CQX_RULE_{key}_{suffix} is not a number"))?;
                    match field {
                        0 => rule.weight = v,
                        1 => rule.free = v,
                        _ => rule.full = v,
                    }
                    touched = true;
                }
            }
            if let Some(raw) = std::env::var_os(format!("CQX_RULE_{key}_ENABLED")) {
                rule.enabled = matches!(
                    raw.to_string_lossy().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                );
                touched = true;
            }
            if touched {
                origins.insert(id.clone(), Origin::Env);
            }
        }
        if let Some(raw) = std::env::var_os("CQX_MIN_SCORE") {
            min_score = raw.to_string_lossy().parse().ok();
        }

        for (id, rule) in &rules {
            if rule.full <= rule.free {
                return Err(format!(
                    "rule '{id}': full ({}) must be greater than free ({})",
                    rule.full, rule.free
                ));
            }
        }

        Ok(Config {
            rules,
            origins,
            min_score,
            exclude,
            loaded_from,
        })
    }
}

fn apply(rule: &mut Rule, patch: &RulePatch) {
    if let Some(v) = patch.weight {
        rule.weight = v;
    }
    if let Some(v) = patch.free {
        rule.free = v;
    }
    if let Some(v) = patch.full {
        rule.full = v;
    }
    if let Some(v) = patch.enabled {
        rule.enabled = v;
    }
    if let Some(v) = &patch.category {
        rule.category = v.clone();
    }
}

/// Walks up from the scanned root looking for a config, the way a formatter or
/// linter does.
fn discover(root: &Path) -> Option<PathBuf> {
    let mut dir = Some(root);
    while let Some(d) = dir {
        for name in ["cqx.json", ".cqx.json"] {
            let candidate = d.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        dir = d.parent();
    }
    None
}
