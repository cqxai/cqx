//! Recognition of Swift closure signatures.
//!
//! `SwiftSyntax` speculatively scans a closure signature before parsing it. A
//! signature must start with attributes, a capture/parameter clause, or a
//! shorthand parameter, and must reach a top-level `in`. Keeping that decision
//! outside the LR grammar prevents a parenthesized closure body from starting a
//! second parse stack merely because it shares the signature's `(` prefix.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{is_identifier_start, scan_lookahead_identifier};
use crate::terms;

use super::lookahead::{skip_quoted_text, skip_trivia};

const AT_SIGN: u32 = b'@' as u32;
const BACKTICK: u32 = b'`' as u32;
const LEFT_BRACE: u32 = b'{' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const RIGHT_BRACE: u32 = b'}' as u32;
const RIGHT_BRACKET: u32 = b']' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const SEMICOLON: u32 = b';' as u32;
const DOUBLE_QUOTE: u32 = b'"' as u32;

pub(super) fn scan(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    if starts(input) {
        input.accept_token(terms::closureSignatureLookahead)?;
    }
    Ok(())
}

fn starts(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_from(&mut lookahead)
}

fn starts_from(lookahead: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    skip_trivia(lookahead);

    match lookahead.peek().copied() {
        Some(AT_SIGN | LEFT_PAREN | LEFT_BRACKET) => {}
        Some(first) if first == BACKTICK || is_identifier_start(first) => {
            let Some(word) = scan_lookahead_identifier(lookahead) else {
                return false;
            };
            if !word.is_pattern_binding_target() {
                return false;
            }
        }
        _ => return false,
    }

    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    loop {
        skip_trivia(lookahead);
        let Some(next) = lookahead.peek().copied() else {
            return false;
        };
        let at_top_level = paren_depth == 0 && bracket_depth == 0;

        if next == BACKTICK || is_identifier_start(next) {
            let Some(word) = scan_lookahead_identifier(lookahead) else {
                return false;
            };
            if at_top_level && word.is(b"in") {
                return true;
            }
            continue;
        }

        lookahead.next();
        match next {
            DOUBLE_QUOTE if !skip_quoted_text(lookahead) => return false,
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
            LEFT_BRACE | RIGHT_BRACE | SEMICOLON if at_top_level => return false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::starts_from;

    fn starts(source: &str) -> bool {
        starts_from(&mut source.chars().map(u32::from).peekable())
    }

    fn counted_code_points<'a>(
        source: &'a str,
        inspected: &'a Cell<usize>,
    ) -> impl Iterator<Item = u32> + 'a {
        source.chars().map(u32::from).inspect(move |_| {
            inspected.set(inspected.get() + 1);
        })
    }

    #[test]
    fn signatures_require_a_complete_top_level_in_boundary() {
        let cases = [
            ("parenthesized", "(value: Int) async throws -> Int in", true),
            ("shorthand", "value, other in", true),
            ("capture", "[weak self] value in", true),
            ("attribute", "@MainActor (value: Int) in", true),
            ("dollar tuple body", "($0, $1)", false),
            ("ordinary tuple body", "(value, other)", false),
            ("implicit parameter body", "$0.value", false),
            ("collection body", "[value, other]", false),
            ("keyword body", "if condition", false),
            (
                "nested closure body",
                "values.map { value in value }",
                false,
            ),
        ];

        for (name, source, expected) in cases {
            assert_eq!(starts(source), expected, "{name}: {source}");
        }
    }

    #[test]
    fn deep_signature_does_not_scan_the_remaining_source() {
        const DEPTH: usize = 128;
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let prefix = format!("{}value{} in", "(".repeat(DEPTH), ")".repeat(DEPTH));
        let source = format!("{prefix}{}", " sentinel".repeat(4_096));
        let inspected = Cell::new(0usize);
        let input = counted_code_points(&source, &inspected);

        assert!(starts_from(&mut input.peekable()));
        assert!(
            inspected.get() <= prefix.len() + ALLOWED_BOUNDARY_READS,
            "closure signature inspected {} code points for a {}-point prefix",
            inspected.get(),
            prefix.len(),
        );
    }
}
