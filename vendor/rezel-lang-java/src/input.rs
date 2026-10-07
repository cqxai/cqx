use std::borrow::Cow;
use std::sync::Arc;

use rezel_common::{
    CodePoint, Input, InputCharacter, InputChunk, LexicalInput, TextRange, TextSize,
};

// Bound translation searches to one 4 KiB raw-input page.
const TRANSLATION_PAGE_SHIFT: u32 = 12;

/// Java's JLS §3.3 Unicode-escape view over original UTF-8 input.
///
/// The parser observes translated code points while all ranges continue to
/// address the wrapped input. UTF-16 surrogate code units are confined to this
/// language adapter.
#[derive(Clone)]
pub(crate) struct JavaInput {
    raw: Arc<dyn Input>,
    translations: TranslationIndex,
    final_sub: Option<TextSize>,
    malformed_escapes: Arc<[TextSize]>,
}

impl std::fmt::Debug for JavaInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JavaInput")
            .field("length", &self.raw.len())
            .field("translation_count", &self.translations.len())
            .field("final_sub", &self.final_sub)
            .field("malformed_escape_count", &self.malformed_escapes.len())
            .finish()
    }
}

impl JavaInput {
    pub(crate) fn new(raw: Arc<dyn Input>) -> Self {
        let ScanResult {
            escapes,
            final_sub,
            malformed_escapes,
        } = scan_unicode_escapes(&*raw);
        let raw_length = raw.len();
        Self {
            raw,
            translations: TranslationIndex::new(combine_escapes(&escapes), raw_length),
            final_sub,
            malformed_escapes: malformed_escapes.into(),
        }
    }

    pub(crate) fn malformed_escape_in(&self, ranges: &[TextRange]) -> Option<TextSize> {
        for range in ranges {
            let index = self
                .malformed_escapes
                .partition_point(|position| *position < range.start());
            if let Some(position) = self.malformed_escapes.get(index).copied()
                && position < range.end()
            {
                return Some(position);
            }
        }
        None
    }

    fn translation_at_or_after(&self, position: TextSize) -> Option<Translation> {
        self.translations.at_or_after(position)
    }

    fn range_has_translation(&self, range: TextRange) -> bool {
        if self
            .final_sub
            .is_some_and(|position| range.start() <= position && position < range.end())
        {
            return true;
        }
        let index = self.translations.first_ending_after(range.start());
        self.translations
            .get(index)
            .is_some_and(|translation| translation.range.start() < range.end())
    }

    /// Read translated text for AST lowering.
    ///
    /// AST string storage cannot represent isolated surrogates. Such values
    /// are replaced here, after tokenization, rather than in the lexical view.
    pub(crate) fn read_logical(&self, range: TextRange) -> Cow<'_, str> {
        if let Some(text) = self.scalar_text(range) {
            return text;
        }
        let mut result = String::new();
        let mut position = range.start();
        while position < range.end() {
            let Some(character) = self.character(position) else {
                break;
            };
            if character.raw_end() > range.end() {
                break;
            }
            result.push(
                character
                    .value()
                    .as_char()
                    .unwrap_or(char::REPLACEMENT_CHARACTER),
            );
            position = character.raw_end();
        }
        Cow::Owned(result)
    }
}

impl LexicalInput for JavaInput {
    fn raw(&self) -> &dyn Input {
        &*self.raw
    }

    fn identity_chunk(&self, from: TextSize) -> Option<InputChunk> {
        if self.final_sub.is_none() && self.translations.is_empty() {
            return self.raw.identity_chunk(from);
        }
        let translation = self.translation_at_or_after(from);
        if translation.is_some_and(|translation| translation.range.start() <= from) {
            return None;
        }
        let next_translation = translation
            .map(|translation| translation.range.start())
            .into_iter()
            .chain(self.final_sub.filter(|position| *position >= from))
            .min()
            .unwrap_or_else(|| self.raw.len());
        if next_translation == from {
            return None;
        }
        let mut chunk = self.raw.identity_chunk(from)?;
        let end = chunk.raw_end().min(next_translation);
        chunk.truncate(end);
        chunk.contains(from).then_some(chunk)
    }

    fn character(&self, from: TextSize) -> Option<InputCharacter> {
        if self.final_sub == Some(from) {
            return Some(InputCharacter::new(
                CodePoint::from(b' '),
                from + TextSize::from(1),
            ));
        }
        if let Some(translation) = self.translation_at_or_after(from) {
            if translation.range.start() == from {
                return Some(InputCharacter::new(
                    translation.value,
                    translation.range.end(),
                ));
            }
            if translation.range.start() < from {
                return None;
            }
        }
        raw_character(&*self.raw, from)
    }

