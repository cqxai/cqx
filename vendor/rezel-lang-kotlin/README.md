# rezel-lang-kotlin

Kotlin 2.4 concrete syntax for Rezel.

The parser implements Kotlin 2.4.10 concrete syntax with the same ownership
boundaries as the repository's mature languages:

- regular tokens, including ordinary identifiers, are generated DFA tokens;
- physical line breaks are generated trivia and never enter the visible CST;
- statement ownership is expressed by CFG `statementEnd` sites;
- precedence markers resolve local grammar ambiguity;
- external tokenizers are reserved for lexical decisions that cannot be
  represented by the generated lexer.

The external tokenizers are limited to line-layout facts, exact structural
keyword roles, and the dynamic interpolation threshold of multi-dollar
strings. A context tracker records whether generated line-break trivia occurred
between significant tokens. Line breaks inside block comments remain lexically
hidden, matching the K1 lexer; they preserve an earlier layout boundary but do
not create one. Small zero-width guards expose same-line, line-prefix, and
statement-end facts to the CFG; they do not scan expressions or declarations.
Ordinary identifiers, operators, delimiters, modifiers, physical line-break
recognition, and ordinary line or multiline strings remain generated DFA or
local-DFA tokens.

## Development contract

Parser changes follow the Kotlin 2.4.10 grammar and compiler parser, preserve
the declared positive witnesses, and keep the runtime and generated-table
budgets healthy. `KotlinLightParser` is the direct parse-only oracle; successful
K2 compilation is additional positive compatibility evidence, while K2
semantic diagnostics do not define the parser's rejection boundary. Negative
acceptance is not a gate unless it affects positive parsing, runtime cost, or
artifact size.

The focused positive contract lives in `tests/parser.rs` and the
compiler-accepted reference inventory. Every current witness is active. The
broad reference gates lock any strict rejection by path and byte offset, so new
regressions fail without making compiler-rejected syntax an acceptance gate.

The positive contract includes:

- files, packages, imports, aliases, and repeated top-level declarations;
- classes, interfaces, named objects, functions, properties, type aliases,
  constructors, class members, common modifiers, parameters, accessors,
  explicit backing fields, delegates, and multiline type constraints;
- named, nullable, parenthesized, function, and definitely-non-nullable types;
- literals, names, `this` and `super` references, object literals, ordinary
  calls, index and callable references, member access, not-null suffixes,
  unary, cast, arithmetic, comparison,
  equality, containment, type-check, logical, Elvis, range, postfix updates,
  and common infix expressions;
- blocks, local declarations, assignments, loops, `if`, `when` subjects and
  guards, `try`/`catch`/`finally`, collection literals, and jump expressions;
- value arguments, trailing and standalone lambdas, modifier- and
  type-parameter-bearing anonymous functions, nested block comments, and line,
  triple-quoted, or multi-dollar strings with interpolation;
- boolean, null, decimal/hexadecimal/binary integer, leading-dot and exponent
  real, and character literals.

Strict parsing accepts all 61 focused Kotlin compiler fixtures and all 601
additional inline witnesses accepted by the Kotlin 2.4.10 light parser, all
380 files in the pinned standard-library source archive, and all 1,299 sources
across the 13 shipped source archives. The inline and broad-corpus rejection
manifests are empty. These are positive-coverage snapshots, not completeness
claims.

The typed schema is intentionally `partial`; it is not used as a proxy for
language coverage.

## Parsing

`KotlinFile` is the default entry point. The default parser recovers from syntax
errors. Strict mode rejects parser errors and additionally validates broad DFA
identifier tokens against the `unicode-ident` XID profile before an LR action
consumes them. Escaped identifiers retain their Kotlin-specific rules.

```rust
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let source = "fun main() { val answer = 42 }\n";
let tree = rezel_lang_kotlin::parser().parse(source)?;
let strict_tree = rezel_lang_kotlin::parser()
    .with_strict(true)
    .parse(source)?;
assert_eq!(tree.to_string(), strict_tree.to_string());
# Ok(())
# }
```

All ranges are original UTF-8 byte offsets. Ordinary and escaped identifiers
are emitted by the generated lexer. Recovering parsing keeps its deliberately
broad non-ASCII class; strict parsing validates each selected base token before
its LR action and does not traverse the completed CST. Validation remains keyed
to the base identifier when specialization changes the parser-visible term.

Nested block comments and ordinary line or multiline strings use generated
local token groups. Multi-dollar strings use a separate external mode because
their interpolation delimiter width is source-defined; its persistent context
stack pushes and pops one frame per nested string, while run scanning is
linear. Recursive nesting and interpolation ownership remain in CFG. The
statement-boundary classifier may inspect comment trivia, but its work is
linear in the consumed trivia and stops at the first significant token.

Annotation arguments and immediately following function-type parameters share
one bounded GLR decision. Both readings survive only through the closing
parenthesis, and the following token selects the owner; no source-scanning
lookahead is involved.

Control-body braces retain only the block and lambda interpretations until an
arrow decides ownership. Both parameterized and zero-parameter forms use the
ordinary CFG and share the existing `body` cut at the opening brace; no
source-scanning lookahead is involved.

The generated lexer retains longest-match ownership of ordinary identifiers
and attached labels such as `loop@`. When one annotation immediately follows
another, a fallback tokenizer exposes the shorter final `TypeName` ending just
before the next `@`. It inspects at most that one identifier and keeps both
lexical readings only until the following `@` selects annotation or label
ownership.

Function declarations use a function-specific receiver production that keeps
the final declaration name and the K1 parser's missing-name boundary in one
CFG path. A real identifier wins the named branch; an immediately following
parameter list may close the receiver without a name. In local expression
contexts, the same parameter token selects the anonymous-function branch. This
decision uses ordinary precedence and does not scan source text.

Assignment remains a statement role rather than a member of the recursive
expression union. Prefix-starting targets and annotation-prefixed control-body
targets use dedicated statement productions, so their outer assignment
operator stays visible without adding assignment recursion to every expression
state. Identifier, postfix, binary, and ordinary assignment paths are unchanged.

A same-line property initializer can be followed by a modifier-led class member
without a semicolon. A contextual class-member boundary recognizes only a
modifier prefix that reaches a real member introducer; accessor prefixes such as
`private get`, `private set`, and `private field` remain owned by the property.
The lookahead is linear in the inspected prefix and ordinary identifiers remain
generated DFA tokens.

## Typed CST and highlighting

Typed wrappers are zero-copy views over the current CST. The schema exposes
the families covered by the current typed contract.

```rust
use rezel_lang_kotlin::{KotlinFile, TypedNode};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let source = "fun main() {}\n";
# let tree = rezel_lang_kotlin::parser().with_strict(true).parse(source)?;
let file = KotlinFile::downcast_from(tree.top_node())
    .expect("the Kotlin parser returns a KotlinFile root");
assert_eq!(file.text(source), Some(source));
# Ok(())
# }
```

The optional `highlight` feature exposes `highlight_spans`. Highlighting is
syntactic and covers only the node kinds currently emitted by the grammar.
