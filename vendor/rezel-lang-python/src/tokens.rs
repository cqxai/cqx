use rezel_common::{CodePoint, ParseError, ParseErrorKind};
use rezel_lr::{
    ContextTracker, ContextValue, ExternalTokenizer, ExternalTokenizerStart, InputStream, Stack,
    TokenizerFlags,
};

use crate::{indentation::IndentColumns, terms};

const LF: u32 = 10;
const CR: u32 = 13;
const SPACE: u32 = 32;
const TAB: u32 = 9;
const HASH: u32 = 35;
const OPEN_BRACE: u32 = 123;
const CLOSE_BRACE: u32 = 125;
const SINGLE_QUOTE: u32 = 39;
const DOUBLE_QUOTE: u32 = 34;
const BACKSLASH: u32 = 92;

const BRACKETED: u8 = 1;
const STRING: u8 = 2;
const DOUBLE: u8 = 4;
const LONG: u8 = 8;
const RAW: u8 = 16;
const FORMAT: u8 = 32;

const SHIFT_CONTEXT_TERMS: &[u16] = &[
    terms::indent,
    terms::dedent,
    terms::ParenL,
    terms::BracketL,
    terms::BraceL,
    terms::replacementStart,
    terms::stringStart,
    terms::stringStartD,
    terms::stringStartL,
    terms::stringStartLD,
    terms::stringStartR,
    terms::stringStartRD,
    terms::stringStartRL,
    terms::stringStartRLD,
    terms::stringStartF,
    terms::stringStartFD,
    terms::stringStartFL,
    terms::stringStartFLD,
    terms::stringStartFR,
    terms::stringStartFRD,
    terms::stringStartFRL,
    terms::stringStartFRLD,
    terms::stringStartT,
    terms::stringStartTD,
    terms::stringStartTL,
    terms::stringStartTLD,
    terms::stringStartTR,
    terms::stringStartTRD,
    terms::stringStartTRL,
    terms::stringStartTRLD,
];

const REDUCE_CONTEXT_TERMS: &[u16] = &[
    terms::ParenthesizedExpression,
    terms::parenthesizedWithItems,
    terms::TupleExpression,
    terms::ComprehensionExpression,
    terms::importList,
    terms::ArgList,
    terms::ParamList,
    terms::ArrayExpression,
    terms::ArrayComprehensionExpression,
    terms::subscript,
    terms::SetExpression,
    terms::SetComprehensionExpression,
    terms::FormatString,
    terms::TemplateString,
    terms::FormatReplacement,
    terms::TemplateInterpolation,
    terms::nestedFormatReplacement,
    terms::DictionaryExpression,
    terms::DictionaryComprehensionExpression,
    terms::SequencePattern,
    terms::MappingPattern,
    terms::PatternArgList,
    terms::TypeParamList,
    terms::String,
];

#[derive(Clone)]
struct PythonContext {
    parent: Option<ContextValue>,
    indent: usize,
    flags: u8,
    hash: u64,
}

pub(crate) static NEWLINES: ExternalTokenizer = ExternalTokenizer::new(
    scan_newlines,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
)
.with_start(
    ExternalTokenizerStart::NONE
        .with_ascii(b' ')
        .with_ascii(b'\t')
        .with_ascii(b'#')
        .with_ascii(b'\n')
        .with_ascii(b'\r')
        .with_end(),
);

pub(crate) static INDENTATION: ExternalTokenizer = ExternalTokenizer::new(
    scan_indentation,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
);

pub(crate) static STRINGS: ExternalTokenizer = ExternalTokenizer::new(
    scan_strings,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
);

pub(crate) static TRACK_INDENT: ContextTracker =
    ContextTracker::new(start_context, None, None, hash_context)
        .with_context_only_shift(shift_context)
        .with_input_shift_for_terms(shift_indent_context, &[terms::indent])
        .with_shift_terms(SHIFT_CONTEXT_TERMS)
        .with_reduce_without_input(reduce_context)
        .with_reduce_terms(REDUCE_CONTEXT_TERMS);

