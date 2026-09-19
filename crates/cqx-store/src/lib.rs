//! Loading a fact stream into a queryable graph.
//!
//! The fact schema is the contract; storage is a backend. This crate keeps that
//! seam honest — the loader reads NDJSON and knows nothing about the engine
//! behind it.

pub mod facts;

#[cfg(feature = "zega")]
pub mod zega;

use std::path::PathBuf;

use deka_cli_core::{CommandSpec, Context, FlagSpec, ParamSpec, Registry};

pub const QUERY_COMMAND: CommandSpec = CommandSpec {
    name: "query",
    owner: "cqx-store",
    category: "index",
    summary: "Load a fact stream and run a query against it",
    aliases: &[],
    subcommands: &[],
    handler: cmd_query,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(QUERY_COMMAND);
    registry.add_param(ParamSpec {
        name: "--facts",
        description: "fact stream to load (newline-delimited JSON)",
    });
    registry.add_param(ParamSpec {
        name: "--zql",
        description: "a query to run against the loaded graph",
    });
    registry.add_param(ParamSpec {
        name: "--named",
        description: "run a built-in query by name; --named list shows them",
    });
    registry.add_flag(FlagSpec {
        name: "--stats",
        aliases: &[],
        description: "summarise what was loaded and stop",
    });
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What `cqx query` should exit with. Read by the binary; a library that
/// calls `process::exit` is a finding this tool reports.
pub fn exit_code() -> i32 {
    i32::from(FAILED.load(std::sync::atomic::Ordering::Relaxed))
}

/// Reports a problem, and makes the process say so. `query` reported a bad
/// query on stderr and exited zero, so a script could not tell a query that
/// found nothing from one that never ran.
fn fail(message: impl std::fmt::Display) {
    eprintln!("cqx query: {message}");
    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn cmd_query(context: &Context) {
    let Some(path) = context.args.params.get("--facts").map(PathBuf::from) else {
        fail("--facts <file> is required");
        return;
    };
    let stream = match facts::Stream::load(&path) {
        Ok(stream) => stream,
        Err(e) => {
            fail(e);
            return;
        }
    };

    if context.args.flags.get("--stats").copied().unwrap_or(false) {
        stream.report();
        return;
    }

    #[cfg(not(feature = "zega"))]
    {
        // Asked for something this build cannot do. Printing a node count and
        // exiting zero answered a different question than the one asked — and
        // a script could not tell that from a query that ran and matched
        // nothing.
        let asked = context.args.params.contains_key("--zql")
            || context.args.params.contains_key("--named")
            || !context.args.positionals.is_empty();
        if asked {
            fail(
                "this build has no graph backend, so --zql and --named cannot run. \
Rebuild with `--features zega`, or use --stats for counts.",
            );
            return;
        }
        stream.report();
        eprintln!(
            "\nThis build has no graph backend, so only counting is available.\n\
             Rebuild with `--features zega` for traversal queries."
        );
    }

    #[cfg(feature = "zega")]
    {
        let named = context.args.params.get("--named").map(String::as_str);
        let zql = context
            .args
            .params
            .get("--zql")
            .map(String::as_str)
            .or_else(|| context.args.positionals.first().map(String::as_str));
        let json = context.args.flags.get("--json").copied().unwrap_or(false);
        if let Err(e) = zega::run(&stream, named, zql, json) {
            fail(e);
        }
    }
}
