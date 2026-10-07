//! Hand-written string syntax views.

use rezel_common::{SyntaxNode, TextRange, TextSize, TypedNode};

use crate::{
    PythonFormatReplacement, PythonFormatSpec, PythonFormatString, PythonTemplateInterpolation,
    PythonTemplateString,
};

use super::{PythonCstInvariantError, PythonExpressionGroup, expression_group};

/// The AST-relevant kind of one interpolated string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonInterpolatedStringKind {
    Format,
    Template,
}

/// One source-backed literal segment or interpolation.
///
/// Literal text intentionally remains a source range here. Escape decoding and
/// Python string values belong to AST lowering, not to the syntax facade.
#[derive(Clone, Debug)]
pub(crate) enum PythonInterpolatedPart {
    Literal {
        range: TextRange,
        kind: PythonInterpolatedLiteralKind,
    },
    Interpolation(PythonInterpolation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonInterpolatedLiteralKind {
    Escaped,
    Debug,
}

/// A normalized replacement shared by f-strings, t-strings, and nested format
/// specifications.
#[derive(Clone, Debug)]
pub(crate) struct PythonInterpolation {
    kind: PythonInterpolatedStringKind,
    expression: PythonExpressionGroup,
    expression_source: TextRange,
    self_documenting: Option<TextRange>,
    conversion: Option<SyntaxNode>,
    format_spec: Option<PythonFormatSpec>,
    range: TextRange,
}

impl PythonInterpolation {
    #[must_use]
    pub(crate) const fn kind(&self) -> PythonInterpolatedStringKind {
        self.kind
    }

    #[must_use]
    pub(crate) const fn expression(&self) -> &PythonExpressionGroup {
        &self.expression
    }

    /// The spelling between `{` and the first `=`, `!`, `:`, or `}`.
    ///
    /// This is distinct from the expression node range because template
    /// strings preserve leading whitespace in `Interpolation.str`.
    #[must_use]
    pub(crate) const fn expression_source(&self) -> TextRange {
        self.expression_source
    }

    /// Source spelling emitted as a literal immediately before the value for
    /// a debug (`{expr=}`) interpolation.
    #[must_use]
    pub(crate) const fn self_documenting(&self) -> Option<TextRange> {
        self.self_documenting
    }

    #[must_use]
    pub(crate) fn conversion(&self) -> Option<&SyntaxNode> {
        self.conversion.as_ref()
    }

    #[must_use]
    pub(crate) fn format_spec(&self) -> Option<&PythonFormatSpec> {
        self.format_spec.as_ref()
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

/// A short-lived view of one f-string or t-string.
#[derive(Clone, Debug)]
pub(crate) struct PythonInterpolatedString {
    kind: PythonInterpolatedStringKind,
    raw: bool,
    range: TextRange,
    parts: Vec<PythonInterpolatedPart>,
}

impl PythonInterpolatedString {
    #[must_use]
    pub(crate) const fn kind(&self) -> PythonInterpolatedStringKind {
        self.kind
    }

    #[must_use]
    pub(crate) const fn is_raw(&self) -> bool {
        self.raw
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }

    #[must_use]
    pub(crate) fn parts(&self) -> &[PythonInterpolatedPart] {
        &self.parts
    }
}

impl PythonFormatString {
    /// # Errors
    ///
    /// Returns an error when the source range or strict interpolation shape is inconsistent.
    pub(crate) fn interpolated(
        &self,
        source: &str,
    ) -> Result<PythonInterpolatedString, PythonCstInvariantError> {
        interpolated_string(self.syntax(), source, PythonInterpolatedStringKind::Format)
    }
}

impl PythonTemplateString {
    /// # Errors
    ///
    /// Returns an error when the source range or strict interpolation shape is inconsistent.
    pub(crate) fn interpolated(
        &self,
        source: &str,
    ) -> Result<PythonInterpolatedString, PythonCstInvariantError> {
        interpolated_string(
            self.syntax(),
            source,
            PythonInterpolatedStringKind::Template,
        )
    }
}

/// Interpret a format specification as the values of its AST `JoinedStr`.
///
/// # Errors
///
/// Returns an error when the source range or strict replacement shape is inconsistent.
pub(crate) fn format_spec_parts(
    node: &PythonFormatSpec,
    source: &str,
) -> Result<Vec<PythonInterpolatedPart>, PythonCstInvariantError> {
    let colon = node
        .syntax()
        .children()
        .find(|child| child.name().as_ref() == ":")
        .ok_or(PythonCstInvariantError::new(
            "a colon starting a format specification",
        ))?;
    partition_parts(
        TextRange::new(colon.to(), node.syntax().to()),
        node.syntax().children(),
        source,
        PythonInterpolatedStringKind::Format,
    )
}

fn interpolated_string(
    node: &SyntaxNode,
    source: &str,
    kind: PythonInterpolatedStringKind,
) -> Result<PythonInterpolatedString, PythonCstInvariantError> {
    let spelling = source_slice(source, node.range())?;
    let (content_start, content_end, raw) = string_content(node, spelling)?;
    let content = TextRange::new(content_start, content_end);
    let parts = partition_parts(content, node.children(), source, kind)?;
    Ok(PythonInterpolatedString {
        kind,
        raw,
        range: node.range(),
        parts,
    })
}

fn string_content(
    node: &SyntaxNode,
    spelling: &str,
) -> Result<(TextSize, TextSize, bool), PythonCstInvariantError> {
    let (quote_offset, quote) = spelling
        .char_indices()
        .find(|(_, character)| matches!(character, '\'' | '"'))
        .ok_or(PythonCstInvariantError::new(
            "a quote in an interpolated string",
        ))?;
    let prefix = spelling[..quote_offset].to_ascii_lowercase();
    let delimiter_width = if spelling[quote_offset..].starts_with(&quote.to_string().repeat(3)) {
        3
    } else {
        1
    };
    let start_offset = quote_offset + delimiter_width;
    let end_offset = spelling
        .len()
        .checked_sub(delimiter_width)
        .filter(|end| *end >= start_offset)
        .ok_or(PythonCstInvariantError::new(
            "a closing interpolated-string quote",
        ))?;
    let absolute_start = usize::from(node.from()) + start_offset;
    let absolute_end = usize::from(node.from()) + end_offset;
    let start = TextSize::try_from(absolute_start)
        .map_err(|_| PythonCstInvariantError::new("an interpolated-string source range"))?;
    let end = TextSize::try_from(absolute_end)
        .map_err(|_| PythonCstInvariantError::new("an interpolated-string source range"))?;
    Ok((start, end, prefix.contains('r')))
}

fn partition_parts(
    content: TextRange,
    children: impl IntoIterator<Item = SyntaxNode>,
    source: &str,
    kind: PythonInterpolatedStringKind,
) -> Result<Vec<PythonInterpolatedPart>, PythonCstInvariantError> {
    let mut parts = Vec::new();
    let mut literal_start = content.start();
    for child in children {
        let replacement_kind = if PythonFormatReplacement::downcast_from(child.clone()).is_ok() {
            Some(PythonInterpolatedStringKind::Format)
        } else if PythonTemplateInterpolation::downcast_from(child.clone()).is_ok() {
            Some(PythonInterpolatedStringKind::Template)
        } else {
            None
        };
        let Some(replacement_kind) = replacement_kind else {
            continue;
        };
        if replacement_kind != kind {
            return Err(PythonCstInvariantError::new(
                "an interpolation matching its containing string",
            ));
        }
        if child.from() > literal_start {
            parts.push(PythonInterpolatedPart::Literal {
                range: TextRange::new(literal_start, child.from()),
                kind: PythonInterpolatedLiteralKind::Escaped,
            });
        }
        let interpolation = interpolation(&child, source, replacement_kind)?;
        if let Some(debug_range) = interpolation.self_documenting() {
            parts.push(PythonInterpolatedPart::Literal {
                range: debug_range,
                kind: PythonInterpolatedLiteralKind::Debug,
            });
        }
        parts.push(PythonInterpolatedPart::Interpolation(interpolation));
        literal_start = child.to();
    }
    if literal_start < content.end() {
        parts.push(PythonInterpolatedPart::Literal {
            range: TextRange::new(literal_start, content.end()),
            kind: PythonInterpolatedLiteralKind::Escaped,
        });
    }
    Ok(parts)
}

fn interpolation(
    node: &SyntaxNode,
    source: &str,
    kind: PythonInterpolatedStringKind,
) -> Result<PythonInterpolation, PythonCstInvariantError> {
    let expression = expression_group(node)?.ok_or(PythonCstInvariantError::new(
        "an expression in an interpolation",
    ))?;
    let opening = node
        .children()
        .find(|child| child.name().as_ref() == "{")
        .ok_or(PythonCstInvariantError::new(
            "an opening interpolation brace",
        ))?;
    let self_documenting = node
        .children()
        .find(|child| child.name().as_ref() == "FormatSelfDoc");
    let conversion = node
        .children()
        .find(|child| child.name().as_ref() == "FormatConversion");
    let format_spec = node
        .children()
        .find_map(|child| PythonFormatSpec::downcast_from(child).ok());
    let closing = node
        .children()
        .find(|child| child.name().as_ref() == "}")
        .ok_or(PythonCstInvariantError::new(
            "a closing interpolation brace",
        ))?;
    let expression_end = self_documenting
        .as_ref()
        .map(SyntaxNode::from)
        .or_else(|| conversion.as_ref().map(SyntaxNode::from))
        .or_else(|| format_spec.as_ref().map(|spec| spec.syntax().from()))
        .unwrap_or_else(|| closing.from());
    let expression_source = trim_range_end(source, TextRange::new(opening.to(), expression_end))?;
    let self_documenting = self_documenting.map(|_| {
        let end = conversion
            .as_ref()
            .map(SyntaxNode::from)
            .or_else(|| format_spec.as_ref().map(|spec| spec.syntax().from()))
            .unwrap_or_else(|| closing.from());
        TextRange::new(opening.to(), end)
    });
    Ok(PythonInterpolation {
        kind,
        expression,
        expression_source,
        self_documenting,
        conversion,
        format_spec,
        range: node.range(),
    })
}

fn trim_range_end(source: &str, range: TextRange) -> Result<TextRange, PythonCstInvariantError> {
    let spelling = source_slice(source, range)?;
    let trimmed = spelling.trim_end();
    let end = usize::from(range.start()) + trimmed.len();
    let end = TextSize::try_from(end)
        .map_err(|_| PythonCstInvariantError::new("an interpolation expression range"))?;
    Ok(TextRange::new(range.start(), end))
}

fn source_slice(source: &str, range: TextRange) -> Result<&str, PythonCstInvariantError> {
    source
        .get(usize::from(range.start())..usize::from(range.end()))
        .ok_or(PythonCstInvariantError::new("a valid UTF-8 source range"))
}