fn scan_newlines(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let context = context(stack);
    if input.next().is_none() {
        if stack.can_shift(terms::eof) {
            input.accept_token(terms::eof)?;
        }
        return Ok(());
    }
    if context.flags & BRACKETED != 0 {
        if next_value(input).is_some_and(is_line_break) && stack.can_shift(terms::newlineBracketed)
        {
            input.advance(1);
            input.accept_token(terms::newlineBracketed)?;
        }
        return Ok(());
    }
    let previous = input.previous().map(CodePoint::as_u32);
    if previous.is_none_or(is_line_break) && stack.can_shift(terms::blankLineStart) {
        let line_start = input.mark();
        while matches!(next_value(input), Some(SPACE | TAB)) {
            let advance =
                input.advance_ascii_while_with_stop(|byte| matches!(u32::from(byte), SPACE | TAB));
            if advance.stopped_on_mismatch() {
                break;
            }
            if advance.count() == 0 {
                input.advance(1);
            }
        }
        if next_value(input).is_none_or(|next| is_line_break(next) || next == HASH) {
            input.accept_token_to(terms::blankLineStart, line_start)?;
        }
    } else if next_value(input).is_some_and(is_line_break) && stack.can_shift(terms::newline) {
        input.advance(1);
        input.accept_token(terms::newline)?;
    }
    Ok(())
}

fn scan_indentation(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let context = context(stack);
    if context.flags != 0 {
        return Ok(());
    }
    let previous = input.previous().map(CodePoint::as_u32);
    if previous.is_some_and(|character| !is_line_break(character)) {
        return Ok(());
    }
    let mut columns = IndentColumns::default();
    let indent_start = input.mark();
    loop {
        let advance = input.advance_ascii_while_with_stop(|byte| columns.advance(u32::from(byte)));
        if advance.stopped_on_mismatch() {
            break;
        }
        if advance.count() != 0 {
            continue;
        }
        let Some(character) = next_value(input) else {
            break;
        };
        if !columns.advance(character) {
            break;
        }
        input.advance(1);
    }
    let next_is_blank = next_value(input).is_none_or(|next| is_line_break(next) || next == HASH);
    if next_is_blank {
        return Ok(());
    }
    match columns.visual().cmp(&context.indent) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Greater => {
            input.accept_token(terms::indent)?;
        }
        std::cmp::Ordering::Less => {
            input.accept_token_to(terms::dedent, indent_start)?;
        }
    }
    Ok(())
}

fn scan_strings(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let flags = context(stack).flags;
    let quote = if flags & DOUBLE != 0 {
        DOUBLE_QUOTE
    } else {
        SINGLE_QUOTE
    };
    let long = flags & LONG != 0;
    let escapes = flags & RAW == 0;
    let format = flags & FORMAT != 0;
    let start = input.position();
    loop {
        let advance = input.advance_ascii_while_with_stop(|byte| {
            let value = u32::from(byte);
            value != quote && value != LF && value != BACKSLASH && (!format || value != OPEN_BRACE)
        });
        if advance.count() != 0 && !advance.stopped_on_mismatch() {
            continue;
        }
        match next_value(input) {
            None => break,
            Some(OPEN_BRACE) if format => {
                if peek_value(input, 1) == Some(OPEN_BRACE) {
                    input.advance(2);
                } else if input.position() == start {
                    input.advance(1);
                    input.accept_token(terms::replacementStart)?;
                    return Ok(());
                } else {
                    break;
                }
            }
            Some(BACKSLASH) if escapes => {
                if input.position() != start {
                    break;
                }
                input.advance(1);
                if let Some(escaped) = next_value(input) {
                    input.advance(1);
                    skip_escape(input, escaped);
                }
                input.accept_token(terms::Escape)?;
                return Ok(());
            }
            Some(BACKSLASH)
                if format && matches!(peek_value(input, 1), Some(OPEN_BRACE | CLOSE_BRACE)) =>
            {
                input.advance(1);
            }
            Some(BACKSLASH) if peek_value(input, 1).is_some() => {
                input.advance(2);
            }
            Some(next)
                if next == quote
                    && (!long
                        || peek_value(input, 1) == Some(quote)
                            && peek_value(input, 2) == Some(quote)) =>
            {
                if input.position() == start {
                    input.advance(if long { 3 } else { 1 });
                    input.accept_token(terms::stringEnd)?;
                    return Ok(());
                }
                break;
            }
            Some(LF) => {
                if long {
                    input.advance(1);
                } else if input.position() == start {
                    input.accept_token(terms::stringEnd)?;
                    return Ok(());
                } else {
                    break;
                }
            }
            Some(_) => {
                input.advance(1);
            }
        }
    }
    if input.position() > start {
        input.accept_token(terms::stringContent)?;
    }
    Ok(())
}

