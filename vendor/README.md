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

-codex