    fn character_before(&self, before: TextSize) -> Option<(TextSize, InputCharacter)> {
        if self
            .final_sub
            .is_some_and(|position| position + TextSize::from(1) == before)
        {
            let start = before - TextSize::from(1);
            return Some((start, InputCharacter::new(CodePoint::from(b' '), before)));
        }
        let index = self.translations.first_ending_after(before);
        if let Some(translation) = index
            .checked_sub(1)
            .and_then(|index| self.translations.get(index))
            .copied()
            && translation.range.end() == before
        {
            return Some((
                translation.range.start(),
                InputCharacter::new(translation.value, before),
            ));
        }
        if let Some(translation) = self.translations.get(index)
            && translation.range.start() < before
        {
            return None;
        }
        raw_character_before(&*self.raw, before)
    }

    fn is_boundary(&self, position: TextSize) -> bool {
        if !self.raw.is_boundary(position) {
            return false;
        }
        self.translation_at_or_after(position)
            .is_none_or(|translation| translation.range.start() >= position)
    }

    fn scalar_text(&self, range: TextRange) -> Option<Cow<'_, str>> {
        if !self.range_has_translation(range) {
            return Some(self.raw.read(range));
        }
        debug_assert!(self.is_boundary(range.start()) && self.is_boundary(range.end()));
        let mut result = String::new();
        let mut position = range.start();
        while position < range.end() {
            let character = self.character(position)?;
            if character.raw_end() > range.end() {
                return None;
            }
            result.push(character.value().as_char()?);
            position = character.raw_end();
        }
        (position == range.end()).then_some(Cow::Owned(result))
    }
}

fn raw_character(input: &dyn Input, from: TextSize) -> Option<InputCharacter> {
    if let Some(chunk) = input.identity_chunk(from)
        && let Some(character) = chunk.character(from)
    {
        return Some(character);
    }
    if !input.is_boundary(from) {
        return None;
    }
    let character = input.chunk(from).chars().next()?;
    let width = TextSize::try_from(character.len_utf8()).ok()?;
    Some(InputCharacter::new(
        CodePoint::from(character),
        from + width,
    ))
}

fn raw_character_before(input: &dyn Input, before: TextSize) -> Option<(TextSize, InputCharacter)> {
    input.character_before(before)
}

#[derive(Clone, Copy, Debug)]
struct Translation {
    range: TextRange,
    value: CodePoint,
}

#[derive(Clone)]
struct TranslationIndex {
    values: Arc<[Translation]>,
    // Each entry is the first translation whose raw end is after that page boundary.
    page_offsets: Arc<[usize]>,
}

