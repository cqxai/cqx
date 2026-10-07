//! Structural type recognition used at grammar boundaries.
//!
//! The recognizer advances monotonically through one candidate type and does
//! not build syntax or consult semantic state. Parser-state term selection is
//! confined to [`scan`]; reusable type-shape parsing remains source-only.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{
    LookaheadIdentifier, is_identifier_start, scan_lookahead_identifier,
    scan_lookahead_identifier_after_first,
};
use crate::terms;

use super::attribute::scan_lookahead as scan_attribute_lookahead;
use super::lookahead::{
    current, first_nontrivia_on_same_line, skip_balanced_angles, skip_trivia,
    skip_trivia_with_line_break,
};

const AT_SIGN: u32 = b'@' as u32;
const AMPERSAND: u32 = b'&' as u32;
const EXCLAMATION: u32 = b'!' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COLON: u32 = b':' as u32;
const COMMA: u32 = b',' as u32;
const MINUS: u32 = b'-' as u32;
const PERIOD: u32 = b'.' as u32;
const QUESTION: u32 = b'?' as u32;
const RIGHT_ANGLE: u32 = b'>' as u32;
const RIGHT_BRACKET: u32 = b']' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const TILDE: u32 = b'~' as u32;

pub(super) fn scan(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if current(input) == Some(LEFT_PAREN)
        && stack.can_shift(terms::nonisolatedSpecifierArgumentLookahead)
    {
        let starts_argument = {
            let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
            parse_nonisolated_argument_body(&mut lookahead).is_some()
        };
        if starts_argument {
            input.accept_token(terms::nonisolatedSpecifierArgumentLookahead)?;
            return Ok(());
        }
    }

    if !stack.can_shift(terms::attributedTypeExpressionLookahead) {
        return Ok(());
    }
    let Some(first) = current(input) else {
        return Ok(());
    };
    if first != AT_SIGN && first != u32::from(b'i') && first != u32::from(b'n') {
        return Ok(());
    }
    if first == u32::from(b'i') && stack.can_shift(terms::inout) {
        return Ok(());
    }

    let starts_type_expression = {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        starts_attributed_type_expression(&mut lookahead)
    };
    if starts_type_expression {
        input.accept_token(terms::attributedTypeExpressionLookahead)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct TypeLookaheadShape {
    is_bare_tuple: bool,
}

fn starts_attributed_type_expression(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    skip_trivia(input);
    let initial_word = scan_lookahead_identifier(input);
    parse_type_scalar(input, initial_word, true).is_some()
}

pub(super) fn parse(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    skip_trivia(input);
    let initial_word = scan_lookahead_identifier(input);
    if initial_word.as_ref().is_some_and(|word| word.is(b"repeat")) {
        skip_trivia(input);
        return parse_type_scalar(input, None, false).is_some();
    }
    parse_type_scalar(input, initial_word, false).is_some()
}

pub(super) fn finish_parenthesized(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    consume_type_suffixes(input);
    parse_function_type_tail(input)
}

// SwiftSyntax's `parseQualifiedTypeIdentifier` only consumes another type
// component when `canParseBaseTypeForQualifiedDeclName` finds a later period.
// This role is narrower than an ordinary MemberType: lowercase `self`, `Any`
// without a module selector, wildcard, and lexer-classified keywords remain
// available to the final declaration-name parser instead.
pub(super) fn starts_qualified_decl_type_member(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if input.next() != Some(PERIOD) {
        return false;
    }
    let Some(first) = first_nontrivia_on_same_line(input) else {
        return false;
    };
    let Some(first_name) = scan_lookahead_identifier_after_first(first, input) else {
        return false;
    };
    skip_trivia(input);

    if input.peek() == Some(&COLON) {
        if !first_name.is_identifier() {
            return false;
        }
        input.next();
        if input.next() != Some(COLON) {
            return false;
        }
        let Some(first) = first_nontrivia_on_same_line(input) else {
            return false;
        };
        let Some(selected_name) = scan_lookahead_identifier_after_first(first, input) else {
            return false;
        };
        if !is_qualified_decl_module_selected_name(&selected_name) {
            return false;
        }
    } else if !is_qualified_decl_type_name(&first_name) {
        return false;
    }

    skip_trivia(input);
    if input.peek() == Some(&LEFT_ANGLE) && !skip_balanced_angles(input) {
        return false;
    }
    skip_trivia(input);
    input.peek() == Some(&PERIOD)
}

fn is_qualified_decl_type_name(word: &LookaheadIdentifier) -> bool {
    word.is(b"Self") || word.is_identifier()
}

fn is_qualified_decl_module_selected_name(word: &LookaheadIdentifier) -> bool {
    word.is(b"Self") || word.is(b"Any") || word.is_identifier()
}

fn is_member_identifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"self") || word.is(b"Self") || !word.is_lexer_keyword()
}

