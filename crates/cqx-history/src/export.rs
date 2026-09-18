//! Writing a repository's recent commits out as static files.
//!
//! An explorer needs a dataset per commit. Building one takes seconds; serving
//! one takes milliseconds. So the work is done once, wherever there is a
//! checkout — a CI run, a laptop — and the result is a directory of plain JSON
//! that any static host will serve.
//!
//! Two properties make the directory worth keeping rather than rebuilding:
//!
//! * A dataset is named by its commit and describes only that commit, so it can
//!   never go stale and never needs rewriting. The timeline lives in the index
//!   beside it, which is the one file that changes.
//! * A commit already present is not analysed again. Pointing successive runs
//!   at the same directory — or syncing it from a bucket first — turns "analyse
//!   the last five commits" into "analyse the one that is new".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use deka_cli_core::{CommandSpec, Context, ParamSpec, Registry};

use crate::{branch_name, commits_url, git, materialise, remote_url, CommitScore};

pub const EXPORT_COMMAND: CommandSpec = CommandSpec {
    name: "export",
    owner: "cqx-history",
    category: "index",
    summary: "Write recent commits out as datasets an explorer can serve",
    aliases: &[],
    subcommands: &[],
    handler: cmd_export,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(EXPORT_COMMAND);
    registry.add_param(ParamSpec {
        name: "--out",
        description: "directory to write the dataset tree into",
    });
    registry.add_param(ParamSpec {
        name: "--name",
        description: "publish as this org/repo instead of the one the remote names",
    });
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn exit_code() -> i32 {
    i32::from(FAILED.load(std::sync::atomic::Ordering::Relaxed))
}

fn cmd_export(context: &Context) {
    if let Err(e) = run(context) {
        eprintln!("cqx export: {e}");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `https://github.com/dekaruntime/deka` → `dekaruntime/deka`.
///
/// The published path, not a display name: it is what a URL on the explorer
/// reads as, so it has to come from the remote rather than from the directory
/// the checkout happens to sit in.
fn slug(remote: &str) -> Option<String> {
    let rest = remote.split_once("://")?.1;
    let mut parts = rest.split('/').skip(1);
    let org = parts.next()?;
    let repo = parts.next()?;
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{org}/{repo}"))
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

    let remote = remote_url(&repo);
    let name = match context.args.params.get("--name") {
        Some(given) => given.trim_matches('/').to_string(),
        None => remote.as_deref().and_then(slug).ok_or(
            "this checkout has no github-shaped origin, so there is no published name for it. \
             Pass --name <org>/<repo>.",
        )?,
    };

    let out = context
        .args
        .params
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join(".cqx-export"));
    let dir = out.join(&name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let count = context
        .args
        .params
        .get("--commits")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5)
        .to_string();
    // First-parent and oldest-first: the same walk `history` does, so a repo
    // exported and a repo scored describe the same line of commits.
    let shas: Vec<String> = git(
        &repo,
        &["rev-list", "--first-parent", "--reverse", "-n", &count, "HEAD"],
    )?
    .lines()
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .collect();
    if shas.is_empty() {
        return Err("no commits to export".into());
    }

    let config = cqx_score::config::Config::resolve(
        context.args.params.get("--config").map(Path::new),
        &repo,
    )?;
    let branch = branch_name(&repo);
    let history_page = remote.as_deref().and_then(commits_url);

    let scratch = out.join(".work");
    let mut commits: Vec<CommitScore> = Vec::new();
    let mut previous: Option<BTreeMap<String, u32>> = None;

    for (index, sha) in shas.iter().enumerate() {
        let short = &sha[..8];
        let path = dir.join(format!("{short}.json"));
        let (scores, lines, reused) = if path.is_file() {
            // A dataset describes one commit and one commit only, so an
            // existing file is the answer rather than a cache of it.
            let existing: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("{e}"))?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            let scores: BTreeMap<String, u32> =
                serde_json::from_value(existing["score"]["scores"].clone())
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            let lines = existing["score"]["lines"].as_u64().unwrap_or(0);
            (scores, lines, true)
        } else {
            let _ = std::fs::remove_dir_all(&scratch);
            materialise(&repo, sha, &scratch)?;
            let snapshot = cqx_vfs::from_dir(&scratch).map_err(|e| format!("{sha}: {e}"))?;

            // The clock starts with the source in hand, because that is where
            // the browser starts too. What git and the disk cost is real, but
            // it is not what this measures.
            let started = std::time::Instant::now();
            let mut facts = Vec::new();
            cqx_rust::extract::run(&snapshot, &mut facts).map_err(|e| format!("{sha}: {e}"))?;
            let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8_lossy(&facts));
            let metrics = cqx_score::metrics::Metrics::compute(&stream, &config);
            let report = cqx_score::report_json(&config, &metrics);
            let scores: BTreeMap<String, u32> =
                serde_json::from_value(report["scores"].clone()).unwrap_or_default();
            // The timeline lives in the index, not in here: a file that carries
            // its neighbours would be rewritten every time a commit lands.
            let meta = cqx_view::Meta {
                repo: &name,
                branch: &branch,
                remote: remote.as_deref(),
                commits_url: history_page.as_deref(),
                analysed_ms: Some(started.elapsed().as_millis() as u64),
            };
            let dataset = cqx_view::dataset(&stream, report, serde_json::json!([]), &meta);

            std::fs::write(&path, dataset.to_string()).map_err(|e| format!("{e}"))?;
            (scores, metrics.lines, false)
        };

        let facts = git(
            &repo,
            &["show", "-s", "--format=%s%x1f%an%x1f%aI", sha],
        )?;
        let f: Vec<&str> = facts.trim().split('\u{1f}').collect();
        let mut record = CommitScore {
            sha: sha.clone(),
            short: short.to_string(),
            subject: f.first().unwrap_or(&"").to_string(),
            author: f.get(1).unwrap_or(&"").to_string(),
            date: f.get(2).unwrap_or(&"").to_string(),
            lines,
            scores,
            delta: BTreeMap::new(),
        };
        if let Some(prev) = &previous {
            for (category, now) in &record.scores {
                let before = prev.get(category).copied().unwrap_or(*now);
                if *now as i64 != before as i64 {
                    record.delta.insert(category.clone(), *now as i64 - before as i64);
                }
            }
        }
        previous = Some(record.scores.clone());
        eprintln!(
            "  [{}/{}] {} {:<48} {}{}",
            index + 1,
            shas.len(),
            record.short,
            record.subject.chars().take(48).collect::<String>(),
            record
                .scores
                .iter()
                .map(|(c, v)| format!("{}:{v}", &c[..3]))
                .collect::<Vec<_>>()
                .join(" "),
            if reused { "  (already exported)" } else { "" },
        );
        commits.push(record);
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let index = serde_json::json!({
        "repo": name,
        "branch": branch,
        "remote": remote,
        "commits_url": history_page,
        "generated": git(&repo, &["show", "-s", "--format=%aI", "HEAD"])?.trim(),
        "config": cqx_score::config_json(&config),
        "commits": commits,
    });
    let index_path = dir.join("index.json");
    std::fs::write(
        &index_path,
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
    .map_err(|e| format!("{e}"))?;

    eprintln!("\n{} commits → {}", commits.len(), dir.display());
    Ok(())
}
