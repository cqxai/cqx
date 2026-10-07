#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use std::sync::{Arc, OnceLock};

use rezel_common::{ParseError, ParseErrorKind, ParseRequest, PartialParse};
use rezel_lr::LRParser;

#[cfg(feature = "highlight")]
use rezel_common::{TextRange, Tree};
#[cfg(feature = "highlight")]
pub use rezel_highlight::HighlightSpan;

#[rustfmt::skip]
mod generated;
mod highlighting;
mod identifier;
mod input;
mod syntax;

pub mod ast;
pub mod terms;
#[rustfmt::skip]
pub mod typed;

use input::JavaInput;

/// Java parser with JLS Unicode-escape translation.
pub type JavaParser = LRParser;

/// Return a cheap clone of the default recovering Java parser.
#[must_use]
pub fn parser() -> JavaParser {
    default_parser().clone()
}

/// Project the Java CST into Lezer-compatible syntactic highlight tags.
///
/// This deliberately performs no name binding, scope analysis, or semantic
/// classification. Downstream consumers decide how abstract tags map to
/// their own token legends.
#[cfg(feature = "highlight")]
pub fn highlight_spans(tree: &Tree, range: Option<TextRange>, put_span: impl FnMut(HighlightSpan)) {
    rezel_highlight::highlight_spans(tree, range, put_span);
}

fn default_parser() -> &'static LRParser {
    static PARSER: OnceLock<LRParser> = OnceLock::new();
    PARSER.get_or_init(|| {
        LRParser::from_language(&generated::LANGUAGE)
            .with_strict_token_validators(&identifier::STRICT_TOKEN_VALIDATORS)
            .with_create_parse(create_java_parse)
    })
}

fn create_java_parse(
    parser: &LRParser,
    request: ParseRequest,
) -> Result<Box<dyn PartialParse>, ParseError> {
    let request = request.into_validated()?;
    let input = Arc::new(JavaInput::new(Arc::clone(request.input())));
    if parser.is_strict()
        && let Some(position) = input.malformed_escape_in(request.selected_ranges())
    {
        return Err(ParseError::new(
            ParseErrorKind::Syntax,
            Some(position),
            "malformed eligible Java Unicode escape",
        ));
    }
    let request = request.with_lexical_input(input)?;
    parser.create_lr_parse(request)
}
