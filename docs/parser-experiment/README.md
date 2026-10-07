# Parser experiment: zig cc, Lezer C/C++, per-language wasm

**Recommendation: use pinned Zig for the tree-sitter build, and consider separate language modules to keep C# out of unrelated snapshots. Do not replace the held C/C++ or C# frontends with the generated Rezel parsers on this evidence.** This is an experiment for Sami's decision on [#63](https://github.com/cqxai/cqx/pull/63) and [#65](https://github.com/cqxai/cqx/pull/65), not a production frontend change.

The held branches were only read. `prepare.py` assembles copies of their frontend/rule changes on main `519c1a3`, retaining main's newer TypeScript fixes. No merge, release, system install, or secret movement is involved. Scratch sources, compilers, parser downloads, logs, and wasm files are under `.experiment/`, `.target/`, and `.tmp/`. [results.json](results.json) contains exact artifact hashes, timings, coverage, and parity receipts.

## Decision table

Sizes are raw bytes, release, Rust 1.98.1; MB below means decimal MB. R/D/H denote Redis, double-conversion, and Humanizer. Coverage counts eligible files with **no syntax error**, applying the held frontend's vendor/generated exclusions and whole-file skip policy; tests remain eligible for parsing. Times here are extraction including facts emission, except the explicitly marked parser probes.

| Option | Builds / toolchain | Raw wasm bytes | Coverage, parsed / skipped | Findings/report parity | Time |
|---|---|---:|---|---|---|
| Clang monolith, C/C++ + C# | macOS passed; LLVM clang 23.1.2 + llvm-ar + Rust | 18,371,595 | R 225/55; D 35/5; H 728/7 | Reference | R 1.924 s; D 5.880 s; H 3.002 s |
| Zig monolith, C/C++ + C# | macOS and local Ubuntu 24.04 passed; Zig 0.15.2 + Rust; system LLVM absent from PATH | macOS **18,360,071**; Ubuntu **18,359,081** | Identical to reference | **Exact facts bytes and full report bytes** on all three repos; Ubuntu/macOS also exact | R 1.962 s; D 5.992 s; H 3.145 s |
| Rezel-generated C/C++ | macOS wasm build passed; Rust runtime; rezel-generator at generation time | Core with callable parser probe **5,135,117**, **+359,587** over split core; standalone diagnostics probe 482,733 | R **128/152**; D **5/35** | **Fails the parser gate**: every reported C/C++ finding location is in a rejected file. A rule adapter was not built after this gate failed | Parse + CST walk: R 1.012 s; D 1.352 s |
| Community Lezer C# → Rezel | macOS wasm build passed; Rust + generator; minimal highlighting grammar | Standalone diagnostics probe **307,798**; no scoring frontend/module delta claimed | H **570/165** | Cannot preserve #65: lacks method/call/catch syntax nodes; rejects files containing baseline findings | Parse + CST walk: H 0.384 s |
| Split tree-sitter modules with Zig | All modules built on macOS; same Zig/Rust build path | Core **4,775,530**; C/C++ **6,833,786**; C# **11,937,965** | Identical to monolith | **One complete report**, structurally identical on all real repos and a Rust/TS/Go/C/C++/C# fixture with root config | Load + extraction + fold: R 2.170 s; D 6.074 s; H 3.859 s |

GitHub Ubuntu CI is **configured, not observed**: no CI polling was performed. The Ubuntu pass above is an actual local Ubuntu 24.04.4 container build with Rust 1.98.1, Zig 0.15.2, and job-level sccache 0.18.0; no clang/llvm-ar or GCC package was installed. Zig also linked native Rust build scripts there. The CI recipe uses the runner's existing GCC only for native Rust linking, and verifies that system LLVM is absent from the wasm build PATH. Pure Rust probes have a CI build recipe; no Linux probe success is claimed before CI runs.

Separate language-set monoliths were also built and checked against the combined reports:

| Language set | clang bytes | Zig bytes | Difference |
|---|---:|---:|---:|
| Core + C/C++ | 9,114,347 | 9,103,087 | −11,260 |
| Core + C# | 14,219,584 | 14,207,643 | −11,941 |
| Core + C/C++ + C# | 18,371,595 | 18,360,071 | −11,524 |

Fresh unmodified main is **4,755,814 bytes** on this toolchain. These measurements supersede comparisons against older PR-body build outputs. Redis now has containment **100**, rather than #63's historical 70, because the experiment retains main's #62 TypeScript entry fix. Double-conversion quality is **94**; Humanizer quality is **90**. Other categories are 100. The full reports, not just these scores, were compared.

## Zig: small build change, no parser change

Pinned downloads from the [Zig release index](https://ziglang.org/download/index.json):

| Archive | SHA-256 |
|---|---|
| zig-x86_64-macos-0.15.2.tar.xz | `375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f` |
| zig-x86_64-linux-0.15.2.tar.xz | `02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239` |

The first attempt failed exactly with `unable to parse target query 'wasm32-unknown-unknown': UnknownOperatingSystem`. cc-rs supplied `--target=wasm32-unknown-unknown`, overriding the wrapper's earlier `-target wasm32-freestanding`. The smallest fix is an argument adapter translating that one spelling to `--target=wasm32-freestanding`; AR calls `zig ar`.

**No missing libc function or header blocked Zig. No shim change was needed.** Both compilers use tree-sitter-language 0.1.8's existing freestanding includes plus `-include wchar.h -Dstatic_assert=_Static_assert`, as in the held PRs. Versions remain tree-sitter 0.27.0, C 0.24.1, C++ 0.23.4, and WillBooster C# 2.0.2. Imports remain only `cqx.parsing_total` and `cqx.parsed_one`; no WASI/C host imports are required.

On macOS native Rust build scripts still link against Apple's SDK with `/usr/bin/cc`, explicitly by absolute path. That compiler never compiles the wasm grammars and neither it nor LLVM clang is on the experiment's restricted PATH. Zig itself contains LLVM/Clang; this removes the **separate system LLVM installation**, not LLVM from the implementation.

Production cost: integrate the target adapter and checksum-verified Zig bootstrap into the existing build script/CI; choose cache paths; test the other supported contributor architectures. The measured reduction is about 12 KB, so the reason to choose Zig is reproducible packaging/toolchain setup, not frontend size or scan performance.

## Lezer/Rezel: small tables, failed coverage gate

The [official C++ grammar](https://github.com/lezer-parser/cpp) is `@lezer/cpp` **1.1.6**. `rezel-generator` **0.0.0** successfully generates Rust tables; runtime is `rezel-lr` **0.0.0**. The two external tokenizers are direct Rust ports of upstream raw-string and macro/template fallback tokenization. Highlight properties alone are omitted; no grammar production is simplified. Dependencies and downloads are pinned.

The same pinned upstream JavaScript parser was run as an oracle:

| Repo | Eligible | tree-sitter clean | Rezel clean | Upstream JS Lezer clean | Rezel/JS clean-status disagreements |
|---|---:|---:|---:|---:|---:|
| Redis | 280 | 225 | 128 | 201 | 73 |
| double-conversion | 40 | 35 | 5 | 37 | 32 |

A minimal valid `template <typename T> T f(T t) { return t; }` has **3** error nodes in the generated Rust path and **0** in upstream JS. Basic declarations, ordinary functions, namespace/class bodies, and a simple include guard do work. This isolates a generator/runtime integration gap; it is not evidence that the official Lezer grammar inherently has the measured Rust coverage. Resolving that upstream gap is prerequisite to a fair replacement test.

There is already a decisive findings-parity failure under cqx's existing skip policy: Redis's duplicated-body location, all **5** exit findings, all **13** oversized-file locations and both suppressions are in Rezel-rejected files. Double-conversion's duplicate finding in `double-conversion/ieee.h` is also rejected. **No full Rezel rule/scoring frontend was implemented**, and the +359,587-byte figure is explicitly a **linked parser-only ablation**, not the final frontend cost. It retains a callable wasm parse export, so linker elimination cannot produce a misleading zero delta.

Neither parser expands macros, chooses build-specific `#ifdef` branches, resolves typedefs/types, or uses include files. Conditional fragments that split a signature across branches fail; the included sample fails in Rezel as well. Redis's macro-heavy files remain a weakness for both. Lezer documents that its C++ grammar guesses ambiguous syntax without a symbol table/preprocessor. Tree-sitter recognizes nested directive structure used by #63's REDIS_TEST exclusion; Lezer's `PreprocDirective` nodes would need a separate balanced-directive implementation to preserve those exclusions.

C# search covered the official [Lezer grammar catalog](https://lezer.codemirror.net/), npm, and GitHub/web queries `lezer csharp grammar`, `lezer C# grammar parser`. No official C# grammar appeared. The credible community candidate is [ashmind/lezer-csharp-simple](https://github.com/ashmind/lezer-csharp-simple), pinned to `23b0bda779e091f31d6581d7f4c46125fd084df6`. Generation and wasm execution succeed, but it intentionally models blocks, groups, identifiers, literals and punctuation for highlighting. Humanizer has 165 error-bearing files versus tree-sitter's 7; rejected files include 6 of the report's displayed duplicate locations, 2 oversized files, its empty catch, and both suppressions. Accepting an error-free token/block tree would still not establish C# syntax suitable for #65's rules.

Production cost: fix and differentially test Rezel's C++ generation/runtime behavior, then build a CST-to-facts adapter for every #63 rule, spans, shadowing, function fingerprinting and preprocessor/test ranges. Run its existing fires/quiet tests and real-report comparisons. C# needs a substantially richer grammar before any rule adapter. This is parser/front-end work with unresolved scope, not a toolchain substitution.

Comparable timing probes parse the same eligible source and walk the CST to determine error status, returning a small summary. Medians of three runs, excluding loading/file I/O/facts/rules:

| Repo | tree-sitter parse + walk | Rezel parse + walk |
|---|---:|---:|
| Redis | 1,323 ms | 1,012 ms |
| double-conversion | 2,935 ms | 1,352 ms |
| Humanizer | 1,721 ms | 384 ms |

These times accompany different accepted trees and, for C#, a much simpler grammar; they are not equivalent-work speedups.

## Per-language modules: preserve facts, score once

The scratch analysis dispatcher has build features `core`, `c`, and `cs`. Every module keeps the existing `cqx_reset/add_file/score/facts` ABI and the gather/emit/fold functions. The host selects modules by extensions. It merges manifest metadata across readers, gathers/resolves once, emits core/C/C# facts in monolithic order, and calls **the core's fold/scorer once**. It never combines independently calculated language scores.

Fresh-process Node 26.3.1 cold compile + instantiate medians, five runs, excluding byte reads:

| Module | Raw bytes | Cold compile + instantiate |
|---|---:|---:|
| Combined Zig | 18,360,071 | 18.94 ms |
| Core | 4,775,530 | 7.03 ms |
| C/C++ | 6,833,786 | 7.69 ms |
| C# | 11,937,965 | 12.03 ms |

A core-only snapshot loads 4.78 MB, **74.0% less** than the combined monolith. Core + C/C++ is **11,609,316 bytes**; core + C# is **16,713,495**; all three are **23,547,281**, **5,187,210 more** than the combined Zig monolith. Shared code is duplicated: splitting reduces irrelevant download/instantiation, not total bytes for repositories containing everything.

The mixed fixture includes Rust, TS, Go, C, C++, C#, and a root `cqx.json` override. Its entire report matches, with containment 0 and security 70, demonstrating that deductions survive across modules. Redis loads core + C; Humanizer loads core + C#; double-conversion loads core + C. Each real report, including finding locations, skipped files, rule configuration and denominators, matches the monolith. A TypeScript-only snapshot selects only core.

Production cost: stable module manifest/ABI/schema/version checks, selective source routing, compiled-module caching, browser/Worker host integration and failure handling when a requested module cannot load. The prototype passes the full snapshot to each selected module to preserve all coordinator context; route language sources with shared metadata before production to avoid multiplying source memory. The [gild host pattern](https://github.com/gildforge/gild-site/blob/c6957263471125da4ee68bc90a67c4622694cd61/lib/cqx-wasm.ts) caches compiled modules and instantiates per scan; this experiment exercises Node rather than a deployed Worker/browser. Workers would need the existing precompiled-binding strategy, not an assumption that runtime byte compilation is available. Cache modules and analysis results by an immutable manifest containing **all selected module hashes**, rather than only the core hash. No server endpoint or request-time computation is introduced by this PR.

## C++ frontend written in C++: cheap evidence only

No clang frontend was built. The [clang-wasm project's asset table](https://github.com/live-codes/clang-wasm) reports **44.2 MB uncompressed / 15.7 MB compressed** for a full clang wasm compiler, plus separate linker/sysroot assets. That is a full compiler, not a measured frontend-only lower bound. A frontend-only size remains unknown. [Clang's compilation database documentation](https://clang.llvm.org/docs/JSONCompilationDatabase.html) establishes that AST tools need the translation unit's compilation information, including its command and working directory. A clang-based scanner would additionally need to supply project headers/generated files and defines, a different input contract from the source snapshots used here. There is no cheap measured replacement justified by this evidence.

## Validation and reproduction

Passed: workspace `cargo check --locked --workspace --all-targets`; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` (zero warnings); C/C#, TS and Go wasm ABI harnesses; compiler facts/full-report parity on all three corpora; Ubuntu/macOS parity; individual language-set parity; split complete-report parity; checksum-verified downloads; script syntax checks; reproduction of parser generation/builds. Removing either heavy frontend changes the full mixed report in negative controls. Reassembling the pinned copies and rebuilding reproduces the C module byte-for-byte. The workspace commands used Rust 1.98.1 and the configured sccache. No CI status was polled.

From this branch, with the three pinned corpus checkouts in the Codex folder:

```sh
umask 022
mkdir -p .tmp
chmod 700 .tmp
python3 scripts/parser-experiment/download.py
python3 scripts/parser-experiment/prepare.py
chmod +x scripts/parser-experiment/zig-*.py
python3 scripts/parser-experiment/build.py clang
python3 scripts/parser-experiment/build.py zig
python3 scripts/parser-experiment/build.py zig --features core --name core
python3 scripts/parser-experiment/build.py zig --features c --name c
python3 scripts/parser-experiment/build.py zig --features cs --name cs
node scripts/parser-experiment/measure.mjs --measure .experiment/artifacts .experiment/results.json \
  /Volumes/Projects/codex/redis /Volumes/Projects/codex/double-conversion /Volumes/Projects/codex/humanizer
python3 scripts/parser-experiment/lezer.py
node scripts/parser-experiment/measure-lezer.mjs .experiment/artifacts .experiment/lezer-eligible.json \
  /Volumes/Projects/codex/redis /Volumes/Projects/codex/double-conversion /Volumes/Projects/codex/humanizer
python3 scripts/parser-experiment/tree.py
node scripts/parser-experiment/measure-tree.mjs .experiment/artifacts/tree_probe.wasm .experiment/tree-times.json \
  /Volumes/Projects/codex/redis /Volumes/Projects/codex/double-conversion /Volumes/Projects/codex/humanizer
python3 scripts/parser-experiment/link-lezer.py
python3 scripts/parser-experiment/build.py zig --features core --name lezer-linked --source .experiment/lezer-linked
```

Additional oracle setup: `npm_config_cache="$PWD/.experiment/npm-cache" npm install --prefix .experiment/lezer-js --no-audit --no-fund @lezer/cpp@1.1.6`, then `node scripts/parser-experiment/compare-lezer-js.mjs .experiment/lezer-js-results.json /Volumes/Projects/codex/redis /Volumes/Projects/codex/double-conversion`. `ubuntu.sh` records the local container's pinned toolchain and build commands; the Actions workflow is the runner-native equivalent. Exact source revisions and artifact hashes are in results.json. Timings are local observations, not browser/network latency guarantees.

-codex
