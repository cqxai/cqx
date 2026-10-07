use rezel_common::{CodePoint, ParseError, first_invalid_identifier_offset};
use rezel_lr::{
    ExternalTokenizer, ExternalTokenizerStart, InputStream, Stack, StrictTokenValidationError,
    StrictTokenValidator, TokenizerFlags,
};

const IDENTIFIER_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'$')
    .with_ascii(b'_')
    .with_ascii_range(b'A'..=b'Z')
    .with_ascii_range(b'a'..=b'z')
    .with_non_ascii();
static ASCII_IDENTIFIER_PART: [bool; 128] = {
    let mut table = [false; 128];
    let mut index = 0;
    let mut code_point = 0_u32;
    while index < table.len() {
        table[index] = is_identifier_candidate_part(code_point);
        index += 1;
        code_point += 1;
    }
    table
};

// Java accepts several compatibility characters, currency symbols, and connector
// punctuation that XID_Start deliberately excludes. These are the Java 26
// additions to the XID profile used by unicode-ident. Inclusive ranges keep the
// language-specific part compact without carrying a second Unicode database.
const JAVA_IDENTIFIER_START_ADDITIONS: &[(u32, u32)] = &[
    // Relative to XID_Start, Java 26 adds 22 letters excluded by XID's
    // normalization profile, 64 currency symbols (Sc), and 10 connector
    // punctuation characters (Pc). Ranges stay scalar-sorted for binary search.
    (0x24, 0x24),       // Sc '$'
    (0x5f, 0x5f),       // Pc '_'
    (0xa2, 0xa5),       // Sc '¢' .. Sc '¥'
    (0x37a, 0x37a),     // Lm 'ͺ'
    (0x58f, 0x58f),     // Sc '֏'
    (0x60b, 0x60b),     // Sc '؋'
    (0x7fe, 0x7ff),     // Sc '߾' .. Sc '߿'
    (0x9f2, 0x9f3),     // Sc '৲' .. Sc '৳'
    (0x9fb, 0x9fb),     // Sc '৻'
    (0xaf1, 0xaf1),     // Sc '૱'
    (0xbf9, 0xbf9),     // Sc '௹'
    (0xe33, 0xe33),     // Lo 'ำ'
    (0xe3f, 0xe3f),     // Sc '฿'
    (0xeb3, 0xeb3),     // Lo 'ຳ'
    (0x17db, 0x17db),   // Sc '៛'
    (0x203f, 0x2040),   // Pc '‿' .. Pc '⁀'
    (0x2054, 0x2054),   // Pc '⁔'
    (0x20a0, 0x20c1),   // Sc '₠' .. Sc '⃁'
    (0x2e2f, 0x2e2f),   // Lm 'ⸯ'
    (0xa838, 0xa838),   // Sc '꠸'
    (0xfc5e, 0xfc63),   // Lo 'ﱞ' .. Lo 'ﱣ'
    (0xfdfa, 0xfdfc),   // Lo 'ﷺ' .. Sc '﷼'
    (0xfe33, 0xfe34),   // Pc '︳' .. Pc '︴'
    (0xfe4d, 0xfe4f),   // Pc '﹍' .. Pc '﹏'
    (0xfe69, 0xfe69),   // Sc '﹩'
    (0xfe70, 0xfe70),   // Lo 'ﹰ'
    (0xfe72, 0xfe72),   // Lo 'ﹲ'
    (0xfe74, 0xfe74),   // Lo 'ﹴ'
    (0xfe76, 0xfe76),   // Lo 'ﹶ'
    (0xfe78, 0xfe78),   // Lo 'ﹸ'
    (0xfe7a, 0xfe7a),   // Lo 'ﹺ'
    (0xfe7c, 0xfe7c),   // Lo 'ﹼ'
    (0xfe7e, 0xfe7e),   // Lo 'ﹾ'
    (0xff04, 0xff04),   // Sc '＄'
    (0xff3f, 0xff3f),   // Pc '＿'
    (0xff9e, 0xff9f),   // Lm 'ﾞ' .. Lm 'ﾟ'
    (0xffe0, 0xffe1),   // Sc '￠' .. Sc '￡'
    (0xffe5, 0xffe6),   // Sc '￥' .. Sc '￦'
    (0x11fdd, 0x11fe0), // Sc '𑿝' .. Sc '𑿠'
    (0x1e2ff, 0x1e2ff), // Sc '𞋿'
    (0x1ecb0, 0x1ecb0), // Sc '𞲰'
];

