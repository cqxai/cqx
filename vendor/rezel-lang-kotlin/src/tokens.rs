use std::sync::{Arc, LazyLock};

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{
    ContextTracker, ContextValue, ExternalTokenizer, ExternalTokenizerStart, InputStream, Stack,
    TokenizerFlags,
};

use crate::terms;

const BANG: u32 = 33;
const QUOTE: u32 = 34;
const DOLLAR: u32 = 36;
const PERCENT: u32 = 37;
const APOSTROPHE: u32 = 39;
const OPEN_PAREN: u32 = 40;
const ASTERISK: u32 = 42;
const PLUS: u32 = 43;
const MINUS: u32 = 45;
const DOT: u32 = 46;
const SLASH: u32 = 47;
const ZERO: u32 = 48;
const NINE: u32 = 57;
const COLON: u32 = 58;
const LESS_THAN: u32 = 60;
const EQUALS: u32 = 61;
const GREATER_THAN: u32 = 62;
const QUESTION: u32 = 63;
const OPEN_BRACKET: u32 = 91;
const BACKSLASH: u32 = 92;
const OPEN_BRACE: u32 = 123;
const CLOSE_BRACE: u32 = 125;

#[derive(Clone)]
struct StringFrame {
    interpolation_dollars: usize,
    multiline: bool,
    parent: Option<Arc<Self>>,
    hash: u64,
}

#[derive(Clone, Default)]
struct KotlinContext {
    has_line_break: bool,
    string: Option<Arc<StringFrame>>,
}

static BOOLEAN_CONTEXTS: LazyLock<[ContextValue; 2]> = LazyLock::new(|| {
    [
        ContextValue::new(KotlinContext::default()),
        ContextValue::new(KotlinContext {
            has_line_break: true,
            string: None,
        }),
    ]
});

const IMPORT_KEYWORD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE.with_ascii(b'i');

const TRY_CLAUSE_KEYWORD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'c')
    .with_ascii(b'f');

const CONTROL_FUNCTION_KEYWORD_START: ExternalTokenizerStart =
    ExternalTokenizerStart::NONE.with_ascii(b'f');

const MULTI_DOLLAR_STRING_START: ExternalTokenizerStart =
    ExternalTokenizerStart::NONE.with_ascii(b'$');

const IDENTIFIER_ROLE_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii_range(b'A'..=b'Z')
    .with_ascii(b'@')
    .with_ascii(b'_')
    .with_ascii(b'`')
    .with_ascii_range(b'a'..=b'z')
    .with_non_ascii();

const SAME_LINE_FALLBACK_START: ExternalTokenizerStart = IDENTIFIER_ROLE_START
    .with_ascii(b'!')
    .with_ascii(b'%')
    .with_ascii(b'*')
    .with_ascii(b'+')
    .with_ascii(b'-')
    .with_ascii(b'/')
    .with_ascii(b':')
    .with_ascii(b'<')
    .with_ascii(b'=')
    .with_ascii(b'>')
    .with_ascii(b'[');

const SAME_LINE_JUMP_START: ExternalTokenizerStart = IDENTIFIER_ROLE_START
    .with_ascii(b'!')
    .with_ascii(b'"')
    .with_ascii(b'\'')
    .with_ascii(b'(')
    .with_ascii(b'+')
    .with_ascii(b'-')
    .with_ascii(b'.')
    .with_ascii_range(b'0'..=b'9')
    .with_ascii(b':')
    .with_ascii(b'[')
    .with_ascii(b'{');

const DECLARATION_MODIFIER_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'a')
    .with_ascii(b'c')
    .with_ascii(b'd')
    .with_ascii(b'e')
    .with_ascii(b'f')
    .with_ascii(b'i')
    .with_ascii(b'l')
    .with_ascii(b'n')
    .with_ascii(b'o')
    .with_ascii(b'p')
    .with_ascii(b'r')
    .with_ascii(b's')
    .with_ascii(b't')
    .with_ascii(b'v');

pub(crate) static STATEMENT_ENDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_statement_end,
    TokenizerFlags {
        contextual: true,
        fallback: true,
        extend: false,
    },
);

pub(crate) static ADJACENT_CLASS_MEMBER_ENDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_adjacent_class_member_end,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
)
.with_start(DECLARATION_MODIFIER_START);

pub(crate) static SAME_LINE_GUARDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_same_line_guard,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(
    ExternalTokenizerStart::NONE
        .with_ascii(b'(')
        .with_ascii(b'!')
        .with_ascii(b'.')
        .with_ascii(b'?')
        .with_ascii(b':')
        .with_ascii(b'='),
);

