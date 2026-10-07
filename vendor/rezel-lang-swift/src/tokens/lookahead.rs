//! Source-only traversal shared by Swift's structural lookaheads.
//!
//! This module owns trivia, comment, quoted-text, and balanced-delimiter
//! mechanics. It deliberately has no parser stack or term-selection access.

use rezel_common::CodePoint;
use rezel_lr::InputStream;

use super::lexical::is_identifier_start;

const CARRIAGE_RETURN: u32 = b'\r' as u32;
const LINE_FEED: u32 = b'\n' as u32;
const SLASH: u32 = b'/' as u32;
const STAR: u32 = b'*' as u32;
const BACKSLASH: u32 = b'\\' as u32;
const BACKTICK: u32 = b'`' as u32;
const DOUBLE_QUOTE: u32 = b'"' as u32;
const DOLLAR: u32 = b'$' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const RIGHT_ANGLE: u32 = b'>' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const RIGHT_PAREN: u32 = b')' as u32;

pub(super) fn current(input: &InputStream) -> Option<u32> {
    input.next().map(CodePoint::as_u32)
}

pub(super) fn peek(input: &InputStream, offset: isize) -> Option<u32> {
    input.peek(offset).map(CodePoint::as_u32)
}

pub(super) fn previous(input: &InputStream) -> Option<u32> {
    input.previous().map(CodePoint::as_u32)
}

pub(super) fn starts_comment(first: u32, second: Option<u32>) -> bool {
    first == SLASH && matches!(second, Some(SLASH | STAR))
}

pub(super) fn starts_member_access_continuation(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    input.peek().copied().is_some_and(|next| {
        matches!(next, BACKTICK | DOLLAR)
            || (u32::from(b'0')..=u32::from(b'9')).contains(&next)
            || is_identifier_start(next)
    })
}

