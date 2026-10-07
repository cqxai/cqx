//! Source-only recognition of compound declaration names and module selectors.
//!
//! Parser-state term selection remains in the identifier tokenizer.

use super::lexical::{is_identifier_start, is_operator_start, scan_lookahead_identifier};

use super::lookahead::skip_trivia;

const BACKTICK: u32 = b'`' as u32;
const COLON: u32 = b':' as u32;
const DOLLAR: u32 = b'$' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const RIGHT_PAREN: u32 = b')' as u32;

fn starts_module_selector_separator(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    skip_trivia(input);
    input.next() == Some(COLON) && input.next() == Some(COLON)
}

pub(super) fn starts_module_selector(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if !starts_module_selector_separator(input) {
        return false;
    }
    skip_trivia(input);
    match input.peek().copied() {
        Some(BACKTICK) => true,
        Some(DOLLAR) => {
            input.next();
            input.peek().copied().is_some_and(is_identifier_start)
        }
        Some(first) => is_identifier_start(first) || is_operator_start(first),
        None => false,
    }
}

pub(super) fn starts_decl_name_arguments(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if input.next() != Some(LEFT_PAREN) {
        return false;
    }

    // SwiftSyntax's `parseArgLabelList` has an early colon check, but its
    // `canParseArgumentLabelList` validation requires `atArgumentLabel()`.
    // Therefore `(:)` is not a declaration-name argument list.
    let mut has_argument = false;
    loop {
        skip_trivia(input);
        if input.peek() == Some(&RIGHT_PAREN) {
            return has_argument;
        }
        let Some(label) = scan_lookahead_identifier(input) else {
            return false;
        };
        if !label.is_argument_label() {
            return false;
        }
        skip_trivia(input);
        if input.next() != Some(COLON) {
            return false;
        }
        has_argument = true;
    }
}