impl TranslationIndex {
    fn new(values: Vec<Translation>, raw_length: TextSize) -> Self {
        let page_offsets = build_translation_page_offsets(&values, raw_length);
        Self {
            values: values.into(),
            page_offsets: page_offsets.into(),
        }
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn get(&self, index: usize) -> Option<&Translation> {
        self.values.get(index)
    }

    fn first_ending_after(&self, position: TextSize) -> usize {
        if self.values.is_empty() {
            return 0;
        }
        let page = usize::from(position) >> TRANSLATION_PAGE_SHIFT;
        let Some(next_page) = page.checked_add(1) else {
            return self.full_search(position);
        };
        let Some(start) = self.page_offsets.get(page).copied() else {
            return self.full_search(position);
        };
        let Some(end) = self.page_offsets.get(next_page).copied() else {
            return self.full_search(position);
        };
        let local = self.values[start..end]
            .partition_point(|translation| translation.range.end() <= position);
        start + local
    }

    fn at_or_after(&self, position: TextSize) -> Option<Translation> {
        self.values.get(self.first_ending_after(position)).copied()
    }

    fn full_search(&self, position: TextSize) -> usize {
        self.values
            .partition_point(|translation| translation.range.end() <= position)
    }
}

fn build_translation_page_offsets(
    translations: &[Translation],
    raw_length: TextSize,
) -> Vec<usize> {
    if translations.is_empty() {
        return Vec::new();
    }
    let page_count = (usize::from(raw_length) >> TRANSLATION_PAGE_SHIFT) + 2;
    let mut offsets = Vec::with_capacity(page_count);
    let mut translation_index = 0;
    for page in 0..page_count {
        let page = u64::try_from(page).expect("translation page index fits u64");
        let boundary = page << TRANSLATION_PAGE_SHIFT;
        while translations
            .get(translation_index)
            .is_some_and(|translation| u64::from(u32::from(translation.range.end())) <= boundary)
        {
            translation_index += 1;
        }
        offsets.push(translation_index);
    }
    offsets
}

#[derive(Clone, Copy, Debug)]
struct UnicodeEscape {
    range: TextRange,
    value: u16,
}

fn combine_escapes(escapes: &[UnicodeEscape]) -> Vec<Translation> {
    let mut translations = Vec::with_capacity(escapes.len());
    let mut index = 0;
    while index < escapes.len() {
        let current = escapes[index];
        if (0xd800..=0xdbff).contains(&current.value)
            && let Some(next) = escapes.get(index + 1).copied()
            && next.range.start() == current.range.end()
            && (0xdc00..=0xdfff).contains(&next.value)
        {
            let high = u32::from(current.value) - 0xd800;
            let low = u32::from(next.value) - 0xdc00;
            let value = 0x1_0000 + (high << 10) + low;
            translations.push(Translation {
                range: TextRange::new(current.range.start(), next.range.end()),
                value: CodePoint::new(value).expect("a valid surrogate pair is a code point"),
            });
            index += 2;
            continue;
        }
        translations.push(Translation {
            range: current.range,
            value: CodePoint::from(current.value),
        });
        index += 1;
    }
    translations
}

struct ScanResult {
    escapes: Vec<UnicodeEscape>,
    final_sub: Option<TextSize>,
    malformed_escapes: Vec<TextSize>,
}

fn scan_unicode_escapes(input: &dyn Input) -> ScanResult {
    let mut escapes = Vec::new();
    let mut position = TextSize::from(0);
    let mut trailing_backslashes = 0_usize;
    let mut previous_was_escape = false;
    let mut malformed_escapes = Vec::new();

    'scan: while position < input.len() {
        let chunk = input.chunk(position);
        if chunk.is_empty() {
            break;
        }
        let mut cursor = 0_usize;
        while cursor < chunk.len() {
            let remaining = &chunk[cursor..];
            let Some(relative) = remaining.find('\\') else {
                previous_was_escape = false;
                trailing_backslashes = 0;
                break;
            };
            if relative != 0 {
                previous_was_escape = false;
                trailing_backslashes = 0;
            }
            let offset = cursor + relative;
            let offset = TextSize::try_from(offset).expect("input chunk fits in text coordinates");
            let character_start = position + offset;
            let eligible = previous_was_escape || trailing_backslashes.is_multiple_of(2);
            if eligible {
                match unicode_escape_at(input, character_start) {
                    EscapeScan::Valid { value, end } => {
                        escapes.push(UnicodeEscape {
                            range: TextRange::new(character_start, end),
                            value,
                        });
                        previous_was_escape = true;
                        trailing_backslashes = if value == u16::from(b'\\') {
                            trailing_backslashes.saturating_add(1)
                        } else {
                            0
                        };
                        position = end;
                        continue 'scan;
                    }
                    EscapeScan::Malformed => {
                        malformed_escapes.push(character_start);
                    }
                    EscapeScan::NotEscape => {}
                }
            }

            previous_was_escape = false;
            trailing_backslashes = trailing_backslashes.saturating_add(1);
            cursor = usize::from(offset) + 1;
        }
        position += TextSize::try_from(chunk.len()).expect("input chunk fits in text coordinates");
    }

    let final_sub = raw_character_before(input, input.len()).and_then(|(start, character)| {
        (character.raw_end() == input.len() && character.value() == CodePoint::from(0x1a_u8))
            .then_some(start)
    });
    ScanResult {
        escapes,
        final_sub,
        malformed_escapes,
    }
}

enum EscapeScan {
    Valid { value: u16, end: TextSize },
    Malformed,
    NotEscape,
}

fn unicode_escape_at(input: &dyn Input, offset: TextSize) -> EscapeScan {
    let mut cursor = offset + TextSize::from(1);
    if ascii_byte(input, cursor) != Some(b'u') {
        return EscapeScan::NotEscape;
    }
    while ascii_byte(input, cursor) == Some(b'u') {
        cursor += TextSize::from(1);
    }
    let Some(end) = cursor.checked_add(TextSize::from(4)) else {
        return EscapeScan::Malformed;
    };
    if end > input.len() {
        return EscapeScan::Malformed;
    }
    let mut value = 0_u16;
    let mut position = cursor;
    while position < end {
        let Some(digit) = ascii_byte(input, position).and_then(hex_value) else {
            return EscapeScan::Malformed;
        };
        value = value * 16 + u16::from(digit);
        position += TextSize::from(1);
    }
    EscapeScan::Valid { value, end }
}

