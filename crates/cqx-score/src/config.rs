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
    /// What the rule measures, in one line. Present so that a reader — human or
    /// agent — can decide whether to change it without reading this source.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub describes: String,
    /// What to do about it. A finding without a remedy is a complaint, and a
    /// reader who does not already know the fix cannot act on the number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remedy: String,
    /// The most this rule can ever deduct.
    pub weight: f64,
    /// At or below this value, the rule deducts nothing.
    pub free: f64,
    /// At or above this value, the rule deducts its whole weight.
    pub full: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Knobs belonging to this rule alone — a line threshold, a ratio — as
    /// distinct from weight and thresholds, which every rule has.
    ///
    /// These exist because some standards are a house style rather than a fact
    /// about good code. File size is the clearest case: measured across ripgrep,
    /// tokio, deno, deka and dsc, it tracks a project's habits and not its
    /// quality, so cqx ships a lenient default and makes the knob obvious.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f64>,
}

impl Rule {
    pub fn param(&self, name: &str, fallback: f64) -> f64 {
        self.params.get(name).copied().unwrap_or(fallback)
    }
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
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
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
    let mut add = |id: &str,
                   category: &str,
                   weight: f64,
                   free: f64,
                   full: f64,
                   describes: &str,
                   remedy: &str| {
        rules.insert(
            id.to_string(),
            Rule {
                category: category.to_string(),
                describes: describes.to_string(),
                remedy: remedy.to_string(),
                weight,
                free,
                full,
                enabled: true,
                params: BTreeMap::new(),
            },
        );
    };
    // Quality — reinvention and silencing.
    add("result-string-density", "quality", 30.0, 1.0, 20.0, "functions returning Result<_, String> instead of a real error type, per 10k lines", "Give the crate an error type and return that. A String error cannot be matched on, so every caller either re-parses the text or swallows it.");
    add("broad-lint-silencing", "quality", 25.0, 0.3, 2.0, "crate-wide allow(clippy::all) or allow(warnings), per 10k lines", "Replace the blanket allow with the specific lints meant, or fix them. A crate-wide allow also silences everything written after it, including code nobody has typed yet.");
    add("duplicated-bodies", "quality", 10.0, 0.5, 3.0, "function bodies that are exact token copies of another, per 10k lines", "Move the shared body somewhere both callers can reach. The copies are listed below; they were found by exact token match, so they were copied rather than merely similar.");
    add("undocumented-suppressions", "quality", 10.0, 1.0, 6.0, "crate-level lint suppressions with no reason given, per 10k lines", "Add reason = \"...\" to each suppression. deno documents every one of its crate-level suppressions, which is why theirs read as decisions rather than residue.");
    // Containment — effects escaping the crate that should own them.
    add("exit-in-library", "containment", 30.0, 1.0, 15.0, "process::exit called from a library crate, per 10k lines", "Return an error and let the binary decide the exit code. A library that exits takes that decision away from every caller, including a test harness.");
    // Legibility — what the tool, and the next author, can follow.
    add("bare-string-params", "legibility", 20.0, 0.12, 0.35, "share of parameters declared as a bare string rather than a domain type", "Introduce a newtype for the concept. Three interchangeable strings in one signature is an argument swap that still compiles and still runs.");
    add("unproven-spawn-targets", "legibility", 30.0, 0.2, 0.8, "share of spawn and env targets the extractor could not resolve", "Where the target comes from a parameter, the caller is deciding it — hoisting the spawn there makes it knowable. Where it comes from the environment, that is the finding rather than a gap.");
    // Security — reach that an attacker could steer.
    add("env-controlled-spawn", "security", 30.0, 0.0, 2.0, "spawn targets chosen by an environment variable, per 10k lines", "Resolve the program from a known location, or validate it before spawning. As it stands, whoever sets the variable chooses what runs.");
    add("shell-invocation", "security", 20.0, 0.0, 1.0, "spawning a shell, which turns an argument into a command, per 10k lines", "Pass the program and its arguments directly rather than through a shell. A shell turns an argument into a command.");
    // Modularity — house style, so the defaults are deliberately lenient.
    //
    // Across the five reference projects, files over 1000 lines run from 1.07
    // per 10 kLOC (deka) to 3.04 (ripgrep) — the best-regarded codebase in the
    // set is the one with the most large files. A default that condemned that
    // would be wrong, so these begin to bite well above the field and the
    // threshold is a parameter for teams whose standard is stricter.
    add("oversized-files", "modularity", 25.0, 3.5, 12.0, "files longer than max_lines, per 10k lines — a house standard, not a fact", "Split the file, or raise the standard if this is simply how the project is written: cqx config set oversized-files.max_lines N. Measured across ripgrep, tokio and deno, file length tracks habit rather than quality.");
    add("oversized-line-share", "modularity", 15.0, 0.70, 0.95, "share of all lines living in files longer than max_lines", "Same standard as oversized-files, measured by weight rather than count: it catches a codebase where most of the code lives in a handful of very large files.");
    add("crate-type-scatter", "modularity", 10.0, 0.20, 1.50, "crates whose signatures are mostly built from three or more other crates' types", "A crate built mostly from other crates' types, pulled from several of them, usually wants splitting or absorbing. One strong pull is an adapter and perfectly fine.");
    rules.get_mut("oversized-files").unwrap().params.insert("max_lines".into(), 1000.0);
    rules
        .get_mut("oversized-line-share")
        .unwrap()
        .params
        .insert("max_lines".into(), 1000.0);
    rules
}

