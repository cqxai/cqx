//! Bounded recognition of Swift generic arguments and value-generic forms.
//!
//! Each argument is parsed up to its immediate comma or closing angle. This
//! keeps a rejected comparison from scanning the remaining source file.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{is_identifier_start, is_operator_continue, scan_lookahead_identifier};
use crate::terms;

use super::lookahead::{
    current, skip_balanced_parentheses, skip_quoted_text, skip_trivia, skip_trivia_with_line_break,
};
use super::type_lookahead;

const BACKTICK: u32 = b'`' as u32;
const AMPERSAND: u32 = b'&' as u32;
const EXCLAMATION: u32 = b'!' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const LEFT_BRACE: u32 = b'{' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COLON: u32 = b':' as u32;
const COMMA: u32 = b',' as u32;
const MINUS: u32 = b'-' as u32;
const PERIOD: u32 = b'.' as u32;
const QUESTION: u32 = b'?' as u32;
const RIGHT_ANGLE: u32 = b'>' as u32;
const RIGHT_BRACE: u32 = b'}' as u32;
const RIGHT_BRACKET: u32 = b']' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const SEMICOLON: u32 = b';' as u32;
const DOUBLE_QUOTE: u32 = b'"' as u32;

pub(super) fn scan_value_lookahead(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<bool, ParseError> {
    let term = match current(input) {
        Some(LEFT_BRACKET)
            if stack.can_shift(terms::inlineArrayTypeExpressionLookahead)
                && starts_inline_array_type_expression(input) =>
        {
            Some(terms::inlineArrayTypeExpressionLookahead)
        }
        Some(LEFT_PAREN)
            if stack.can_shift(terms::parenthesizedGenericValueLookahead)
                && starts_parenthesized_generic_value(input) =>
        {
            Some(terms::parenthesizedGenericValueLookahead)
        }
        _ => None,
    };
    if let Some(term) = term {
        input.accept_token_to(term, input.mark())?;
        return Ok(true);
    }
    Ok(false)
}

fn starts_inline_array_type_expression(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_inline_array_type_expression_from(&mut lookahead)
}

fn starts_inline_array_type_expression_from(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if input.next() != Some(LEFT_BRACKET) {
        return false;
    }

    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut has_count = false;

    loop {
        let Some(has_line_break) = skip_trivia_with_line_break(input) else {
            return false;
        };
        let Some(next) = input.peek().copied() else {
            return false;
        };
        let at_top_level =
            paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 && angle_depth == 0;

        if next == BACKTICK || is_identifier_start(next) {
            let Some(word) = scan_lookahead_identifier(input) else {
                return false;
            };
            if at_top_level && has_count && word.is(b"of") {
                return !has_line_break;
            }
            has_count = true;
            continue;
        }

        if at_top_level && matches!(next, RIGHT_BRACKET | COMMA | COLON | SEMICOLON) {
            return false;
        }
        if at_top_level && next == PERIOD {
            input.next();
            if input.peek() == Some(&PERIOD) {
                input.next();
                if input.peek() == Some(&PERIOD) {
                    return false;
                }
            }
            has_count = true;
            continue;
        }

        input.next();
        match next {
            DOUBLE_QUOTE if !skip_quoted_text(input) => return false,
            LEFT_PAREN => paren_depth += 1,
            RIGHT_PAREN => {
                let Some(depth) = paren_depth.checked_sub(1) else {
                    return false;
                };
                paren_depth = depth;
            }
            LEFT_BRACKET => bracket_depth += 1,
            RIGHT_BRACKET => {
                let Some(depth) = bracket_depth.checked_sub(1) else {
                    return false;
                };
                bracket_depth = depth;
            }
            LEFT_BRACE => brace_depth += 1,
            RIGHT_BRACE => {
                let Some(depth) = brace_depth.checked_sub(1) else {
                    return false;
                };
                brace_depth = depth;
            }
            LEFT_ANGLE => angle_depth += 1,
            RIGHT_ANGLE if angle_depth > 0 => angle_depth -= 1,
            _ => {}
        }
        has_count = true;
    }
}

fn starts_parenthesized_generic_value(input: &InputStream) -> bool {
    if current(input) != Some(LEFT_PAREN) {
        return false;
    }

    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_parenthesized_generic_value_from(&mut lookahead)
}

fn starts_parenthesized_generic_value_from(
    lookahead: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if !skip_balanced_parentheses(lookahead) {
        return false;
    }

    let Some(has_line_break) = skip_trivia_with_line_break(lookahead) else {
        return false;
    };
    if matches!(lookahead.peek().copied(), Some(COMMA | RIGHT_ANGLE)) {
        return true;
    }
    !has_line_break && scan_lookahead_identifier(lookahead).is_some_and(|word| word.is(b"of"))
}

#[derive(Clone, Copy)]
pub(super) enum ArgumentContext {
    Expression,
    Type,
}

pub(super) fn starts_clause(input: &InputStream, context: ArgumentContext) -> bool {
    if current(input) != Some(LEFT_ANGLE) {
        return false;
    }
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_clause_from(&mut lookahead, context)
}

pub(super) fn starts_clause_from(
    lookahead: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    context: ArgumentContext,
) -> bool {
    if lookahead.next() != Some(LEFT_ANGLE) {
        return false;
    }
    if matches!(context, ArgumentContext::Expression) && lookahead.peek() == Some(&RIGHT_ANGLE) {
        return false;
    }

    skip_trivia(lookahead);
    if lookahead.peek() == Some(&RIGHT_ANGLE) {
        lookahead.next();
        return follow_is_disambiguating(lookahead);
    }

    loop {
        if !consume_argument(lookahead) {
            return false;
        }
        skip_trivia(lookahead);
        match lookahead.next() {
            Some(RIGHT_ANGLE) => return follow_is_disambiguating(lookahead),
            Some(COMMA) => {
                skip_trivia(lookahead);
                if lookahead.peek() == Some(&RIGHT_ANGLE) {
                    lookahead.next();
                    return follow_is_disambiguating(lookahead);
                }
            }
            _ => return false,
        }
    }
}

fn consume_argument(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    skip_trivia(input);
    if consume_signed_integer(input) {
        return true;
    }
    if input.peek() == Some(&LEFT_PAREN) {
        if !skip_balanced_parentheses(input) {
            return false;
        }
        return type_lookahead::finish_parenthesized(input);
    }
    type_lookahead::parse(input)
}

fn consume_signed_integer(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if input.peek() == Some(&MINUS) {
        input.next();
        skip_trivia(input);
    }
    let Some(first) = input.peek().copied() else {
        return false;
    };
    if !u8::try_from(first).is_ok_and(|first| first.is_ascii_digit()) {
        return false;
    }
    input.next();
    while input.peek().copied().is_some_and(|next| {
        next == u32::from(b'_') || u8::try_from(next).is_ok_and(|next| next.is_ascii_alphanumeric())
    }) {
        input.next();
    }
    true
}

fn follow_is_disambiguating(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    let Some(has_line_break) = skip_trivia_with_line_break(input) else {
        return false;
    };
    match input.next() {
        None
        | Some(
            RIGHT_PAREN | RIGHT_BRACKET | LEFT_BRACE | RIGHT_BRACE | COMMA | SEMICOLON | COLON,
        ) => true,
        Some(LEFT_PAREN | LEFT_BRACKET) => !has_line_break,
        Some(PERIOD) => input.peek() != Some(&PERIOD),
        Some(EXCLAMATION | QUESTION | AMPERSAND) => {
            !input.peek().copied().is_some_and(is_operator_continue)
        }
        _ => false,
    }
}
