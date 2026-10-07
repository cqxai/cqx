//! Shared snapshot dispatch, including the Rust gather/resolve/emit protocol.
use cqx_vfs::Vfs;
use serde_json::Value;

pub fn manifests(vfs: &Vfs) -> Result<Value, String> {
    let mut metadata = if vfs
        .paths()
        .any(|p| p == "Cargo.toml" || p.ends_with("/Cargo.toml"))
    {
        cqx_rust::manifest::read(vfs)?
    } else {
        serde_json::json!({ "packages": [] })
    };
    metadata["typescript_bins"] = serde_json::json!(cqx_ts::entry_files(vfs));
    metadata["go_modules"] = serde_json::json!(cqx_go::modules(vfs));
    metadata["php"] = serde_json::json!(cqx_php::metadata(vfs));
    Ok(metadata)
}

pub struct Prepared {
    rust: cqx_rust::extract::Prepared,
    bins: std::collections::BTreeSet<String>,
    go: cqx_go::Prepared,
    php: cqx_php::Metadata,
}

pub fn prepare_reporting(
    vfs: &Vfs,
    metadata: Value,
    total: &dyn Fn(u32),
    tick: &dyn Fn(),
) -> Result<Prepared, ExtractError> {
    let bins = metadata["typescript_bins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let php = serde_json::from_value(metadata["php"].clone()).unwrap_or_default();
    let go_modules = serde_json::from_value(metadata["go_modules"].clone()).unwrap_or_default();
    let go_files = vfs.paths().filter(|p| cqx_go::is_source(p)).count() as u32;
    let extra_files = vfs
        .paths()
        .filter(|p| cqx_php::is_source_in(vfs, p, &php))
        .count() as u32;
    let ts_files = vfs.paths().filter(|p| cqx_ts::is_source(p)).count() as u32;
    let rust = cqx_rust::extract::prepare_reporting(
        vfs,
        metadata,
        &|n| total(n + ts_files + go_files + extra_files),
        tick,
    )?;
    let go = cqx_go::prepare(vfs, go_modules, tick)?;
    Ok(Prepared {
        rust,
        bins,
        go,
        php,
    })
}

impl Prepared {
    pub fn gathered(&self) -> cqx_rust::prepass::PackageFacts {
        self.rust.gathered()
    }
    pub fn emit(
        &self,
        vfs: &Vfs,
        facts: &cqx_rust::prepass::PackageFacts,
        out: impl std::io::Write,
    ) -> Result<Stats, ExtractError> {
        self.emit_watched(vfs, facts, out, &|| {})
    }
    pub fn emit_watched(
        &self,
        vfs: &Vfs,
        facts: &cqx_rust::prepass::PackageFacts,
        mut out: impl std::io::Write,
        tick: &dyn Fn(),
    ) -> Result<Stats, ExtractError> {
        let mut stats = self.rust.emit(vfs, facts, &mut out)?;
        let ts = cqx_ts::run_with_entries(vfs, &mut out, &self.bins, tick)?;
        stats.packages += usize::from(ts.files > 0);
        stats.files += ts.files;
        stats.nodes += ts.nodes;
        stats.edges += ts.edges;
        let go = self.go.emit(vfs, &mut out)?;
        stats.packages += go.packages;
        stats.files += go.files;
        stats.nodes += go.nodes;
        stats.edges += go.edges;
        let extra = cqx_php::run_with_metadata(vfs, &mut out, tick, &self.php)?;
        stats.packages += usize::from(extra.files > 0);
        stats.files += extra.files;
        stats.nodes += extra.nodes;
        stats.edges += extra.edges;
        stats.unparsed.extend(extra.unparsed);
        stats.unparsed.extend(ts.unparsed);
        stats.unparsed.extend(go.unparsed);
        Ok(stats)
    }
}

pub fn run(vfs: &Vfs, out: impl std::io::Write) -> Result<Stats, ExtractError> {
    let metadata = manifests(vfs).map_err(ExtractError::Metadata)?;
    let prepared = prepare_reporting(vfs, metadata, &|_| {}, &|| {})?;
    let mut facts = prepared.gathered();
    facts.resolve();
    prepared.emit(vfs, &facts, out)
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};

pub use cqx_rust::extract::{ExtractError, Stats};

/// Handlers return `()`, so failure is recorded here and the binary exits with
/// it. A library that calls `process::exit` steals that decision from its
/// caller — which is a pattern cqx itself reports.
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::Relaxed)
}

fn fail(message: impl std::fmt::Display) {
    eprintln!("cqx extract: {message}");
    EXIT_CODE.store(1, Ordering::Relaxed);
}

pub const EXTRACT_COMMAND: CommandSpec = CommandSpec {
    name: "extract",
    owner: "cqx-analysis",
    category: "index",
    summary: "Read Rust, Go, TypeScript and JavaScript and emit facts as newline-delimited JSON",
    // `scan` was an alias here. It is now its own command — the one the front
    // page has always shown — and the registry took the second registration
    // without a word, so `cqx scan` quietly went on emitting facts.
    aliases: &[],
    subcommands: &[],
    handler: cmd_extract,
};

/// Every bundled extractor registers the same way; a language is a crate.
/// Flags belong to the crate that reads them, not to the binary.
pub fn register(registry: &mut Registry) {
    registry.add_command(EXTRACT_COMMAND);
    registry.add_flag(FlagSpec {
        name: "--quiet",
        aliases: &["-q"],
        description: "suppress the summary line",
    });
    registry.add_param(ParamSpec {
        name: "--out",
        description: "write facts to a file instead of stdout",
    });
}

fn cmd_extract(context: &Context) {
    let root = context
        .args
        .positionals
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| context.env.cwd.clone());

    let out_path = context.args.params.get("--out").map(PathBuf::from);
    let quiet = context.args.flags.get("--quiet").copied().unwrap_or(false);

    // The one place a filesystem is touched, and it is not analysis — it is
    // what happens before it.
    let vfs = match cqx_vfs::from_dir(&root) {
        Ok(vfs) => vfs,
        Err(e) => {
            fail(format!("{}: {e}", root.display()));
            return;
        }
    };

    let result = match &out_path {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => run(&vfs, std::io::BufWriter::new(file)),
            Err(e) => {
                fail(format!("{}: {e}", path.display()));
                return;
            }
        },
        None => run(&vfs, std::io::BufWriter::new(std::io::stdout().lock())),
    };

    match result {
        Ok(stats) => {
            if !quiet {
                report(&stats, out_path.as_deref());
            }
        }
        Err(e) => fail(e),
    }
}

fn report(stats: &Stats, out_path: Option<&std::path::Path>) {
    // Facts go to stdout when there is no --out, so the summary goes to stderr
    // either way and the stream stays pipeable.
    eprintln!(
        "{} packages · {} files · {} nodes · {} edges{}",
        stats.packages,
        stats.files,
        stats.nodes,
        stats.edges,
        match out_path {
            Some(p) => format!(" → {}", p.display()),
            None => String::new(),
        }
    );
    if !stats.unparsed.is_empty() {
        eprintln!("{} file(s) could not be parsed:", stats.unparsed.len());
        for problem in &stats.unparsed {
            eprintln!("  {problem}");
        }
    }
}