pub(crate) static SAME_LINE_JUMP_GUARDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_same_line_jump,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(SAME_LINE_JUMP_START);

pub(crate) static SAME_LINE_LAMBDA_GUARDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_same_line_lambda,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: true,
    },
)
.with_start(ExternalTokenizerStart::NONE.with_ascii(b'{'));

pub(crate) static NULLABLE_RECEIVER_QUESTIONS: ExternalTokenizer = ExternalTokenizer::new(
    scan_nullable_receiver_question,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(ExternalTokenizerStart::NONE.with_ascii(b'?'));

pub(crate) static GENERIC_NULLABLE_CALLABLE_QUESTIONS: ExternalTokenizer = ExternalTokenizer::new(
    scan_generic_nullable_callable_question,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(ExternalTokenizerStart::NONE.with_ascii(b'?'));

pub(crate) static MULTI_DOLLAR_STRING_STARTS: ExternalTokenizer = ExternalTokenizer::new(
    scan_multi_dollar_string_start,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(MULTI_DOLLAR_STRING_START);

pub(crate) static MULTI_DOLLAR_STRING_SEGMENTS: ExternalTokenizer = ExternalTokenizer::new(
    scan_multi_dollar_string_segment,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
)
.with_start(
    ExternalTokenizerStart::NONE
        .with_ascii(b'$')
        .with_ascii(b'"')
        .with_ascii(b'}'),
);

pub(crate) static MULTI_DOLLAR_STRING_CONTENTS: ExternalTokenizer = ExternalTokenizer::new(
    scan_multi_dollar_string_content_token,
    TokenizerFlags {
        contextual: true,
        fallback: false,
        extend: false,
    },
);

pub(crate) static ADJACENT_ANNOTATION_IDENTIFIERS: ExternalTokenizer = ExternalTokenizer::new(
    scan_adjacent_annotation_identifier,
    TokenizerFlags {
        contextual: false,
        fallback: true,
        extend: true,
    },
)
.with_start(IDENTIFIER_ROLE_START);

pub(crate) static SAME_LINE_FALLBACK_GUARDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_same_line_fallback_guard,
    TokenizerFlags {
        contextual: false,
        fallback: true,
        extend: false,
    },
)
.with_start(SAME_LINE_FALLBACK_START);

pub(crate) static LINE_BREAK_PREFIXES: ExternalTokenizer = ExternalTokenizer::new(
    scan_line_break_prefix,
    TokenizerFlags {
        contextual: true,
        fallback: true,
        extend: true,
    },
)
.with_start(IDENTIFIER_ROLE_START);

pub(crate) static TRACK_LINE_BREAK: ContextTracker =
    ContextTracker::new(start_context, None, None, hash_context)
        .with_context_only_shift(shift_context)
        .with_input_shift_for_terms(
            shift_string_context,
            &[
                terms::multiDollarLineStringStart,
                terms::multiDollarMultiLineStringStart,
            ],
        );

pub(crate) static IMPORT_KEYWORDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_import_keyword,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(IMPORT_KEYWORD_START);

pub(crate) static TRY_CLAUSE_KEYWORDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_try_clause_keyword,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(TRY_CLAUSE_KEYWORD_START);

pub(crate) static CONTROL_FUNCTION_KEYWORDS: ExternalTokenizer = ExternalTokenizer::new(
    scan_control_function_keyword,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: true,
    },
)
.with_start(CONTROL_FUNCTION_KEYWORD_START);

fn scan_import_keyword(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    accept_exact_keyword(input, terms::importKeyword, b"import")
}

fn scan_try_clause_keyword(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    let (term, keyword) = match input.next().map(CodePoint::as_u32) {
        Some(value) if value == u32::from(b'c') => (terms::catchKeyword, b"catch".as_slice()),
        Some(value) if value == u32::from(b'f') => (terms::finallyKeyword, b"finally".as_slice()),
        _ => return Ok(()),
    };
    accept_exact_keyword(input, term, keyword)
}

fn scan_control_function_keyword(
    input: &mut InputStream,
    _stack: &Stack,
) -> Result<(), ParseError> {
    accept_exact_keyword(input, terms::controlFunctionKeyword, b"fun")
}

fn skip_kotlin_trivia(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> Option<bool> {
    loop {
        while input
            .peek()
            .is_some_and(|value| matches!(value, 9 | 10 | 12 | 13 | 32))
        {
            input.next();
        }
        if input.peek() != Some(&SLASH) {
            break;
        }
        input.next();
        match input.next()? {
            SLASH => while !matches!(input.next()?, 10 | 13) {},
            ASTERISK => {
                let mut depth = 1usize;
                while depth != 0 {
                    let value = input.next()?;
                    if value == SLASH && input.peek() == Some(&ASTERISK) {
                        input.next();
                        depth += 1;
                    } else if value == ASTERISK && input.peek() == Some(&SLASH) {
                        input.next();
                        depth -= 1;
                    }
                }
            }
            _ => return Some(false),
        }
    }
    Some(true)
}

fn accept_exact_keyword(
    input: &mut InputStream,
    term: u16,
    keyword: &[u8],
) -> Result<(), ParseError> {
    if !starts_exact_ascii_identifier(input, keyword) {
        return Ok(());
    }
    input.advance(keyword.len());
    input.accept_token(term)
}

fn starts_exact_ascii_identifier(input: &InputStream, keyword: &[u8]) -> bool {
    for (offset, expected) in keyword.iter().enumerate() {
        let Ok(offset) = isize::try_from(offset) else {
            return false;
        };
        if input.peek(offset).map(CodePoint::as_u32) != Some(u32::from(*expected)) {
            return false;
        }
    }
    let Ok(end) = isize::try_from(keyword.len()) else {
        return false;
    };
    input
        .peek(end)
        .map(CodePoint::as_u32)
        .is_none_or(is_keyword_boundary)
}

fn scan_same_line_guard(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if has_line_break(stack) {
        return Ok(());
    }
    let first = input.next().map(CodePoint::as_u32);
    let second = input.peek(1).map(CodePoint::as_u32);
    let term = match (first, second) {
        (Some(OPEN_PAREN), _) => terms::sameLineCall,
        (Some(DOT), Some(DOT)) => terms::sameLineRange,
        (Some(BANG), Some(BANG)) => terms::sameLineNotNull,
        (Some(BANG | EQUALS), Some(EQUALS)) => terms::sameLineEquality,
        (Some(COLON), Some(COLON)) => terms::sameLineCallableReference,
        (Some(QUESTION), Some(QUESTION)) => terms::nullableCallableQuestion,
        (Some(QUESTION), Some(COLON))
            if input.peek(2).is_some_and(|value| value.as_u32() == COLON) =>
        {
            terms::nullableCallableQuestion
        }
        (Some(BANG), Some(value)) if value == u32::from(b'i') => {
            let third = input.peek(2).map(CodePoint::as_u32);
            if !matches!(third, Some(value) if value == u32::from(b's') || value == u32::from(b'n'))
            {
                return Ok(());
            }
            let boundary = input
                .peek(3)
                .map(CodePoint::as_u32)
                .is_none_or(is_keyword_boundary);
            if !boundary {
                return Ok(());
            }
            terms::sameLineNegatedTypeOperator
        }
        _ => return Ok(()),
    };
    if term == terms::nullableCallableQuestion {
        input.advance(1);
    }
    input.accept_token(term)?;
    Ok(())
}

fn scan_nullable_receiver_question(
    input: &mut InputStream,
    _stack: &Stack,
) -> Result<(), ParseError> {
    if nullable_receiver_question_starts(input) {
        input.advance(1);
        input.accept_token(terms::receiverQuestion)?;
    }
    Ok(())
}

fn nullable_receiver_question_starts(input: &InputStream) -> bool {
    let chunk = input.identity_lookahead_chunk();
    if let Some(result) = classify_nullable_receiver_question(chunk.iter().copied().map(u32::from))
    {
        return result;
    }
    classify_nullable_receiver_question(input.lookahead().map(CodePoint::as_u32)).unwrap_or(false)
}

fn classify_nullable_receiver_question(input: impl Iterator<Item = u32>) -> Option<bool> {
    let mut input = input.peekable();
    if input.next()? != QUESTION {
        return Some(false);
    }
    if !skip_kotlin_trivia(&mut input)? {
        return Some(false);
    }
    Some(input.next()? == DOT)
}

fn scan_same_line_lambda(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if !has_line_break(stack) {
        input.accept_token(terms::sameLineLambda)?;
    }
    Ok(())
}

fn scan_generic_nullable_callable_question(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    if !has_line_break(stack)
        && input.next().map(CodePoint::as_u32) == Some(QUESTION)
        && input.peek(1).map(CodePoint::as_u32) == Some(COLON)
        && input.peek(2).map(CodePoint::as_u32) == Some(COLON)
    {
        input.advance(1);
        input.accept_token(terms::genericNullableCallableQuestion)?;
    }
    Ok(())
}

fn scan_multi_dollar_string_start(
    input: &mut InputStream,
    _stack: &Stack,
) -> Result<(), ParseError> {
    if input.next().map(CodePoint::as_u32) != Some(DOLLAR) {
        return Ok(());
    }
    let dollar_count = consecutive_ascii(input, DOLLAR);
    if code_point_at(input, dollar_count) != Some(QUOTE) {
        return Ok(());
    }
    let quote_count = input
        .lookahead()
        .skip(dollar_count)
        .take_while(|value| value.as_u32() == QUOTE)
        .count();
    let (term, opener_quotes) = if quote_count >= 3 {
        (terms::multiDollarMultiLineStringStart, 3)
    } else {
        (terms::multiDollarLineStringStart, 1)
    };
    input.advance(dollar_count + opener_quotes);
    input.accept_token(term)
}

fn scan_multi_dollar_string_segment(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    let Some(frame) = active_string_frame(stack) else {
        return Ok(());
    };
    let current = input.next().map(CodePoint::as_u32);
    if current == Some(DOLLAR) {
        let interpolation_dollars = frame.interpolation_dollars;
        let dollar_count = consecutive_ascii(input, DOLLAR);
        let follower = code_point_at(input, interpolation_dollars);
        if dollar_count == interpolation_dollars
            && follower == Some(OPEN_BRACE)
            && stack.can_shift(terms::multiDollarInterpolationExpressionStart)
        {
            input.advance(interpolation_dollars + 1);
            return input.accept_token(terms::multiDollarInterpolationExpressionStart);
        }
        if dollar_count == interpolation_dollars
            && follower.is_some_and(is_string_identifier_start)
            && stack.can_shift(terms::multiDollarInterpolationIdentifierStart)
        {
            input.advance(interpolation_dollars);
            return input.accept_token(terms::multiDollarInterpolationIdentifierStart);
        }
    }
    if current == Some(CLOSE_BRACE) && stack.can_shift(terms::multiDollarInterpolationExpressionEnd)
    {
        input.advance(1);
        return input.accept_token(terms::multiDollarInterpolationExpressionEnd);
    }
    if current == Some(QUOTE) {
        if frame.multiline {
            let quote_count = consecutive_ascii(input, QUOTE);
            if quote_count == 3 && stack.can_shift(terms::multiDollarMultiLineStringEnd) {
                input.advance(3);
                return input.accept_token(terms::multiDollarMultiLineStringEnd);
            }
        } else if stack.can_shift(terms::multiDollarLineStringEnd) {
            input.advance(1);
            return input.accept_token(terms::multiDollarLineStringEnd);
        }
    }
    Ok(())
}

fn scan_multi_dollar_string_content_token(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    let Some(frame) = active_string_frame(stack) else {
        return Ok(());
    };
    scan_multi_dollar_string_content(input, &frame)
}

fn scan_multi_dollar_string_content(
    input: &mut InputStream,
    frame: &StringFrame,
) -> Result<(), ParseError> {
    let start = input.position();
    loop {
        match input.next().map(CodePoint::as_u32) {
            Some(10 | 13) if !frame.multiline => break,
            Some(QUOTE) if frame.multiline => {
                let quote_count = consecutive_ascii(input, QUOTE);
                if quote_count >= 3 {
                    input.advance(quote_count - 3);
                    break;
                }
                input.advance(quote_count);
            }
            None | Some(QUOTE) => break,
            Some(BACKSLASH) if !frame.multiline => {
                let Some(length) = line_string_escape_length(input) else {
                    break;
                };
                input.advance(length);
            }
            Some(DOLLAR) => {
                let dollar_count = consecutive_ascii(input, DOLLAR);
                let follower = code_point_at(input, dollar_count);
                let interpolation_follows = dollar_count >= frame.interpolation_dollars
                    && follower.is_some_and(|value| {
                        value == OPEN_BRACE || is_string_identifier_start(value)
                    });
                if interpolation_follows {
                    let excess = dollar_count - frame.interpolation_dollars;
                    if input.position() == start && excess > 0 {
                        input.advance(excess);
                    }
                    break;
                }
                input.advance(dollar_count);
            }
            Some(_) => {
                input.advance(1);
            }
        }
    }
    if input.position() > start {
        input.accept_token(terms::multiDollarStringContent)?;
    }
    Ok(())
}

fn consecutive_ascii(input: &InputStream, expected: u32) -> usize {
    input
        .lookahead()
        .take_while(|value| value.as_u32() == expected)
        .count()
}

fn code_point_at(input: &InputStream, offset: usize) -> Option<u32> {
    let offset = isize::try_from(offset).ok()?;
    input.peek(offset).map(CodePoint::as_u32)
}

fn line_string_escape_length(input: &InputStream) -> Option<usize> {
    let escaped = code_point_at(input, 1)?;
    if matches!(escaped, 34 | 36 | 39 | 92 | 98 | 110 | 114 | 116) {
        return Some(2);
    }
    if escaped != u32::from(b'u') {
        return None;
    }
    (2..6)
        .all(|offset| code_point_at(input, offset).is_some_and(is_ascii_hex_digit))
        .then_some(6)
}

const fn is_ascii_hex_digit(value: u32) -> bool {
    matches!(value, 48..=57 | 65..=70 | 97..=102)
}

fn is_string_identifier_start(value: u32) -> bool {
    value >= 0xa1
        || u8::try_from(value)
            .is_ok_and(|value| value.is_ascii_alphabetic() || matches!(value, b'_' | b'`'))
}

fn scan_adjacent_annotation_identifier(
    input: &mut InputStream,
    _stack: &Stack,
) -> Result<(), ParseError> {
    if let Some(length) = adjacent_annotation_identifier_length(input) {
        input.advance(length);
        input.accept_token(terms::adjacentAnnotationIdentifier)?;
    }
    Ok(())
}

fn adjacent_annotation_identifier_length(input: &InputStream) -> Option<usize> {
    classify_adjacent_annotation_identifier(input.lookahead().map(CodePoint::as_u32))
}

fn classify_adjacent_annotation_identifier(characters: impl Iterator<Item = u32>) -> Option<usize> {
    let mut characters = characters;
    let first = characters.next()?;
    if first == u32::from(b'`') {
        let mut has_content = false;
        for (offset, value) in characters.by_ref().enumerate() {
            if value == u32::from(b'`') {
                return (has_content && characters.next() == Some(u32::from(b'@')))
                    .then_some(offset + 2);
            }
            if matches!(value, 10 | 13) {
                return None;
            }
            has_content = true;
        }
        return None;
    }
    if !is_generated_identifier_start(first) {
        return None;
    }
    for (offset, value) in characters.enumerate() {
        if !is_generated_identifier_part(value) {
            return (value == u32::from(b'@')).then_some(offset + 1);
        }
    }
    None
}

fn is_generated_identifier_start(value: u32) -> bool {
    value >= 0xa1
        || u8::try_from(value).is_ok_and(|value| value.is_ascii_alphabetic() || value == b'_')
}

fn is_generated_identifier_part(value: u32) -> bool {
    value >= 0xa1
        || u8::try_from(value).is_ok_and(|value| value.is_ascii_alphanumeric() || value == b'_')
}

fn scan_same_line_fallback_guard(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if has_line_break(stack) {
        return Ok(());
    }
    let Some(first) = input.next().map(CodePoint::as_u32) else {
        return Ok(());
    };
    let term = if begins_guarded_operator(input, first) {
        terms::sameLineOperator
    } else if is_identifier_role_start(first) {
        terms::sameLineIdentifier
    } else {
        return Ok(());
    };
    input.accept_token(term)?;
    Ok(())
}

fn scan_same_line_jump(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if !has_line_break(stack) && begins_expression_token(input) {
        input.accept_token(terms::sameLineJump)?;
    }
    Ok(())
}

fn scan_line_break_prefix(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if has_line_break(stack) {
        input.accept_token(terms::lineBreakPrefix)?;
    }
    Ok(())
}

fn begins_guarded_operator(input: &InputStream, first: u32) -> bool {
    match first {
        BANG | PERCENT | ASTERISK | PLUS | MINUS | LESS_THAN | EQUALS | GREATER_THAN
        | OPEN_BRACKET => true,
        COLON => input.peek(1).is_some_and(|next| next.as_u32() == COLON),
        SLASH => input
            .peek(1)
            .is_none_or(|next| !matches!(next.as_u32(), SLASH | ASTERISK)),
        _ => false,
    }
}

fn is_identifier_role_start(value: u32) -> bool {
    if value >= 0xa1 {
        return true;
    }
    u8::try_from(value)
        .is_ok_and(|value| value.is_ascii_alphabetic() || matches!(value, b'@' | b'_' | b'`'))
}

fn begins_expression_token(input: &InputStream) -> bool {
    let Some(first) = input.next().map(CodePoint::as_u32) else {
        return false;
    };
    match first {
        BANG
        | QUOTE
        | APOSTROPHE
        | OPEN_PAREN
        | PLUS
        | MINUS
        | ZERO..=NINE
        | OPEN_BRACKET
        | OPEN_BRACE => true,
        DOT => input
            .peek(1)
            .is_some_and(|value| (ZERO..=NINE).contains(&value.as_u32())),
        COLON => input.peek(1).is_some_and(|value| value.as_u32() == COLON),
        value => is_identifier_role_start(value),
    }
}

fn scan_statement_end(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let next = input.next().map(CodePoint::as_u32);
    let boundary = has_line_break(stack) || next.is_none() || next == Some(CLOSE_BRACE);
    if boundary {
        input.accept_token(terms::insertedStatementEnd)?;
    }
    Ok(())
}

fn scan_adjacent_class_member_end(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    if !has_line_break(stack)
        && adjacent_class_member_starts(input)
        && stack.can_shift(terms::adjacentClassMemberEnd)
    {
        input.accept_token(terms::adjacentClassMemberEnd)?;
    }
    Ok(())
}

fn adjacent_class_member_starts(input: &InputStream) -> bool {
    classify_adjacent_class_member(input.lookahead().map(CodePoint::as_u32)).unwrap_or(false)
}

fn classify_adjacent_class_member(input: impl Iterator<Item = u32>) -> Option<bool> {
    let mut input = input.peekable();
    let mut saw_modifier = false;
    loop {
        if !skip_kotlin_trivia(&mut input)? {
            return Some(false);
        }
        let mut word = [0_u8; 16];
        let mut length = 0_usize;
        while let Some(&value) = input.peek() {
            let Ok(value) = u8::try_from(value) else {
                return Some(false);
            };
            if !value.is_ascii_alphanumeric() && value != b'_' {
                break;
            }
            input.next();
            if length == word.len() {
                return Some(false);
            }
            word[length] = value;
            length += 1;
        }
        let word = &word[..length];
        if is_declaration_modifier(word) {
            saw_modifier = true;
            continue;
        }
        return Some(saw_modifier && is_class_member_introducer(word));
    }
}

fn is_declaration_modifier(word: &[u8]) -> bool {
    matches!(
        word,
        b"abstract"
            | b"actual"
            | b"annotation"
            | b"const"
            | b"crossinline"
            | b"data"
            | b"enum"
            | b"expect"
            | b"external"
            | b"final"
            | b"infix"
            | b"inline"
            | b"inner"
            | b"internal"
            | b"lateinit"
            | b"noinline"
            | b"open"
            | b"operator"
            | b"override"
            | b"private"
            | b"protected"
            | b"public"
            | b"reified"
            | b"sealed"
            | b"suspend"
            | b"tailrec"
            | b"vararg"
    )
}

fn is_class_member_introducer(word: &[u8]) -> bool {
    matches!(
        word,
        b"class"
            | b"companion"
            | b"constructor"
            | b"fun"
            | b"init"
            | b"interface"
            | b"object"
            | b"typealias"
            | b"val"
            | b"var"
    )
}

fn start_context() -> ContextValue {
    static_context(false)
}

#[allow(clippy::unnecessary_wraps)] // ContextTracker callbacks share a fallible signature.
fn shift_context(
    context: &ContextValue,
    term: u16,
) -> Result<Option<(ContextValue, u64)>, ParseError> {
    if preserves_line_break(term) {
        return Ok(None);
    }
    if context.same_identity(&BOOLEAN_CONTEXTS[0]) {
        return Ok((term == terms::lineBreakTrivia).then(|| (BOOLEAN_CONTEXTS[1].clone(), 1)));
    }
    if context.same_identity(&BOOLEAN_CONTEXTS[1]) {
        return Ok((term != terms::lineBreakTrivia).then(|| (BOOLEAN_CONTEXTS[0].clone(), 0)));
    }
    let Some(previous) = context.downcast_ref::<KotlinContext>() else {
        return Ok(None);
    };
    let has_line_break = term == terms::lineBreakTrivia;
    if matches!(
        term,
        terms::multiDollarLineStringEnd | terms::multiDollarMultiLineStringEnd
    ) {
        let string = previous
            .string
            .as_ref()
            .and_then(|frame| frame.parent.clone());
        return Ok(Some(kotlin_context_update(KotlinContext {
            has_line_break,
            string,
        })));
    }
    if has_line_break == previous.has_line_break {
        return Ok(None);
    }
    Ok(Some(kotlin_context_update(KotlinContext {
        has_line_break,
        string: previous.string.clone(),
    })))
}

#[allow(clippy::unnecessary_wraps)] // ContextTracker callbacks share a fallible signature.
fn shift_string_context(
    context: &ContextValue,
    term: u16,
    _stack: &Stack,
    input: &mut InputStream,
) -> Result<ContextValue, ParseError> {
    let previous = context
        .downcast_ref::<KotlinContext>()
        .cloned()
        .unwrap_or_default();
    let interpolation_dollars = consecutive_ascii(input, DOLLAR).max(1);
    let multiline = term == terms::multiDollarMultiLineStringStart;
    let string = push_string_frame(previous.string, interpolation_dollars, multiline);
    Ok(kotlin_context(KotlinContext {
        has_line_break: false,
        string: Some(string),
    }))
}

fn preserves_line_break(term: u16) -> bool {
    matches!(
        term,
        terms::horizontalWhitespace
            | terms::LineComment
            | terms::blockCommentOpen
            | terms::blockCommentStart
            | terms::blockCommentEnd
            | terms::blockCommentLineBreak
            | terms::blockCommentContent
    )
}

fn has_line_break(stack: &Stack) -> bool {
    if stack.context_same_identity(&BOOLEAN_CONTEXTS[0]) {
        return false;
    }
    if stack.context_same_identity(&BOOLEAN_CONTEXTS[1]) {
        return true;
    }
    stack
        .context::<KotlinContext>()
        .is_some_and(|context| context.has_line_break)
}

fn active_string_frame(stack: &Stack) -> Option<Arc<StringFrame>> {
    stack
        .context::<KotlinContext>()
        .and_then(|context| context.string.clone())
}

fn push_string_frame(
    parent: Option<Arc<StringFrame>>,
    interpolation_dollars: usize,
    multiline: bool,
) -> Arc<StringFrame> {
    let parent_hash = parent.as_ref().map_or(0, |frame| frame.hash);
    let interpolation_dollar_hash =
        u64::try_from(interpolation_dollars).expect("string prefix length fits u64");
    let hash = parent_hash
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(interpolation_dollar_hash)
        .wrapping_mul(67)
        .wrapping_add(u64::from(multiline));
    Arc::new(StringFrame {
        interpolation_dollars,
        multiline,
        parent,
        hash,
    })
}

fn kotlin_context(value: KotlinContext) -> ContextValue {
    if value.string.is_none() {
        return static_context(value.has_line_break);
    }
    ContextValue::new(value)
}

fn kotlin_context_update(value: KotlinContext) -> (ContextValue, u64) {
    let hash = kotlin_context_hash(&value);
    (kotlin_context(value), hash)
}

fn static_context(value: bool) -> ContextValue {
    BOOLEAN_CONTEXTS[usize::from(value)].clone()
}

fn hash_context(context: &ContextValue) -> u64 {
    if context.same_identity(&BOOLEAN_CONTEXTS[0]) {
        return 0;
    }
    if context.same_identity(&BOOLEAN_CONTEXTS[1]) {
        return 1;
    }
    let Some(context) = context.downcast_ref::<KotlinContext>() else {
        return 0;
    };
    kotlin_context_hash(context)
}

fn kotlin_context_hash(context: &KotlinContext) -> u64 {
    context
        .string
        .as_ref()
        .map_or(0, |frame| frame.hash)
        .wrapping_mul(67)
        .wrapping_add(u64::from(context.has_line_break))
}

fn is_keyword_boundary(next: u32) -> bool {
    next < 128
        && !matches!(
            u8::try_from(next).expect("ASCII code point fits u8"),
            b'0'..=b'9' | b'A'..=b'Z' | b'_' | b'a'..=b'z'
        )
}

#[cfg(test)]
mod tests {
    use super::{
        BOOLEAN_CONTEXTS, KotlinContext, classify_adjacent_annotation_identifier,
        classify_adjacent_class_member, classify_nullable_receiver_question, hash_context,
        is_keyword_boundary, kotlin_context, push_string_frame, shift_context, start_context,
    };
    use crate::terms;

    #[test]
    fn line_break_context_reuses_boolean_states_outside_strings() {
        let without_break = start_context();
        assert!(without_break.same_identity(&BOOLEAN_CONTEXTS[0]));
        assert!(
            shift_context(&without_break, terms::horizontalWhitespace)
                .unwrap()
                .is_none()
        );
        assert!(
            shift_context(&without_break, terms::Identifier)
                .unwrap()
                .is_none()
        );

        let (with_break, with_break_hash) = shift_context(&without_break, terms::lineBreakTrivia)
            .unwrap()
            .expect("a line break changes the context");
        assert_eq!(with_break_hash, 1);
        assert!(with_break.same_identity(&BOOLEAN_CONTEXTS[1]));
        assert!(
            shift_context(&with_break, terms::lineBreakTrivia)
                .unwrap()
                .is_none()
        );
        assert!(
            shift_context(&with_break, terms::horizontalWhitespace)
                .unwrap()
                .is_none()
        );

        let (cleared, cleared_hash) = shift_context(&with_break, terms::Identifier)
            .unwrap()
            .expect("a non-trivia token clears the line break");
        assert_eq!(cleared_hash, 0);
        assert!(cleared.same_identity(&BOOLEAN_CONTEXTS[0]));
    }

    #[test]
    fn line_break_context_preserves_string_frames_on_the_fallback_path() {
        let string = push_string_frame(None, 2, false);
        let context = kotlin_context(KotlinContext {
            has_line_break: false,
            string: Some(string.clone()),
        });

        let (with_break, with_break_hash) = shift_context(&context, terms::lineBreakTrivia)
            .unwrap()
            .expect("a line break changes the string context");
        assert_eq!(with_break_hash, hash_context(&with_break));
        let with_break = with_break
            .downcast_ref::<KotlinContext>()
            .expect("the string context keeps its concrete type");
        assert!(with_break.has_line_break);
        assert!(
            with_break
                .string
                .as_ref()
                .is_some_and(|frame| std::sync::Arc::ptr_eq(frame, &string))
        );

        let (closed, closed_hash) = shift_context(&context, terms::multiDollarLineStringEnd)
            .unwrap()
            .expect("closing the outer string pops its frame");
        assert_eq!(closed_hash, 0);
        assert!(closed.same_identity(&BOOLEAN_CONTEXTS[0]));
    }

    #[test]
    fn adjacent_class_members_require_a_modifier_and_declaration_introducer() {
        for source in [
            "public fun next",
            "override final val next",
            "actual /* bounded */ class Next",
            "annotation\nclass Next",
        ] {
            assert_eq!(
                classify_adjacent_class_member(source.chars().map(u32::from)),
                Some(true),
                "{source}"
            );
        }
        for source in [
            "fun next",
            "private get",
            "public set",
            "public field",
            "publicValue fun next",
        ] {
            assert_eq!(
                classify_adjacent_class_member(source.chars().map(u32::from)),
                Some(false),
                "{source}"
            );
        }
    }

    #[test]
    fn adjacent_class_member_prefixes_are_inspected_linearly() {
        let mut source = String::from("public");
        source.push_str(&" /* modifier trivia */".repeat(4096));
        source.push_str(" fun next");
        let mut inspected = 0_usize;
        let input = source.chars().map(u32::from).inspect(|_| inspected += 1);
        assert_eq!(classify_adjacent_class_member(input), Some(true));
        assert!(inspected <= source.chars().count());
    }

    #[test]
    fn escaped_identifiers_start_after_exact_keywords() {
        assert!(is_keyword_boundary(u32::from('`')));
        for continuation in ['0', 'A', '_', 'a'] {
            assert!(!is_keyword_boundary(u32::from(continuation)));
        }
    }

    #[test]
    fn adjacent_annotation_identifier_is_one_attached_lexeme() {
        let classify =
            |source: &str| classify_adjacent_annotation_identifier(source.chars().map(u32::from));
        assert_eq!(classify("First@Second"), Some(5));
        assert_eq!(classify("λ2@Second"), Some(2));
        assert_eq!(classify("`First`@Second"), Some(7));
        for source in [
            "First @Second",
            "First/* trivia */@Second",
            "`First` @Second",
        ] {
            assert_eq!(classify(source), None, "{source}");
        }
    }

    #[test]
    fn adjacent_annotation_identifier_scan_is_linear() {
        let source = format!("{}@Second", "a".repeat(4096));
        let mut inspected = 0usize;
        let input = source.chars().map(u32::from).inspect(|_| inspected += 1);
        assert_eq!(classify_adjacent_annotation_identifier(input), Some(4096));
        assert!(inspected <= source.chars().count());
    }

    #[test]
    fn nullable_receiver_question_skips_trivia_linearly() {
        for source in [
            "?.member",
            "? .member",
            "? /* outer /* inner */ tail */\n .member",
        ] {
            assert_eq!(
                classify_nullable_receiver_question(source.chars().map(u32::from)),
                Some(true)
            );
        }
        for source in ["??.member", "? value", "? /* unterminated"] {
            assert_ne!(
                classify_nullable_receiver_question(source.chars().map(u32::from)),
                Some(true)
            );
        }

        let mut source = String::from("?");
        source.push_str(&" /* trivia */".repeat(4096));
        source.push_str(" .member");
        let mut inspected = 0usize;
        let input = source.chars().map(u32::from).inspect(|_| inspected += 1);
        assert_eq!(classify_nullable_receiver_question(input), Some(true));
        assert!(inspected <= source.chars().count());
    }
}
