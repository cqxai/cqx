#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use std::sync::OnceLock;

use rezel_lr::LRParser;

#[cfg(feature = "highlight")]
use rezel_common::{TextRange, Tree};
#[cfg(feature = "highlight")]
pub use rezel_highlight::HighlightSpan;

#[rustfmt::skip]
mod generated;
mod highlighting;
mod identifier;
mod tokens;
#[rustfmt::skip]
mod typed;

/// Named grammar terms emitted by `rezel-generator`.
pub mod terms;

pub use rezel_common::TypedNode;
pub use typed::{
    SwiftCodeBlock, SwiftCodeBlockItem, SwiftDeclaration, SwiftDollarIdentifier,
    SwiftFunctionDeclaration, SwiftFunctionName, SwiftFunctionParameter,
    SwiftFunctionParameterClause, SwiftIdentifier, SwiftImportDeclaration, SwiftIntegerLiteral,
    SwiftKind, SwiftLanguage, SwiftMemberBlock, SwiftPatternBinding, SwiftReturnClause,
    SwiftSourceFile, SwiftStringLiteral, SwiftStructDeclaration, SwiftTypeName,
    SwiftVariableDeclaration,
};

/// Swift concrete-syntax parser.
pub type SwiftParser = LRParser;

/// Return a cheaply cloned recovering Swift parser.
#[must_use]
pub fn parser() -> SwiftParser {
    default_parser().clone()
}

/// Project the Swift CST into abstract syntactic highlight tags.
#[cfg(feature = "highlight")]
pub fn highlight_spans(tree: &Tree, range: Option<TextRange>, put_span: impl FnMut(HighlightSpan)) {
    rezel_highlight::highlight_spans(tree, range, put_span);
}

fn default_parser() -> &'static LRParser {
    static PARSER: OnceLock<LRParser> = OnceLock::new();
    PARSER.get_or_init(|| {
        LRParser::from_language(&generated::LANGUAGE)
            .with_strict_token_validators(&identifier::STRICT_TOKEN_VALIDATORS)
    })
}
