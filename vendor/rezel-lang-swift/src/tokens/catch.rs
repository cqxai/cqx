//! Recognition of catch patterns that require the matching-expression parser.
//!
//! Basic declaration patterns stay on the compact parser path. This scanner
//! emits a phase marker only when the remaining catch item needs the shared
//! matching-expression automaton.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::InputStream;

use super::lexical::{LookaheadIdentifier, scan_lookahead_identifier};
use crate::terms;

use super::lookahead::skip_trivia_with_line_break;

const LEFT_BRACE: u32 = b'{' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COMMA: u32 = b',' as u32;
const RIGHT_PAREN: u32 = b')' as u32;

pub(super) fn scan(input: &mut InputStream) -> Result<(), ParseError> {
    let requires_matching = {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        catch_pattern_requires_matching(&mut lookahead)
    };
    if requires_matching {
        input.accept_token(terms::catchPatternLookahead)?;
    }
    Ok(())
}

fn catch_pattern_requires_matching(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if skip_trivia_with_line_break(input).is_none() {
        return true;
    }
    if matches!(input.peek(), None | Some(&LEFT_BRACE)) {
        return false;
    }
    if input.peek() == Some(&u32::from(b'w')) {
        let Some(word) = scan_lookahead_identifier(input) else {
            return true;
        };
        if word.is(b"where") {
            return false;
        }
        return !word.is_pattern_binding_target() || !basic_catch_pattern_ends_here(input);
    }
    if !parse_basic_catch_pattern(input) {
        return true;
    }
    !basic_catch_pattern_ends_here(input)
}

fn parse_basic_catch_pattern(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if skip_trivia_with_line_break(input).is_none() {
        return false;
    }
    if let Some(word) = scan_lookahead_identifier(input) {
        if is_pattern_binding_specifier(&word) {
            return parse_basic_catch_pattern(input);
        }
        return word.is_pattern_binding_target();
    }
    if input.next() != Some(LEFT_PAREN) {
        return false;
    }
    if skip_trivia_with_line_break(input).is_none() {
        return false;
    }
    if input.next_if_eq(&RIGHT_PAREN).is_some() {
        return true;
    }
    loop {
        if !parse_basic_catch_pattern(input) {
            return false;
        }
        if skip_trivia_with_line_break(input).is_none() {
            return false;
        }
        match input.next() {
            Some(RIGHT_PAREN) => return true,
            Some(COMMA) => {
                if skip_trivia_with_line_break(input).is_none() {
                    return false;
                }
                if input.next_if_eq(&RIGHT_PAREN).is_some() {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

fn is_pattern_binding_specifier(word: &LookaheadIdentifier) -> bool {
    word.is(b"let")
        || word.is(b"var")
        || word.is(b"inout")
        || word.is(b"_mutating")
        || word.is(b"_borrowing")
        || word.is(b"_consuming")
        || word.is(b"borrowing")
}

fn basic_catch_pattern_ends_here(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if skip_trivia_with_line_break(input).is_none() {
        return false;
    }
    match input.peek().copied() {
        None | Some(LEFT_BRACE | COMMA) => true,
        Some(first) if first == u32::from(b'w') => {
            scan_lookahead_identifier(input).is_some_and(|word| word.is(b"where"))
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::catch_pattern_requires_matching;

    fn counted_code_points<'a>(
        source: &'a str,
        inspected: &'a Cell<usize>,
    ) -> impl Iterator<Item = u32> + 'a {
        source.chars().map(u32::from).inspect(move |_| {
            inspected.set(inspected.get() + 1);
        })
    }

    #[test]
    fn catch_pattern_boundary_matches_the_guarded_catch_entry() {
        let cases = [
            ("empty", "", false),
            ("body", " { value }", false),
            ("commented body", " /* trivia */ { value }", false),
            ("implicit where", " where condition", false),
            ("commented where", " /* trivia */ where condition", false),
            ("identifier prefix", " whereValue", false),
            ("binding", " let error", false),
            ("wildcard", " _ where true", false),
            ("tuple", " (let error, _)", false),
            ("enum case", " _MergeError.keyCollision", true),
            ("typed binding", " let error as Error", true),
            ("is type", " is Error", true),
            ("regex", " /error/", true),
            ("nested member", " (let error, E.member)", true),
        ];
        for (name, source, expected) in cases {
            let mut input = source.chars().map(u32::from).peekable();
            assert_eq!(
                catch_pattern_requires_matching(&mut input),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn deep_nested_catch_patterns_do_not_scan_the_remaining_source() {
        const DEPTH: usize = 128;
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let unrelated_tail = " sentinel".repeat(4_096);
        let catch_prefix = format!("{}error{} {{", "(".repeat(DEPTH), ")".repeat(DEPTH));
        let catch_source = format!("{catch_prefix}{unrelated_tail}");
        let catch_inspected = Cell::new(0usize);
        let catch_input = counted_code_points(&catch_source, &catch_inspected);
        assert!(!catch_pattern_requires_matching(
            &mut catch_input.peekable()
        ));
        assert!(
            catch_inspected.get() <= catch_prefix.len() + ALLOWED_BOUNDARY_READS,
            "nested catch pattern inspected {} code points for a {}-point prefix",
            catch_inspected.get(),
            catch_prefix.len(),
        );
    }
}
