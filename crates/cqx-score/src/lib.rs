//! The CodeQuality Score.
//!
//! Every category starts at 100 and is degraded only by a named rule with a
//! published weight and a cap. There is no reference population of codebases the
//! way there is of websites, so a score is a list of findings rather than a
//! curve — which also means it can be explained, argued with, and configured.

pub mod config;
pub mod edit;
pub mod metrics;
pub mod ratchet;

use std::collections::BTreeMap;
use std::path::PathBuf;

use config::{Config, MinScore, Origin};
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
        name: "--strict",
        aliases: &[],
        description: "fail when a configured language floor has no scored product lines",
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
    registry.add_flag(FlagSpec {
        name: "--tighten-only",
        aliases: &[],
        description: "refuse a change that lowers the standard; what an agent is given",
    });
    registry.add_param(ParamSpec {
        name: "--why",
        description: "the reason for a rule change, kept beside it in cqx.json",
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

/// One language, scored using only its own rules and product lines.
pub struct LanguageScore {
    pub lines: u64,
    pub scores: BTreeMap<String, u32>,
}

/// Shared by native gates, history, and the browser report.
pub struct Scoring {
    pub scores: BTreeMap<String, u32>,
    pub deductions: Vec<Deduction>,
    pub languages: BTreeMap<String, LanguageScore>,
}

pub fn score(config: &Config, m: &Metrics) -> (BTreeMap<String, u32>, Vec<Deduction>) {
    let scored = evaluate(config, m);
    (scored.scores, scored.deductions)
}

pub fn evaluate(config: &Config, m: &Metrics) -> Scoring {
    let mut deductions = Vec::new();
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for (id, rule) in &config.rules {
        totals.entry(rule.category.clone()).or_insert(0.0);
        if !rule.enabled {
            continue;
        }
        let Some(measure) = m.get(id) else { continue };
        // Keep the zero-deduction report entries for compatibility, but a
        // language with no scored product lines cannot incur a product loss.
        let taken = if m.language_lines.get(&rule.language) == Some(&0) {
            0.0
        } else {
            deduction(measure.value, rule.free, rule.full, rule.weight)
        };
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
    let scores = category_scores(totals);
    // Round and clamp each language exactly as the single-language scorer does,
    // then weight those category scores by actual lines, never the density floor.
    let languages: BTreeMap<String, LanguageScore> = m
        .language_lines
        .iter()
        .filter(|(_, lines)| **lines > 0)
        .map(|(language, lines)| {
            let mut lost: BTreeMap<String, f64> = config
                .rules
                .values()
                .map(|rule| (rule.category.clone(), 0.0))
                .collect();
            for d in &deductions {
                if config.rules[&d.rule].language == *language {
                    *lost.entry(d.category.clone()).or_default() += d.taken;
                }
            }
            let scores = category_scores(lost);
            (
                language.clone(),
                LanguageScore {
                    lines: *lines,
                    scores,
                },
            )
        })
        .collect();
    // Zero deductions need no bucket. Every actual loss must enter the headline.
    for d in &deductions {
        assert!(
            d.taken == 0.0 || languages.contains_key(&config.rules[&d.rule].language),
            "deduction {} has no scored language bucket ({})",
            d.rule,
            config.rules[&d.rule].language
        );
    }
    let scores = if languages.len() == 1 {
        languages.values().next().unwrap().scores.clone()
    } else if languages.len() > 1 {
        let lines: u64 = languages.values().map(|part| part.lines).sum();
        config
            .rules
            .values()
            .map(|rule| rule.category.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|category| {
                let weighted: f64 = languages
                    .values()
                    .map(|part| f64::from(part.scores[&category]) * part.lines as f64)
                    .sum();
                (category, (weighted / lines as f64).round() as u32)
            })
            .collect()
    } else {
        scores
    };
    Scoring {
        scores,
        deductions,
        languages,
    }
}

fn category_scores(totals: BTreeMap<String, f64>) -> BTreeMap<String, u32> {
    totals
        .into_iter()
        .map(|(category, lost)| (category, (100.0 - lost).max(0.0).round() as u32))
        .collect()
}

fn cmd_score(context: &Context) {
    let facts = context.args.params.get("--facts").map(PathBuf::from);
    let Some(facts) = facts else {
        fail("--facts <file> is required");
        return;
    };
    let root = facts.parent().unwrap_or(&context.env.cwd).to_path_buf();
    let explicit = context.args.params.get("--config").map(PathBuf::from);

    let mut config = match Config::resolve(explicit.as_deref(), &root) {
        Ok(c) => c,
        Err(e) => {
            fail(e);
            return;
        }
    };
    if let Some(v) = context
        .args
        .params
        .get("--min-score")
        .and_then(|s| s.parse().ok())
    {
        config.min_score = Some(MinScore::Headline(v));
    }

    if context
        .args
        .flags
        .get("--explain")
        .copied()
        .unwrap_or(false)
    {
        explain(&config);
        return;
    }

    let stream = match cqx_store::facts::Stream::load(&facts) {
        Ok(s) => s,
        Err(e) => {
            fail(e);
            return;
        }
    };
    let m = Metrics::compute(&stream, &config);
    let scoring = evaluate(&config, &m);

    if context.args.flags.get("--json").copied().unwrap_or(false) {
        print_json(&config, &scoring, &m);
    } else {
        print_report(&config, &scoring.scores, &scoring.deductions, &m);
    }

    if let Some(min) = &config.min_score {
        let strict = context.args.flags.get("--strict").copied().unwrap_or(false);
        if !strict {
            for warning in min.warnings(&scoring) {
                eprintln!("cqx: warning: {warning}");
            }
        }
        for failure in min.failures(&scoring, strict) {
            eprintln!("\ncqx: {failure}");
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reports a problem, and makes the process say so.
///
/// Every error path in `score` goes through this. `score` is the command a
/// CI job gates on, and it used to print "no such file" to stderr and then
/// exit zero — so a mistyped `--facts` path did not fail a build, it passed
/// one without measuring anything. A gate that cannot fail is a decoration.
fn fail(message: impl std::fmt::Display) {
    eprintln!("cqx score: {message}");
    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

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
                    "language": r.language,
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
        context
            .args
            .params
            .get("--config")
            .map(std::path::Path::new),
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
        eprintln!("usage: cqx config set <rule>.<field> <value> [--why \"...\"]");
        eprintln!("  e.g. cqx config set oversized-files.max_lines 2000");
        eprintln!("       cqx config set oversized-files.enabled false");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    let Some((rule, field)) = target.rsplit_once('.') else {
        eprintln!("cqx config set: expected <rule>.<field>, got '{target}'");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    };

    let path = context
        .args
        .params
        .get("--config")
        .map(PathBuf::from)
        .unwrap_or_else(|| context.env.cwd.join("cqx.json"));

    // Worked out in full before anything is written — see `edit.rs`. The same
    // function answers for the desktop application and for an agent over MCP,
    // so a change cannot mean one thing here and another there.
    let proposal = match edit::propose(
        &path,
        &context.env.cwd,
        rule,
        field,
        value,
        context.args.params.get("--why").map(String::as_str),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cqx config set: {e}");
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    };

    // Which way it moves, said on every set and not only under the flag. A
    // person changing a rule deserves to be told which direction they went:
    // "looser" on screen is the difference between a decision and a drift,
    // and it costs a line.
    if context
        .args
        .flags
        .get("--tighten-only")
        .copied()
        .unwrap_or(false)
        && !proposal.tightens
    {
        eprintln!("cqx config set: refused — --tighten-only, and this does not tighten.");
        for objection in proposal.objections() {
            eprintln!(
                "  {}.{}  {} → {}  {}",
                objection.rule, objection.field, objection.before, objection.after, objection.why
            );
        }
        if proposal.changes.is_empty() {
            eprintln!("  nothing would change");
        }
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }

    if let Err(e) = proposal.write() {
        eprintln!("cqx config set: {e}");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    println!("{} · {rule}.{field} = {value}", path.display());
    for moved in &proposal.changes {
        println!(
            "  {} → {}  {}: {}",
            moved.before, moved.after, moved.direction, moved.why
        );
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
        "\nSet TypeScript rule patches in the repository root's cqx.json.\n\
         Legacy Rust rules also accept CQX_RULE_<RULE>_{{WEIGHT,FREE,FULL,ENABLED}},\n\
         for example CQX_RULE_EXIT_IN_LIBRARY_WEIGHT=10. A cqx.json need only\n\
         mention the rules it changes."
    );
}

/// Surface partial analysis in both human-readable scan and score output.
pub fn print_skipped_files(skipped: &[serde_json::Value]) {
    if !skipped.is_empty() {
        println!("\n{} skipped file(s):", skipped.len());
        for file in skipped {
            println!(
                "  {}: {}",
                file["file"].as_str().unwrap_or("?"),
                file["reason"].as_str().unwrap_or("?")
            );
        }
    }
}

fn print_report(
    config: &Config,
    scores: &BTreeMap<String, u32>,
    deductions: &[Deduction],
    m: &Metrics,
) {
    println!("CodeQuality Score · {} product lines", m.lines);
    print_skipped_files(&m.skipped_files);
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
    build_report(config, &evaluate(config, m), m)
}

fn build_report(config: &Config, scoring: &Scoring, m: &Metrics) -> serde_json::Value {
    let rules: Vec<serde_json::Value> = scoring
        .deductions
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
                            // The item it sits inside, so a reader can zoom
                            // out to the thing that is wrong rather than the
                            // line it happens to be on.
                            if let Some(item) = &f.item {
                                one["item"] = serde_json::json!({
                                    "name": item.name, "kind": item.kind,
                                    "from": item.from, "to": item.to,
                                });
                            }
                            one
                        })
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "rule": d.rule.rsplit('/').next().unwrap_or(&d.rule), "category": d.category,
                // The language prefixes the name wherever it is shown and
                // names its page in the docs, so it travels with the rule
                // rather than being assumed by whoever renders it.
                "language": config.rules.get(&d.rule).map(|r| r.language.clone()).unwrap_or_default(),
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
    let mut shown_config = config.clone();
    // A Rust-only report retains main's full JSON, including its rule config.
    for language in config.frontend_languages() {
        if m.get(&format!("{language}/duplicated-bodies")).is_none() {
            shown_config.rules.retain(|_, r| r.language != language);
        }
    }
    let mut report = serde_json::json!({
        "lines": m.lines,
        "scores": scoring.scores,
        "rules": rules,
        "config": config_json(&shown_config),
    });
    // Keep every single-language golden byte-identical. A mixed report adds
    // only this block; top-level rule rows remain the original unweighted data.
    if scoring.languages.len() > 1 {
        let languages: serde_json::Map<String, serde_json::Value> = scoring
            .languages
            .iter()
            .map(|(language, part)| {
                let rules: Vec<_> = report["rules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|rule| rule["language"].as_str() == Some(language.as_str()))
                    .cloned()
                    .collect();
                (
                    language.clone(),
                    serde_json::json!({"lines": part.lines, "scores": part.scores, "rules": rules}),
                )
            })
            .collect();
        report["languages"] = serde_json::Value::Object(languages);
    }
    if !m.skipped_files.is_empty() {
        report["skipped_files"] = serde_json::json!(m.skipped_files);
    }
    report
}

fn print_json(config: &Config, scoring: &Scoring, m: &Metrics) {
    println!(
        "{}",
        serde_json::to_string_pretty(&build_report(config, scoring, m)).unwrap_or_default()
    );
}

#[cfg(test)]
mod scoring_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "has no scored language bucket (synthetic)")]
    fn deduction_cannot_disappear_from_weighted_headline() {
        let mut config = Config::from_text(None).unwrap();
        // A synthetic rule deliberately assigns an existing measured loss to
        // a language absent from the line inventory.
        let rule = config.rules.get_mut("oversized-line-share").unwrap();
        rule.language = "synthetic".into();
        rule.free = -1.0;
        let mut metrics = Metrics::compute(&cqx_store::facts::Stream::default(), &config);
        metrics.language_lines.insert("rust".into(), 1);
        evaluate(&config, &metrics);
    }
}
