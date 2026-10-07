//! Source recognition at Swift statement-condition boundaries.
//!
//! Parser-state checks and token selection remain in the parent tokenizer.
//! This module only determines whether the source at that state ends a
//! condition list or forms a trailing closure within one.

use rezel_common::CodePoint;
use rezel_lr::InputStream;

use super::lexical::{
    is_identifier_start, is_operator_start, scan_lookahead_identifier,
    scan_lookahead_identifier_after_first,
};

use super::accessor;
use super::literal;
use super::lookahead::{skip_block_comment, skip_line_comment, skip_trivia_with_line_break};
use super::operator::starts_binary_operator_like_continuation;

const CARRIAGE_RETURN: u32 = b'\r' as u32;
const LINE_FEED: u32 = b'\n' as u32;
const SLASH: u32 = b'/' as u32;
const STAR: u32 = b'*' as u32;
const AT_SIGN: u32 = b'@' as u32;
const POUND: u32 = b'#' as u32;
const EQUAL: u32 = b'=' as u32;
const EXCLAMATION: u32 = b'!' as u32;
const LEFT_BRACE: u32 = b'{' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COLON: u32 = b':' as u32;
const COMMA: u32 = b',' as u32;
const PERIOD: u32 = b'.' as u32;
const QUESTION: u32 = b'?' as u32;
const RIGHT_BRACE: u32 = b'}' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const SEMICOLON: u32 = b';' as u32;
const DOUBLE_QUOTE: u32 = b'"' as u32;

pub(super) fn list_ends_here(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    list_ends_here_from(&mut lookahead)
}

pub(super) fn allows_trailing_closure(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    allows_trailing_closure_from(&mut lookahead)
}

// SwiftSyntax's trailing-comma feature keeps a comma on the final condition
// only when the following token is `else` or a brace that starts the statement
// body. A brace can also be a closure condition, so inspect its balanced body
// and the token that follows it before selecting the zero-width grammar role.
fn list_ends_here_from(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if input.peek() != Some(&LEFT_BRACE) {
        return scan_lookahead_identifier(input).is_some_and(|word| word.is(b"else"));
    }
    if !skip_balanced_braces(input, BalancedBraceContext::Any) {
        return false;
    }
    let Some(has_line_break) = skip_trivia_with_line_break(input) else {
        return false;
    };
    match input.peek().copied() {
        None | Some(SEMICOLON | RIGHT_BRACE | RIGHT_PAREN) => true,
        Some(next) if next == u32::from(b'e') => {
            scan_lookahead_identifier(input).is_some_and(|word| word.is(b"else"))
        }
        Some(COMMA) => false,
        Some(_) if !has_line_break => false,
        Some(first) => {
            input.next();
            !is_operator_start(first) || !starts_binary_continuation(first, input)
        }
    }
}

// SwiftSyntax's statement-condition flavor normally reserves a brace for the
// statement body. It nevertheless accepts a complete closure when the token
// after that closure proves that another body or condition delimiter follows.
// Keep this arbitrary lookahead out of the LR table and mirror the token-level
// boundary from `atValidTrailingClosure`.
fn allows_trailing_closure_from(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if !skip_balanced_braces(input, BalancedBraceContext::StatementConditionClosure) {
        return false;
    }

    let mut has_left_trivia = false;
    let mut has_line_break = false;
    let first = loop {
        let Some(next) = input.next() else {
            return false;
        };
        match next {
            LINE_FEED | CARRIAGE_RETURN => {
                has_left_trivia = true;
                has_line_break = true;
            }
            9 | 11 | 12 | 32 => has_left_trivia = true,
            SLASH if input.peek() == Some(&SLASH) => {
                has_left_trivia = true;
                has_line_break |= skip_line_comment(input);
            }
            SLASH if input.peek() == Some(&STAR) => {
                has_left_trivia = true;
                has_line_break |= skip_block_comment(input);
            }
            _ => break next,
        }
    };

    if matches!(first, LEFT_BRACE | COMMA) {
        return true;
    }
    if is_identifier_start(first) {
        let Some(word) = scan_lookahead_identifier_after_first(first, input) else {
            return false;
        };
        if word.is(b"where") {
            return true;
        }
        return !has_line_break && (word.is(b"as") || word.is(b"is"));
    }
    if has_line_break {
        return false;
    }
    if matches!(
        first,
        LEFT_BRACKET | LEFT_PAREN | PERIOD | QUESTION | EXCLAMATION | COLON | EQUAL
    ) {
        return true;
    }
    is_operator_start(first) && (!has_left_trivia || starts_binary_continuation(first, input))
}

fn starts_binary_continuation(
    first: u32,
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    starts_binary_operator_like_continuation(first, input, true)
}