// Java identifier parts additionally admit identifier-ignorable characters and
// the Java-specific start characters that XID_Continue does not already cover.
const JAVA_IDENTIFIER_PART_ADDITIONS: &[(u32, u32)] = &[
    // Relative to XID_Continue, Java 26 adds 18 compatibility letters,
    // 64 currency symbols, and 224 identifier-ignorable controls/formats.
    // U+200C and U+200D are ignorable too, but XID_Continue already covers them.
    (0x0, 0x8),         // Cc .. Cc
    (0xe, 0x1b),        // Cc .. Cc
    (0x24, 0x24),       // Sc '$'
    (0x7f, 0x9f),       // Cc .. Cc
    (0xa2, 0xa5),       // Sc '¢' .. Sc '¥'
    (0xad, 0xad),       // Cf
    (0x37a, 0x37a),     // Lm 'ͺ'
    (0x58f, 0x58f),     // Sc '֏'
    (0x600, 0x605),     // Cf .. Cf
    (0x60b, 0x60b),     // Sc '؋'
    (0x61c, 0x61c),     // Cf
    (0x6dd, 0x6dd),     // Cf
    (0x70f, 0x70f),     // Cf
    (0x7fe, 0x7ff),     // Sc '߾' .. Sc '߿'
    (0x890, 0x891),     // Cf .. Cf
    (0x8e2, 0x8e2),     // Cf
    (0x9f2, 0x9f3),     // Sc '৲' .. Sc '৳'
    (0x9fb, 0x9fb),     // Sc '৻'
    (0xaf1, 0xaf1),     // Sc '૱'
    (0xbf9, 0xbf9),     // Sc '௹'
    (0xe3f, 0xe3f),     // Sc '฿'
    (0x17db, 0x17db),   // Sc '៛'
    (0x180e, 0x180e),   // Cf
    (0x200b, 0x200b),   // Cf
    (0x200e, 0x200f),   // Cf .. Cf
    (0x202a, 0x202e),   // Cf .. Cf
    (0x2060, 0x2064),   // Cf .. Cf
    (0x2066, 0x206f),   // Cf .. Cf
    (0x20a0, 0x20c1),   // Sc '₠' .. Sc '⃁'
    (0x2e2f, 0x2e2f),   // Lm 'ⸯ'
    (0xa838, 0xa838),   // Sc '꠸'
    (0xfc5e, 0xfc63),   // Lo 'ﱞ' .. Lo 'ﱣ'
    (0xfdfa, 0xfdfc),   // Lo 'ﷺ' .. Sc '﷼'
    (0xfe69, 0xfe69),   // Sc '﹩'
    (0xfe70, 0xfe70),   // Lo 'ﹰ'
    (0xfe72, 0xfe72),   // Lo 'ﹲ'
    (0xfe74, 0xfe74),   // Lo 'ﹴ'
    (0xfe76, 0xfe76),   // Lo 'ﹶ'
    (0xfe78, 0xfe78),   // Lo 'ﹸ'
    (0xfe7a, 0xfe7a),   // Lo 'ﹺ'
    (0xfe7c, 0xfe7c),   // Lo 'ﹼ'
    (0xfe7e, 0xfe7e),   // Lo 'ﹾ'
    (0xfeff, 0xfeff),   // Cf
    (0xff04, 0xff04),   // Sc '＄'
    (0xffe0, 0xffe1),   // Sc '￠' .. Sc '￡'
    (0xffe5, 0xffe6),   // Sc '￥' .. Sc '￦'
    (0xfff9, 0xfffb),   // Cf .. Cf
    (0x110bd, 0x110bd), // Cf
    (0x110cd, 0x110cd), // Cf
    (0x11fdd, 0x11fe0), // Sc '𑿝' .. Sc '𑿠'
    (0x13430, 0x1343f), // Cf .. Cf
    (0x1bca0, 0x1bca3), // Cf .. Cf
    (0x1d173, 0x1d17a), // Cf .. Cf
    (0x1e2ff, 0x1e2ff), // Sc '𞋿'
    (0x1ecb0, 0x1ecb0), // Sc '𞲰'
    (0xe0001, 0xe0001), // Cf
    (0xe0020, 0xe007f), // Cf .. Cf
];

