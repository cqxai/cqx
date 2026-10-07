pub(super) const BACKTICK: u32 = b'`' as u32;
pub(super) const CARRIAGE_RETURN: u32 = b'\r' as u32;
pub(super) const DOLLAR: u32 = b'$' as u32;
pub(super) const LINE_FEED: u32 = b'\n' as u32;
pub(super) const UNDERSCORE: u32 = b'_' as u32;

// The longest contextual word classified by this scanner is
// `mutableAddressWithNativeOwner` (29 ASCII bytes). Longer identifiers remain
// distinguishable from every keyword without allocating.
const LOOKAHEAD_IDENTIFIER_CAPACITY: usize = 32;

pub(super) struct LookaheadIdentifier {
    spelling: [u8; LOOKAHEAD_IDENTIFIER_CAPACITY],
    length: usize,
}

impl LookaheadIdentifier {
    pub(super) fn ascii_length(&self) -> Option<usize> {
        (self.length <= self.spelling.len()).then_some(self.length)
    }

    pub(super) fn is(&self, expected: &[u8]) -> bool {
        self.length == expected.len() && self.spelling[..self.length] == *expected
    }

    pub(super) fn is_argument_label(&self) -> bool {
        // SwiftSyntax's `Lexer.Lexeme.isArgumentLabel` excludes only the
        // lexer-classified `inout` spelling from otherwise valid identifiers.
        !self.is(b"inout")
    }

    pub(super) fn is_lexer_keyword(&self) -> bool {
        self.ascii_spelling()
            .is_some_and(is_lexer_classified_keyword)
    }

    pub(super) fn ascii_spelling(&self) -> Option<&[u8]> {
        (self.length <= self.spelling.len()).then(|| &self.spelling[..self.length])
    }

    pub(super) fn is_pattern_binding_target(&self) -> bool {
        self.is(b"_") || !self.is_lexer_keyword()
    }

    pub(super) fn is_identifier(&self) -> bool {
        !self.is(b"_") && !self.is_lexer_keyword()
    }

    pub(super) fn is_generic_parameter_start(&self) -> bool {
        if self.is(b"_") {
            return false;
        }
        self.is(b"let") || !self.is_lexer_keyword()
    }
}

