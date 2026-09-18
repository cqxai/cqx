//! Scoring a repository over its history.
//!
//! A single score answers "how are we doing". A history answers "what did this
//! change", which is the question a reviewer actually has — and it needs no
//! reference population, because the baseline is the repository's own past.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};
use serde::{Deserialize, Serialize};

pub const HISTORY_COMMAND: CommandSpec = CommandSpec {
    name: "history",
    owner: "cqx-history",
    category: "index",
    summary: "Score a repository commit by commit",
    aliases: &[],
    subcommands: &[],
    handler: cmd_history,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(HISTORY_COMMAND);
    registry.add_param(ParamSpec {
        name: "--commits",
        description: "how many commits back to score (default 20)",
    });
    registry.add_param(ParamSpec {
        name: "--since",
        description: "score every commit after this one instead of a count",
    });
    registry.add_flag(FlagSpec {
        name: "--keep-facts",
        aliases: &[],
        description: "keep each commit's fact stream instead of only its scores",
    });
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitScore {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub lines: u64,
    pub scores: BTreeMap<String, u32>,
    /// Change against the previous commit in this walk, per category. Always
    /// present, even when empty: a consumer should not have to handle two
    /// shapes of the same record.
    #[serde(default)]
    pub delta: BTreeMap<String, i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    pub repo: String,
    /// The branch that was walked. Recorded rather than assumed: a reader that
    /// prints "commits to main" is wrong on every repository using master, and
    /// on every one scored from a feature branch.
    pub branch: String,
    /// The origin remote as a browsable https URL, if there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Where a reader can see the full history. Points at the repository's own
    /// history page rather than a specific branch: a branch URL goes stale the
    /// moment the branch is deleted, and the branch itself is recorded above for
    /// anyone who needs it. Only set for hosts whose path layout is known —
    /// guessing one produces a link that 404s, which is worse than no link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_url: Option<String>,
    pub generated: String,
    /// The configuration every commit here was scored with, so a reader shows
    /// the project's own standards rather than cqx's defaults — and can say
    /// which knobs were turned.
    ///
    /// One configuration across the whole walk on purpose: scoring each commit
    /// against the config as of that commit would make the line jump when
    /// somebody edits a threshold, which is the opposite of a trend.
    pub config: serde_json::Value,
    pub commits: Vec<CommitScore>,
}

/// Detached head has no branch name; say so rather than inventing one.
fn branch_name(repo: &Path) -> String {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .map(|b| if b == "HEAD" { "detached".to_string() } else { b })
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Normalises whatever form origin is configured in into a browsable URL.
///
/// `git@github.com:owner/repo.git`, `ssh://git@github.com/owner/repo.git` and
/// the https form all name the same page.
fn remote_url(repo: &Path) -> Option<String> {
    let raw = git(repo, &["remote", "get-url", "origin"]).ok()?;
    let raw = raw.trim().trim_end_matches('/');
    let raw = raw.strip_suffix(".git").unwrap_or(raw);
    let normalised = if let Some(rest) = raw.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        format!("https://{host}/{path}")
    } else if let Some(rest) = raw.strip_prefix("ssh://git@") {
        format!("https://{rest}")
    } else if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        return None;
    };
    Some(normalised)
}