#[cfg(test)]
pub(super) fn next_nontrivia(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<u32> {
    match scan_trivia_boundary(input, || false) {
        TriviaBoundary::Complete { first, .. } => first,
        TriviaBoundary::StoppedAtLineBreak => unreachable!("the callback never stops"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TriviaBoundary {
    Complete {
        first: Option<u32>,
        has_line_break: bool,
    },
    StoppedAtLineBreak,
}

/// Finds the first non-trivia code point while retaining Swift's layout
/// boundary semantics. The callback is invoked at most once, at the first
/// physical line break or line comment (including a line comment at EOF).
pub(super) fn scan_trivia_boundary(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    mut stop_at_line_break: impl FnMut() -> bool,
) -> TriviaBoundary {
    let mut has_line_break = false;
    loop {
        let Some(next) = input.next() else {
            return TriviaBoundary::Complete {
                first: None,
                has_line_break,
            };
        };
        match next {
            LINE_FEED | CARRIAGE_RETURN => {
                if mark_line_break(&mut has_line_break, &mut stop_at_line_break) {
                    return TriviaBoundary::StoppedAtLineBreak;
                }
            }
            9 | 11 | 12 | 32 => {}
            SLASH => match input.peek().copied() {
                Some(SLASH) => {
                    // Like SwiftSyntax, a line comment ends a layout line even
                    // when EOF arrives before a physical newline.
                    if mark_line_break(&mut has_line_break, &mut stop_at_line_break) {
                        return TriviaBoundary::StoppedAtLineBreak;
                    }
                    skip_line_comment(input);
                }
                Some(STAR) => {
                    let block = scan_block_comment(input, || {
                        mark_line_break(&mut has_line_break, &mut stop_at_line_break)
                    });
                    if block == BlockCommentScan::StoppedAtLineBreak {
                        return TriviaBoundary::StoppedAtLineBreak;
                    }
                    has_line_break |= block == BlockCommentScan::CompleteWithLineBreak;
                }
                _ => {
                    return TriviaBoundary::Complete {
                        first: Some(SLASH),
                        has_line_break,
                    };
                }
            },
            first => {
                return TriviaBoundary::Complete {
                    first: Some(first),
                    has_line_break,
                };
            }
        }
    }
}

fn mark_line_break(
    has_line_break: &mut bool,
    stop_at_line_break: &mut impl FnMut() -> bool,
) -> bool {
    if *has_line_break {
        return false;
    }
    *has_line_break = true;
    stop_at_line_break()
}

pub(super) fn skip_trivia(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) {
    let _ = skip_trivia_with_line_break(input);
}

/// Returns the first non-trivia code point only when all preceding trivia is
/// on the current physical line. A line boundary rejects immediately, without
/// scanning the remaining comment or indentation.
pub(super) fn first_nontrivia_on_same_line(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<u32> {
    match scan_trivia_boundary(input, || true) {
        TriviaBoundary::Complete {
            first: Some(first),
            has_line_break: false,
        } => Some(first),
        TriviaBoundary::Complete { .. } | TriviaBoundary::StoppedAtLineBreak => None,
    }
}

pub(super) fn skip_trivia_with_line_break(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<bool> {
    let mut has_line_break = false;
    loop {
        match input.peek().copied() {
            Some(LINE_FEED | CARRIAGE_RETURN) => {
                input.next();
                has_line_break = true;
            }
            Some(9 | 11 | 12 | 32) => {
                input.next();
            }
            Some(SLASH) => {
                input.next();
                match input.peek().copied() {
                    Some(SLASH) => has_line_break |= skip_line_comment(input),
                    Some(STAR) => has_line_break |= skip_block_comment(input),
                    _ => return None,
                }
            }
            _ => return Some(has_line_break),
        }
    }
}

pub(super) fn skip_line_comment(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    for next in input.by_ref() {
        if matches!(next, LINE_FEED | CARRIAGE_RETURN) {
            return true;
        }
    }
    false
}

pub(super) fn skip_block_comment(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    match scan_block_comment(input, || false) {
        BlockCommentScan::Complete => false,
        BlockCommentScan::CompleteWithLineBreak => true,
        BlockCommentScan::StoppedAtLineBreak => unreachable!("the callback never stops"),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BlockCommentScan {
    Complete,
    CompleteWithLineBreak,
    StoppedAtLineBreak,
}

fn scan_block_comment(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    mut stop_at_line_break: impl FnMut() -> bool,
) -> BlockCommentScan {
    input.next();
    let mut depth = 1usize;
    let mut has_line_break = false;
    while let Some(next) = input.next() {
        if matches!(next, LINE_FEED | CARRIAGE_RETURN) && !has_line_break {
            has_line_break = true;
            if stop_at_line_break() {
                return BlockCommentScan::StoppedAtLineBreak;
            }
        }
        if next == SLASH && input.peek() == Some(&STAR) {
            input.next();
            depth += 1;
            continue;
        }
        if next == STAR && input.peek() == Some(&SLASH) {
            input.next();
            depth -= 1;
            if depth == 0 {
                return if has_line_break {
                    BlockCommentScan::CompleteWithLineBreak
                } else {
                    BlockCommentScan::Complete
                };
            }
        }
    }
    if has_line_break {
        BlockCommentScan::CompleteWithLineBreak
    } else {
        BlockCommentScan::Complete
    }
}

pub(super) fn skip_directive_line(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    while let Some(next) = input.next() {
        match next {
            LINE_FEED | CARRIAGE_RETURN => return true,
            SLASH if input.peek() == Some(&SLASH) => return skip_line_comment(input),
            SLASH if input.peek() == Some(&STAR) && skip_block_comment(input) => return true,
            DOUBLE_QUOTE if !skip_quoted_text(input) => return false,
            _ => {}
        }
    }
    false
}

pub(super) fn trivia_contains_line_break(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    loop {
        let Some(next) = input.next() else {
            return false;
        };
        match next {
            LINE_FEED | CARRIAGE_RETURN => return true,
            9 | 11 | 12 | 32 => {}
            SLASH if input.peek() == Some(&SLASH) => return true,
            SLASH if input.peek() == Some(&STAR) => {
                if scan_block_comment(input, || true) == BlockCommentScan::StoppedAtLineBreak {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

pub(super) fn skip_quoted_text(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    let multiline = if input.peek() == Some(&DOUBLE_QUOTE) {
        input.next();
        if input.peek() == Some(&DOUBLE_QUOTE) {
            input.next();
            true
        } else {
            return true;
        }
    } else {
        false
    };
    while let Some(next) = input.next() {
        if next == BACKSLASH {
            input.next();
            continue;
        }
        if next == DOUBLE_QUOTE {
            if !multiline {
                return true;
            }
            if input.peek() == Some(&DOUBLE_QUOTE) {
                input.next();
                if input.peek() == Some(&DOUBLE_QUOTE) {
                    input.next();
                    return true;
                }
            }
        }
    }
    false
}

pub(super) fn skip_balanced_angles(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    skip_balanced(input, LEFT_ANGLE, RIGHT_ANGLE)
}

pub(super) fn skip_balanced_parentheses(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    skip_balanced(input, LEFT_PAREN, RIGHT_PAREN)
}

fn skip_balanced(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    open: u32,
    close: u32,
) -> bool {
    input.next();
    let mut depth = 1usize;
    while let Some(next) = input.next() {
        match next {
            SLASH if input.peek() == Some(&SLASH) => {
                skip_line_comment(input);
            }
            SLASH if input.peek() == Some(&STAR) => {
                skip_block_comment(input);
            }
            DOUBLE_QUOTE if !skip_quoted_text(input) => return false,
            next if next == open => depth += 1,
            next if next == close => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{first_nontrivia_on_same_line, starts_member_access_continuation};

    #[test]
    fn same_line_trivia_stops_at_the_first_physical_boundary() {
        let source = format!(" /*\n{}*/ target", "trailing trivia ".repeat(4_096));
        let inspected = Cell::new(0usize);
        let mut input = source
            .chars()
            .map(u32::from)
            .inspect(|_| inspected.set(inspected.get() + 1))
            .peekable();

        assert_eq!(first_nontrivia_on_same_line(&mut input), None);
        assert!(
            inspected.get() < 16,
            "inspected {} code points",
            inspected.get()
        );
    }

    #[test]
    fn member_continuations_match_decl_reference_lexical_heads() {
        let cases = [
            ("identifier", "member", true),
            ("lexer keyword", "self", true),
            ("raw identifier", "`class`", true),
            ("dollar identifier", "$projection", true),
            ("tuple index", "42", true),
            ("range operator", "..value", false),
            ("optional operator", "?", false),
            ("missing name", "", false),
        ];

        for (name, source, expected) in cases {
            assert_eq!(
                starts_member_access_continuation(&mut source.chars().map(u32::from).peekable()),
                expected,
                "{name}: {source}"
            );
        }
    }
}
