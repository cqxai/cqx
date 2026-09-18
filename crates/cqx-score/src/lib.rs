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
use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry, SubcommandSpec};
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

pub const CONFIG_COMMAND: CommandSpec = CommandSpec {
    name: "config",
    owner: "cqx-score",
    category: "index",
    summary: "Read and change the rule configuration",
    aliases: &[],
    subcommands: &[
        SubcommandSpec {
            name: "show",
            summary: "print the effective configuration",
            aliases: &[],
            handler: cmd_config_show,
        },
        SubcommandSpec {
            name: "set",
            summary: "change one field, e.g. cqx config set oversized-files.max_lines 2000",
            aliases: &[],
            handler: cmd_config_set,
        },
    ],
    handler: cmd_config_show,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(SCORE_COMMAND);
    registry.add_command(CONFIG_COMMAND);
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

/// Enough to show the shape of a problem without turning the output into the
/// problem.
const FINDINGS_PER_RULE: usize = 25;

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
    let m = Metrics::compute(&stream, &config);
    let (scores, deductions) = score(&config, &m);

    if context.args.flags.get("--json").copied().unwrap_or(false) {
        print_json(&config, &scores, &deductions, &m);
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

/// The whole effective configuration as data.
///
/// An agent asked to "stop cqx complaining about file length" should be able to
/// read this, see `oversized-files` with its description and `max_lines`, and
/// change one field — rather than infer the shape from a printed table.
pub fn config_json(config: &Config) -> serde_json::Value {
    let rules: serde_json::Map<String, serde_json::Value> = config
        .rules
        .iter()
        .map(|(id, r)| {
            (
                id.clone(),
                serde_json::json!({
                    "category": r.category,
                    "describes": r.describes,
                    "weight": r.weight,
                    "free": r.free,
                    "full": r.full,
                    "enabled": r.enabled,
                    "params": r.params,
                    "remedy": r.remedy,
                    "source": config.origins.get(id).map(|o| o.to_string()).unwrap_or_default(),
                }),
            )
        })
        .collect();
    serde_json::json!({
        "version": 1,
        "config_path": config.loaded_from.as_ref().map(|p| p.display().to_string()),
        "min_score": config.min_score,
        "exclude": config.exclude,
        "rules": rules,
    })
}

fn cmd_config_show(context: &Context) {
    let root = context.env.cwd.clone();
    let config = match Config::resolve(
        context.args.params.get("--config").map(std::path::Path::new),
        &root,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cqx config: {e}");
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    };
    if context.args.flags.get("--json").copied().unwrap_or(false) {
        println!(
            "{}",
            serde_json::to_string_pretty(&config_json(&config)).unwrap_or_default()
        );
    } else {
        explain(&config);
    }
}

/// `cqx config set <rule>.<field> <value>` — one field at a time, rewriting the
/// file rather than regenerating it, so nothing else in it is disturbed.
fn cmd_config_set(context: &Context) {
    let mut args = context.args.positionals.iter();
    let (Some(target), Some(value)) = (args.next(), args.next()) else {
        eprintln!("usage: cqx config set <rule>.<field> <value>");
        eprintln!("  e.g. cqx config set oversized-files.max_lines 2000");
        eprintln!("       cqx config set oversized-files.enabled false");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    let Some((rule_id, field)) = target.rsplit_once('.') else {
        eprintln!("cqx config set: expected <rule>.<field>, got '{target}'");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    let defaults = config::defaults();
    let Some(default_rule) = defaults.get(rule_id) else {
        eprintln!("cqx config set: no rule named '{rule_id}'. `cqx config show` lists them.");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };

    let path = context
        .args
        .params
        .get("--config")
        .map(PathBuf::from)
        .unwrap_or_else(|| context.env.cwd.join("cqx.json"));
    let mut doc: serde_json::Value = if path.is_file() {
        match std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            Some(v) => v,
            None => {
                eprintln!("cqx config set: {} is not valid JSON", path.display());
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    } else {
        serde_json::json!({ "version": 1, "rules": {} })
    };

    let known_field = matches!(field, "weight" | "free" | "full" | "enabled")
        || default_rule.params.contains_key(field);
    if !known_field {
        eprintln!(
            "cqx config set: rule '{rule_id}' has no field '{field}'. It accepts weight, free, full, enabled{}.",
            if default_rule.params.is_empty() {
                String::new()
            } else {
                format!(
                    ", and {}",
                    default_rule.params.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            }
        );
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }

    let parsed: serde_json::Value = match field {
        "enabled" => match value.as_str() {
            "true" | "1" | "yes" => serde_json::Value::Bool(true),
            "false" | "0" | "no" => serde_json::Value::Bool(false),
            other => {
                eprintln!("cqx config set: enabled expects true or false, got '{other}'");
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        },
        _ => match value.parse::<f64>() {
            // A whole number is written as one: this file is read by people as
            // well as parsers, and `max_lines: 2500.0` reads like a mistake.
            Ok(v) if v.fract() == 0.0 && v.abs() < 9e15 => serde_json::json!(v as i64),
            Ok(v) => serde_json::json!(v),
            Err(_) => {
                eprintln!("cqx config set: {field} expects a number, got '{value}'");
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        },
    };

    let rules = doc
        .as_object_mut()
        .and_then(|o| {
            o.entry("rules")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .expect("rules is an object");
    let entry = rules
        .entry(rule_id.to_string())
        .or_insert_with(|| serde_json::json!({}));
    let is_param = default_rule.params.contains_key(field);
    if is_param {
        entry
            .as_object_mut()
            .expect("rule entry is an object")
            .entry("params")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("params is an object")
            .insert(field.to_string(), parsed);
    } else {
        entry
            .as_object_mut()
            .expect("rule entry is an object")
            .insert(field.to_string(), parsed);
    }

    let mut text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    text.push('\n');
    match std::fs::write(&path, text) {
        Ok(()) => println!("{} · {rule_id}.{field} = {value}", path.display()),
        Err(e) => {
            eprintln!("cqx config set: {}: {e}", path.display());
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn explain(config: &Config) {
    match &config.loaded_from {
        Some(p) => println!("configuration: {}", p.display()),
        None => println!("configuration: built-in defaults (no cqx.json found)"),
    }
    println!(
        "\n{:<26}{:<13}{:>8}{:>8}{:>8}   from",
        "rule", "category", "weight", "free", "full"
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
        if !rule.describes.is_empty() {
            println!("{:<26}{}", "", rule.describes);
        }
        if !rule.remedy.is_empty() {
            println!("{:<26}→ {}", "", rule.remedy);
        }
        for (name, value) in &rule.params {
            println!("{:<26}  {name} = {value}", "");
        }
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

/// The whole result as data: scores, every rule with its deduction and
/// findings, and the configuration it was all measured against.
///
/// Separate from printing it, because a browser wants the value and a terminal
/// wants the text, and neither should have to go through the other.
pub fn report_json(config: &Config, m: &Metrics) -> serde_json::Value {
    let (scores, deductions) = score(config, m);
    build_report(config, &scores, &deductions, m)
}

fn build_report(
    config: &Config,
    scores: &BTreeMap<String, u32>,
    deductions: &[Deduction],
    m: &Metrics,
) -> serde_json::Value {
    let rules: Vec<serde_json::Value> = deductions
        .iter()
        .map(|d| {
            // The findings travel with the number. A score a reader cannot open
            // is a score they have to take on trust, and this one is meant to be
            // argued with.
            let measure = m.get(&d.rule);
            let findings: Vec<serde_json::Value> = measure
                .map(|measure| {
                    measure
                        .findings
                        .iter()
                        .take(FINDINGS_PER_RULE)
                        .map(|f| {
                            // The span and the line itself travel with it, so
                            // a reader is shown the code rather than sent to
                            // find it. Both are omitted when absent rather
                            // than sent empty: a column of zero would be a
                            // claim about where the problem is.
                            let mut one = serde_json::json!({
                                "what": f.what, "file": f.file, "line": f.line,
                            });
                            if f.col != [0, 0] {
                                one["col"] = serde_json::json!(f.col);
                            }
                            if !f.text.is_empty() {
                                one["text"] = serde_json::json!(f.text);
                            }
                            one
                        })
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "rule": d.rule, "category": d.category,
                "describes": config.rules.get(&d.rule).map(|r| r.describes.clone()).unwrap_or_default(),
                "remedy": config.rules.get(&d.rule).map(|r| r.remedy.clone()).unwrap_or_default(),
                "value": (d.value * 1000.0).round() / 1000.0,
                "weight": d.weight, "deducted": d.taken, "capped": d.capped,
                "total_findings": measure.map(|x| x.findings.len()).unwrap_or(0),
                "findings": findings,
            })
        })
        .collect();
    // The configuration travels with the result: a consumer that renders this
    // should show the standards it was actually scored against.
    serde_json::json!({
        "lines": m.lines,
        "scores": scores,
        "rules": rules,
        "config": config_json(config),
    })
}

fn print_json(
    config: &Config,
    scores: &BTreeMap<String, u32>,
    deductions: &[Deduction],
    m: &Metrics,
) {
    println!(
        "{}",
        serde_json::to_string_pretty(&build_report(config, scores, deductions, m))
            .unwrap_or_default()
    );
}
