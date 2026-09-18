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
    owner: "dcx-store",
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

fn cmd_query(context: &Context) {
    let Some(path) = context.args.params.get("--facts").map(PathBuf::from) else {
        eprintln!("dcx query: --facts <file> is required");
        return;
    };
    let stream = match facts::Stream::load(&path) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("dcx query: {e}");
            return;
        }
    };

    if context.args.flags.get("--stats").copied().unwrap_or(false) {
        stream.report();
        return;
    }

    #[cfg(not(feature = "zega"))]
    {
        let _ = context;
        stream.report();
        eprintln!(
            "\ndcx was built without a graph backend, so only counting is available.\n\
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
        if let Err(e) = zega::run(&stream, named, zql) {
            eprintln!("dcx query: {e}");
        }
    }
}
