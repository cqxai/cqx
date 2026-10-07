# Parser source provenance

Rezel Swift 0.0.0 (Swift 6.3), https://github.com/magic-akari/rezel,
crates.io archive SHA256 755c7d4e288ba7ca837631a6d81011361e707da0f1cfce8e6bd7a3629ab81b06.
Unicode-ident 1.0.24 archive SHA256 e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75.

Parser and Unicode sources/generated tables are unchanged. Manifest-only path
aliasing uses private cqx-swift-unicode so the parser retains Unicode 17 without
downgrading existing Rust parsing from Unicode 18. Each source's licenses and
notices are retained. Rust dependency updates require the same isolation check.
