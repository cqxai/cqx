//! One command, for the place most people will meet this tool: a CI job.
//!
//! `extract` writes facts and `score` reads them, which is the right seam and
//! the wrong first impression. A person who has just installed cqx, and a
//! GitHub Action that has thirty seconds, both want one command that reads a
//! tree and says whether it may merge. That is this.
//!
//! Nothing here is new analysis. It is the same extractor, the same scorer and
//! the same report — held in memory rather than written to a file between the
//! two, because the only reason the file existed was that two commands cannot
//! pass a value to each other.
//!
//! ## Why it can fail two different ways
//!
//! A floor (`--min-score`) is what a team sets once and mostly passes. A
//! ratchet (`--against`) is what actually moves a codebase: score the branch,
//! score what it will merge into, and refuse anything that makes a category
//! worse. The second is the one worth putting in a pull request, so it is the
//! one this makes easy — but both are here, and a run may use either or both.

mod sarif;
mod summary;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};

pub const SCAN_COMMAND: CommandSpec = CommandSpec {
    name: "scan",
    owner: "cqx-scan",
    category: "index",
    summary: "Read a tree, score it, and say whether it may merge",
    aliases: &[],
    subcommands: &[],
    handler: cmd_scan,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(SCAN_COMMAND);
    registry.add_param(ParamSpec {
        name: "--sarif",
        description: "write findings as SARIF, which GitHub shows inline on a pull request",
    });
    registry.add_param(ParamSpec {
        name: "--summary",
        description: "write a Markdown summary (point it at $GITHUB_STEP_SUMMARY)",
    });
    registry.add_param(ParamSpec {
        name: "--report",
        description: "write the whole report as JSON to a file, as well as reading it here",
    });
    registry.add_param(ParamSpec {
        name: "--against",
        description: "also score this git ref, and fail if any category is worse than it",
    });
    registry.add_flag(FlagSpec {
        name: "--no-quote",
        aliases: &[],
        description: "leave out the line of code beside each finding",
    });
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What `cqx scan` should exit with. Read by the binary; a library that calls
/// `process::exit` is a finding this tool reports.
pub fn exit_code() -> i32 {
    i32::from(FAILED.load(std::sync::atomic::Ordering::Relaxed))
}

fn fail(message: impl std::fmt::Display) {
    eprintln!("cqx scan: {message}");
    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn cmd_scan(context: &Context) {
    if let Err(e) = run(context) {
        fail(e);
    }
}

/// How many findings a terminal shows before it stops being a terminal. The
/// rest are in the SARIF, on the lines they belong to.
const SHOWN: usize = 8;

/// What one pass over a tree produced.
///
/// Public because `cqx scan` is not the only thing that wants it. The desktop
/// application scores the tree a person is editing, and an MCP server answers
/// an agent asking about that same tree — both want the pipeline and none of
/// the terminal around it. A command is a way to reach a library, not the
/// place the work should live.
pub struct Scanned {
    pub report: serde_json::Value,
    pub files: usize,
    pub lines: u64,
    pub ms: u128,
    /// Every path the scan actually read. SARIF has a place for this, and
    /// GitHub's code scanning page says "no summary of scanned files
    /// reported by cqx" when it is left out — a reader cannot tell a tool
    /// that found nothing from one that looked at nothing.
    pub paths: Vec<String>,
}

impl Scanned {
    /// The category scores, in the order they are printed.
    pub fn scores(&self) -> Vec<(String, u64)> {
        self.report
            .get("scores")
            .and_then(serde_json::Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn run(context: &Context) -> Result<(), String> {
    let root = context
        .args
        .positionals
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| context.env.cwd.clone());
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;

    let config_path = context.args.params.get("--config").map(Path::new);
    let quote = !context
        .args
        .flags
        .get("--no-quote")
        .copied()
        .unwrap_or(false);

    let mut here = tree(&root, config_path, quote)?;

    // `--min-score` is a `cqx.json` field first and an override second, so it
    // is applied to the report rather than carried beside it: everything that
    // reads the floor — the gate, the summary, whatever reads the JSON — then
    // sees the same number.
    if let Some(min) = context
        .args
        .params
        .get("--min-score")
        .and_then(|s| s.parse::<u64>().ok())
    {
        if let Some(config) = here.report.get_mut("config") {
            config["min_score"] = serde_json::json!(min);
        }
    }

    // The ref this branch would merge into, scored the same way. Materialised
    // into a scratch directory rather than checked out: a CI job's working
    // tree is the thing being measured, and moving HEAD under it is how a
    // tool corrupts the run it was asked to judge.
    let against = match context.args.params.get("--against") {
        Some(reference) => Some((reference.clone(), scan_ref(&root, reference, config_path)?)),
        None => None,
    };

    // What it was compared against travels with the report. The movement is
    // the point of a ratchet, and a consumer that only receives the new
    // numbers has to score the base itself to find it — which is the work
    // this command already did.
    if let Some((reference, before)) = &against {
        let was: serde_json::Map<String, serde_json::Value> = before
            .scores()
            .into_iter()
            .map(|(k, v)| (k, serde_json::json!(v)))
            .collect();
        here.report["against"] = serde_json::json!({
            "ref": reference,
            "scores": was,
            "lines": before.lines,
        });
    }

    // Written before the gate runs, and whatever it decides: a run that was
    // refused is exactly the one somebody wants the numbers from.
    if let Some(path) = context.args.params.get("--report") {
        write(Path::new(path), &format!("{:#}\n", here.report))?;
        eprintln!("cqx scan: report \u{2192} {path}");
    }

    if let Some(path) = context.args.params.get("--sarif") {
        let doc = sarif::build(&here.report, &root, &here.paths);
        write(Path::new(path), &format!("{doc:#}\n"))?;
        eprintln!("cqx scan: SARIF → {path}");
    }

    if let Some(path) = context.args.params.get("--summary") {
        let text = summary::build(&here, against.as_ref().map(|(r, s)| (r.as_str(), s)));
        append(Path::new(path), &text)?;
        eprintln!("cqx scan: summary → {path}");
    }

    if context.args.flags.get("--json").copied().unwrap_or(false) {
        println!("{:#}", here.report);
    } else if !context.args.flags.get("--quiet").copied().unwrap_or(false) {
        print_report(&here, against.as_ref().map(|(r, s)| (r.as_str(), s)));
    }

    gate(context, &here, against.as_ref().map(|(r, s)| (r.as_str(), s)));
    Ok(())
}

/// The whole pipeline, in memory.
///
/// The facts never reach a file. `extract` writes them and `score` reads them,
/// and the only reason that was a file is that two commands cannot hand each
/// other a value.
pub fn tree(root: &Path, config_path: Option<&Path>, quote: bool) -> Result<Scanned, String> {
    let started = std::time::Instant::now();
    let snapshot = cqx_vfs::from_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let files = snapshot.len();
    let paths: Vec<String> = snapshot.paths().map(str::to_string).collect();

    let mut facts: Vec<u8> = Vec::new();
    cqx_rust::extract::run(&snapshot, &mut facts).map_err(|e| format!("{e}"))?;
    let text = String::from_utf8(facts).map_err(|e| format!("the extractor emitted: {e}"))?;
    let stream = cqx_store::facts::Stream::from_ndjson(&text);

    let config = cqx_score::config::Config::resolve(config_path, root)?;
    let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
    let mut report = cqx_score::report_json(&config, &metrics);

    // The line of code beside each finding. The scorer works from a graph and
    // has no repository to read; here the tree is still on disk, so this is
    // where it can be answered.
    if quote {
        let base = root.to_path_buf();
        cqx_view::quote(&mut report, &|path: &str| {
            std::fs::read_to_string(base.join(path)).ok()
        });
    }

    let ms = started.elapsed().as_millis();

    // What the run itself cost, stamped here rather than by the caller. The
    // human output has always said "58 files · 7,802 lines · 0.1s" and for a
    // while the report carried only the lines, so anything drawing from the
    // JSON could not say the same sentence. Every caller wants it, so no
    // caller has to remember it.
    report["scan"] = serde_json::json!({
        "files": files,
        "lines": metrics.lines,
        "ms": ms,
        "cqx": env!("CARGO_PKG_VERSION"),
    });

    Ok(Scanned {
        lines: metrics.lines,
        files,
        paths,
        report,
        ms,
    })
}

/// Scores a git ref without disturbing the tree being measured.
fn scan_ref(repo: &Path, reference: &str, config_path: Option<&Path>) -> Result<Scanned, String> {
    let sha = git(repo, &["rev-parse", reference])?.trim().to_string();
    if sha.is_empty() {
        return Err(format!("{reference} does not name a commit"));
    }
    let scratch = repo.join(".cqx-scan");
    let _ = std::fs::remove_dir_all(&scratch);
    materialise(repo, &sha, &scratch)?;
    // The comparison is of the code, not of the configuration: both sides are
    // scored against the standards in force now, so a rule added today does
    // not read as a regression introduced yesterday.
    let scanned = tree(&scratch, config_path, false);
    let _ = std::fs::remove_dir_all(&scratch);
    scanned
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn materialise(repo: &Path, sha: &str, into: &Path) -> Result<(), String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let archive = Command::new("git")
        .args(["archive", "--format=tar", sha])
        .current_dir(repo)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git archive: {e}"))?;
    let status = Command::new("tar")
        .args(["-x", "-C"])
        .arg(into)
        .stdin(archive.stdout.ok_or("git archive produced no output")?)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err(format!("extracting {sha} failed"));
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Appends, because `$GITHUB_STEP_SUMMARY` is a file several steps write to,
/// and a step that truncates it deletes what the steps before it said.
fn append(path: &Path, text: &str) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether this may merge.
fn gate(context: &Context, here: &Scanned, against: Option<(&str, &Scanned)>) {
    let mut refused = false;

    if let Some((reference, before)) = against {
        let was: std::collections::BTreeMap<String, u64> = before.scores().into_iter().collect();
        for (category, now) in here.scores() {
            let Some(then) = was.get(&category) else { continue };
            if now < *then {
                eprintln!(
                    "cqx scan: {category} is {now}, down from {then} on {reference}"
                );
                refused = true;
            }
        }
    }

    // The floor is read from the configuration rather than the flags, because
    // `--min-score` is a `cqx.json` field first and an override second.
    if let Some(min) = here
        .report
        .get("config")
        .and_then(|c| c.get("min_score"))
        .and_then(serde_json::Value::as_u64)
    {
        for (category, value) in here.scores() {
            if value < min {
                eprintln!("cqx scan: {category} scored {value}, below the required {min}");
                refused = true;
            }
        }
    }

    if refused {
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
    } else if !context.args.flags.get("--quiet").copied().unwrap_or(false)
        && !context.args.flags.get("--json").copied().unwrap_or(false)
    {
        println!("\n  passed");
    }
}

/// What a person reads.
///
/// The shape rustc uses, because a Rust developer already knows how to read
/// it: where, the code, the part that is wrong, and what is wrong with it.
fn print_report(here: &Scanned, against: Option<(&str, &Scanned)>) {
    println!(
        "cqx {} · {} files · {} lines · {:.1}s",
        env!("CARGO_PKG_VERSION"),
        here.files,
        here.lines,
        here.ms as f64 / 1000.0
    );

    let empty = Vec::new();
    let rules = here
        .report
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    // Worst first: the deduction is the reason anybody is reading this.
    let mut costly: Vec<&serde_json::Value> = rules
        .iter()
        .filter(|r| r.get("deducted").and_then(serde_json::Value::as_f64).unwrap_or(0.0) > 0.0)
        .collect();
    costly.sort_by(|a, b| {
        let f = |v: &serde_json::Value| v.get("deducted").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        f(b).total_cmp(&f(a))
    });

    let mut shown = 0usize;
    for rule in &costly {
        let id = format!(
            "{}/{}",
            rule.get("language").and_then(serde_json::Value::as_str).unwrap_or("rust"),
            rule.get("rule").and_then(serde_json::Value::as_str).unwrap_or("?"),
        );
        let findings = rule
            .get("findings")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for finding in &findings {
            if shown >= SHOWN {
                continue;
            }
            shown += 1;
            print_finding(finding, &id);
        }
    }
    let total: usize = costly
        .iter()
        .map(|r| {
            r.get("total_findings")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize
        })
        .sum();
    if total > shown {
        println!("\n  … {} more", total - shown);
    }

    println!();
    let was: std::collections::BTreeMap<String, u64> = against
        .map(|(_, s)| s.scores().into_iter().collect())
        .unwrap_or_default();
    for (category, value) in here.scores() {
        let movement = match was.get(&category) {
            Some(then) if *then == value => "      ".to_string(),
            Some(then) if value > *then => format!("   +{}", value - then),
            Some(then) => format!("   −{}", then - value),
            None => String::new(),
        };
        println!("  {category:<14} {value:>3}{movement}");
    }
}

fn print_finding(finding: &serde_json::Value, id: &str) {
    let get = |k: &str| finding.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    let file = get("file");
    let line = finding.get("line").and_then(serde_json::Value::as_u64).unwrap_or(0);
    println!();
    if line > 0 {
        println!("  {file}:{line}");
    } else {
        println!("  {file}");
    }

    let text = get("text");
    if !text.is_empty() {
        // Shown without the indentation it happens to sit at: a line forty
        // columns deep would push its own code off the side of a terminal,
        // and the span moves with it so the marking still lands.
        let lead = text.len() - text.trim_start().len();
        let code = &text[lead..];
        println!("      {code}");
        if let Some(col) = finding.get("col").and_then(serde_json::Value::as_array) {
            let from = col.first().and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
            let to = col.get(1).and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
            if to > from {
                let from = from.saturating_sub(lead);
                let to = to.saturating_sub(lead);
                println!("      {}{}", " ".repeat(from), "^".repeat(to - from));
            }
        }
    }
    println!("      {}  [{id}]", get("what"));
}
