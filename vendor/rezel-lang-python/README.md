# rezel-lang-python

Python 3.14 parser, typed syntax, and owned AST for Rezel.

The grammar is derived from a pinned Lezer grammar and updated to the Python
3.14 language contract. Strict acceptance and the owned AST are calibrated
against `CPython` 3.14.5; focused references and a standard-library runner
verify those layers. Recovering identifiers use broad candidates, while strict
parsing uses the XID profile supplied by `unicode-ident`.

## Parsing

The default `Module` entry point parses files. `Expression`, `Interactive`, and
`FunctionType` correspond to the other public `ast.parse` modes.

The default parser recovers from syntax errors and records recovery points with
`⚠` nodes. Strict mode also applies language-owned indentation and completed-CST
validation:

```rust
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let source = "answer = 42\n";
let tree = rezel_lang_python::parser().parse(source)?;
let strict_tree = rezel_lang_python::parser()
    .with_strict(true)
    .parse(source)?;
assert_eq!(tree.to_string(), strict_tree.to_string());
# Ok(())
# }
```

External tokenizers implement indentation, newline, string, and permissive
identifier behavior with an immutable indentation context. Strict identifier
validation runs on the selected base token before its LR action. Parsing
accepts UTF-8 Rust strings and reports raw UTF-8 byte offsets. It does not
decode byte streams or execute source-encoding cookies.

## Typed CST

Generated typed syntax provides zero-copy direct-child access:

```rust
use rezel_lang_python::{PythonModule, TypedNode};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let source = "answer = 42\n";
# let tree = rezel_lang_python::parser().with_strict(true).parse(source)?;
let module = PythonModule::downcast_from(tree.top_node())
    .expect("the Python parser returns a Module top node");
assert_eq!(module.statements().count(), 1);
# Ok(())
# }
```

The typed API is a CST view. It retains concrete syntax and ranges without
constructing an owned Python object model.

## Owned AST

`PythonAst::lower` converts a strict tree into an owned arena whose node kinds,
fields, constants, and source ranges follow Python 3.14's public `ast` model:

```rust
use rezel_lang_python::ast::{PythonAst, PythonAstKind};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let source = "answer = 42\n";
# let tree = rezel_lang_python::parser().with_strict(true).parse(source)?;
let ast = PythonAst::lower(&tree, source)?;
assert_eq!(ast.root().kind(), PythonAstKind::Module);
# Ok(())
# }
```

Lowering rejects recovery trees. `lower_with_options` exposes AST options such
as type-comment handling where they are part of the public model.

## Highlighting

The optional `highlight` Cargo feature exposes `highlight_spans`. It projects
syntactic tags and performs no binding, scope, type, or semantic analysis.
