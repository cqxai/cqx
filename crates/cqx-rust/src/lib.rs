//! The Rust extractor, as a cqx command.
//!
//! Per rfd#61 the handler body lives here, in the owning crate; the `cqx`
//! binary only registers it. Adding a command requires touching an owner crate
//! because that is the only place a handler can exist.

pub mod extract;
mod prepass;
mod visit;

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};

pub use extract::{ExtractError, Stats};

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
    owner: "cqx-rust",
    category: "index",
    summary: "Scan a cargo workspace and emit facts as newline-delimited JSON",
    aliases: &["scan"],
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

    let result = match &out_path {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => extract::run(&root, std::io::BufWriter::new(file)),
            Err(e) => {
                fail(format!("{}: {e}", path.display()));
                return;
            }
        },
        None => extract::run(&root, std::io::BufWriter::new(std::io::stdout().lock())),
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