const JAVA_IDENTIFIER_IGNORABLE: &[(u32, u32)] = &[
    (0x0, 0x8),
    (0xe, 0x1b),
    (0x7f, 0x9f),
    (0xad, 0xad),
    (0x600, 0x605),
    (0x61c, 0x61c),
    (0x6dd, 0x6dd),
    (0x70f, 0x70f),
    (0x890, 0x891),
    (0x8e2, 0x8e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x202a, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

pub(crate) static TOKENIZER: ExternalTokenizer = ExternalTokenizer::new(
    scan,
    TokenizerFlags {
        contextual: false,
        fallback: false,
        extend: false,
    },
)
.with_start(IDENTIFIER_START);

pub(crate) static STRICT_TOKEN_VALIDATORS: [StrictTokenValidator; 1] = [StrictTokenValidator::new(
    crate::terms::identifier,
    validate,
)];

fn validate(spelling: &str) -> Result<(), StrictTokenValidationError> {
    let invalid = first_invalid_identifier_offset(
        spelling,
        is_java_identifier_start,
        is_java_identifier_part,
    );
    match invalid {
        Some(offset) => Err(StrictTokenValidationError::new(
            offset,
            "invalid Java identifier",
        )),
        None => Ok(()),
    }
}

fn scan(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    let Some(first) = input.next().map(CodePoint::as_u32) else {
        return Ok(());
    };
    if !is_identifier_candidate_start(first) {
        return Ok(());
    }
    let (advanced_ascii, mut stopped_at_ascii_boundary) = if first < 0x80 {
        advance_ascii_identifier(input)
    } else {
        (0, false)
    };
    if advanced_ascii == 0 {
        input.advance(1);
        (_, stopped_at_ascii_boundary) = advance_ascii_identifier(input);
    }
    loop {
        if stopped_at_ascii_boundary {
            break;
        }
        let Some(character) = input.next().map(CodePoint::as_u32) else {
            break;
        };
        if character < 0x80 {
            let (advanced_ascii, stopped) = advance_ascii_identifier(input);
            stopped_at_ascii_boundary = stopped;
            if advanced_ascii != 0 || stopped {
                continue;
            }
        }
        if !is_identifier_candidate_part(character) {
            break;
        }
        input.advance(1);
        (_, stopped_at_ascii_boundary) = advance_ascii_identifier(input);
    }
    input.accept_token(crate::terms::identifier)
}

fn advance_ascii_identifier(input: &mut InputStream) -> (usize, bool) {
    let result =
        input.advance_ascii_while_with_stop(|byte| ASCII_IDENTIFIER_PART[usize::from(byte)]);
    (result.count(), result.stopped_on_mismatch())
}

const fn is_identifier_candidate_start(value: u32) -> bool {
    matches!(value, 0x24 | 0x41..=0x5a | 0x5f | 0x61..=0x7a | 0xa1..=0x0010_ffff)
}

const fn is_identifier_candidate_part(value: u32) -> bool {
    matches!(
        value,
        0x00..=0x08
            | 0x0e..=0x1b
            | 0x24
            | 0x30..=0x39
            | 0x41..=0x5a
            | 0x5f
            | 0x61..=0x7a
            | 0x7f..=0x0010_ffff
    )
}

pub(crate) fn specialize_record(value: &str, stack: &Stack) -> Option<u16> {
    if value != "record" {
        return None;
    }
    stack
        .can_shift(crate::terms::record)
        .then_some(crate::terms::record)
}

fn is_java_identifier_start(character: char) -> bool {
    unicode_ident::is_xid_start(character)
        || in_ranges(JAVA_IDENTIFIER_START_ADDITIONS, u32::from(character))
}

fn is_java_identifier_part(character: char) -> bool {
    unicode_ident::is_xid_continue(character)
        || in_ranges(JAVA_IDENTIFIER_PART_ADDITIONS, u32::from(character))
}

fn in_ranges(ranges: &[(u32, u32)], value: u32) -> bool {
    let index = ranges.partition_point(|&(_, end)| end < value);
    ranges
        .get(index)
        .is_some_and(|&(start, end)| start <= value && value <= end)
}

/// Return the JLS identifier spelling after removing ignorable characters.
#[must_use]
pub(crate) fn canonical_name(spelling: &str) -> String {
    spelling
        .chars()
        .filter(|character| !in_ranges(JAVA_IDENTIFIER_IGNORABLE, u32::from(*character)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_ranges_cover_java_identifier_additions() {
        assert_eq!(JAVA_IDENTIFIER_START_ADDITIONS.len(), 41);
        assert_eq!(JAVA_IDENTIFIER_PART_ADDITIONS.len(), 57);
        assert_eq!(JAVA_IDENTIFIER_IGNORABLE.len(), 24);
        assert!(is_java_identifier_start('$'));
        assert!(is_java_identifier_start('€'));
        assert!(!is_java_identifier_start('\u{200c}'));
        assert!(is_java_identifier_part('\u{200c}'));
        assert!(in_ranges(JAVA_IDENTIFIER_IGNORABLE, 0x200c));
        assert!(!is_java_identifier_part('😀'));
    }

    #[test]
    fn compact_ranges_are_sorted_and_disjoint() {
        for ranges in [
            JAVA_IDENTIFIER_START_ADDITIONS,
            JAVA_IDENTIFIER_PART_ADDITIONS,
            JAVA_IDENTIFIER_IGNORABLE,
        ] {
            assert!(ranges.iter().all(|&(start, end)| start <= end));
            assert!(ranges.windows(2).all(|pair| pair[0].1 < pair[1].0));
        }
    }

    #[test]
    fn canonical_names_remove_ignorable_characters() {
        assert_eq!(canonical_name("a\u{200c}b"), "ab");
        assert_eq!(canonical_name("€value"), "€value");
    }

    #[test]
    fn strict_profile_combines_xid_with_java_additions() {
        for valid in ["name", "_name", "λ2", "$name", "€value"] {
            assert!(validate(valid).is_ok(), "{valid:?}");
        }
        assert!(validate("name😀").is_err());
    }

    #[test]
    fn parser_applies_validation_only_in_strict_mode() {
        let source = "class C { int name😀; }";
        crate::parser()
            .parse(source)
            .expect("recovering Java accepts the broad identifier candidate");
        let error = crate::parser()
            .with_strict(true)
            .parse(source)
            .expect_err("strict Java validates the selected identifier token");
        assert_eq!(error.message(), "invalid Java identifier");
    }
}
