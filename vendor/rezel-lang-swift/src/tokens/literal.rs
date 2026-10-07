use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use crate::terms;

use super::lookahead::{current, peek};

const CARRIAGE_RETURN: u32 = b'\r' as u32;
const LINE_FEED: u32 = b'\n' as u32;
const SPACE: u32 = b' ' as u32;
const TAB: u32 = b'\t' as u32;
const SLASH: u32 = b'/' as u32;
const STAR: u32 = b'*' as u32;
const POUND: u32 = b'#' as u32;
const BACKSLASH: u32 = b'\\' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const RIGHT_BRACKET: u32 = b']' as u32;
const RIGHT_PAREN: u32 = b')' as u32;
const DOUBLE_QUOTE: u32 = b'"' as u32;

pub(super) fn scan_literal(input: &mut InputStream, stack: &Stack) -> Result<bool, ParseError> {
    match literal_delimiter(input) {
        Some(SLASH) if stack.can_shift(terms::RegexLiteral) => scan_regex_literal(input, stack),
        Some(DOUBLE_QUOTE) if stack.can_shift(terms::StringLiteral) => scan_string_literal(input),
        _ => Ok(false),
    }
}

fn scan_regex_literal(input: &mut InputStream, stack: &Stack) -> Result<bool, ParseError> {
    let scan = {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        scan_regex_literal_candidate(&mut lookahead, RegexLiteralContext::Expression)
    };
    // SwiftSyntax keeps `/` as an unapplied or prefix operator after an
    // argument-list delimiter when a tentative regex would cross that list's
    // unmatched right parenthesis. `customOperator` is only shiftable in those
    // list-item states, so the LR state supplies the same context without
    // tracking previous tokens in the lexer.
    if scan.is_some_and(|scan| {
        scan.crosses_unmatched_right_parenthesis && stack.can_shift(terms::customOperator)
    }) {
        return Ok(false);
    }
    if let Some(scan) = scan {
        input.advance(scan.width);
        input.accept_token(terms::RegexLiteral)?;
        return Ok(true);
    }
    Ok(false)
}

fn scan_string_literal(input: &mut InputStream) -> Result<bool, ParseError> {
    let scan = {
        let mut cursor = InputStringCursor { input };
        scan_string_literal_cursor(&mut cursor)
    };
    if scan == StringLiteralScan::Complete {
        input.accept_token(terms::StringLiteral)?;
        return Ok(true);
    }
    Ok(false)
}