/// The path to a repository's history, for hosts whose layout is known.
fn commits_url(remote: &str) -> Option<String> {
    if remote.contains("github.com") {
        Some(format!("{remote}/commits/"))
    } else if remote.contains("gitlab.com") {
        Some(format!("{remote}/-/commits/"))
    } else {
        None
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Materialises one commit into a scratch directory.
///
/// `git archive` rather than a checkout: the working tree is never touched, so
/// this is safe to run against a repository somebody is using.
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

fn cmd_history(context: &Context) {
    if let Err(e) = run(context) {
        eprintln!("cqx history: {e}");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn exit_code() -> i32 {
    i32::from(FAILED.load(std::sync::atomic::Ordering::Relaxed))
}

fn run(context: &Context) -> Result<(), String> {
    let repo = context
        .args
        .positionals
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| context.env.cwd.clone());
    let repo = repo
        .canonicalize()
        .map_err(|e| format!("{}: {e}", repo.display()))?;

    let out_dir = context
        .args
        .params
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join(".cqx-history"));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    // First-parent only: merge commits describe the branch, and walking into
    // them would score the same work several times.
    let range = match context.args.params.get("--since") {
        Some(since) => format!("{since}..HEAD"),
        None => String::new(),
    };
    let count = context
        .args
        .params
        .get("--commits")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20)
        .to_string();
    let mut args = vec!["rev-list", "--first-parent", "--reverse"];
    if range.is_empty() {
        args.push("-n");
        args.push(&count);
        args.push("HEAD");
    } else {
        args.push(&range);
    }
    let shas: Vec<String> = git(&repo, &args)?
        .lines()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect();
    if shas.is_empty() {
        return Err("no commits to score".into());
    }

    let keep = context
        .args
        .flags
        .get("--keep-facts")
        .copied()
        .unwrap_or(false);
    let config = cqx_score::config::Config::resolve(
        context.args.params.get("--config").map(Path::new),
        &repo,
    )?;

    let mut commits: Vec<CommitScore> = Vec::new();
    let mut previous: Option<BTreeMap<String, u32>> = None;
    let scratch = out_dir.join(".work");

    for (index, sha) in shas.iter().enumerate() {
        let cached = out_dir.join(format!("{}.json", &sha[..12]));
        // A commit's score can never change, so it is written once.
        let record: CommitScore = if cached.is_file() {
            serde_json::from_str(
                &std::fs::read_to_string(&cached).map_err(|e| format!("{e}"))?,
            )
            .map_err(|e| format!("{}: {e}", cached.display()))?
        } else {
            let _ = std::fs::remove_dir_all(&scratch);
            materialise(&repo, sha, &scratch)?;
            let facts_path = out_dir.join(format!("{}.ndjson", &sha[..12]));
            let file = std::fs::File::create(&facts_path).map_err(|e| format!("{e}"))?;
            let stats = cqx_rust::extract::run(&scratch, std::io::BufWriter::new(file))
                .map_err(|e| format!("{sha}: {e}"))?;
            let _ = stats;
            let stream = cqx_store::facts::Stream::load(&facts_path)
                .map_err(|e| format!("{}: {e}", facts_path.display()))?;
            let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
            let (scores, _) = cqx_score::score(&config, &metrics);
            if !keep {
                let _ = std::fs::remove_file(&facts_path);
            }
            let meta = git(
                &repo,
                &["show", "-s", "--format=%h%x1f%s%x1f%an%x1f%aI", sha],
            )?;
            let f: Vec<&str> = meta.trim().split('\u{1f}').collect();
            let record = CommitScore {
                sha: sha.clone(),
                short: f.first().unwrap_or(&"").to_string(),
                subject: f.get(1).unwrap_or(&"").to_string(),
                author: f.get(2).unwrap_or(&"").to_string(),
                date: f.get(3).unwrap_or(&"").to_string(),
                lines: metrics.lines,
                scores,
                delta: BTreeMap::new(),
            };
            std::fs::write(&cached, serde_json::to_string(&record).unwrap_or_default())
                .map_err(|e| format!("{e}"))?;
            record
        };

        let mut record = record;
        if let Some(prev) = &previous {
            for (cat, now) in &record.scores {
                let before = prev.get(cat).copied().unwrap_or(*now);
                let change = *now as i64 - before as i64;
                if change != 0 {
                    record.delta.insert(cat.clone(), change);
                }
            }
        }
        previous = Some(record.scores.clone());
        eprintln!(
            "  [{:>3}/{}] {} {:<52} {}",
            index + 1,
            shas.len(),
            record.short,
            record.subject.chars().take(52).collect::<String>(),
            record
                .scores
                .iter()
                .map(|(c, v)| format!("{}:{v}", &c[..3]))
                .collect::<Vec<_>>()
                .join(" ")
        );
        commits.push(record);
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let history = History {
        config: cqx_score::config_json(&config),
        repo: repo.display().to_string(),
        // Detached head has no branch name; say so rather than inventing one.
        remote: remote_url(&repo),
        commits_url: remote_url(&repo).and_then(|u| commits_url(&u)),
        branch: branch_name(&repo),
        generated: git(&repo, &["show", "-s", "--format=%aI", "HEAD"])?
            .trim()
            .to_string(),
        commits,
    };
    let index_path = out_dir.join("history.json");
    std::fs::write(
        &index_path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
    )
    .map_err(|e| format!("{e}"))?;
    eprintln!("\n{} commits → {}", history.commits.len(), index_path.display());
    Ok(())
}
