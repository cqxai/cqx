# rezel-lang-swift

`rezel-lang-swift` is Rezel's recovering concrete-syntax parser for Swift 6.3
source files. The maintained syntax surface also includes selected
feature-enabled forms from the pinned `SwiftSyntax` parser when they have an
explicit CST contract and focused tests. These forms are currently part of the
single parser configuration rather than individually selectable dialects. The
grammar and lexical callbacks are generated and linked as static Rust data;
parsing does not invoke a Swift toolchain.

The maintained grammar is exercised against the pinned Swift standard-library
sources and strict `SwiftSyntax` parser cases in the P0 corpus. Focused tests
define CST ownership for syntax that requires contextual tokenization or
lookahead. Newlines in trivia and nested comments are recognized as code-item
boundaries following `SwiftSyntax`'s parsing model.
Recovering identifiers use the grammar's broad scalar candidates. Strict mode
validates selected identifier tokens with `unicode-ident` XID plus Swift's
identifier, dollar-name, and escaped-name additions before LR consumption.

Generated files are checked in. Regenerate them with:

```text
mise run codegen:rezel:scope swift --update
```

The development oracle for the default language is Apple Swift 6.3.3.
`mise run reference:swift` checks the small repository-owned acceptance
boundary with the official frontend. `SwiftSyntax` is the authority for its
feature-enabled CST forms. `mise run reference:swift:p0:prepare` materializes
the pinned Swift and `SwiftSyntax` corpora used by the broad P0 gate.

## Highlighting

The optional `highlight` Cargo feature exposes `highlight_spans`. It projects
the Swift CST into abstract syntactic tags for declarations, references, types,
attributes, literals, comments, keywords, operators, and delimiters without
semantic analysis.