fn parse_type_scalar(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: Option<LookaheadIdentifier>,
    require_expression_prefix: bool,
) -> Option<TypeLookaheadShape> {
    let mut pending_word = initial_word;
    let mut first_prefix = true;
    let mut expression_prefix = false;

    let base_word = loop {
        if input.peek() == Some(&AT_SIGN) {
            if !scan_attribute_lookahead(input) {
                return None;
            }
            if first_prefix {
                expression_prefix = true;
            }
            first_prefix = false;
            skip_trivia(input);
            pending_word = scan_lookahead_identifier(input);
            continue;
        }

        let word = if let Some(word) = pending_word.take() {
            word
        } else {
            let Some(word) = scan_lookahead_identifier(input) else {
                break None;
            };
            word
        };
        if !is_type_specifier(&word) {
            break Some(word);
        }

        let nonisolated_argument = if word.is(b"nonisolated") {
            parse_nonisolated_specifier_argument(input)?
        } else {
            false
        };
        if first_prefix {
            expression_prefix = word.is(b"inout") || nonisolated_argument;
        }
        first_prefix = false;
        skip_trivia(input);
        pending_word = scan_lookahead_identifier(input);
    };

    if require_expression_prefix && !expression_prefix {
        return None;
    }

    let shape = parse_type_composition(input, base_word)?;
    if !shape.is_bare_tuple {
        return Some(shape);
    }
    parse_function_type_tail(input).then_some(TypeLookaheadShape {
        is_bare_tuple: false,
    })
}

fn parse_nonisolated_specifier_argument(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<bool> {
    let trivia_has_line_break = skip_trivia_with_line_break(input)?;
    if trivia_has_line_break || input.peek() != Some(&LEFT_PAREN) {
        return Some(false);
    }
    parse_nonisolated_argument_body(input)
}

fn parse_nonisolated_argument_body(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<bool> {
    (input.next() == Some(LEFT_PAREN)).then_some(())?;
    if skip_trivia_with_line_break(input)? {
        return None;
    }
    let argument = scan_lookahead_identifier(input)?;
    if !argument.is(b"nonsending") {
        return None;
    }
    if skip_trivia_with_line_break(input)? || input.next() != Some(RIGHT_PAREN) {
        return None;
    }
    Some(true)
}

fn is_type_specifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"inout")
        || word.is(b"borrowing")
        || word.is(b"consuming")
        || word.is(b"sending")
        || word.is(b"isolated")
        || word.is(b"nonisolated")
        || word.is(b"__shared")
        || word.is(b"__owned")
}

fn parse_type_composition(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: Option<LookaheadIdentifier>,
) -> Option<TypeLookaheadShape> {
    let mut shape = parse_type_element(input, initial_word)?;
    loop {
        skip_trivia(input);
        if input.peek() != Some(&AMPERSAND) {
            return Some(shape);
        }
        input.next();
        if input.peek() == Some(&AMPERSAND) {
            return Some(shape);
        }
        skip_trivia(input);
        shape = parse_type_element(input, None)?;
        shape.is_bare_tuple = false;
    }
}

fn parse_type_element(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: Option<LookaheadIdentifier>,
) -> Option<TypeLookaheadShape> {
    if initial_word.as_ref().is_some_and(|word| word.is(b"each")) {
        parse_type_suffix(input, None)?;
        return Some(TypeLookaheadShape {
            is_bare_tuple: false,
        });
    }

    if initial_word
        .as_ref()
        .is_some_and(|word| word.is(b"some") || word.is(b"any"))
    {
        parse_some_or_any_constraint(input)?;
        consume_type_suffixes(input);
        return Some(TypeLookaheadShape {
            is_bare_tuple: false,
        });
    }

    parse_type_suffix(input, initial_word)
}

