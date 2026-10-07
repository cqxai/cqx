# Parser source provenance

These are unmodified source distributions of Rezel Java 0.0.0 and
unicode-ident 1.0.24 from crates.io, including their licenses and notices.
Only Cargo manifests differ: Java's pinned Unicode 17 dependency has a private
package name, while Rust keeps its existing Unicode 18 dependency. This avoids
changing which Rust identifiers parse when Java is added. Rezel Java's parser,
AST and generated parser table are unchanged. Update both copies together when
upgrading Java's parser; do not widen its Unicode tables silently.

Upstream crate archive SHA-256:
- rezel-lang-java 0.0.0: 90b2765c429b0859de610ad1dc11cce3a7aab9d05ab491321b330f2e8da9f413
- unicode-ident 1.0.24: e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75


Rezel Kotlin 0.0.0 parser source and unicode-ident 1.0.24 source are unchanged
from their crates.io distributions; licenses and upstream notices are retained.
Cargo manifests isolate Kotlin's Unicode 17 dependency under a private package
name so Rust keeps its existing Unicode 18 table. Vendor packages are outside
the workspace. The parser sources and generated parser tables are unchanged.

Crate archive SHA-256:
- rezel-lang-kotlin 0.0.0: 21d3356412d7f8766b5360d7c48ceb7db4895496575431dc6ae1c1cfaf9c1f6d
- unicode-ident 1.0.24: e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75


Rezel Swift 0.0.0 (Swift 6.3), https://github.com/magic-akari/rezel,
crates.io archive SHA256 755c7d4e288ba7ca837631a6d81011361e707da0f1cfce8e6bd7a3629ab81b06.
Unicode-ident 1.0.24 archive SHA256 e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75.

Parser and Unicode sources/generated tables are unchanged. Manifest-only path
aliasing uses private cqx-swift-unicode so the parser retains Unicode 17 without
downgrading existing Rust parsing from Unicode 18. Each source's licenses and
notices are retained. Rust dependency updates require the same isolation check.

Rezel Python 0.0.0 (Python 3.14), https://github.com/magic-akari/rezel.
crates.io archive SHA256 903604a61b88e08ebb0e5b5c570ea6d995c13c8347a1b0babb98f27c527de352.
Unicode-ident 1.0.24 archive SHA256 e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75.

Parser/Unicode sources and generated tables are unchanged. Manifest-only private
cqx-python-unicode aliasing retains the parser's Unicode 17 pin without changing
existing Rust's Unicode 18 dependency. Each upstream license/notice is retained.
Python syntax validation and NFKC bindings are parser/source concerns, not a new
type system or calibration.

## zigsyn license verification

zigsyn 0.1.0 is MIT in both the [crates.io version metadata](https://crates.io/crates/zigsyn/0.1.0)
and the [upstream repository license](https://github.com/ydah/zigsyn/blob/main/LICENSE).
Verified 2026-10-07 via crates.io's API and GitHub's license API. The published
archive also contains the MIT license (copyright 2026 ydah). Its SHA-256 is
18c2e53e0acf479814aa8b9e94ccfe0cce54151f1354df4537b318a344d38bcb;
upstream LICENSE blob is 6ba80dcfd12ab97ce945175ffe8fc392515fccb5.
MIT permits inclusion in this Apache-2.0 project with its copyright/license notice
retained. [zigsyn-LICENSE](zigsyn-LICENSE) preserves that notice for distributions.
This parser is a registry dependency, not a modified/vendored parser experiment.

-codex