fn skip_escape(input: &mut InputStream, escaped: u32) {
    match escaped {
        111 => {
            for _ in 0..2 {
                if next_value(input).is_some_and(|next| (48..=55).contains(&next)) {
                    input.advance(1);
                }
            }
        }
        120 => skip_hex(input, 2),
        117 => skip_hex(input, 4),
        85 => skip_hex(input, 8),
        78 if next_value(input) == Some(OPEN_BRACE) => {
            input.advance(1);
            while next_value(input).is_some_and(|next| {
                next != CLOSE_BRACE && next != SINGLE_QUOTE && next != DOUBLE_QUOTE && next != LF
            }) {
                input.advance(1);
            }
            if next_value(input) == Some(CLOSE_BRACE) {
                input.advance(1);
            }
        }
        _ => {}
    }
}

fn skip_hex(input: &mut InputStream, count: usize) {
    for _ in 0..count {
        if next_value(input).is_some_and(is_hex) {
            input.advance(1);
        }
    }
}

fn next_value(input: &InputStream) -> Option<u32> {
    input.next().map(CodePoint::as_u32)
}

fn peek_value(input: &InputStream, offset: isize) -> Option<u32> {
    input.peek(offset).map(CodePoint::as_u32)
}

fn is_hex(value: u32) -> bool {
    (48..=57).contains(&value) || (65..=70).contains(&value) || (97..=102).contains(&value)
}

fn is_line_break(value: u32) -> bool {
    matches!(value, LF | CR)
}

fn start_context() -> ContextValue {
    ContextValue::new(PythonContext {
        parent: None,
        indent: 0,
        flags: 0,
        hash: 0,
    })
}

#[allow(clippy::unnecessary_wraps)] // ContextTracker callbacks share a fallible signature.
fn shift_context(
    value: &ContextValue,
    term: u16,
) -> Result<Option<(ContextValue, u64)>, ParseError> {
    if term == terms::dedent {
        let current = value
            .downcast_ref::<PythonContext>()
            .expect("Python parser context");
        let parent = current.parent.clone().unwrap_or_else(start_context);
        let hash = python_context_hash(&parent);
        return Ok(Some((parent, hash)));
    }
    if matches!(
        term,
        terms::ParenL | terms::BracketL | terms::BraceL | terms::replacementStart
    ) {
        let current = value
            .downcast_ref::<PythonContext>()
            .expect("Python parser context");
        return Ok(Some(child_context_update(value, current, 0, BRACKETED)));
    }
    if let Some(flags) = string_flags(term) {
        let current = value
            .downcast_ref::<PythonContext>()
            .expect("Python parser context");
        return Ok(Some(child_context_update(
            value,
            current,
            0,
            flags | (current.flags & BRACKETED),
        )));
    }
    Ok(None)
}

#[allow(clippy::unnecessary_wraps)] // ContextTracker callbacks share a fallible signature.
fn shift_indent_context(
    value: &ContextValue,
    _term: u16,
    stack: &Stack,
    input: &mut InputStream,
) -> Result<ContextValue, ParseError> {
    let whitespace = input
        .read_scalar(input.position(), stack.position())
        .ok_or_else(|| {
            ParseError::new(
                ParseErrorKind::Input,
                Some(input.position()),
                "Python indentation contains a non-scalar code point",
            )
        })?;
    let indentation = count_indent(&whitespace);
    Ok(child_context(value, indentation.visual(), 0))
}

#[allow(clippy::unnecessary_wraps)] // ContextTracker callbacks share a fallible signature.
fn reduce_context(
    value: &ContextValue,
    term: u16,
    _stack: &Stack,
) -> Result<ContextValue, ParseError> {
    let current = value
        .downcast_ref::<PythonContext>()
        .expect("Python parser context");
    let closes_bracket = current.flags & BRACKETED != 0 && is_bracketed_node(term);
    let closes_string = current.flags & STRING != 0
        && matches!(
            term,
            terms::String | terms::FormatString | terms::TemplateString
        );
    if closes_bracket || closes_string {
        return Ok(current.parent.clone().unwrap_or_else(start_context));
    }
    Ok(value.clone())
}

