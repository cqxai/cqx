# Parser source provenance

Rezel Python 0.0.0 (Python 3.14), https://github.com/magic-akari/rezel.
crates.io archive SHA256 903604a61b88e08ebb0e5b5c570ea6d995c13c8347a1b0babb98f27c527de352.
Unicode-ident 1.0.24 archive SHA256 e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75.

Parser/Unicode sources and generated tables are unchanged. Manifest-only private
cqx-python-unicode aliasing retains the parser's Unicode 17 pin without changing
existing Rust's Unicode 18 dependency. Each upstream license/notice is retained.
Python syntax validation and NFKC bindings are parser/source concerns, not a new
type system or calibration.
