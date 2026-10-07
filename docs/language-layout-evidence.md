# Shared language layout evidence

Verified 2026-10-07 under `/Volumes/Projects/codex/cqx`, branch
`codex/shared-language-layout-42`. Base includes merged #62 and main
`519c1a303bfb41f2d22e5dcf2c7d967bb62728c4`. Supersedes #64, #66, #67, #68,
#69 and #70; #63/#65 remain untouched. Nothing was closed, merged on GitHub,
approved, tagged, published or released. CI was not polled.

## Behavior and boundaries

`cqx-layout` owns manifest-root anchors, exact generated markers, layout test
roles, declaration-scoped entries and suppression idioms. Discovery and every
frontend use it. Coordinator metadata carries the full layout to readers whose
shards have no manifests. Rust target semantics and existing TS/Go entry binding
remain intact. Shared rules introduce no weights or calibration changes.

The six frontends retain their existing parser versions. Kotlin still visibly
skips unsupported Gradle statements/infix syntax; Swift still visibly skips one
unsupported test fixture. No parser experiment or C/C# frontend is included.

Python eval/exec literals are quiet. `pickle.loads` is a review site only for a
nonliteral/nonconstant payload; constants are uniquely assigned uppercase module
literal names, with shadowing/reassignment rejected. Stream `pickle.load` still
fires; bound SafeLoader/CSafeLoader stays quiet. PHP exit/die is deduplicated by
source offset; tests assert one `exit(1)` finding, two independent same-line calls,
quiet query builders and concatenated placeholder SQL. Swift density excludes
Package.swift, tests and main.swift script expressions while retaining helper
findings. Zig catch-unreachable is quiet in tests, explicit comptime ranges and
direct calls to unique local functions with explicit `error{}!T`; unknown,
inferred, external and nonempty sets remain review sites.

## Pinned real repositories

Scores below are **containment / legibility / modularity / quality / security**.
Before uses the original PR frontend source at the recorded commit; after uses
that language's consolidated frontend. Both emit into the same scorer, with the
same default ramps and the same pinned repository snapshot. This reproduces all
six original PR score tuples. [Machine-readable evidence](language-layout-evidence.json)
records pins, parse skips, product lines and fired-rule counts.

| Language / repository | Before | After | Fired rules before → after (counts) |
| --- | --- | --- | --- |
| Java / gson | 100/100/100/96/100 | 100/100/100/96/100 | duplicated-bodies 2→2; oversized-files 4→4; undocumented-suppressions 5→5 |
| Kotlin / okio | 100/100/100/87/100 | 100/100/100/89/100 | duplicated-bodies 2→2; oversized-files 0→1; swallowed-errors 5→19; undocumented-suppressions 55→2 |
| Swift / swift-argument-parser | 70/100/100/97/96 | 70/100/100/96/95 | duplicated-bodies 1→1; exit-in-library 30→30; forced-operations 23→29; nonliteral-process 3→4; oversized-files 2→2 |
| Zig / zig-clap | 100/100/96/100/100 | 100/100/96/100/100 | oversized-files 1→1 |
| Python / requests | 100/100/100/90/100 | 100/100/100/99/100 | oversized-files 2→2; undocumented-suppressions 29→1 |
| PHP / symfony/yaml | 100/100/97/100/94 | 100/100/97/100/94 | oversized-files 2→2; unsafe-deserialization 1→1 |

The complete CLI agrees with the after tuples except **okio**, whose combined
Java+Kotlin score is **100/100/100/84/100** with one additional Java
swallowed-errors finding. The Kotlin-only column preserves comparability with
#66. Run `cargo run --locked -p cqx -- scan <pinned-checkout> --json` for the
consolidated report. Original source commits and exact repository commits are
in the JSON evidence rather than moving branch names.

Kotlin product lines grow 25,839→28,658: reusable testing-support code in
`commonMain/AbstractFileSystemTest.kt` is product despite its filename. Its empty
catches now count. Swift product lines grow 17,894→19,319 because importing XCTest
or being named TestHelpers does not define a SwiftPM test target. PHP grows
4,277→4,311 after root-relative test detection. These are classification changes,
not score tuning. Parsed files/skips remain Java 264/0, Kotlin 311/16,
Swift 169/1, Zig 12/0, Python 37/0, PHP 30/0. Skips are disclosed, never converted
to successful analysis. The remaining PHP deserialization site is Inline.php:758.

## Golden provenance

There is one `fixtures/<language>/report.json` for each of nine supported
languages (Rust is `rust-compat`). The single
`all_language_goldens_are_byte_identical` test serializes all reports and compares
exact bytes, including whitespace/newlines. Rust/TS/Go golden files have no diff
against main after #62 (which added Go's golden). Running main's actual scorer
at 519c1a3 on Rust/TS/Go reproduced all
three expected byte streams; SHA-256s are recorded in the JSON evidence.

