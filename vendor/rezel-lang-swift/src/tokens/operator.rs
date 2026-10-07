//! Swift operator token shape, fixity, and continuation boundaries.
//!
//! Runtime tokenizer registration and syntax-lookahead priority remain in the
//! parent module. Scalar lexical classes live in `super::lexical`; this module
//! owns operator shape, fixity, and source-boundary decisions.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{
    LookaheadIdentifier, is_operator_continue, is_operator_start, scan_lookahead_identifier,
};
use super::lookahead::{current, peek, previous, skip_trivia, starts_comment};
use crate::terms;

use super::{
    COMMA, EXCLAMATION, LEFT_ANGLE, LEFT_BRACE, LEFT_BRACKET, LEFT_PAREN, PERIOD, QUESTION, SLASH,
    literal,
};

const STAR: u32 = b'*' as u32;
const AMPERSAND: u32 = b'&' as u32;
const EQUAL: u32 = b'=' as u32;
const COLON: u32 = b':' as u32;
const MINUS: u32 = b'-' as u32;
const PERCENT: u32 = b'%' as u32;
const PIPE: u32 = b'|' as u32;
const PLUS: u32 = b'+' as u32;
const RIGHT_ANGLE: u32 = b'>' as u32;
const RIGHT_BRACE: u32 = b'}' as u32;
const RIGHT_BRACKET: u32 = b']' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const SEMICOLON: u32 = b';' as u32;
const TILDE: u32 = b'~' as u32;