fn parse_some_or_any_constraint(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<()> {
    parse_simple_type(input, None)?;
    loop {
        skip_trivia(input);
        if input.peek() != Some(&AMPERSAND) {
            return Some(());
        }
        input.next();
        if input.peek() == Some(&AMPERSAND) {
            return Some(());
        }
        skip_trivia(input);
        parse_simple_type(input, None)?;
    }
}

fn parse_type_suffix(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: Option<LookaheadIdentifier>,
) -> Option<TypeLookaheadShape> {
    let mut shape = parse_simple_type(input, initial_word)?;
    if consume_type_suffixes(input) {
        shape.is_bare_tuple = false;
    }
    Some(shape)
}

fn consume_type_suffixes(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    let mut consumed = false;
    loop {
        skip_trivia(input);
        if !matches!(input.peek().copied(), Some(QUESTION | EXCLAMATION)) {
            return consumed;
        }
        input.next();
        consumed = true;
    }
}

fn parse_simple_type(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: Option<LookaheadIdentifier>,
) -> Option<TypeLookaheadShape> {
    if let Some(word) = initial_word {
        return parse_identifier_type(input, word);
    }

    skip_trivia(input);
    if let Some(word) = scan_lookahead_identifier(input) {
        return parse_identifier_type(input, word);
    }
    match input.next()? {
        TILDE => {
            parse_simple_type(input, None)?;
            Some(TypeLookaheadShape {
                is_bare_tuple: false,
            })
        }
        LEFT_BRACKET => {
            parse_collection_type(input)?;
            Some(TypeLookaheadShape {
                is_bare_tuple: false,
            })
        }
        LEFT_PAREN => {
            parse_tuple_type(input)?;
            Some(TypeLookaheadShape {
                is_bare_tuple: true,
            })
        }
        _ => None,
    }
}

fn parse_identifier_type(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    mut word: LookaheadIdentifier,
) -> Option<TypeLookaheadShape> {
    if !is_root_type_identifier(&word) {
        return None;
    }

    skip_trivia(input);
    if input.peek() == Some(&LEFT_ANGLE) && !skip_balanced_angles(input) {
        return None;
    }

    loop {
        skip_trivia(input);
        if input.peek() != Some(&PERIOD) {
            break;
        }
        input.next();
        if input.peek() == Some(&PERIOD) {
            input.next();
            if input.next() != Some(PERIOD) {
                return None;
            }
            break;
        }

        skip_trivia(input);
        word = scan_lookahead_identifier(input)?;
        if !is_member_identifier(&word) {
            return None;
        }
        skip_trivia(input);
        if input.peek() == Some(&LEFT_ANGLE) && !skip_balanced_angles(input) {
            return None;
        }
    }

    Some(TypeLookaheadShape {
        is_bare_tuple: false,
    })
}

fn is_root_type_identifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"_") || word.is(b"Any") || word.is(b"Self") || !word.is_lexer_keyword()
}

fn parse_collection_type(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> Option<()> {
    if !parse(input) {
        return None;
    }
    skip_trivia(input);
    if input.peek() == Some(&COLON) {
        input.next();
        if !parse(input) {
            return None;
        }
        skip_trivia(input);
    }
    (input.next() == Some(RIGHT_BRACKET)).then_some(())
}

fn parse_tuple_type(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> Option<()> {
    skip_trivia(input);
    if input.peek() == Some(&RIGHT_PAREN) {
        input.next();
        return Some(());
    }

    loop {
        parse_tuple_type_element(input)?;
        skip_trivia(input);
        if input.peek() != Some(&COMMA) {
            break;
        }
        input.next();
        skip_trivia(input);
        if input.peek() == Some(&RIGHT_PAREN) {
            break;
        }
    }
    (input.next() == Some(RIGHT_PAREN)).then_some(())
}

fn parse_tuple_type_element(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<()> {
    skip_trivia(input);
    let Some(first_word) = scan_lookahead_identifier(input) else {
        return parse(input).then_some(());
    };

    if is_type_specifier(&first_word)
        || first_word.is(b"repeat")
        || first_word.is(b"some")
        || first_word.is(b"any")
        || first_word.is(b"each")
    {
        return parse_type_from_initial_word(input, first_word).then_some(());
    }

    skip_trivia(input);
    if input.peek() == Some(&COLON) {
        input.next();
        return parse(input).then_some(());
    }
    if input.peek().copied().is_some_and(is_identifier_start) {
        scan_lookahead_identifier(input)?;
        skip_trivia(input);
        if input.next() != Some(COLON) {
            return None;
        }
        return parse(input).then_some(());
    }

    parse_type_from_initial_word(input, first_word).then_some(())
}

fn parse_type_from_initial_word(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    initial_word: LookaheadIdentifier,
) -> bool {
    if initial_word.is(b"repeat") {
        return parse_type_scalar(input, None, false).is_some();
    }
    parse_type_scalar(input, Some(initial_word), false).is_some()
}

fn parse_function_type_tail(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    loop {
        skip_trivia(input);
        if input.peek() == Some(&MINUS) {
            input.next();
            if input.next() != Some(RIGHT_ANGLE) {
                return true;
            }
            return parse(input);
        }

        let Some(effect) = scan_lookahead_identifier(input) else {
            return true;
        };
        if effect.is(b"async") || effect.is(b"rethrows") {
            continue;
        }
        if !effect.is(b"throws") {
            return true;
        }

        skip_trivia(input);
        if input.peek() != Some(&LEFT_PAREN) {
            continue;
        }
        input.next();
        if !parse(input) {
            return false;
        }
        skip_trivia(input);
        if input.next() != Some(RIGHT_PAREN) {
            return false;
        }
    }
}
