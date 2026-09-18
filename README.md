# dcx — deka code explorer

A queryable graph of a codebase: what it contains, what it touches, and what
crosses its boundaries.

`dcx` scans source, emits a language-agnostic stream of facts, loads them into a
graph database, and serves a web explorer over the result.

It is two halves, and they are deliberately separable:

1. **The index** — scan → facts → queryable graph. Runs in CI, answers questions
   in milliseconds, and can fail a build when a new boundary gets crossed.
2. **The explorer** — a web view over that same graph. 2D first; 3D later.

The index is useful with no UI at all. That is on purpose: the UI is how you
explore a codebase, the index is how you defend one.

## Status

Early, and nothing here is stable yet. The Rust extractor lands first because it
is what we need on our own toolchain; the schema is designed to be
language-agnostic from the first commit so that other extractors slot in without
touching the core.

## The model

### One graph, not several views

Zoom levels are not separate datasets. They are aggregations over a single node
set joined by a containment spine:

```
system ⊃ binary ⊃ package ⊃ directory ⊃ file ⊃ symbol ⊃ span
```

A package→package arrow is not a primitive fact. It is a rollup of the file and
symbol edges beneath it, so clicking one decomposes into the call sites that
constitute it. Compute an edge twice by two different routes and the levels
disagree; then nobody trusts the picture. So: compute once, aggregate up.

The spatial spine is the **filesystem**, not the module tree. Directories,
files and symbols exist in every language; module systems do not.

### Every edge carries evidence

```
node     := (id, kind, attrs)
edge     := (kind, from_id, to_id, evidence[], confidence)
evidence := (file, line_span, extractor, source: static | runtime, observed_at?)
```

An edge without a file and a line span does not exist. This is what separates a
map from a diagram — every arrow is clickable down to the line that created it,
and an edge nobody can justify is a bug in an extractor rather than a lie in
the UI.

### Edge kinds

Structural edges are numerous and boring. Effect edges are few and dangerous.
The default view shows effects; structure is a layer you turn on.

| kind | meaning |
|---|---|
| `contains` | the zoom spine |
| `depends_on` | package → package |
| `imports` | file → symbol |
| `calls` | symbol → symbol |
| `spawns` | symbol → external process |
| `reads_env` | symbol → environment variable |
| `crosses` | a declared boundary (language edge, client/server split, host call) |
| `effect_fs` / `effect_net` / `effect_exec` | symbol → capability |

Only `contains` and `depends_on` are required. An extractor emits what it can
prove and says so; missing edge kinds degrade the view, they never corrupt it.

### Stable IDs

Identifiers are path-addressed, never line-addressed:

```
pkg:deka_hir
file:crates/deka_hir/src/resolve.rs
sym:deka_hir::resolve::resolve_import
```

Three properties follow, and none of them are available otherwise:

- **Annotations survive refactors.** Notes and generated summaries attach to a
  symbol, not to line 74.
- **Two graphs can be diffed.** Main versus a branch.
- **The diff is the detection.** You do not have to enumerate every bad pattern
  in advance: a *new edge kind appearing where none existed* is itself a signal.

### One temporal graph, not one graph per commit

Edges carry validity in commit space (`first_seen`, `last_seen`). The graph at
any commit is a filter; "when did this edge appear" is a field read rather than
a bisect. With stable IDs the commit-to-commit delta is small, so CI appends
rather than snapshots.

## Extractor contract

The core knows nothing about any language, and there are two ways in.

**Bundled extractors are crates.** Each one owns a command and registers it with
the CLI registry from `deka-cli-core`; the `dcx` binary is composition only, so a
handler can exist nowhere but its owning crate (the rfd#61 pattern, which exists
precisely because implementations kept leaking into the core crate).

```rust
// crates/dcx-rust/src/lib.rs — the handler body lives with the language
pub fn register(registry: &mut Registry) {
    registry.add_command(EXTRACT_COMMAND);
    registry.add_flag(FlagSpec { name: "--quiet", .. });
}
```

```rust
// crates/dcx/src/main.rs — the binary's whole job
RegistryBuilder::new().with(dcx_rust::register)
```

**Out-of-tree extractors are programs.** Anything that writes the same facts to
stdout participates without being in this repo:

```
dcx-extract-<lang> <path>  >  facts.ndjson
```

Either way the schema is identical. Language-specific vocabulary lives in a
`lang:` attribute namespace, never in core node or edge kinds.

## Non-goals

- Inferring architecture from code. Declared intent is checked against observed
  facts; what the tool infers on its own, it does not enforce.
- Being a linter. Existing tools are better at single-file rules. `dcx` is for
  relationships between things.
- A pretty dependency hairball. If the default view needs a legend, it failed.

## Layout

```
crates/
  dcx/             the binary — registry + dispatch, no handler bodies
  dcx-schema/      fact schema + stable ids (the contract everything else depends on)
  dcx-rust/        Rust extractor: owns the `extract` command
samples/basic/     a workspace with deliberately planted facts
```

Planned: `dcx-core` (load, query, diff), `dcx-serve` (the local server behind
`deka explore`), and the web explorer.

## Try it

```
cargo run -p dcx -- extract samples/basic
cargo run -p dcx -- extract /path/to/a/workspace --out facts.ndjson
```

`samples/basic` exists so output can be checked against a known answer instead of
eyeballed: a planted process spawn, two env reads, an unsafe block, a `static
mut`, filesystem and network effects, and a library that calls `process::exit`.