pub(super) fn scan_question_mark(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<bool, ParseError> {
    if current(input) != Some(QUESTION) {
        return Ok(false);
    }
    let left_bound = is_left_bound(input);
    let continues_operator = question_mark_continues_operator(input);
    let splits_regex = !left_bound
        && stack.can_shift(terms::infixQuestionMark)
        && literal::regex_literal_width_at_operator_suffix(input, 1).is_some();
    if continues_operator && !splits_regex && !(left_bound && peek(input, 1) == Some(PERIOD)) {
        return Ok(false);
    }
    let term = if left_bound {
        terms::postfixQuestionMark
    } else {
        terms::infixQuestionMark
    };
    if stack.can_shift(term) {
        input.advance(1);
        input.accept_token(term)?;
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn scan(input: &mut InputStream, stack: &Stack, first: u32) -> Result<(), ParseError> {
    debug_assert!(is_operator_start(first));
    if starts_comment(first, peek(input, 1)) {
        return Ok(());
    }
    if is_generated_only_operator(input, first) {
        return Ok(());
    }

    // SwiftSyntax splits the trailing `<` from a function-operator token when
    // it starts a generic parameter clause (`func *<let N: Int>`). The
    // function-only custom-operator terminal is available solely in
    // FunctionName. Expression operator references use a different term, so
    // they cannot accidentally split a trailing `<` as a generic clause.
    // Classify the physical token before consulting the LR state. Most
    // operator spellings need only one fixity-specific capability probe; the
    // extra prefix/binary probes are reserved for the rare regex split path.
    let left_bound = is_left_bound(input);
    let full_operator_shape = operator_token_shape(input, left_bound);
    let function_operator_prefix_width = if full_operator_shape.length >= 2
        && full_operator_shape.last == LEFT_ANGLE
        && stack.can_shift(terms::functionCustomOperator)
    {
        function_operator_generic_prefix_width(input, full_operator_shape)
    } else {
        None
    };
    let regex_operator_prefix_width = literal::contextual_regex_operator_prefix_width(
        input,
        stack,
        left_bound,
        full_operator_shape.first_internal_slash,
    );
    let operator_prefix_width = [function_operator_prefix_width, regex_operator_prefix_width]
        .into_iter()
        .flatten()
        .min();
    if left_bound {
        let type_suffix = match first {
            QUESTION if stack.can_shift(terms::optionalTypeQuestionMark) => {
                Some(terms::optionalTypeQuestionMark)
            }
            EXCLAMATION if stack.can_shift(terms::typeExclamationMark) => {
                Some(terms::typeExclamationMark)
            }
            _ => None,
        };
        if let Some(term) = type_suffix {
            input.advance(1);
            input.accept_token(term)?;
            return Ok(());
        }
    }
    let prefix = full_operator_shape.prefix;
    let length = operator_prefix_width.unwrap_or(full_operator_shape.length);
    let right_bound = if length == full_operator_shape.length {
        full_operator_shape.right_bound
    } else {
        is_right_bound_at(input, length, left_bound)
    };
    let fixity = operator_fixity(left_bound, right_bound);
    input.advance(length);

    accept_operator(input, stack, prefix, length, fixity)
}

fn is_generated_only_operator(input: &InputStream, first: u32) -> bool {
    if first == EQUAL && operator_ends_at(input, 1) {
        return true;
    }
    first == MINUS && peek(input, 1) == Some(RIGHT_ANGLE) && operator_ends_at(input, 2)
}

fn operator_ends_at(input: &InputStream, offset: isize) -> bool {
    let Some(next) = peek(input, offset) else {
        return true;
    };
    next == PERIOD || starts_comment(next, peek(input, offset + 1)) || !is_operator_continue(next)
}

fn accept_operator(
    input: &mut InputStream,
    stack: &Stack,
    prefix: [u32; 3],
    length: usize,
    fixity: OperatorFixity,
) -> Result<(), ParseError> {
    if length == 1 && prefix[0] == AMPERSAND {
        // The contextual `any` prefix has already committed this state to a
        // type constraint, so its ampersand belongs to CompositionType rather
        // than the surrounding expression sequence.
        if stack.can_shift(terms::anyTypeCompositionAmpersand) {
            input.accept_token(terms::anyTypeCompositionAmpersand)?;
            return Ok(());
        }
        if stack.can_shift(terms::functionCustomOperator) {
            input.accept_token(terms::functionCustomOperator)?;
            return Ok(());
        }
        let term = match fixity {
            OperatorFixity::Prefix => Some(terms::prefixAmpersand),
            OperatorFixity::Binary => Some(terms::binaryAmpersand),
            OperatorFixity::Postfix => Some(terms::postfixCustomOperator),
        };
        if let Some(term) = term.filter(|term| stack.can_shift(*term)) {
            input.accept_token(term)?;
        }
        return Ok(());
    }

    if length == 1 && prefix[0] == TILDE {
        if stack.can_shift(terms::functionCustomOperator) {
            input.accept_token(terms::functionCustomOperator)?;
            return Ok(());
        }
        let term = match fixity {
            OperatorFixity::Prefix => Some(terms::prefixTilde),
            OperatorFixity::Binary => Some(terms::binaryCustomOperator),
            OperatorFixity::Postfix => Some(terms::postfixCustomOperator),
        };
        if let Some(term) = term.filter(|term| stack.can_shift(*term)) {
            input.accept_token(term)?;
        }
        return Ok(());
    }

    if is_range_operator(prefix, length) {
        let term = match fixity {
            OperatorFixity::Prefix => terms::prefixRangeOperator,
            OperatorFixity::Binary => terms::binaryRangeOperator,
            OperatorFixity::Postfix => terms::postfixRangeOperator,
        };
        if stack.can_shift(term) {
            input.accept_token(term)?;
        } else if stack.can_shift(terms::binaryRangeOperator) {
            input.accept_token(terms::binaryRangeOperator)?;
        }
        return Ok(());
    }

    if !operator_has_fixed_role(prefix, length, fixity) {
        let fixity_term = match fixity {
            OperatorFixity::Prefix => terms::prefixCustomOperator,
            OperatorFixity::Binary => terms::binaryCustomOperator,
            OperatorFixity::Postfix => terms::postfixCustomOperator,
        };
        if stack.can_shift(fixity_term) {
            input.accept_token(fixity_term)?;
        } else if !is_reserved_operator(prefix, length)
            && stack.can_shift(terms::functionCustomOperator)
        {
            input.accept_token(terms::functionCustomOperator)?;
        } else if !is_reserved_operator(prefix, length) && stack.can_shift(terms::customOperator) {
            input.accept_token(terms::customOperator)?;
        }
    }
    Ok(())
}

pub(super) fn starts_binary_operator_like_continuation(
    first: u32,
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    can_shift_custom_binary: bool,
) -> bool {
    let starts_with_period = first == PERIOD;
    let mut prefix = [0_u32; 3];
    prefix[0] = first;
    let mut length = 1_usize;
    while let Some(next) = input.peek().copied() {
        if next == PERIOD && !starts_with_period {
            break;
        }
        if !is_operator_continue(next) {
            break;
        }
        input.next();
        if starts_comment(next, input.peek().copied()) {
            return is_binary_operator_like_continuation(
                prefix,
                length,
                true,
                can_shift_custom_binary,
            );
        }
        if length < prefix.len() {
            prefix[length] = next;
        }
        length += 1;
    }
    is_binary_operator_like_continuation(
        prefix,
        length,
        operator_has_right_bound(input),
        can_shift_custom_binary,
    )
}

fn function_operator_generic_prefix_width(
    input: &InputStream,
    shape: OperatorShape,
) -> Option<usize> {
    debug_assert!(shape.length >= 2 && shape.last == LEFT_ANGLE);

    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    for _ in 0..shape.length {
        lookahead.next();
    }
    skip_trivia(&mut lookahead);
    scan_lookahead_identifier(&mut lookahead)
        .filter(LookaheadIdentifier::is_generic_parameter_start)
        .map(|_| shape.length - 1)
}

fn question_mark_continues_operator(input: &InputStream) -> bool {
    let Some(next) = peek(input, 1) else {
        return false;
    };
    is_operator_continue(next) && !starts_comment(next, peek(input, 2))
}

fn is_left_bound(input: &InputStream) -> bool {
    match previous(input) {
        None
        | Some(
            0x00A0 | 0x0009 | 0x000A | 0x000D | 0x0020 | LEFT_PAREN | LEFT_BRACKET | LEFT_BRACE
            | COMMA | SEMICOLON | COLON,
        ) => false,
        Some(SLASH) if peek(input, -2) == Some(STAR) => false,
        Some(_) => true,
    }
}

#[derive(Clone, Copy)]
struct OperatorShape {
    prefix: [u32; 3],
    length: usize,
    last: u32,
    first_internal_slash: Option<usize>,
    right_bound: bool,
}

fn operator_token_shape(input: &InputStream, left_bound: bool) -> OperatorShape {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    let starts_with_period = lookahead.peek().copied() == Some(PERIOD);
    let mut prefix = [0_u32; 3];
    let mut length = 0_usize;
    let mut last = 0_u32;
    let mut first_internal_slash = None;
    let boundary = loop {
        let Some(next) = lookahead.next() else {
            break None;
        };
        if length > 0 && next == PERIOD && !starts_with_period {
            break Some((next, None));
        }
        if length > 0 && starts_comment(next, lookahead.peek().copied()) {
            break Some((next, lookahead.peek().copied()));
        }
        let valid = if length == 0 {
            is_operator_start(next)
        } else {
            is_operator_continue(next)
        };
        if !valid {
            break Some((next, lookahead.peek().copied()));
        }
        if length > 0 && next == SLASH && first_internal_slash.is_none() {
            first_internal_slash = Some(length);
        }
        if length < prefix.len() {
            prefix[length] = next;
        }
        last = next;
        length += 1;
    };
    let right_bound = boundary
        .is_some_and(|(next, following)| boundary_has_right_bound(next, following, left_bound));
    OperatorShape {
        prefix,
        length,
        last,
        first_internal_slash,
        right_bound,
    }
}

pub(super) fn operator_has_fixed_role(
    prefix: [u32; 3],
    length: usize,
    fixity: OperatorFixity,
) -> bool {
    // These spellings already have a literal grammar term in the indicated
    // role. Other fixities remain contextual operators, just as SwiftSyntax's
    // `classifyOperatorToken` classifies them from surrounding boundness.
    let [first, second, _] = prefix;
    let fixed_in_every_position = (length == 1 && matches!(first, EQUAL | PERIOD | QUESTION))
        || (length == 2 && matches!((first, second), (MINUS, RIGHT_ANGLE) | (STAR, SLASH)));
    if fixed_in_every_position {
        return true;
    }

    match fixity {
        OperatorFixity::Prefix => length == 1 && matches!(first, EXCLAMATION | MINUS | PLUS),
        OperatorFixity::Binary => {
            let fixed_single = length == 1
                && matches!(
                    first,
                    EQUAL | MINUS | PLUS | STAR | SLASH | PERCENT | LEFT_ANGLE | RIGHT_ANGLE
                );
            let fixed_double = length == 2
                && matches!(
                    (first, second),
                    (EQUAL | EXCLAMATION | LEFT_ANGLE | RIGHT_ANGLE, EQUAL)
                        | (AMPERSAND, AMPERSAND)
                        | (PIPE, PIPE)
                        | (QUESTION, QUESTION)
                );
            fixed_single || fixed_double
        }
        OperatorFixity::Postfix => length == 1 && matches!(first, EXCLAMATION | QUESTION),
    }
}

fn is_range_operator(prefix: [u32; 3], length: usize) -> bool {
    length == 3 && matches!(prefix, [PERIOD, PERIOD, PERIOD | LEFT_ANGLE])
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum OperatorFixity {
    Binary,
    Prefix,
    Postfix,
}

fn operator_fixity(left_bound: bool, right_bound: bool) -> OperatorFixity {
    match (left_bound, right_bound) {
        (false, true) => OperatorFixity::Prefix,
        (true, false) => OperatorFixity::Postfix,
        _ => OperatorFixity::Binary,
    }
}

fn is_right_bound_at(input: &InputStream, offset: usize, left_bound: bool) -> bool {
    let Ok(offset) = isize::try_from(offset) else {
        return false;
    };
    let Some(next) = peek(input, offset) else {
        return false;
    };
    boundary_has_right_bound(next, peek(input, offset + 1), left_bound)
}

fn boundary_has_right_bound(next: u32, following: Option<u32>, left_bound: bool) -> bool {
    match next {
        0x00A0 | 0x0009 | 0x000A | 0x000D | 0x0020 | RIGHT_PAREN | RIGHT_BRACKET | RIGHT_BRACE
        | COMMA | SEMICOLON | COLON => false,
        PERIOD => !left_bound,
        SLASH if matches!(following, Some(SLASH | STAR)) => false,
        _ => true,
    }
}

fn is_reserved_operator(prefix: [u32; 3], length: usize) -> bool {
    let [first, second, third] = prefix;
    let reserved_single = length == 1
        && matches!(
            first,
            SLASH
                | EQUAL
                | MINUS
                | PLUS
                | STAR
                | PERCENT
                | LEFT_ANGLE
                | RIGHT_ANGLE
                | EXCLAMATION
                | AMPERSAND
                | PERIOD
                | QUESTION
        );
    let reserved_double = length == 2
        && matches!(
            (first, second),
            (MINUS, RIGHT_ANGLE)
                | (EQUAL | EXCLAMATION | LEFT_ANGLE | RIGHT_ANGLE, EQUAL)
                | (AMPERSAND, AMPERSAND)
                | (PIPE, PIPE)
                | (QUESTION, QUESTION)
        );
    let reserved_ellipsis = length == 3 && [first, second, third] == [PERIOD; 3];
    reserved_single || reserved_double || reserved_ellipsis
}

fn is_binary_operator_like_continuation(
    prefix: [u32; 3],
    length: usize,
    has_right_bound: bool,
    can_shift_custom_binary: bool,
) -> bool {
    // SwiftSyntax's `BinaryOperatorLike` includes `infixQuestionMark`, and
    // `parseSequenceExpressionOperator` accepts it at line start. Unlike
    // ordinary binary operators, a single `?` remains a ternary delimiter
    // without a right bound, including before comment trivia or `?then`.
    if length == 1 && prefix[0] == QUESTION {
        return true;
    }
    let is_binary = if is_reserved_operator(prefix, length) {
        is_binary_operator(prefix, length)
    } else {
        can_shift_custom_binary
    };
    is_binary && has_right_bound
}

fn is_binary_operator(prefix: [u32; 3], length: usize) -> bool {
    if !is_reserved_operator(prefix, length) {
        return true;
    }
    let [first, second, _] = prefix;
    let fixed_single = length == 1
        && matches!(
            first,
            EQUAL | MINUS | PLUS | STAR | SLASH | PERCENT | LEFT_ANGLE | RIGHT_ANGLE | AMPERSAND
        );
    let fixed_double = length == 2
        && matches!(
            (first, second),
            (EQUAL | EXCLAMATION | LEFT_ANGLE | RIGHT_ANGLE, EQUAL)
                | (AMPERSAND, AMPERSAND)
                | (PIPE, PIPE)
                | (QUESTION, QUESTION)
        );
    fixed_single || fixed_double
}

fn operator_has_right_bound(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    matches!(
        input.peek().copied(),
        None | Some(
            9 | 10
                | 11
                | 12
                | 13
                | 32
                | RIGHT_PAREN
                | RIGHT_BRACKET
                | RIGHT_BRACE
                | COMMA
                | SEMICOLON
                | COLON
        )
    )
}