fn child_context(parent: &ContextValue, indent: usize, flags: u8) -> ContextValue {
    let current = parent
        .downcast_ref::<PythonContext>()
        .expect("Python parser context");
    child_context_update(parent, current, indent, flags).0
}

fn child_context_update(
    parent: &ContextValue,
    current: &PythonContext,
    indent: usize,
    flags: u8,
) -> (ContextValue, u64) {
    let hash = current
        .hash
        .wrapping_mul(257)
        .wrapping_add(indent as u64)
        .wrapping_mul(67)
        .wrapping_add(u64::from(flags));
    let value = ContextValue::new(PythonContext {
        parent: Some(parent.clone()),
        indent,
        flags,
        hash,
    });
    (value, hash)
}

fn context(stack: &Stack) -> &PythonContext {
    stack
        .context::<PythonContext>()
        .expect("Python parser context")
}

fn count_indent(value: &str) -> IndentColumns {
    IndentColumns::from_whitespace(value)
        .expect("shifted Python indentation contains only space, tab, and form feed")
}

fn hash_context(value: &ContextValue) -> u64 {
    python_context_hash(value)
}

fn python_context_hash(value: &ContextValue) -> u64 {
    value
        .downcast_ref::<PythonContext>()
        .map_or(0, |context| context.hash)
}

fn string_flags(term: u16) -> Option<u8> {
    Some(match term {
        terms::stringStart => STRING,
        terms::stringStartD => STRING | DOUBLE,
        terms::stringStartL => STRING | LONG,
        terms::stringStartLD => STRING | LONG | DOUBLE,
        terms::stringStartR => STRING | RAW,
        terms::stringStartRD => STRING | RAW | DOUBLE,
        terms::stringStartRL => STRING | RAW | LONG,
        terms::stringStartRLD => STRING | RAW | LONG | DOUBLE,
        terms::stringStartF | terms::stringStartT => STRING | FORMAT,
        terms::stringStartFD | terms::stringStartTD => STRING | FORMAT | DOUBLE,
        terms::stringStartFL | terms::stringStartTL => STRING | FORMAT | LONG,
        terms::stringStartFLD | terms::stringStartTLD => STRING | FORMAT | LONG | DOUBLE,
        terms::stringStartFR | terms::stringStartTR => STRING | FORMAT | RAW,
        terms::stringStartFRD | terms::stringStartTRD => STRING | FORMAT | RAW | DOUBLE,
        terms::stringStartFRL | terms::stringStartTRL => STRING | FORMAT | RAW | LONG,
        terms::stringStartFRLD | terms::stringStartTRLD => STRING | FORMAT | RAW | LONG | DOUBLE,
        _ => return None,
    })
}

fn is_bracketed_node(term: u16) -> bool {
    matches!(
        term,
        terms::ParenthesizedExpression
            | terms::parenthesizedWithItems
            | terms::TupleExpression
            | terms::ComprehensionExpression
            | terms::importList
            | terms::ArgList
            | terms::ParamList
            | terms::ArrayExpression
            | terms::ArrayComprehensionExpression
            | terms::subscript
            | terms::SetExpression
            | terms::SetComprehensionExpression
            | terms::FormatString
            | terms::TemplateString
            | terms::FormatReplacement
            | terms::TemplateInterpolation
            | terms::nestedFormatReplacement
            | terms::DictionaryExpression
            | terms::DictionaryComprehensionExpression
            | terms::SequencePattern
            | terms::MappingPattern
            | terms::PatternArgList
            | terms::TypeParamList
    )
}

#[cfg(test)]
mod tests {
    use super::{
        REDUCE_CONTEXT_TERMS, SHIFT_CONTEXT_TERMS, is_bracketed_node, string_flags, terms,
    };

    #[test]
    fn context_term_filters_cover_every_transition() {
        for term in 0..=u16::MAX {
            let shifts = matches!(
                term,
                terms::indent
                    | terms::dedent
                    | terms::ParenL
                    | terms::BracketL
                    | terms::BraceL
                    | terms::replacementStart
            ) || string_flags(term).is_some();
            assert_eq!(SHIFT_CONTEXT_TERMS.contains(&term), shifts);

            let reduces = is_bracketed_node(term)
                || matches!(
                    term,
                    terms::String | terms::FormatString | terms::TemplateString
                );
            assert_eq!(REDUCE_CONTEXT_TERMS.contains(&term), reduces);
        }
    }
}
