//! dcx — composition only.
//!
//! This crate registers commands and dispatches. It holds no handler bodies:
//! adding a command means touching an owner crate, which is the point (rfd#61).

use deka_cli_core::{Context, FlagSpec, Registry, RegistryBuilder};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn build_registry() -> Registry {
    RegistryBuilder::new()
        .with(dcx_rust::register)
        .flags([
            FlagSpec {
                name: "--help",
                aliases: &["-h"],
                description: "show usage",
            },
            FlagSpec {
                name: "--version",
                aliases: &["-V"],
                description: "show the dcx version",
            },
        ])
        .build()
        .unwrap_or_else(|e| {
            eprintln!("dcx: {e:?}");
            std::process::exit(70);
        })
}

fn main() {
    let registry = build_registry();

    let context = match Context::from_env(&registry) {
        Ok(context) => context,
        Err(e) => {
            eprintln!("dcx: {e:?}");
            std::process::exit(2);
        }
    };

    if context.args.flags.get("--version").copied().unwrap_or(false) {
        println!("dcx {VERSION}");
        return;
    }
    if context.args.flags.get("--help").copied().unwrap_or(false) || context.args.commands.is_empty()
    {
        usage(&registry);
        return;
    }

    if let Err(e) = registry.dispatch(&context) {
        eprintln!("dcx: {e:?}");
        usage(&registry);
        std::process::exit(2);
    }

    std::process::exit(dcx_rust::exit_code());
}

fn usage(registry: &Registry) {
    println!("dcx {VERSION} — deka code explorer\n");
    println!("usage: dcx <command> [path] [flags]\n");
    println!("commands:");
    for command in registry.commands() {
        println!("  {:<12} {}", command.name, command.summary);
    }
    println!("\nflags:");
    for flag in registry.flags() {
        println!("  {:<12} {}", flag.name, flag.description);
    }
    for param in registry.params() {
        println!("  {:<12} {}", format!("{} <v>", param.name), param.description);
    }
}
