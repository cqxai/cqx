# rezel-lang-java

Java SE 26 parser, typed syntax, and owned compiler-tree projection for Rezel.

The grammar is derived from a pinned Lezer grammar and updated to the Java SE 26
language contract. Focused reference tests compare acceptance and public tree
projections with JDK 26 `javac`; a standard-library runner provides broad
coverage.

## Parsing and source translation

`Program` parses complete source and `ClassContent` parses a class-body
fragment. The default parser recovers from syntax errors and records recovery
points with `⚠` nodes. Enable strict mode when invalid input must be rejected:

```rust
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let source = "class Sample {}";
let tree = rezel_lang_java::parser().parse(source)?;
let strict_tree = rezel_lang_java::parser()
    .with_strict(true)
    .parse(source)?;
assert_eq!(tree.to_string(), strict_tree.to_string());
# Ok(())
# }
```

The language facade applies Java's eligible Unicode-escape translation before
tokenization while preserving original UTF-8 byte ranges. A malformed eligible
escape is an input error and cannot be hidden by syntax recovery.
Recovering identifiers use a broad scalar candidate range. Strict parsing
validates each selected identifier with `unicode-ident` XID plus Java's
identifier additions before the token participates in an LR action. Those
additions and Java's identifier-ignorable ranges are compact language-owned
tables; the crate does not embed a second complete Unicode identifier database.

## Typed CST

Generated typed syntax provides zero-copy direct-child access:

```rust
use rezel_common::TypedNode;
use rezel_lang_java::typed::JavaProgram;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let source = "class Sample {}";
# let tree = rezel_lang_java::parser().with_strict(true).parse(source)?;
let program = JavaProgram::downcast_from(tree.top_node())
    .expect("the Java parser returns a Program top node");
assert!(program.compilation_unit().is_some());
# Ok(())
# }
```

The typed API remains backed by the CST. Comments and concrete tokens therefore
remain available even when a later projection omits them.

## Owned AST

`JavaAst::lower` converts a strict complete-source tree into an owned arena
whose node kinds, ordered getters, properties, and source ranges follow the
public JDK compiler-tree API:

```rust
use rezel_lang_java::ast::{JavaAst, JavaAstKind};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let source = "class Sample {}";
# let tree = rezel_lang_java::parser().with_strict(true).parse(source)?;
let ast = JavaAst::lower(&tree, source)?;
assert_eq!(ast.root().kind(), JavaAstKind::CompilationUnit);
# Ok(())
# }
```

Lowering rejects recovery trees. Comments remain a CST concern and are not
nodes in the compiler-tree projection.

## Highlighting

The optional `highlight` Cargo feature exposes `highlight_spans`. It projects
syntactic tags and performs no binding, scope, type, or semantic analysis.