#[derive(Clone, Copy)]
enum BalancedBraceContext {
    Any,
    StatementConditionClosure,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TrailingClosureBracePrefix {
    Ordinary,
    LineStart,
    AccessorOrSwitch,
}

fn scan_trailing_closure_brace_prefix(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<TrailingClosureBracePrefix> {
    (input.next() == Some(LEFT_BRACE)).then_some(())?;
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
                    _ => break,
                }
            }
            _ => break,
        }
    }

    let has_accessor_marker = if input.peek() == Some(&AT_SIGN) {
        accessor::scan_attributes(input)?
    } else {
        false
    };
    let first_word = scan_lookahead_identifier(input);
    let starts_accessor_or_switch = has_accessor_marker
        || first_word
            .as_ref()
            .is_some_and(|word| word.is(b"case") || accessor::is_observer_specifier(word));
    if starts_accessor_or_switch {
        return Some(TrailingClosureBracePrefix::AccessorOrSwitch);
    }
    Some(if has_line_break {
        TrailingClosureBracePrefix::LineStart
    } else {
        TrailingClosureBracePrefix::Ordinary
    })
}

fn skip_balanced_braces(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    context: BalancedBraceContext,
) -> bool {
    if matches!(context, BalancedBraceContext::StatementConditionClosure) {
        if scan_trailing_closure_brace_prefix(input) != Some(TrailingClosureBracePrefix::Ordinary) {
            return false;
        }
    } else if input.next() != Some(LEFT_BRACE) {
        return false;
    }
    let mut depth = 1_usize;
    while let Some(next) = input.peek().copied() {
        match next {
            SLASH => {
                input.next();
                match input.peek().copied() {
                    Some(SLASH) => {
                        skip_line_comment(input);
                    }
                    Some(STAR) => {
                        skip_block_comment(input);
                    }
                    _ => {}
                }
            }
            DOUBLE_QUOTE | POUND => {
                if !literal::skip_string_literal_or_advance(input) {
                    return false;
                }
            }
            LEFT_BRACE => {
                input.next();
                depth += 1;
            }
            RIGHT_BRACE => {
                input.next();
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {
                input.next();
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{allows_trailing_closure_from, list_ends_here_from};

    fn counted_code_points<'a>(
        source: &'a str,
        inspected: &'a Cell<usize>,
    ) -> impl Iterator<Item = u32> + 'a {
        source.chars().map(u32::from).inspect(move |_| {
            inspected.set(inspected.get() + 1);
        })
    }

    #[test]
    fn trailing_commas_stop_at_statement_bodies() {
        let cases = [
            ("guard else", "else", true),
            ("empty body", "{}", true),
            ("body before else", "{ print(0) } else", true),
            ("nested body", "{ if true, { } }", true),
            ("line-start successor", "{ print(0) }\nnext", true),
            ("body before closure call", "{ print(0) }\n{ }()", true),
            ("string brace", "{ print(\"}\") }", true),
            ("comment brace", "{ /* } */ print(0) }", true),
            ("closure before comma", "{ true },", false),
            ("line-start comma", "{ true }\n, { print(0) }", false),
            ("same-line operator", "{ true }+++ { print(0) }", false),
            ("line-start operator", "{ true }\n!= nil", false),
            ("same-line successor", "{ true } next", false),
            ("identifier prefix", "elsewhere", false),
        ];
        for (name, source, expected) in cases {
            let mut input = source.chars().map(u32::from).peekable();
            assert_eq!(list_ends_here_from(&mut input), expected, "{name}");
        }
    }

    #[test]
    fn trailing_closures_require_a_following_delimiter() {
        let cases = [
            ("following body", "{ value } {}", true),
            ("line-start body", "{ value }\n{}", true),
            ("where clause", "{ value } where true", true),
            ("commented where", "{ value } /* trivia */ where true", true),
            ("condition comma", "{ value }, next", true),
            ("same-line colon", "{ value }:", true),
            ("same-line binary", "{ value } + next", true),
            ("same-line postfix", "{ value }!", true),
            ("statement body", "{ value }", false),
            ("line-start closure body", "{\nvalue\n} {}", false),
            (
                "commented line-start closure body",
                "{ /* before */\nvalue\n} {}",
                false,
            ),
            ("switch body", "{ case true: value } as Int", false),
            (
                "commented switch body",
                "{ /* trivia */ case true: value } as Int",
                false,
            ),
            ("observer body", "{ didSet {} } {}", false),
            ("attributed observer body", "{ @foo didSet {} } {}", false),
            (
                "accessor marker body",
                "{ @_accessorBlock value } {}",
                false,
            ),
            ("identifier successor", "{ value } next", false),
            ("line-start colon", "{ value }\n:", false),
            ("line-start prefix", "{ value }\n+next", false),
        ];
        for (name, source, expected) in cases {
            let mut input = source.chars().map(u32::from).peekable();
            assert_eq!(
                allows_trailing_closure_from(&mut input),
                expected,
                "{name}: {source}"
            );
        }
    }

    #[test]
    fn deep_trailing_closure_does_not_scan_the_remaining_source() {
        const DEPTH: usize = 128;
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let prefix = format!("{{ value {}0{} }},", "{".repeat(DEPTH), "}".repeat(DEPTH));
        let source = format!("{prefix}{}", " sentinel".repeat(4_096));
        let inspected = Cell::new(0usize);
        let input = counted_code_points(&source, &inspected);

        assert!(allows_trailing_closure_from(&mut input.peekable()));
        assert!(
            inspected.get() <= prefix.len() + ALLOWED_BOUNDARY_READS,
            "condition closure inspected {} code points for a {}-point prefix",
            inspected.get(),
            prefix.len(),
        );
    }
}