fn ascii_byte(input: &dyn Input, position: TextSize) -> Option<u8> {
    input
        .chunk(position)
        .as_bytes()
        .first()
        .copied()
        .filter(u8::is_ascii)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use rezel_common::StringInput;

    use super::*;

    struct ChunkedInput {
        source: Arc<str>,
        chunk_length: usize,
    }

    impl Input for ChunkedInput {
        fn len(&self) -> TextSize {
            self.source.len().try_into().unwrap()
        }

        fn chunk(&self, from: TextSize) -> Cow<'_, str> {
            let from = usize::from(from);
            let mut to = (from + self.chunk_length).min(self.source.len());
            while to > from && !self.source.is_char_boundary(to) {
                to -= 1;
            }
            if to == from && from < self.source.len() {
                let width = self.source[from..]
                    .chars()
                    .next()
                    .expect("input position precedes one character")
                    .len_utf8();
                to = from + width;
            }
            Cow::Borrowed(&self.source[from..to])
        }

        fn read(&self, range: TextRange) -> Cow<'_, str> {
            Cow::Borrowed(&self.source[usize::from(range.start())..usize::from(range.end())])
        }

        fn is_boundary(&self, position: TextSize) -> bool {
            let position = usize::from(position);
            position <= self.source.len() && self.source.is_char_boundary(position)
        }
    }

    fn input(source: &str) -> JavaInput {
        JavaInput::new(Arc::new(StringInput::try_new(source).unwrap()))
    }

    fn translation(start: u32, end: u32) -> Translation {
        Translation {
            range: TextRange::new(start.into(), end.into()),
            value: CodePoint::from(b'a'),
        }
    }

    fn assert_paged_lookup_matches_full_search(translations: &[Translation], raw_length: u32) {
        let index = TranslationIndex::new(translations.to_vec(), raw_length.into());
        for position in 0..=raw_length {
            let position = TextSize::from(position);
            let expected =
                translations.partition_point(|translation| translation.range.end() <= position);
            assert_eq!(
                index.first_ending_after(position),
                expected,
                "translation lookup differed at {position:?}"
            );
        }
    }

    fn cooked(source: &str) -> String {
        input(source)
            .read_logical(TextRange::new(0.into(), source.len().try_into().unwrap()))
            .into_owned()
    }

    #[test]
    fn translates_only_eligible_unicode_escapes() {
        assert_eq!(cooked(r"cl\u0061ss"), "class");
        assert_eq!(cooked(r"\\u0061"), r"\\u0061");
        assert_eq!(cooked(r"\\\u0061"), r"\\a");
        assert_eq!(cooked(r"\u005cu0061"), r"\u0061");
        assert_eq!(cooked(r"\u005c\u005c"), r"\\");
    }

    #[test]
    fn scans_unicode_escapes_across_input_chunks() {
        for (raw, expected) in [
            (r"cl\u0061ss", "class"),
            (r"\\u0061", r"\\u0061"),
            (r"\x\u0061", r"\xa"),
        ] {
            let raw: Arc<str> = Arc::from(raw);
            let input = JavaInput::new(Arc::new(ChunkedInput {
                source: Arc::clone(&raw),
                chunk_length: 1,
            }));
            assert_eq!(
                input
                    .read_logical(TextRange::new(0.into(), raw.len().try_into().unwrap()))
                    .as_ref(),
                expected
            );
        }
    }

    #[test]
    fn retains_raw_boundaries_for_escaped_units() {
        let input = input(r"cl\u0061ss");
        let character = input.character(2.into()).unwrap();
        assert_eq!(character.value(), CodePoint::from(b'a'));
        assert_eq!(character.raw_end(), 8.into());
        let (start, previous) = input.character_before(8.into()).unwrap();
        assert_eq!(start, 2.into());
        assert_eq!(previous, character);
        assert!(input.is_boundary(2.into()));
        assert!(!input.is_boundary(3.into()));
        assert!(input.is_boundary(8.into()));
    }

    #[test]
    fn shared_chunks_stop_at_translated_units() {
        let input = input(r"cl\u0061ss");
        let prefix = input.identity_chunk(0.into()).expect("identity prefix");
        assert_eq!(prefix.raw_start(), 0.into());
        assert_eq!(prefix.raw_end(), 2.into());
        assert!(input.identity_chunk(2.into()).is_none());

        let suffix = input.identity_chunk(8.into()).expect("identity suffix");
        assert_eq!(suffix.raw_start(), 8.into());
        assert_eq!(suffix.raw_end(), 10.into());
    }

    #[test]
    fn paged_translation_index_matches_full_search() {
        let sparse = vec![
            translation(0, 6),
            translation(4084, 4090),
            translation(4090, 4096),
            translation(4096, 4102),
            translation(4200, 9000),
            translation(10_000, 10_006),
            translation(12_282, 12_288),
        ];
        assert_paged_lookup_matches_full_search(&sparse, 12_288);

        let dense: Vec<_> = (0_u32..12_288)
            .step_by(6)
            .map(|start| translation(start, start + 6))
            .collect();
        assert_paged_lookup_matches_full_search(&dense, 12_288);

        let empty = TranslationIndex::new(Vec::new(), TextSize::from(12_288));
        assert!(empty.page_offsets.is_empty());
        assert_eq!(empty.first_ending_after(TextSize::from(12_288)), 0);
    }

    #[test]
    fn combines_valid_pairs_and_preserves_isolated_surrogates() {
        let pair = input(r"\uD83D\uDE00");
        let character = pair.character(0.into()).unwrap();
        assert_eq!(character.value(), CodePoint::from('😀'));
        assert_eq!(character.raw_end(), 12.into());
        assert!(!pair.is_boundary(6.into()));
        assert!(pair.character(6.into()).is_none());
        let (start, previous) = pair.character_before(12.into()).unwrap();
        assert_eq!(start, TextSize::from(0));
        assert_eq!(previous, character);
        assert_eq!(cooked(r"\uD83D\uDE00"), "😀");

        let high = input(r"\uD800");
        let high_character = high.character(0.into()).unwrap();
        assert_eq!(
            high_character.value(),
            CodePoint::new(0xd800).expect("surrogate is a code point")
        );
        assert!(
            high.scalar_text(TextRange::new(0.into(), 6.into()))
                .is_none()
        );
        assert_eq!(high.character_before(6.into()).unwrap().1, high_character);

        let low = input(r"\uDC00");
        assert_eq!(
            low.character(0.into()).unwrap().value(),
            CodePoint::new(0xdc00).expect("surrogate is a code point")
        );
        assert!(
            low.scalar_text(TextRange::new(0.into(), 6.into()))
                .is_none()
        );
    }

    #[test]
    fn distinguishes_isolated_surrogates_from_replacement_characters() {
        let surrogate = input(r"\uD800");
        let escaped_replacement = input(r"\uFFFD");
        let direct_replacement = input("\u{fffd}");

        assert_eq!(
            surrogate.character(0.into()).unwrap().value(),
            CodePoint::new(0xd800).unwrap()
        );
        assert_eq!(
            escaped_replacement.character(0.into()).unwrap().value(),
            CodePoint::from(char::REPLACEMENT_CHARACTER)
        );
        assert_eq!(
            direct_replacement.character(0.into()).unwrap().value(),
            CodePoint::from(char::REPLACEMENT_CHARACTER)
        );
        assert!(
            surrogate
                .scalar_text(TextRange::new(0.into(), 6.into()))
                .is_none()
        );
        assert_eq!(
            escaped_replacement
                .scalar_text(TextRange::new(0.into(), 6.into()))
                .unwrap(),
            "\u{fffd}"
        );
        assert_eq!(
            direct_replacement
                .scalar_text(TextRange::new(0.into(), 3.into()))
                .unwrap(),
            "\u{fffd}"
        );
    }

    #[test]
    fn ignores_final_sub_in_the_lexical_view() {
        assert_eq!(cooked("class A {}\u{1a}"), "class A {} ");
    }

    #[test]
    fn records_malformed_eligible_escapes() {
        let translated = input(r"\uu12xz \u123x");
        assert_eq!(
            &*translated.malformed_escapes,
            &[TextSize::from(0), TextSize::from(8)]
        );
        assert_eq!(
            translated.malformed_escape_in(&[TextRange::new(8.into(), 14.into())]),
            Some(8.into())
        );
        assert!(input(r"\\uu12xz").malformed_escapes.is_empty());
    }
}
