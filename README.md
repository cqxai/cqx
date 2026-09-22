# cqx — CodeQuality Explorer

<p align="left">
  <img src="screenshot-deno.png" alt="CQX Screenshot" width="80%">
</p>

See it in your browser: https://cqx.bio/

A queryable graph of a codebase: what it contains, what it touches, and what
crosses its boundaries — and a **CodeQuality Score** derived from it.

<p align="left">
  <img src="cqx-action.png" alt="CQX Screenshot" width="80%">
</p>

`cqx` has an [official github action](https://github.com/cqxai/action). it is repository-agnostic. It was built while auditing a large Rust workspace
and is calibrated against ripgrep, tokio and deno, but nothing in it is specific
to any project.

`cqx` scans source, emits a language-agnostic stream of facts, loads them into a
graph database, and serves a web explorer over the result.

<p align="left">
  <img src="cqx-zega-deka.png" alt="CQX Screenshot" width="80%">
</p>

It is two halves, and they are deliberately separable:

1. **The index** — scan → facts → queryable graph. Runs in CI, answers questions
   in milliseconds, and can fail a build when a new boundary gets crossed.
2. **The explorer** — a web view over that same graph.

The index is useful with no UI at all. That is on purpose: the UI is how you
explore a codebase, the index is how you defend one.

## Install

```sh
npm install -g @cqxai/cli
cqx --version
```

Or run once with `npx @cqxai/cli --help`. The package is `@cqxai/cli`;
the installed command is still `cqx`.

The npm release includes macOS arm64/x64, Linux x64, and Windows x64.
npm selects the matching `@cqxai/cqx-<platform>-<arch>` optional dependency.

The standalone installer is also available:

```sh
curl -fsSL https://cqx.bio/install | sh
```

## The CodeQuality Score

Every category starts at **100** and is degraded only by a named rule with a
published weight and a cap, so a score is a list of findings rather than a curve
fitted to an imaginary average codebase. No reference population is needed, and
every deduction opens to the file and line that caused it.

The rules target the failure modes of code written fast by many hands — effects
escaping their crate, lints switched off wholesale, the same concept spelled
four ways, bodies copied rather than shared — not abstract elegance.

Thresholds are anchored on real projects rather than intuition, which has already
corrected two wrong assumptions: large files turn out to be normal in Rust
(ripgrep keeps 81% of its lines in files over 500), and raw `unsafe` counts
measure a project's domain rather than its discipline (tokio and deno carry an
order of magnitude more than a CLI does).

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
pkg:grep_searcher
file:crates/searcher/src/sink.rs
sym:grep_searcher::sink::matched
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
the CLI registry from `deka-cli-core`; the `cqx` binary is composition only, so a
handler can exist nowhere but its owning crate (the rfd#61 pattern, which exists
precisely because implementations kept leaking into the core crate).

```rust
// crates/cqx-rust/src/lib.rs — the handler body lives with the language
pub fn register(registry: &mut Registry) {
    registry.add_command(EXTRACT_COMMAND);
    registry.add_flag(FlagSpec { name: "--quiet", .. });
}
```

```rust
// crates/cqx/src/main.rs — the binary's whole job
RegistryBuilder::new().with(cqx_rust::register)
```

**Out-of-tree extractors are programs.** Anything that writes the same facts to
stdout participates without being in this repo:

```
cqx-extract-<lang> <path>  >  facts.ndjson
```

Either way the schema is identical. Language-specific vocabulary lives in a
`lang:` attribute namespace, never in core node or edge kinds.

## Non-goals

- Inferring architecture from code. Declared intent is checked against observed
  facts; what the tool infers on its own, it does not enforce.
- Being a linter. Existing tools are better at single-file rules. `cqx` is for
  relationships between things.
- A pretty dependency hairball. If the default view needs a legend, it failed.

## Layout

```
crates/
  cqx/             the binary — registry + dispatch, no handler bodies
  cqx-schema/      fact schema + stable ids (the contract everything else depends on)
  cqx-rust/        Rust extractor: owns the `extract` command
fixtures/basic/     a workspace with deliberately planted facts
```

