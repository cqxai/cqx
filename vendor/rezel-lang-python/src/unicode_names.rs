use std::cmp::Ordering;

#[path = "unicode16_names.rs"]
mod unicode16;

pub(crate) fn character(name: &str) -> Option<u32> {
    if name.is_empty() || !name.is_ascii() {
        return None;
    }
    if let Ok(index) = unicode16::ALIASES
        .binary_search_by(|(alias, _)| compare_ascii_case_insensitive(alias, name))
    {
        return Some(unicode16::ALIASES[index].1);
    }

    let character = unicode_names2::character(name)?;
    canonical_name_matches(character, name).then_some(u32::from(character))
}

fn canonical_name_matches(character: char, input: &str) -> bool {
    let Some(name) = unicode_names2::name(character) else {
        return false;
    };
    let mut offset: usize = 0;
    for part in name {
        let Some(end) = offset.checked_add(part.len()) else {
            return false;
        };
        let Some(candidate) = input.get(offset..end) else {
            return false;
        };
        if !candidate.eq_ignore_ascii_case(part) {
            return false;
        }
        offset = end;
    }
    offset == input.len()
}

fn compare_ascii_case_insensitive(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_uppercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::character;

    #[test]
    fn names_match_cpython_exactly_instead_of_unicode_loose_matching() {
        assert_eq!(character("LATIN SMALL LETTER A"), Some(u32::from('a')));
        assert_eq!(character("latin small letter a"), Some(u32::from('a')));
        assert_eq!(character("BACKSPACE"), Some(0x08));
        assert_eq!(character("KIRAT RAI LETTER A"), Some(0x16d43));
        assert_eq!(character("CJK UNIFIED IDEOGRAPH-4E00"), Some(0x4e00));
        assert_eq!(character("HANGUL SYLLABLE GA"), Some(0xac00));

        assert_eq!(character("LATIN_SMALL_LETTER_A"), None);
        assert_eq!(character("LATINSMALLLETTERA"), None);
        assert_eq!(character("KEYCAP NUMBER SIGN"), None);
        assert_eq!(character("NOT A REAL NAME"), None);
    }
}