Java/Kotlin/Swift/Zig/Python/PHP do **not** exist on main, so claiming their reports
are identical to a main frontend would be false. Their goldens are unchanged
from their respective old PR commits, and the same single test proves the fixes
preserve those expected reports. No language rewrites another language's golden.

## Native checks and revert proofs

- `cargo check --locked --workspace --all-targets`: passed.
- `cargo test --locked --workspace --no-fail-fast`: passed, 187 tests.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`:
  passed, zero warnings.
- `cargo fmt --all -- --check`: existing formatting drift in 17 untouched main
  files; no changed file differs from either rustfmt 1.9/1.10. No unrelated
  formatting sweep was made.
- `cargo test --locked -p cqx --test regression --test project_layout`: passed,
  including all nine byte goldens and actual filesystem discovery.
- `python3 scripts/prove-layout.py`: all four mutations caused assertion failures
  with exit 101; restored shared suite passed. Each proof runs real frontend AST
  extraction through the shared function rather than matching source strings.

| Shared behavior | Reintroduced defect | Failing regression |
| --- | --- | --- |
| Anchored exclusions | exclusion on any matching segment | shared_anchored_exclusions |
| Test layout | tests on any matching segment | shared_test_layouts |
| Scoped entry | entry exempts the entire file | shared_scoped_entries |
| Suppression idiom | every specific annotation code counts | shared_idiomatic_suppressions |

`project_layout` also tests pyvenv markers, generated prose versus real headers,
specific suppression codes, library helpers inside entry files, manifest-free
reader shards, and actual CLI discovery through `fixtures/layout-model`.

## WASM measurements

Release builds use rustup 1.98.1, wasm32-unknown-unknown, the repository release
profile and configured sccache. `scripts/measure-language-wasm.py` measures
linkage ablations by retaining production frontend calls with subsets of optional
language dependencies, then restores source/manifests/lock and builds production
with `--locked`. No reduced parser or changed Unicode data is shipped.

Baseline Rust+TS+Go **with the shared layout** is 4,837,831 bytes. Deltas are
individual linkage measurements, not additive; shared parser/runtime code is
reused when all languages link together.

| Language | Raw WASM bytes (baseline + language) | Delta |
| --- | ---: | ---: |
| Java | 5,575,915 | +738,084 |
| Kotlin | 5,328,753 | +490,922 |
| Swift | 5,511,325 | +673,494 |
| Zig | 5,291,945 | +454,114 |
| Python | 6,432,489 | +1,594,658 |
| Php | 5,513,760 | +675,929 |

All languages together: **8,561,754 bytes**. Python's +1,594,658-byte increment is
dominated by Unicode name lookup: a diagnostic-only removal reduces its module
by **858,717 bytes (54%)**. Remaining parser/normalization/frontend code accounts
for 735,941 bytes. Removing lookup would reject valid Python named Unicode
escapes, so it is not a shipping optimization. A compact lookup needs upstream
parser work and should follow the held parser experiment.

A semantics-preserving size reduction is available now: remove only the optional
WASM `name` custom section. `scripts/strip-wasm-names.py` copies the combined module
from **8,561,754 to 7,552,300 bytes**, saving **1,009,454 bytes**. Executable/data
sections are retained; debug function names are lost. All nine WASM harnesses
passed on both copies (18 runs), including coordinator/reader equality, progress,
parse skips, config and language semantics. This PR records the measurement and
provides the copy tool; it does not change release packaging policy.

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo +1.98.1 build --locked --release --target wasm32-unknown-unknown -p cqx-wasm
python3 scripts/measure-language-wasm.py
python3 scripts/strip-wasm-names.py .target/wasm32-unknown-unknown/release/cqx_wasm.wasm .tmp/layout-stripped.wasm
for language in typescript go java kotlin swift zig python php layout; do
  node scripts/test-$language-wasm.mjs .target/wasm32-unknown-unknown/release/cqx_wasm.wasm || exit $?
  node scripts/test-$language-wasm.mjs .tmp/layout-stripped.wasm || exit $?
done
```

## License and AGENTS evidence

zigsyn 0.1.0 is MIT in both [crates.io metadata](https://crates.io/crates/zigsyn/0.1.0)
and the [upstream license](https://github.com/ydah/zigsyn/blob/main/LICENSE), verified
via both APIs. The published archive includes the same MIT notice. It is compatible
with this Apache-2.0 project when retained; vendor/zigsyn-LICENSE preserves it.
Archive checksum and upstream license blob are recorded in vendor/README.md.

Read `/Volumes/Projects/AGENTS.md` first and the checkout's parent instructions.
Used the existing agent-owned checkout and a dedicated task branch, umask 022,
`.target` and private `.tmp`, configured sccache, existing Sami git identity and
signed commits. Builds ran sequentially. No secrets were copied and no other
agent checkout or held PR branch was modified. No GitHub merge/approval/close,
release, tag, publish or CI poll was performed.

-codex
