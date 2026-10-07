//! Recognition of Swift accessor and observer block starts.
//!
//! Parser-state term selection stays at the tokenizer entry. The remaining
//! helpers only inspect the source shape after an opening brace.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{LookaheadIdentifier, scan_lookahead_identifier};
use crate::terms;

use super::attribute::skip_name_tail;
use super::lookahead::{current, skip_balanced_parentheses, skip_trivia};

const AT_SIGN: u32 = b'@' as u32;
const LEFT_BRACE: u32 = b'{' as u32;
const RIGHT_BRACE: u32 = b'}' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const PERIOD: u32 = b'.' as u32;

pub(super) fn scan_initialized_property_block(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<bool, ParseError> {
    if current(input) == Some(LEFT_BRACE)
        && stack.can_shift(terms::initializedPropertyBlockLookahead)
    {
        let starts_observer_block = {
            let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
            lookahead.next();
            accessor_block_starts(&mut lookahead, true)
        };
        if starts_observer_block {
            input.accept_token(terms::initializedPropertyBlockLookahead)?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn scan_block(input: &mut InputStream, stack: &Stack) -> Result<bool, ParseError> {
    if !stack.can_shift(terms::accessorBlockLookahead) {
        return Ok(false);
    }
    let starts_with_closing_brace = {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        if lookahead.peek() == Some(&LEFT_BRACE) {
            lookahead.next();
        }
        skip_trivia(&mut lookahead);
        lookahead.peek() == Some(&RIGHT_BRACE)
    };
    let starts_accessor_block = starts_with_closing_brace || {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        if lookahead.peek() == Some(&LEFT_BRACE) {
            lookahead.next();
        }
        accessor_block_starts(&mut lookahead, false)
    };
    if starts_accessor_block {
        input.accept_token(terms::accessorBlockLookahead)?;
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn is_block_start(first: u32) -> bool {
    matches!(
        u8::try_from(first),
        Ok(b'@'
            | b'{'
            | b'_'
            | b'a'
            | b'b'
            | b'c'
            | b'd'
            | b'g'
            | b'i'
            | b'm'
            | b'n'
            | b'r'
            | b's'
            | b'u'
            | b'w'
            | b'y')
    )
}

fn accessor_block_starts(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    observers_only: bool,
) -> bool {
    let Some(has_disambiguation_marker) = scan_attribute_prefix(input) else {
        return false;
    };
    if has_disambiguation_marker {
        return true;
    }

    loop {
        let Some(word) = scan_lookahead_identifier(input) else {
            return false;
        };
        if is_modifier(&word) {
            skip_trivia(input);
            continue;
        }
        return if observers_only {
            is_observer_specifier(&word)
        } else {
            is_specifier(&word)
        };
    }
}

fn scan_attribute_prefix(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<bool> {
    skip_trivia(input);
    scan_attributes(input)
}

pub(super) fn scan_attributes(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<bool> {
    let mut has_disambiguation_marker = false;
    let mut is_first_attribute = true;
    while input.peek() == Some(&AT_SIGN) {
        input.next();
        let first_component = scan_lookahead_identifier(input)?;
        let has_name_tail = matches!(input.peek().copied(), Some(LEFT_ANGLE | PERIOD));
        if !skip_name_tail(input) {
            return None;
        }
        skip_trivia(input);
        let has_arguments = input.peek() == Some(&LEFT_PAREN);
        has_disambiguation_marker |= is_first_attribute
            && first_component.is(b"_accessorBlock")
            && !has_name_tail
            && !has_arguments;
        is_first_attribute = false;
        if has_arguments {
            if !skip_balanced_parentheses(input) {
                return None;
            }
            skip_trivia(input);
        }
    }
    Some(has_disambiguation_marker)
}

fn is_modifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"__consuming")
        || word.is(b"consuming")
        || word.is(b"borrowing")
        || word.is(b"mutating")
        || word.is(b"nonmutating")
        || word.is(b"yielding")
}

fn is_specifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"get")
        || word.is(b"set")
        || word.is(b"didSet")
        || word.is(b"willSet")
        || word.is(b"unsafeAddress")
        || word.is(b"addressWithOwner")
        || word.is(b"addressWithNativeOwner")
        || word.is(b"unsafeMutableAddress")
        || word.is(b"mutableAddressWithOwner")
        || word.is(b"mutableAddressWithNativeOwner")
        || word.is(b"_read")
        || word.is(b"read")
        || word.is(b"_modify")
        || word.is(b"modify")
        || word.is(b"init")
        || word.is(b"borrow")
        || word.is(b"mutate")
}

pub(super) fn is_observer_specifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"willSet") || word.is(b"didSet") || word.is(b"init")
}