pub(super) fn scan_lookahead_identifier(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<LookaheadIdentifier> {
    let first = input.peek().copied()?;
    if first != DOLLAR && first != BACKTICK && !is_identifier_start(first) {
        return None;
    }
    input.next();
    scan_lookahead_identifier_after_first(first, input)
}

pub(super) fn scan_lookahead_identifier_after_first(
    first: u32,
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<LookaheadIdentifier> {
    if first == DOLLAR {
        let mut length = 0_usize;
        let mut all_digits = true;
        while let Some(next) = input
            .peek()
            .copied()
            .filter(|next| is_identifier_continue(*next))
        {
            input.next();
            length += 1;
            all_digits &= (u32::from(b'0')..=u32::from(b'9')).contains(&next);
        }
        return (length == 0 || !all_digits).then_some(LookaheadIdentifier {
            spelling: [0; LOOKAHEAD_IDENTIFIER_CAPACITY],
            length: usize::MAX,
        });
    }
    if first == BACKTICK {
        for next in input.by_ref() {
            if next == BACKTICK {
                return Some(LookaheadIdentifier {
                    spelling: [0; LOOKAHEAD_IDENTIFIER_CAPACITY],
                    length: usize::MAX,
                });
            }
            if matches!(next, LINE_FEED | CARRIAGE_RETURN) {
                return None;
            }
        }
        return None;
    }
    if !is_identifier_start(first) {
        return None;
    }

    let mut spelling = [0_u8; LOOKAHEAD_IDENTIFIER_CAPACITY];
    let mut length = 1usize;
    if first < 0x80 {
        spelling[0] = u8::try_from(first).expect("ASCII code point fits in a byte");
    } else {
        length = usize::MAX;
    }
    while let Some(next) = input
        .peek()
        .copied()
        .filter(|next| is_identifier_continue(*next))
    {
        input.next();
        if next < 0x80 && length < spelling.len() {
            spelling[length] = u8::try_from(next).expect("ASCII code point fits in a byte");
        } else {
            length = usize::MAX;
            while input.peek().copied().is_some_and(is_identifier_continue) {
                input.next();
            }
            break;
        }
        length += 1;
    }
    Some(LookaheadIdentifier { spelling, length })
}

// Derived from SwiftSyntax's generated `Keyword.isLexerClassified` set.
pub(super) fn is_lexer_classified_keyword(spelling: &[u8]) -> bool {
    matches!(
        spelling,
        b"Any"
            | b"as"
            | b"associatedtype"
            | b"break"
            | b"case"
            | b"catch"
            | b"class"
            | b"continue"
            | b"default"
            | b"defer"
            | b"deinit"
            | b"do"
            | b"else"
            | b"enum"
            | b"extension"
            | b"fallthrough"
            | b"false"
            | b"fileprivate"
            | b"for"
            | b"func"
            | b"guard"
            | b"if"
            | b"import"
            | b"in"
            | b"init"
            | b"inout"
            | b"internal"
            | b"is"
            | b"let"
            | b"nil"
            | b"operator"
            | b"precedencegroup"
            | b"private"
            | b"protocol"
            | b"public"
            | b"repeat"
            | b"rethrows"
            | b"return"
            | b"self"
            | b"Self"
            | b"static"
            | b"struct"
            | b"subscript"
            | b"super"
            | b"switch"
            | b"throw"
            | b"throws"
            | b"true"
            | b"try"
            | b"typealias"
            | b"var"
            | b"where"
            | b"while"
    )
}

pub(super) const fn is_ascii_digit(code_point: u32) -> bool {
    code_point >= b'0' as u32 && code_point <= b'9' as u32
}

pub(crate) const fn is_identifier_start(code_point: u32) -> bool {
    if code_point < 0x80 {
        return code_point == UNDERSCORE
            || code_point >= b'A' as u32 && code_point <= b'Z' as u32
            || code_point >= b'a' as u32 && code_point <= b'z' as u32;
    }
    is_identifier_continue(code_point)
        && !(code_point >= 0x0300 && code_point <= 0x036F
            || code_point >= 0x1DC0 && code_point <= 0x1DFF
            || code_point >= 0x20D0 && code_point <= 0x20FF
            || code_point >= 0xFE20 && code_point <= 0xFE2F)
}

pub(crate) const fn is_identifier_continue(code_point: u32) -> bool {
    if code_point < 0x80 {
        return is_identifier_start(code_point)
            || is_ascii_digit(code_point)
            || code_point == DOLLAR;
    }
    code_point == 0x00A8
        || code_point == 0x00AA
        || code_point == 0x00AD
        || code_point == 0x00AF
        || code_point >= 0x00B2 && code_point <= 0x00B5
        || code_point >= 0x00B7 && code_point <= 0x00BA
        || code_point >= 0x00BC && code_point <= 0x00BE
        || code_point >= 0x00C0 && code_point <= 0x00D6
        || code_point >= 0x00D8 && code_point <= 0x00F6
        || code_point >= 0x00F8 && code_point <= 0x167F
        || code_point >= 0x1681 && code_point <= 0x180D
        || code_point >= 0x180F && code_point <= 0x1FFF
        || code_point >= 0x200B && code_point <= 0x200D
        || code_point >= 0x202A && code_point <= 0x202E
        || code_point >= 0x203F && code_point <= 0x2040
        || code_point == 0x2054
        || code_point >= 0x2060 && code_point <= 0x218F
        || code_point >= 0x2460 && code_point <= 0x24FF
        || code_point >= 0x2776 && code_point <= 0x2793
        || code_point >= 0x2C00 && code_point <= 0x2DFF
        || code_point >= 0x2E80 && code_point <= 0x2FFF
        || code_point >= 0x3004 && code_point <= 0x3007
        || code_point >= 0x3021 && code_point <= 0x302F
        || code_point >= 0x3031 && code_point <= 0xD7FF
        || code_point >= 0xF900 && code_point <= 0xFD3D
        || code_point >= 0xFD40 && code_point <= 0xFDCF
        || code_point >= 0xFDF0 && code_point <= 0xFE44
        || code_point >= 0xFE47 && code_point <= 0xFFF8
        || code_point >= 0x10000 && code_point <= 0x1FFFD
        || code_point >= 0x20000 && code_point <= 0x2FFFD
        || code_point >= 0x30000 && code_point <= 0x3FFFD
        || code_point >= 0x40000 && code_point <= 0x4FFFD
        || code_point >= 0x50000 && code_point <= 0x5FFFD
        || code_point >= 0x60000 && code_point <= 0x6FFFD
        || code_point >= 0x70000 && code_point <= 0x7FFFD
        || code_point >= 0x80000 && code_point <= 0x8FFFD
        || code_point >= 0x90000 && code_point <= 0x9FFFD
        || code_point >= 0xA0000 && code_point <= 0xAFFFD
        || code_point >= 0xB0000 && code_point <= 0xBFFFD
        || code_point >= 0xC0000 && code_point <= 0xCFFFD
        || code_point >= 0xD0000 && code_point <= 0xDFFFD
        || code_point >= 0xE0000 && code_point <= 0xEFFFD
}

pub(crate) const fn is_operator_start(code_point: u32) -> bool {
    if code_point < 0x80 {
        return matches!(
            code_point,
            0x21 | 0x25
                | 0x26
                | 0x2A
                | 0x2B
                | 0x2D
                | 0x2E
                | 0x2F
                | 0x3C
                | 0x3D
                | 0x3E
                | 0x3F
                | 0x5E
                | 0x7C
                | 0x7E
        );
    }
    code_point >= 0x00A1 && code_point <= 0x00A7
        || matches!(
            code_point,
            0x00A9
                | 0x00AB
                | 0x00AC
                | 0x00AE
                | 0x00B0
                | 0x00B1
                | 0x00B6
                | 0x00BB
                | 0x00BF
                | 0x00D7
                | 0x00F7
                | 0x2016
                | 0x2017
        )
        || code_point >= 0x2020 && code_point <= 0x2027
        || code_point >= 0x2030 && code_point <= 0x203E
        || code_point >= 0x2041 && code_point <= 0x2053
        || code_point >= 0x2055 && code_point <= 0x205E
        || code_point >= 0x2190 && code_point <= 0x23FF
        || code_point >= 0x2500 && code_point <= 0x2775
        || code_point >= 0x2794 && code_point <= 0x2BFF
        || code_point >= 0x2E00 && code_point <= 0x2E7F
        || code_point >= 0x3001 && code_point <= 0x3003
        || code_point >= 0x3008 && code_point <= 0x3030
}

pub(crate) const fn is_operator_continue(code_point: u32) -> bool {
    is_operator_start(code_point)
        || code_point >= 0x0300 && code_point <= 0x036F
        || code_point >= 0x1DC0 && code_point <= 0x1DFF
        || code_point >= 0x20D0 && code_point <= 0x20FF
        || code_point >= 0xFE00 && code_point <= 0xFE0F
        || code_point >= 0xFE20 && code_point <= 0xFE2F
        || code_point >= 0xE0100 && code_point <= 0xE01EF
}

#[cfg(test)]
mod tests {
    use super::{
        is_identifier_continue, is_identifier_start, is_operator_continue, is_operator_start,
    };

    #[test]
    fn swift_identifier_ranges_cover_representative_scalar_classes() {
        for code_point in ['A' as u32, '_' as u32, 'é' as u32, '你' as u32, '😀' as u32] {
            assert!(is_identifier_start(code_point), "U+{code_point:04X}");
        }
        for code_point in ['$' as u32, '0' as u32, '\u{0308}' as u32] {
            assert!(is_identifier_continue(code_point), "U+{code_point:04X}");
        }
        for code_point in ['\u{0308}' as u32, '\u{E000}' as u32, 0x1_FFFE] {
            assert!(!is_identifier_start(code_point), "U+{code_point:04X}");
        }
    }

    #[test]
    fn swift_operator_ranges_distinguish_starts_from_combining_scalars() {
        for code_point in [
            '!' as u32,
            '\u{00A1}' as u32,
            '\u{2190}' as u32,
            '\u{3030}' as u32,
        ] {
            assert!(is_operator_start(code_point), "U+{code_point:04X}");
        }
        for code_point in ['\u{0308}' as u32, '\u{FE0F}' as u32, 0xE0100] {
            assert!(is_operator_continue(code_point), "U+{code_point:04X}");
            assert!(!is_operator_start(code_point), "U+{code_point:04X}");
        }
        for code_point in ['A' as u32, ' ' as u32, '\u{E000}' as u32] {
            assert!(!is_operator_continue(code_point), "U+{code_point:04X}");
        }
    }
}