impl Config {
    /// Builds a configuration from text, or from the defaults when there is
    /// none. No filesystem: this is the form a browser can use, and the form
    /// `resolve` finishes with once it has found a file.
    pub fn from_text(text: Option<&str>) -> Result<Config, String> {
        let mut config = Config {
            rules: defaults(),
            origins: defaults().keys().map(|k| (k.clone(), Origin::Default)).collect(),
            min_score: None,
            exclude: Vec::new(),
            loaded_from: None,
        };
        if let Some(text) = text {
            let file: ConfigFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
            config.apply_file(&file, "configuration")?;
        }
        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    fn apply_file(&mut self, file: &ConfigFile, source: &str) -> Result<(), String> {
        if file.version > 1 {
            return Err(format!(
                "{source}: config version {} is newer than this build understands (1)",
                file.version
            ));
        }
        for (id, patch) in &file.rules {
            let Some(rule) = self.rules.get_mut(id) else {
                return Err(format!(
                    "{source}: no rule named '{id}'. Run `cqx score --explain` for the list."
                ));
            };
            apply(rule, patch);
            self.origins.insert(id.clone(), Origin::File);
        }
        self.min_score = file.min_score;
        self.exclude.clone_from(&file.exclude);
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        for (id, rule) in &self.rules {
            if rule.full <= rule.free {
                return Err(format!(
                    "rule '{id}': full ({}) must be greater than free ({})",
                    rule.full, rule.free
                ));
            }
        }
        Ok(())
    }

    /// Applies the layers in order and records where each rule ended up coming
    /// from: defaults, then a file, then the environment.
    pub fn resolve(explicit: Option<&Path>, root: &Path) -> Result<Config, String> {
        let mut config = Config {
            rules: defaults(),
            origins: defaults()
                .keys()
                .map(|id| (id.clone(), Origin::Default))
                .collect(),
            min_score: None,
            exclude: Vec::new(),
            loaded_from: None,
        };

        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => std::env::var_os("CQX_CONFIG")
                .map(PathBuf::from)
                .or_else(|| discover(root)),
        };
        if let Some(path) = path {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let file: ConfigFile = serde_json::from_str(&text)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            config.apply_file(&file, &path.display().to_string())?;
            config.loaded_from = Some(path);
        }

        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    /// Environment last, because CI sets it per run.
    ///
    /// In a browser there is no environment, so every lookup misses and this is
    /// a no-op — which is the correct behaviour rather than a special case.
    fn apply_env(&mut self) -> Result<(), String> {
        for (id, rule) in self.rules.iter_mut() {
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
            // CQX_RULE_OVERSIZED_FILES_PARAM_MAX_LINES=800
            let names: Vec<String> = rule.params.keys().cloned().collect();
            for name in names {
                let pk = name.to_uppercase().replace('-', "_");
                if let Some(raw) = std::env::var_os(format!("CQX_RULE_{key}_PARAM_{pk}")) {
                    let v: f64 = raw
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| format!("CQX_RULE_{key}_PARAM_{pk} is not a number"))?;
                    rule.params.insert(name, v);
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
                self.origins.insert(id.clone(), Origin::Env);
            }
        }
        if let Some(raw) = std::env::var_os("CQX_MIN_SCORE") {
            self.min_score = raw.to_string_lossy().parse().ok();
        }
        Ok(())
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
    for (name, value) in &patch.params {
        rule.params.insert(name.clone(), *value);
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
