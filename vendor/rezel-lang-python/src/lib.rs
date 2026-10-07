#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use std::sync::{Arc, OnceLock};

use rezel_common::{
    Input, ParseError, ParseErrorKind, ParseRequest, PartialParse, TextRange, TextSize, Tree,
};
use rezel_lr::LRParser;

#[cfg(feature = "highlight")]
pub use rezel_highlight::HighlightSpan;

#[rustfmt::skip]
mod generated;
mod highlighting;
mod identifier;
mod indentation;
mod tokens;
mod unicode_names;
#[rustfmt::skip]
mod typed;

pub mod ast;
mod syntax;
/// Named grammar terms emitted by `rezel-generator`.
pub mod terms;
pub use rezel_common::TypedNode;
pub use typed::*;

/// Unicode version supplied by `unicode-ident` for Python identifiers.
pub const UNICODE_VERSION: (u8, u8, u8) = identifier::UNICODE_VERSION;

/// Python parser with strict indentation and syntax validation.
pub type PythonParser = LRParser;

struct PythonValidatedParse {
    inner: Box<dyn PartialParse>,
    input: Arc<dyn Input>,
}

impl PartialParse for PythonValidatedParse {
    fn advance(&mut self) -> Result<Option<Tree>, ParseError> {
        let Some(tree) = self.inner.advance()? else {
            return Ok(None);
        };
        let range = TextRange::new(TextSize::from(0), self.input.len());
        let source = self.input.read(range);
        syntax::validate_syntax(&tree, &source).map_err(|error| {
            ParseError::new(
                ParseErrorKind::Syntax,
                Some(error.position()),
                error.message(),
            )
        })?;
        Ok(Some(tree))
    }

    fn parsed_position(&self) -> TextSize {
        self.inner.parsed_position()
    }

    fn stop_at(&mut self, position: TextSize) -> Result<(), ParseError> {
        self.inner.stop_at(position)
    }

    fn stopped_at(&self) -> Option<TextSize> {
        self.inner.stopped_at()
    }
}

/// Return a cheap clone of the default recovering Python parser.
#[must_use]
pub fn parser() -> PythonParser {
    default_parser().clone()
}

/// Project the Python CST into Lezer-compatible syntactic highlight tags.
#[cfg(feature = "highlight")]
pub fn highlight_spans(tree: &Tree, range: Option<TextRange>, put_span: impl FnMut(HighlightSpan)) {
    rezel_highlight::highlight_spans(tree, range, put_span);
}

fn default_parser() -> &'static LRParser {
    static PARSER: OnceLock<LRParser> = OnceLock::new();
    PARSER.get_or_init(|| {
        LRParser::from_language(&generated::LANGUAGE)
            .with_strict_token_validators(&identifier::STRICT_TOKEN_VALIDATORS)
            .with_create_parse(create_python_parse)
    })
}

fn create_python_parse(
    parser: &LRParser,
    request: ParseRequest,
) -> Result<Box<dyn PartialParse>, ParseError> {
    let request = request.into_validated()?;
    let full_source = matches!(
        request.selected_ranges(),
        [range]
            if range.start() == TextSize::from(0) && range.end() == request.input().len()
    );
    let validate = parser.is_strict() && full_source;
    if validate {
        let range = TextRange::new(TextSize::from(0), request.input().len());
        let source = request.input().read(range);
        indentation::validate(&source)?;
    }

    let input = validate.then(|| Arc::clone(request.input()));
    let inner = parser.create_lr_parse(request)?;
    let Some(input) = input else {
        return Ok(inner);
    };
    Ok(Box::new(PythonValidatedParse { inner, input }))
}