fn literal_delimiter(input: &InputStream) -> Option<u32> {
    match current(input) {
        Some(delimiter @ (SLASH | DOUBLE_QUOTE)) => Some(delimiter),
        Some(POUND) => {
            let mut offset = 1_isize;
            while peek(input, offset) == Some(POUND) {
                offset += 1;
            }
            peek(input, offset).filter(|delimiter| matches!(*delimiter, SLASH | DOUBLE_QUOTE))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RegexLiteralContext {
    Expression,
    OperatorSuffix,
}

#[cfg(test)]
fn regex_literal_width(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<usize> {
    scan_regex_literal_candidate(input, RegexLiteralContext::Expression).map(|scan| scan.width)
}

#[derive(Clone, Copy)]
struct RegexLiteralScan {
    width: usize,
    crosses_unmatched_right_parenthesis: bool,
}

fn scan_regex_literal_candidate(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
    context: RegexLiteralContext,
) -> Option<RegexLiteralScan> {
    let mut width = 0_usize;
    let mut pound_count = 0_usize;
    let mut crosses_unmatched_right_parenthesis = false;
    while input.peek() == Some(&POUND) {
        input.next();
        width += 1;
        pound_count += 1;
    }
    if input.next() != Some(SLASH) {
        return None;
    }
    width += 1;

    if pound_count == 0 && matches!(input.peek().copied(), Some(SLASH | STAR | SPACE | TAB)) {
        return None;
    }

    let mut multiline = false;
    if pound_count > 0 {
        while matches!(input.peek().copied(), Some(SPACE | TAB)) {
            input.next();
            width += 1;
        }
        multiline = matches!(input.peek().copied(), Some(LINE_FEED | CARRIAGE_RETURN));
    }

    let mut group_depth = 0_usize;
    let mut character_class_depth = 0_usize;
    let mut last_unescaped_space = false;
    loop {
        let next = input.next()?;
        width += 1;
        match next {
            BACKSLASH => {
                let escaped = input.next()?;
                width += 1;
                if !multiline && matches!(escaped, LINE_FEED | CARRIAGE_RETURN) {
                    return None;
                }
                last_unescaped_space = false;
            }
            LINE_FEED | CARRIAGE_RETURN => {
                if !multiline {
                    return None;
                }
                last_unescaped_space = false;
            }
            SLASH if pound_count == 0 => {
                if last_unescaped_space || matches!(input.peek().copied(), Some(SLASH | STAR)) {
                    return None;
                }
                return Some(RegexLiteralScan {
                    width,
                    crosses_unmatched_right_parenthesis,
                });
            }
            SLASH => {
                let mut close_pound_count = 0_usize;
                while input.peek() == Some(&POUND) {
                    input.next();
                    width += 1;
                    close_pound_count += 1;
                }
                if close_pound_count >= pound_count {
                    return Some(RegexLiteralScan {
                        width,
                        crosses_unmatched_right_parenthesis,
                    });
                }
                last_unescaped_space = false;
            }
            LEFT_PAREN if character_class_depth == 0 => {
                group_depth += 1;
                last_unescaped_space = false;
            }
            RIGHT_PAREN if character_class_depth == 0 => {
                if group_depth == 0 {
                    if context == RegexLiteralContext::OperatorSuffix {
                        return None;
                    }
                    crosses_unmatched_right_parenthesis = true;
                } else {
                    group_depth -= 1;
                }
                last_unescaped_space = false;
            }
            LEFT_BRACKET => {
                character_class_depth += 1;
                last_unescaped_space = false;
            }
            RIGHT_BRACKET => {
                character_class_depth = character_class_depth.saturating_sub(1);
                last_unescaped_space = false;
            }
            SPACE | TAB => last_unescaped_space = true,
            _ => last_unescaped_space = false,
        }
    }
}

fn regex_literal_width_at(
    input: &InputStream,
    offset: usize,
    context: RegexLiteralContext,
) -> Option<usize> {
    let mut lookahead = input
        .lookahead()
        .skip(offset)
        .map(CodePoint::as_u32)
        .peekable();
    scan_regex_literal_candidate(&mut lookahead, context).map(|scan| scan.width)
}

pub(super) fn regex_literal_width_at_operator_suffix(
    input: &InputStream,
    offset: usize,
) -> Option<usize> {
    regex_literal_width_at(input, offset, RegexLiteralContext::OperatorSuffix)
}

pub(super) fn contextual_regex_operator_prefix_width(
    input: &InputStream,
    stack: &Stack,
    left_bound: bool,
    first_internal_slash: Option<usize>,
) -> Option<usize> {
    if left_bound {
        return None;
    }
    let prefix_width = first_internal_slash?;
    let can_shift_prefix_operator = stack.can_shift(terms::prefixCustomOperator)
        || stack.can_shift(terms::prefixTilde)
        || stack.can_shift(terms::prefixAmpersand)
        || stack.can_shift(terms::prefixRangeOperator);
    let binary_operator_wins = stack.can_shift(terms::binaryCustomOperator);
    if !can_shift_prefix_operator || binary_operator_wins {
        return None;
    }
    regex_literal_width_at(input, prefix_width, RegexLiteralContext::OperatorSuffix)
        .map(|_| prefix_width)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StringLiteralScan {
    NotString,
    Complete,
    Unterminated,
}

trait StringCursor {
    fn current(&mut self) -> Option<u32>;

    fn advance(&mut self) -> Option<u32>;

    fn advance_ascii_while(&mut self, predicate: impl FnMut(u8) -> bool) -> usize;
}

struct InputStringCursor<'a> {
    input: &'a mut InputStream,
}

impl StringCursor for InputStringCursor<'_> {
    fn current(&mut self) -> Option<u32> {
        self.input.next().map(CodePoint::as_u32)
    }

    fn advance(&mut self) -> Option<u32> {
        let current = self.current();
        if current.is_some() {
            self.input.advance(1);
        }
        current
    }

    fn advance_ascii_while(&mut self, predicate: impl FnMut(u8) -> bool) -> usize {
        self.input.advance_ascii_while(predicate)
    }
}

struct LookaheadStringCursor<'a, I: Iterator<Item = u32>> {
    input: &'a mut std::iter::Peekable<I>,
    width: usize,
}

impl<I: Iterator<Item = u32>> StringCursor for LookaheadStringCursor<'_, I> {
    fn current(&mut self) -> Option<u32> {
        self.input.peek().copied()
    }

    fn advance(&mut self) -> Option<u32> {
        let next = self.input.next();
        if next.is_some() {
            self.width += 1;
        }
        next
    }

    fn advance_ascii_while(&mut self, mut predicate: impl FnMut(u8) -> bool) -> usize {
        let mut count = 0_usize;
        loop {
            let Some(next) = self.input.peek().copied() else {
                return count;
            };
            let Ok(byte) = u8::try_from(next) else {
                return count;
            };
            if !predicate(byte) {
                return count;
            }
            self.input.next();
            self.width += 1;
            count += 1;
        }
    }
}

#[cfg(test)]
fn string_literal_width(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<usize> {
    let mut cursor = LookaheadStringCursor { input, width: 0 };
    (scan_string_literal_cursor(&mut cursor) == StringLiteralScan::Complete).then_some(cursor.width)
}

pub(super) fn skip_string_literal_or_advance(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    let mut cursor = LookaheadStringCursor { input, width: 0 };
    match scan_string_literal_cursor(&mut cursor) {
        StringLiteralScan::Complete => true,
        StringLiteralScan::NotString => {
            if cursor.width == 0 {
                cursor.advance();
            }
            true
        }
        StringLiteralScan::Unterminated => false,
    }
}

fn scan_string_literal_cursor(cursor: &mut impl StringCursor) -> StringLiteralScan {
    let mut pound_count = 0_usize;
    while cursor.current() == Some(POUND) {
        cursor.advance();
        pound_count += 1;
    }
    if cursor.current() != Some(DOUBLE_QUOTE) {
        return StringLiteralScan::NotString;
    }
    cursor.advance();

    let mut quote_count = 1_usize;
    while quote_count < 3 && cursor.current() == Some(DOUBLE_QUOTE) {
        cursor.advance();
        quote_count += 1;
    }
    if quote_count == 2 && (pound_count == 0 || consume_exact_pounds(cursor, pound_count)) {
        return StringLiteralScan::Complete;
    }
    if quote_count < 3 {
        return scan_string_literal_body(cursor, pound_count, false);
    }
    if pound_count == 0 {
        return scan_string_literal_body(cursor, pound_count, true);
    }

    // SwiftSyntax treats a raw triple-quote prefix as a single-line string when
    // the same physical line contains a quote followed by the opening number
    // of pounds. This preserves forms such as #"""# and #""Zeta""#.
    if consume_exact_pounds(cursor, pound_count)
        || scan_raw_single_line_string_end(cursor, pound_count)
    {
        return StringLiteralScan::Complete;
    }
    scan_string_literal_body(cursor, pound_count, true)
}

fn scan_raw_single_line_string_end(cursor: &mut impl StringCursor, pound_count: usize) -> bool {
    loop {
        match cursor.current() {
            None | Some(LINE_FEED | CARRIAGE_RETURN) => return false,
            Some(DOUBLE_QUOTE) => {
                cursor.advance();
                if consume_exact_pounds(cursor, pound_count) {
                    return true;
                }
            }
            Some(_) => {
                let advanced =
                    cursor.advance_ascii_while(|byte| !matches!(byte, b'"' | b'\n' | b'\r'));
                if advanced == 0 {
                    cursor.advance();
                }
            }
        }
    }
}

fn scan_string_literal_body(
    cursor: &mut impl StringCursor,
    pound_count: usize,
    multiline: bool,
) -> StringLiteralScan {
    loop {
        match cursor.current() {
            None => return StringLiteralScan::Unterminated,
            Some(LINE_FEED | CARRIAGE_RETURN) if !multiline => {
                return StringLiteralScan::Unterminated;
            }
            Some(DOUBLE_QUOTE) => {
                if consume_string_literal_end(cursor, pound_count, multiline) {
                    return StringLiteralScan::Complete;
                }
            }
            Some(BACKSLASH) => {
                if !scan_string_escape(cursor, pound_count, multiline) {
                    return StringLiteralScan::Unterminated;
                }
            }
            Some(_) => {
                let advanced = cursor.advance_ascii_while(|byte| {
                    byte != b'"' && byte != b'\\' && (multiline || !matches!(byte, b'\n' | b'\r'))
                });
                if advanced == 0 {
                    cursor.advance();
                }
            }
        }
    }
}

fn consume_string_literal_end(
    cursor: &mut impl StringCursor,
    pound_count: usize,
    multiline: bool,
) -> bool {
    cursor.advance();
    if multiline {
        if cursor.current() != Some(DOUBLE_QUOTE) {
            return false;
        }
        cursor.advance();
        if cursor.current() != Some(DOUBLE_QUOTE) {
            return false;
        }
        cursor.advance();
    }
    pound_count == 0 || consume_exact_pounds(cursor, pound_count)
}

fn scan_string_escape(cursor: &mut impl StringCursor, pound_count: usize, multiline: bool) -> bool {
    cursor.advance();
    if !consume_exact_pounds(cursor, pound_count) {
        return true;
    }
    if cursor.current() == Some(LEFT_PAREN) {
        cursor.advance();
        return scan_string_interpolation(cursor);
    }
    if !multiline && matches!(cursor.current(), Some(LINE_FEED | CARRIAGE_RETURN)) {
        return false;
    }
    cursor.advance().is_some()
}

fn scan_string_interpolation(cursor: &mut impl StringCursor) -> bool {
    let mut depth = 1_usize;
    loop {
        match cursor.current() {
            None => return false,
            Some(LEFT_PAREN) => {
                cursor.advance();
                depth += 1;
            }
            Some(RIGHT_PAREN) => {
                cursor.advance();
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            Some(DOUBLE_QUOTE | POUND) => match scan_string_literal_cursor(cursor) {
                StringLiteralScan::Complete | StringLiteralScan::NotString => {}
                StringLiteralScan::Unterminated => return false,
            },
            Some(SLASH) => {
                cursor.advance();
                match cursor.current() {
                    Some(SLASH) => {
                        cursor.advance();
                        skip_string_interpolation_line_comment(cursor);
                    }
                    Some(STAR) => {
                        cursor.advance();
                        if !skip_string_interpolation_block_comment(cursor) {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            Some(_) => {
                let advanced = cursor
                    .advance_ascii_while(|byte| !matches!(byte, b'(' | b')' | b'"' | b'#' | b'/'));
                if advanced == 0 {
                    cursor.advance();
                }
            }
        }
    }
}

fn skip_string_interpolation_line_comment(cursor: &mut impl StringCursor) {
    cursor.advance_ascii_while(|byte| !matches!(byte, b'\n' | b'\r'));
    if matches!(cursor.current(), Some(LINE_FEED | CARRIAGE_RETURN)) {
        cursor.advance();
    }
}

fn skip_string_interpolation_block_comment(cursor: &mut impl StringCursor) -> bool {
    let mut depth = 1_usize;
    loop {
        let advanced = cursor.advance_ascii_while(|byte| !matches!(byte, b'/' | b'*'));
        if advanced > 0 {
            continue;
        }
        let Some(next) = cursor.advance() else {
            return false;
        };
        if next == SLASH && cursor.current() == Some(STAR) {
            cursor.advance();
            depth += 1;
        } else if next == STAR && cursor.current() == Some(SLASH) {
            cursor.advance();
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
    }
}

fn consume_exact_pounds(cursor: &mut impl StringCursor, pound_count: usize) -> bool {
    for _ in 0..pound_count {
        if cursor.current() != Some(POUND) {
            return false;
        }
        cursor.advance();
    }
    true
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{regex_literal_width, string_literal_width};

    fn regex_width(source: &str) -> Option<usize> {
        regex_literal_width(&mut source.chars().map(u32::from).peekable())
    }

    fn string_width(source: &str) -> Option<usize> {
        string_literal_width(&mut source.chars().map(u32::from).peekable())
    }

    #[test]
    fn regex_literal_scanner_matches_delimiters_without_masking_operators() {
        let cases = [
            ("bare", "/abc/", Some("/abc/")),
            ("escaped slash", r"/a\/b/", Some(r"/a\/b/")),
            ("postfix suffix", "/abc/.self", Some("/abc/")),
            ("extended slash", "#/abc/def/#", Some("#/abc/def/#")),
            (
                "nested delimiter candidate",
                "##/abc/#def/##",
                Some("##/abc/#def/##"),
            ),
            ("empty extended", "#//#", Some("#//#")),
            ("multiline extended", "#/\nabc\n/#", Some("#/\nabc\n/#")),
            ("unicode", "/é/", Some("/é/")),
            ("unclosed group is pattern", "/(/", Some("/(/")),
            ("right parenthesis in class", "/[)]/", Some("/[)]/")),
            ("line comment", "// comment", None),
            ("block comment", "/* comment */", None),
            ("leading space", "/ abc/", None),
            ("trailing space", "/abc /", None),
            ("bare newline", "/abc\n/", None),
            ("non-multiline extended", "#/abc\n/#", None),
            ("unbalanced right group", "/)/", Some("/)/")),
            ("mismatched pounds", "##/abc/#", None),
            ("unterminated", "/abc", None),
            ("macro", "#name", None),
            ("closing comment boundary", "/abc//*", None),
        ];

        for (name, source, expected_prefix) in cases {
            let expected = expected_prefix.map(|prefix| prefix.chars().count());
            assert_eq!(regex_width(source), expected, "{name}: {source}");
        }
    }

    #[test]
    fn string_literal_scanner_matches_raw_multiline_and_interpolation_boundaries() {
        let cases = [
            ("single line", "\"hello\"", Some("\"hello\"")),
            ("empty", "\"\"", Some("\"\"")),
            ("escaped quote", "\"a\\\"b\"", Some("\"a\\\"b\"")),
            (
                "nested interpolation string",
                "\"before \\(\"inner\") after\"",
                Some("\"before \\(\"inner\") after\""),
            ),
            (
                "multiline",
                "\"\"\"\nhello\n\"\"\"",
                Some("\"\"\"\nhello\n\"\"\""),
            ),
            (
                "multiline nested in interpolation",
                "\"hello\\(\"\"\"\nworld\n\"\"\")\"",
                Some("\"hello\\(\"\"\"\nworld\n\"\"\")\""),
            ),
            (
                "raw escaped quote",
                "#\"a raw string with \\\" in it\"#",
                Some("#\"a raw string with \\\" in it\"#"),
            ),
            ("empty raw", "#\"\"#", Some("#\"\"#")),
            (
                "raw interpolation",
                "#\"value \\#(foo)\"#",
                Some("#\"value \\#(foo)\"#"),
            ),
            (
                "raw false multiline",
                "#\"\"Zeta\"\"#",
                Some("#\"\"Zeta\"\"#"),
            ),
            ("raw quote content", "#\"\"\"#", Some("#\"\"\"#")),
            (
                "raw multiline",
                "##\"\"\"\nvalue\n\"\"\"##",
                Some("##\"\"\"\nvalue\n\"\"\"##"),
            ),
            ("postfix suffix", "\"value\".self", Some("\"value\"")),
            ("if config", "#if FLAG", None),
            ("object literal", "#Color", None),
            ("unterminated", "\"value", None),
            ("single-line newline", "\"value\nnext\"", None),
            ("mismatched pounds", "##\"value\"#", None),
            ("unterminated interpolation", "\"value \\(call(1)\"", None),
        ];

        for (name, source, expected_prefix) in cases {
            let expected = expected_prefix.map(|prefix| prefix.chars().count());
            assert_eq!(string_width(source), expected, "{name}: {source}");
        }
    }

    #[test]
    fn nested_string_interpolation_does_not_scan_the_remaining_source() {
        const DEPTH: usize = 128;

        let mut literal = String::from("\"value\"");
        for _ in 0..DEPTH {
            literal = format!("\"\\({literal})\"");
        }
        let source = format!("{literal}{}", " sentinel".repeat(4_096));
        let inspected = Cell::new(0usize);
        let input = source.chars().map(u32::from).inspect(|_| {
            inspected.set(inspected.get() + 1);
        });

        assert_eq!(
            string_literal_width(&mut input.peekable()),
            Some(literal.len())
        );
        assert!(
            inspected.get() <= literal.len() + 1,
            "nested interpolation inspected {} code points for a {}-point literal",
            inspected.get(),
            literal.len(),
        );
    }
}