The web explorer lives in [deka explorer](https://github.com/dekaruntime/explorer), so
this repository stays Rust. `deka explore` in the deka toolchain is a downstream
consumer of cqx, not a part of it.

## Configuring the rules

Defaults are calibrated against ripgrep, tokio and deno. Override any of them in
a `cqx.json`, searched for upward from the scanned path:

```json
{
  "version": 1,
  "min_score": 70,
  "rules": {
    "exit-in-library": { "weight": 10 },
    "duplicated-bodies": { "enabled": false }
  }
}
```

A file need only mention the rules it changes. Every field can also be set from
the environment, which is how CI usually wants to do it:

```
CQX_RULE_EXIT_IN_LIBRARY_WEIGHT=10
CQX_MIN_SCORE=70
CQX_CONFIG=/path/to/cqx.json
```

Precedence is defaults, then file, then environment, then flags.
`cqx score --explain` prints every rule with its thresholds and where each one
came from. `--min-score` exits non-zero when any category falls below it, which
is the CI gate.

## Try it

```
cargo run -p cqx -- extract fixtures/basic
cargo run -p cqx -- extract /path/to/a/workspace --out facts.ndjson
```

`fixtures/basic` exists so output can be checked against a known answer instead of
eyeballed: a planted process spawn, two env reads, an unsafe block, a `static
mut`, filesystem and network effects, and a library that calls `process::exit`.

## Asking cqx from an agent

Point the agent at the local command. For Claude Code:

```
claude mcp add cqx -- cqx mcp
```

Nothing leaves the machine. Four of the five tools are read-only — `score`,
`findings`, `rules`, `explain`.

The fifth is `propose_rule`, and it is the point. An agent may make a rule
**stricter** and may not make it looser:

```
propose_rule oversized-files.weight 40
  because "we split files at review anyway"
→ written, 25 → 40, tighter

propose_rule oversized-files.free 9
→ Refused: this would lower the standard, and only a person may do that.
```

Without that asymmetry, "make the score go up" has two solutions — write
better code, or lower the bar — and the second is faster, always available,
and looks identical in a diff to anybody skimming. It is also why letting an
agent do this is safe rather than merely guarded: tightening a rule makes the
number it is measured by harder to reach, so it is never in its short-term
interest. What it is good for is the thing a reviewer does by hand today —
noticing that a standard should be higher, and saying so once instead of
correcting the same thing every week.

`because` is required, and is kept in `cqx.json` beside the rule. A threshold
somebody finds in a year with no explanation is a threshold nobody dares
change.

A person may go either way:

```
cqx config set oversized-files.free 9        # allowed, and it says "looser"
cqx config set oversized-files.free 9 --tighten-only   # refused
```

## Scoring a history

```
cqx history /path/to/repo --commits 20
```

Materialises each commit with `git archive` — the working tree is never touched,
so this is safe against a repository somebody is using — extracts, scores, and
writes `history.json` with a per-category score and the change against the
previous commit.

A commit's score can never change, so each one is written once and reread
thereafter: twelve commits of a 75k-line workspace take 15 seconds cold and half
a second warm.

## Recalibrating

Nothing here is a fact about good code; some of it is a house standard. File
length is the clearest case — measured across ripgrep, tokio, deno, deka and dsc,
it tracks a project's habits rather than its quality, and the best-regarded
codebase in that set has the *most* large files. So cqx ships a lenient default
and makes the knob obvious.

```
cqx config show                                  # or --json, for a reader that is not a person
cqx config set oversized-files.max_lines 2500
cqx config set oversized-line-share.enabled false
```

`config set` rewrites one field and leaves the rest of the file alone, checks the
rule and field exist, and names the accepted fields when one does not. The
effective configuration travels with every result — `score --json` and
`history.json` both carry it — so anything rendering those shows the standards
they were scored against rather than cqx's defaults.

## Conformance

cqx reads a workspace by parsing manifests rather than by running
`cargo metadata`, so cargo is the oracle: whatever it reports about packages,
versions and target roots is what the parser has to reproduce.

`reference-repos.toml` pins ripgrep, tokio and deno by commit, and CI fetches
them to check the parser against all three. They are here because our own
repositories were not enough — three cargo rules were found only when an
external project disagreed:

- a build script at a repository root would otherwise claim the whole repository
- declared `[[example]]` targets do not replace discovery, they add to it
- a path in `[workspace.dependencies]` makes a member even when nothing draws on it

Locally the test skips when the checkouts are absent. Setting
`CQX_REFERENCE_DIR` asserts they are present, so a failed fetch fails the job
rather than quietly checking nothing.
