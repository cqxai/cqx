//! The CodeQuality Score.
//!
//! Every category starts at 100 and is degraded only by a named rule with a
//! published weight and a cap. There is no reference population of codebases the
//! way there is of websites, so a score is a list of findings rather than a
//! curve — which also means it can be explained, argued with, and configured.

pub mod config;
pub mod metrics;

use std::collections::BTreeMap;
use std::path::PathBuf;

use config::{Config, Origin};
use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};
use metrics::Metrics;

pub const SCORE_COMMAND: CommandSpec = CommandSpec {
    name: "score",
    owner: "cqx-score",
    category: "index",
    summary: "Score a fact stream and explain every deduction",
    aliases: &[],
    subcommands: &[],
    handler: cmd_score,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(SCORE_COMMAND);
    registry.add_param(ParamSpec {
        name: "--config",
        description: "rule configuration (JSON); otherwise cqx.json is searched for upward",
    });
    registry.add_param(ParamSpec {
        name: "--min-score",
        description: "exit non-zero when any category falls below this",
    });
    registry.add_flag(FlagSpec {
        name: "--explain",
        aliases: &[],
        description: "show every rule, its thresholds, and where each came from",
    });
    registry.add_flag(FlagSpec {
        name: "--json",
        aliases: &[],
        description: "emit the result as JSON",
    });
}

pub struct Deduction {
    pub rule: String,
    pub category: String,
    pub value: f64,
    pub weight: f64,
    pub taken: f64,
    pub capped: bool,
}

/// A ramp rather than a step: nothing below `free`, the whole weight at `full`,
/// proportional between. Linear on purpose — a curve nobody can do in their head
/// is a curve nobody trusts.
fn deduction(value: f64, free: f64, full: f64, weight: f64) -> f64 {
    if value <= free {
        return 0.0;
    }
    let fraction = ((value - free) / (full - free)).min(1.0);
    (weight * fraction * 10.0).round() / 10.0
}

pub fn score(config: &Config, m: &Metrics) -> (BTreeMap<String, u32>, Vec<Deduction>) {
    let mut deductions = Vec::new();
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for (id, rule) in &config.rules {
        totals.entry(rule.category.clone()).or_insert(0.0);
        if !rule.enabled {
            continue;
        }
        let Some(measure) = m.get(id) else { continue };
        let taken = deduction(measure.value, rule.free, rule.full, rule.weight);
        *totals.entry(rule.category.clone()).or_default() += taken;
        deductions.push(Deduction {
            rule: id.clone(),
            category: rule.category.clone(),
            value: measure.value,
            weight: rule.weight,
            taken,
            capped: taken >= rule.weight && rule.weight > 0.0,
        });
    }
    let scores = totals
        .into_iter()
        .map(|(cat, lost)| (cat, (100.0 - lost).max(0.0).round() as u32))
        .collect();
    (scores, deductions)
}

fn cmd_score(context: &Context) {
    let facts = context.args.params.get("--facts").map(PathBuf::from);
    let Some(facts) = facts else {
        eprintln!("cqx score: --facts <file> is required");
        return;
    };
    let root = facts.parent().unwrap_or(&context.env.cwd).to_path_buf();
    let explicit = context.args.params.get("--config").map(PathBuf::from);

    let mut config = match Config::resolve(explicit.as_deref(), &root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cqx score: {e}");
            return;
        }
    };
    if let Some(v) = context.args.params.get("--min-score").and_then(|s| s.parse().ok()) {
        config.min_score = Some(v);
    }

    if context.args.flags.get("--explain").copied().unwrap_or(false) {
        explain(&config);
        return;
    }

    let stream = match cqx_store::facts::Stream::load(&facts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cqx score: {e}");
            return;
        }
    };
    let m = Metrics::compute(&stream, &config.exclude);
    let (scores, deductions) = score(&config, &m);

    if context.args.flags.get("--json").copied().unwrap_or(false) {
        print_json(&scores, &deductions, &m);
    } else {
        print_report(&config, &scores, &deductions, &m);
    }

    if let Some(min) = config.min_score {
        if let Some((cat, worst)) = scores.iter().min_by_key(|(_, v)| **v) {
            if *worst < min {
                eprintln!("\ncqx: {cat} scored {worst}, below the required {min}");
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The binary reads this and exits with it; a library that calls `process::exit`
/// is a finding this tool reports.
pub fn exit_code() -> i32 {
    i32::from(FAILED.load(std::sync::atomic::Ordering::Relaxed))
}

fn explain(config: &Config) {
    match &config.loaded_from {
        Some(p) => println!("configuration: {}", p.display()),
        None => println!("configuration: built-in defaults (no cqx.json found)"),
    }
    println!(
        "\n{:<26}{:<13}{:>8}{:>8}{:>8}   {}",
        "rule", "category", "weight", "free", "full", "from"
    );
    for (id, rule) in &config.rules {
        let origin = config.origins.get(id).copied().unwrap_or(Origin::Default);
        println!(
            "{:<26}{:<13}{:>8.0}{:>8.2}{:>8.2}   {}{}",
            id,
            rule.category,
            rule.weight,
            rule.free,
            rule.full,
            origin,
            if rule.enabled { "" } else { "  (disabled)" }
        );
    }
    println!(
        "\nOverride any of them with CQX_RULE_<RULE>_{{WEIGHT,FREE,FULL,ENABLED}},\n\
         for example CQX_RULE_EXIT_IN_LIBRARY_WEIGHT=10. A cqx.json may set the\n\
         same fields, and need only mention the rules it changes."
    );
}

fn print_report(
    config: &Config,
    scores: &BTreeMap<String, u32>,
    deductions: &[Deduction],
    m: &Metrics,
) {
    println!("CodeQuality Score · {} product lines", m.lines);
    if let Some(p) = &config.loaded_from {
        println!("configuration: {}", p.display());
    }
    for (category, value) in scores {
        println!("\n  {category:<14} {value:>3}");
        for d in deductions.iter().filter(|d| &d.category == category) {
            if d.taken == 0.0 {
                println!("      {:<26} {:>8.2}      —", d.rule, d.value);
            } else {
                println!(
                    "      {:<26} {:>8.2}   −{:.1}{}",
                    d.rule,
                    d.value,
                    d.taken,
                    if d.capped { " (capped)" } else { "" }
                );
            }
        }
    }
    // The findings behind the largest deduction, because a score nobody can
    // drill into is a score nobody acts on.
    if let Some(worst) = deductions.iter().max_by(|a, b| a.taken.total_cmp(&b.taken)) {
        if worst.taken > 0.0 {
            if let Some(measure) = m.get(&worst.rule) {
                println!("\n  worst: {} — first findings", worst.rule);
                for f in measure.findings.iter().take(6) {
                    println!("      {}:{}  {}", f.file, f.line, f.what);
                }
                if measure.findings.len() > 6 {
                    println!("      … {} more", measure.findings.len() - 6);
                }
            }
        }
    }
}

fn print_json(scores: &BTreeMap<String, u32>, deductions: &[Deduction], m: &Metrics) {
    let rules: Vec<serde_json::Value> = deductions
        .iter()
        .map(|d| {
            serde_json::json!({
                "rule": d.rule, "category": d.category,
                "value": (d.value * 1000.0).round() / 1000.0,
                "weight": d.weight, "deducted": d.taken, "capped": d.capped,
            })
        })
        .collect();
    let out = serde_json::json!({
        "lines": m.lines,
        "scores": scores,
        "rules": rules,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
